//! Configuration loading for ShrouDB Courier.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use shroudb_courier_protocol::auth::{AuthPolicy, AuthRegistry};

#[derive(Debug, Default, Deserialize)]
pub struct CourierConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub transit: TransitConfig,
    #[serde(default)]
    pub templates: TemplatesConfig,
    #[serde(default)]
    pub adapters: AdaptersConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub tls_client_ca: Option<PathBuf>,
    pub rate_limit: Option<u32>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            tls_cert: None,
            tls_key: None,
            tls_client_ca: None,
            rate_limit: None,
        }
    }
}

fn default_bind() -> SocketAddr {
    "0.0.0.0:6999".parse().unwrap()
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_auth_method")]
    pub method: String,
    #[serde(default)]
    pub policies: HashMap<String, PolicyConfig>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            method: default_auth_method(),
            policies: HashMap::new(),
        }
    }
}

fn default_auth_method() -> String {
    "none".into()
}

#[derive(Debug, Deserialize)]
pub struct PolicyConfig {
    pub token: String,
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransitConfig {
    /// Transit server address (host:port).
    #[serde(default = "default_transit_addr")]
    pub addr: String,
    /// Whether to use TLS for Transit connection.
    #[serde(default)]
    pub tls: bool,
    /// Transit keyring to use for decryption.
    #[serde(default = "default_transit_keyring")]
    pub keyring: String,
    /// Optional auth token for Transit.
    pub auth_token: Option<String>,
}

impl Default for TransitConfig {
    fn default() -> Self {
        Self {
            addr: default_transit_addr(),
            tls: false,
            keyring: default_transit_keyring(),
            auth_token: None,
        }
    }
}

fn default_transit_addr() -> String {
    "127.0.0.1:6399".into()
}

fn default_transit_keyring() -> String {
    "default".into()
}

#[derive(Debug, Deserialize)]
pub struct TemplatesConfig {
    #[serde(default = "default_templates_dir")]
    pub dir: PathBuf,
    #[serde(default)]
    pub watch: bool,
}

impl Default for TemplatesConfig {
    fn default() -> Self {
        Self {
            dir: default_templates_dir(),
            watch: false,
        }
    }
}

fn default_templates_dir() -> PathBuf {
    PathBuf::from("./templates")
}

#[derive(Debug, Default, Deserialize)]
pub struct AdaptersConfig {
    pub smtp: Option<SmtpConfig>,
    pub webhook: Option<WebhookConfig>,
    pub sendgrid: Option<SendGridConfig>,
}

#[derive(Debug, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    #[serde(default = "default_starttls")]
    pub starttls: bool,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_starttls() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct WebhookConfig {
    #[serde(default = "default_webhook_enabled")]
    pub enabled: bool,
}

fn default_webhook_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct SendGridConfig {
    pub api_key: String,
    pub from_email: String,
    pub from_name: Option<String>,
}

/// Load and parse config file with env var interpolation.
pub fn load(path: &Path) -> anyhow::Result<Option<CourierConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)?;
    let expanded = expand_env_vars(&contents);
    let config: CourierConfig = toml::from_str(&expanded)?;
    Ok(Some(config))
}

/// Expand `${VAR_NAME}` patterns in a string.
fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                value,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

/// Build the auth registry from config.
pub fn build_auth_registry(cfg: &CourierConfig) -> AuthRegistry {
    if cfg.auth.method == "none" {
        return AuthRegistry::permissive();
    }

    let policies: HashMap<String, AuthPolicy> = cfg
        .auth
        .policies
        .iter()
        .map(|(name, pc)| {
            let policy = AuthPolicy {
                name: name.clone(),
                commands: if pc.commands.is_empty() {
                    vec!["*".into()]
                } else {
                    pc.commands.clone()
                },
            };
            (pc.token.clone(), policy)
        })
        .collect();

    AuthRegistry::new(policies, true)
}
