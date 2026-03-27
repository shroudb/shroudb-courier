use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use tokio::sync::RwLock;

use shroudb_courier_core::adapter::AdapterRegistry;
use shroudb_courier_core::template::TemplateEngine;
use shroudb_courier_core::transit::TransitDecryptor;
use shroudb_courier_core::ws::ChannelRegistry;

use crate::auth::{AuthPolicy, AuthRegistry};
use crate::command::{Command, command_verb};
use crate::error::CommandError;
use crate::handlers;
use crate::response::{CommandResponse, ResponseMap, ResponseValue};

/// Routes parsed Courier commands to the appropriate handler.
pub struct CommandDispatcher {
    template_engine: Arc<RwLock<TemplateEngine>>,
    adapters: Arc<AdapterRegistry>,
    transit: Arc<TransitDecryptor>,
    auth_registry: Arc<AuthRegistry>,
    templates_dir: PathBuf,
    /// In-memory config store (no WAL — changes are lost on restart).
    config: DashMap<String, String>,
    /// WebSocket channel registry (None if WebSocket is disabled).
    ws_registry: Option<Arc<ChannelRegistry>>,
}

impl CommandDispatcher {
    pub fn new(
        template_engine: Arc<RwLock<TemplateEngine>>,
        adapters: Arc<AdapterRegistry>,
        transit: Arc<TransitDecryptor>,
        auth_registry: Arc<AuthRegistry>,
        templates_dir: PathBuf,
    ) -> Self {
        let config = DashMap::new();
        config.insert("templates.dir".into(), templates_dir.display().to_string());
        Self {
            template_engine,
            adapters,
            transit,
            auth_registry,
            templates_dir,
            config,
            ws_registry: None,
        }
    }

    /// Set the WebSocket channel registry. Must be called before starting the server
    /// if WebSocket support is enabled.
    pub fn set_ws_registry(&mut self, registry: Arc<ChannelRegistry>) {
        self.ws_registry = Some(registry);
    }

    pub fn auth_registry(&self) -> &AuthRegistry {
        &self.auth_registry
    }

    /// Reload templates from disk and return the count of loaded templates.
    pub async fn reload_templates(&self) -> Result<usize, CommandError> {
        let mut engine = self.template_engine.write().await;
        let count = engine
            .reload(&self.templates_dir)
            .map_err(|e| CommandError::Internal(e.to_string()))?;
        tracing::info!(count, dir = %self.templates_dir.display(), "templates reloaded");
        Ok(count)
    }

    pub async fn execute(&self, cmd: Command, auth: Option<&AuthPolicy>) -> CommandResponse {
        // Handle pipeline recursively.
        if let Command::Pipeline(commands) = cmd {
            let mut results = Vec::with_capacity(commands.len());
            for c in commands {
                results.push(Box::pin(self.execute(c, auth)).await);
            }
            return CommandResponse::Array(results);
        }

        // Check auth policy if auth is required.
        if self.auth_registry.is_required()
            && !matches!(
                cmd,
                Command::Auth { .. }
                    | Command::Health
                    | Command::Ping
                    | Command::CommandList
                    | Command::ConfigGet { .. }
                    | Command::ConfigSet { .. }
                    | Command::ConfigList
            )
        {
            match auth {
                None => {
                    return CommandResponse::Error(CommandError::AuthRequired);
                }
                Some(policy) => {
                    if let Err(e) = policy.check(&cmd) {
                        return CommandResponse::Error(e);
                    }
                }
            }
        }

        let verb = command_verb(&cmd);
        let is_read = cmd.is_read();

        let start = Instant::now();
        let result = self.dispatch(cmd).await;
        let duration = start.elapsed();

        let result_label = match &result {
            Ok(_) => "ok",
            Err(_) => "error",
        };

        // Audit log for write operations.
        if !is_read {
            let actor = auth.map(|a| a.name.as_str()).unwrap_or("anonymous");
            tracing::info!(
                target: "courier::audit",
                op = verb,
                result = result_label,
                duration_ms = duration.as_millis() as u64,
                actor = actor,
                "command executed"
            );
        }

        match result {
            Ok(resp) => CommandResponse::Success(resp),
            Err(e) => CommandResponse::Error(e),
        }
    }

