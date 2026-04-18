use std::collections::HashMap;

use serde::Deserialize;
use shroudb_acl::ServerAuthConfig;
use shroudb_engine_bootstrap::{AuditConfig, PolicyConfig};

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
    /// Policy enforcement mode. Default is "closed" (deny when no evaluator).
    /// Set to "open" only for development/testing.
    #[serde(default = "default_policy_mode")]
    pub policy_mode: String,
    /// Optional HMAC-SHA256 signing secret for webhook deliveries.
    /// When set, each webhook POST includes an `X-ShrouDB-Signature` header.
    pub webhook_signing_secret: Option<String>,
    /// Audit (Chronicle) capability slot. Absent = fail-closed at startup.
    #[serde(default)]
    pub audit: Option<AuditConfig>,
    /// Policy (Sentry) capability slot. Same contract.
    #[serde(default)]
    pub policy: Option<PolicyConfig>,
}

fn default_policy_mode() -> String {
    "closed".into()
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_tcp_bind")]
    pub tcp_bind: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub tls: Option<shroudb_server_tcp::TlsConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            tcp_bind: default_tcp_bind(),
            log_level: default_log_level(),
            tls: None,
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

/// Cipher (recipient-decryption) capability slot.
///
/// Two modes:
/// - `mode = "remote"` (default): point at an external `shroudb-cipher`
///   server at `addr`. Keyring and optional auth token are used per call.
/// - `mode = "embedded"`: bundle an in-process `CipherEngine` on the same
///   `StorageEngine` as Courier's metadata (distinct namespace). Requires
///   `store.mode = "embedded"`.
///
/// Omit `[cipher]` entirely to run Courier without decryption — recipients
/// are then treated as already-plaintext and the slot is
/// `DisabledWithJustification` so operators see the posture at startup.
#[derive(Debug, Deserialize)]
pub struct CipherConfig {
    #[serde(default = "default_cipher_mode")]
    pub mode: String,
    #[serde(default = "default_cipher_keyring")]
    pub keyring: String,

    // Remote mode
    #[serde(default)]
    pub addr: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,

    // Embedded mode
    #[serde(default = "default_rotation_days")]
    pub rotation_days: u32,
    #[serde(default = "default_drain_days")]
    pub drain_days: u32,
    #[serde(default = "default_scheduler_interval_secs")]
    pub scheduler_interval_secs: u64,
    #[serde(default = "default_cipher_algorithm")]
    pub algorithm: String,
}

impl CipherConfig {
    pub fn is_embedded(&self) -> bool {
        self.mode == "embedded"
    }

    pub fn is_remote(&self) -> bool {
        self.mode == "remote"
    }

    pub fn validate(&self, store_mode: &str) -> anyhow::Result<()> {
        match self.mode.as_str() {
            "remote" => {
                if self.addr.is_none() {
                    anyhow::bail!("cipher.mode = \"remote\" requires cipher.addr");
                }
            }
            "embedded" => {
                if store_mode != "embedded" {
                    anyhow::bail!(
                        "cipher.mode = \"embedded\" requires store.mode = \"embedded\" \
                         (embedded Cipher shares the StorageEngine with Courier)"
                    );
                }
            }
            other => anyhow::bail!(
                "unknown cipher.mode: {other:?} (expected \"remote\" or \"embedded\")"
            ),
        }
        Ok(())
    }
}

fn default_cipher_mode() -> String {
    "remote".into()
}
fn default_cipher_keyring() -> String {
    "courier-recipients".into()
}
fn default_rotation_days() -> u32 {
    90
}
fn default_drain_days() -> u32 {
    30
}
fn default_scheduler_interval_secs() -> u64 {
    3600
}
fn default_cipher_algorithm() -> String {
    "aes-256-gcm".into()
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
            policy_mode: default_policy_mode(),
            webhook_signing_secret: None,
            audit: None,
            policy: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_embedded_mode() {
        let cfg: CourierServerConfig = toml::from_str("").expect("parse failed");
        assert_eq!(cfg.store.mode, "embedded");
        assert!(cfg.store.uri.is_none());
    }

    #[test]
    fn config_parses_remote_mode_with_uri() {
        let toml = r#"
[store]
mode = "remote"
uri = "shroudb://token@127.0.0.1:6399"
"#;
        let cfg: CourierServerConfig = toml::from_str(toml).expect("parse failed");
        assert_eq!(cfg.store.mode, "remote");
        assert_eq!(
            cfg.store.uri.as_deref(),
            Some("shroudb://token@127.0.0.1:6399")
        );
    }

    #[test]
    fn config_parses_cipher_embedded_mode() {
        let toml = r#"
[cipher]
mode = "embedded"
keyring = "courier-recipients"
rotation_days = 60
"#;
        let cfg: CourierServerConfig = toml::from_str(toml).expect("parse failed");
        let cipher = cfg.cipher.expect("cipher section present");
        assert!(cipher.is_embedded());
        assert_eq!(cipher.rotation_days, 60);
        cipher.validate("embedded").expect("embedded valid");
        assert!(
            cipher.validate("remote").is_err(),
            "embedded cipher requires embedded store"
        );
    }

    #[test]
    fn config_parses_cipher_remote_requires_addr() {
        let toml = r#"
[cipher]
mode = "remote"
keyring = "courier-recipients"
"#;
        let cfg: CourierServerConfig = toml::from_str(toml).unwrap();
        let cipher = cfg.cipher.unwrap();
        assert!(cipher.is_remote());
        assert!(
            cipher.validate("embedded").is_err(),
            "remote without addr fails"
        );
    }

    #[test]
    fn config_rejects_unknown_cipher_mode() {
        let toml = r#"
[cipher]
mode = "bogus"
"#;
        let cfg: CourierServerConfig = toml::from_str(toml).unwrap();
        assert!(cfg.cipher.unwrap().validate("embedded").is_err());
    }

    #[test]
    fn config_parses_remote_mode_tls_uri() {
        let toml = r#"
[store]
mode = "remote"
uri = "shroudb+tls://token@store.example.com:6399"
"#;
        let cfg: CourierServerConfig = toml::from_str(toml).expect("parse failed");
        assert_eq!(cfg.store.mode, "remote");
        assert_eq!(
            cfg.store.uri.as_deref(),
            Some("shroudb+tls://token@store.example.com:6399")
        );
    }
}
