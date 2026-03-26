use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use metrics::{counter, histogram};
use tokio::sync::RwLock;

use shroudb_courier_core::adapter::AdapterRegistry;
use shroudb_courier_core::template::TemplateEngine;
use shroudb_courier_core::transit::TransitDecryptor;

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
}

impl CommandDispatcher {
    pub fn new(
        template_engine: Arc<RwLock<TemplateEngine>>,
        adapters: Arc<AdapterRegistry>,
        transit: Arc<TransitDecryptor>,
        auth_registry: Arc<AuthRegistry>,
        templates_dir: PathBuf,
    ) -> Self {
        Self {
            template_engine,
            adapters,
            transit,
            auth_registry,
            templates_dir,
        }
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
            && !matches!(cmd, Command::Auth { .. } | Command::Health)
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

        counter!("courier_commands_total", "command" => verb, "result" => result_label)
            .increment(1);
        histogram!("courier_command_duration_seconds", "command" => verb)
            .record(duration.as_secs_f64());

        let behavior = if is_read { "read" } else { "write" };
        counter!("courier_commands_by_behavior_total", "behavior" => behavior).increment(1);

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

            Command::Auth { .. } => Ok(ResponseMap::ok()),

            Command::Pipeline(_) => unreachable!("pipeline handled above"),
        }
    }
}
