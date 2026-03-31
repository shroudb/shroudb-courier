use serde::Deserialize;
use shroudb_acl::{Scope, StaticTokenValidator, Token, TokenGrant, TokenValidator};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct CourierServerConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub cipher: Option<CipherConfig>,
    #[serde(default)]
    pub channels: HashMap<String, ChannelSeedConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_tcp_bind")]
    pub tcp_bind: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tcp_bind: default_tcp_bind(),
            log_level: default_log_level(),
        }
    }
}

fn default_tcp_bind() -> String {
    "0.0.0.0:6999".into()
}

fn default_log_level() -> String {
    "info".into()
}

#[derive(Debug, Deserialize)]
pub struct StoreConfig {
    #[serde(default = "default_store_mode")]
    pub mode: String,
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    pub uri: Option<String>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            mode: default_store_mode(),
            data_dir: default_data_dir(),
            uri: None,
        }
    }
}

fn default_store_mode() -> String {
    "embedded".into()
}

fn default_data_dir() -> String {
    "./courier-data".into()
}

#[derive(Debug, Default, Deserialize)]
pub struct AuthConfig {
    pub method: Option<String>,
    #[serde(default)]
    pub tokens: HashMap<String, TokenConfig>,
}

#[derive(Debug, Deserialize)]
pub struct TokenConfig {
    pub tenant: String,
    #[serde(default = "default_actor")]
    pub actor: String,
    #[serde(default)]
    pub platform: bool,
    #[serde(default)]
    pub grants: Vec<GrantConfig>,
}

fn default_actor() -> String {
    "anonymous".into()
}

#[derive(Debug, Deserialize)]
pub struct GrantConfig {
    pub namespace: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CipherConfig {
    pub addr: String,
    pub keyring: String,
    pub auth_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChannelSeedConfig {
    pub channel_type: String,
    #[serde(default)]
    pub smtp: Option<shroudb_courier_core::SmtpConfig>,
    #[serde(default)]
    pub webhook: Option<shroudb_courier_core::WebhookConfig>,
}

pub fn load_config(path: Option<&str>) -> anyhow::Result<CourierServerConfig> {
    match path {
        Some(p) => {
            let content = std::fs::read_to_string(p)?;
            Ok(toml::from_str(&content)?)
        }
        None => Ok(CourierServerConfig {
            server: ServerConfig::default(),
            store: StoreConfig::default(),
            auth: AuthConfig::default(),
            cipher: None,
            channels: HashMap::new(),
        }),
    }
}

pub fn build_token_validator(config: &AuthConfig) -> Option<Arc<dyn TokenValidator>> {
    if config.method.as_deref() != Some("token") {
        return None;
    }
    if config.tokens.is_empty() {
        return None;
    }

    let mut validator = StaticTokenValidator::new();
    for (raw, tc) in &config.tokens {
        let grants: Vec<TokenGrant> = tc
            .grants
            .iter()
            .map(|g| TokenGrant {
                namespace: g.namespace.clone(),
                scopes: g
                    .scopes
                    .iter()
                    .filter_map(|s| match s.as_str() {
                        "read" => Some(Scope::Read),
                        "write" => Some(Scope::Write),
                        _ => None,
                    })
                    .collect(),
            })
            .collect();

        let token = Token {
            tenant: tc.tenant.clone(),
            actor: tc.actor.clone(),
            is_platform: tc.platform,
            grants,
            expires_at: None,
        };

        validator.register(raw.clone(), token);
    }

    Some(Arc::new(validator))
}