    async fn dispatch(&self, cmd: Command) -> Result<ResponseMap, CommandError> {
        match cmd {
            Command::TemplateReload => {
                let count = self.reload_templates().await?;
                Ok(
                    ResponseMap::ok()
                        .with("templates_loaded", ResponseValue::Integer(count as i64)),
                )
            }

            Command::TemplateList => {
                let engine = self.template_engine.read().await;
                handlers::template_list::handle_template_list(&engine)
            }

            Command::TemplateInfo { name } => {
                let engine = self.template_engine.read().await;
                handlers::template_info::handle_template_info(&engine, &name)
            }

            Command::Deliver { json } => {
                let engine = self.template_engine.read().await;
                handlers::deliver::handle_deliver(&json, &engine, &self.adapters, &self.transit)
                    .await
            }

            Command::Health => {
                let engine = self.template_engine.read().await;
                handlers::health::handle_health(&engine, &self.adapters)
            }

            Command::ConfigGet { key } => match self.config.get(&key) {
                Some(v) => Ok(ResponseMap::ok()
                    .with("value", ResponseValue::String(v.value().clone()))
                    .with("source", ResponseValue::String("runtime".into()))),
                None => Err(CommandError::BadArg {
                    message: format!("unknown config key: {key}"),
                }),
            },

            Command::ConfigSet { key, value } => {
                if !self.config.contains_key(&key) {
                    return Err(CommandError::BadArg {
                        message: format!("unknown config key: {key}"),
                    });
                }
                self.config.insert(key, value);
                Ok(ResponseMap::ok())
            }

            Command::ConfigList => {
                let fields: Vec<_> = self
                    .config
                    .iter()
                    .map(|entry| {
                        (
                            entry.key().clone(),
                            ResponseValue::Map(
                                ResponseMap::ok()
                                    .with("value", ResponseValue::String(entry.value().clone()))
                                    .with("source", ResponseValue::String("runtime".into()))
                                    .with("mutable", ResponseValue::Boolean(true)),
                            ),
                        )
                    })
                    .collect();
                Ok(ResponseMap { fields })
            }

            Command::Ping => Ok(ResponseMap::ok().with("message", ResponseValue::String("PONG".into()))),

            Command::CommandList => {
                let commands = vec![
                    "DELIVER", "TEMPLATE_RELOAD", "TEMPLATE_LIST", "TEMPLATE_INFO",
                    "CHANNEL_INFO", "CHANNEL_LIST", "CONNECTIONS",
                    "HEALTH", "CONFIG", "AUTH", "PING", "COMMAND",
                ];
                let values: Vec<ResponseValue> = commands
                    .into_iter()
                    .map(|c| ResponseValue::String(c.into()))
                    .collect();
                Ok(ResponseMap::ok()
                    .with("count", ResponseValue::Integer(values.len() as i64))
                    .with("commands", ResponseValue::Array(values)))
            }

            Command::Auth { .. } => Ok(ResponseMap::ok()),

            Command::ChannelInfo { channel } => {
                let reg = self
                    .ws_registry
                    .as_ref()
                    .ok_or_else(|| CommandError::BadArg {
                        message: "WebSocket not enabled".into(),
                    })?;
                let count = reg.subscriber_count(&channel).await;
                Ok(ResponseMap::ok()
                    .with("channel", ResponseValue::String(channel))
                    .with("subscribers", ResponseValue::Integer(count as i64)))
            }

            Command::ChannelList => {
                let reg = self
                    .ws_registry
                    .as_ref()
                    .ok_or_else(|| CommandError::BadArg {
                        message: "WebSocket not enabled".into(),
                    })?;
                let channels = reg.list_channels().await;
                let list: Vec<ResponseValue> = channels
                    .into_iter()
                    .map(|(name, count)| {
                        ResponseValue::Map(
                            ResponseMap::ok()
                                .with("channel", ResponseValue::String(name))
                                .with("subscribers", ResponseValue::Integer(count as i64)),
                        )
                    })
                    .collect();
                Ok(ResponseMap::ok().with("channels", ResponseValue::Array(list)))
            }

            Command::Connections => {
                let reg = self
                    .ws_registry
                    .as_ref()
                    .ok_or_else(|| CommandError::BadArg {
                        message: "WebSocket not enabled".into(),
                    })?;
                let total = reg.total_connections().await;
                Ok(ResponseMap::ok().with("connections", ResponseValue::Integer(total as i64)))
            }

            Command::Pipeline(_) => unreachable!("pipeline handled above"),
        }
    }
}
