use std::collections::HashMap;

use serde::Deserialize;
use shroudb_acl::ServerAuthConfig;

#[derive(Debug, Deserialize)]
pub struct CourierServerConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub auth: ServerAuthConfig,
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
            auth: ServerAuthConfig::default(),
            cipher: None,
            channels: HashMap::new(),
        }),
    }
}
