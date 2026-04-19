mod adapters;
mod cipher_client;
mod cipher_embedded;
mod config;
mod tcp;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use shroudb_cipher_engine::engine::{CipherConfig as CipherEngineConfig, CipherEngine};
use shroudb_courier_core::{Channel, ChannelType};
use shroudb_courier_engine::CourierEngine;
use shroudb_store::Store;
use tokio::net::TcpListener;

use crate::config::{CourierServerConfig, load_config};

#[derive(Parser)]
#[command(
    name = "shroudb-courier",
    about = "ShrouDB Courier — just-in-time decryption delivery engine"
)]
struct Cli {
    #[arg(long, env = "COURIER_CONFIG")]
    config: Option<String>,

    #[arg(long, env = "COURIER_DATA_DIR")]
    data_dir: Option<String>,

    #[arg(long, env = "COURIER_TCP_BIND")]
    tcp_bind: Option<String>,

    #[arg(long, env = "COURIER_LOG_LEVEL", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut cfg = load_config(cli.config.as_deref())?;

    // CLI overrides
    if let Some(ref data_dir) = cli.data_dir {
        cfg.store.data_dir = data_dir.clone();
    }
    if let Some(ref tcp_bind) = cli.tcp_bind {
        cfg.server.tcp_bind = tcp_bind.clone();
    }
    cfg.server.log_level = cli.log_level.clone();

    // Bootstrap: logging + core dumps + key source
    let key_source = shroudb_server_bootstrap::bootstrap(&cfg.server.log_level);

    // Store: embedded or remote
    match cfg.store.mode.as_str() {
        "embedded" => {
            let data_dir = std::path::PathBuf::from(&cfg.store.data_dir);
            let storage = shroudb_server_bootstrap::open_storage(&data_dir, key_source.as_ref())
                .await
                .context("failed to open storage engine")?;
            let store = Arc::new(shroudb_storage::EmbeddedStore::new(
                storage.clone(),
                "courier",
            ));
            let cipher_embedded = build_cipher_embedded(&cfg, storage.clone()).await?;
            run_server(cfg, store, Some(storage), cipher_embedded).await
        }
        "remote" => {
            let uri = cfg
                .store
                .uri
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("remote mode requires store.uri"))?;
            tracing::info!(uri, "connecting to remote store");
            let store = Arc::new(
                shroudb_client::RemoteStore::connect(uri)
                    .await
                    .context("failed to connect to remote store")?,
            );
            if let Some(ref cipher_cfg) = cfg.cipher
                && cipher_cfg.is_embedded()
            {
                anyhow::bail!(
                    "cipher.mode = \"embedded\" requires store.mode = \"embedded\" \
                     (embedded Cipher needs a co-located StorageEngine)"
                );
            }
            run_server(cfg, store, None, None).await
        }
        other => anyhow::bail!("unknown store mode: {other}"),
    }
}

/// Build an in-process `CipherEngine` on a dedicated namespace of the same
/// storage engine Courier uses. Returns `None` when `[cipher]` is absent or
/// not in embedded mode.
async fn build_cipher_embedded(
    cfg: &CourierServerConfig,
    storage: Arc<shroudb_storage::StorageEngine>,
) -> anyhow::Result<Option<CipherEmbeddedHandle>> {
    use shroudb_cipher_core::keyring::KeyringAlgorithm;
    use shroudb_cipher_engine::keyring_manager::KeyringCreateOpts;

    let cipher_cfg = match cfg.cipher.as_ref() {
        Some(c) => c,
        None => return Ok(None),
    };
    cipher_cfg
        .validate(&cfg.store.mode)
        .context("invalid [cipher] config")?;
    if !cipher_cfg.is_embedded() {
        return Ok(None);
    }

    let store = Arc::new(shroudb_storage::EmbeddedStore::new(storage, "cipher"));
    let engine_cfg = CipherEngineConfig {
        default_rotation_days: cipher_cfg.rotation_days,
        default_drain_days: cipher_cfg.drain_days,
        scheduler_interval_secs: cipher_cfg.scheduler_interval_secs,
    };
    let engine = CipherEngine::new(
        store,
        engine_cfg,
        shroudb_server_bootstrap::Capability::disabled(
            "courier-server embedded Cipher: policy evaluation flows through Courier's own sentry slot",
        ),
        shroudb_server_bootstrap::Capability::disabled(
            "courier-server embedded Cipher: audit events flow through Courier's own chronicle slot",
        ),
    )
    .await
    .context("failed to initialize embedded Cipher engine")?;

    let algorithm: KeyringAlgorithm = cipher_cfg
        .algorithm
        .parse()
        .map_err(|e: String| anyhow::anyhow!("invalid cipher.algorithm: {e}"))?;
    // Idempotent seed — creation races against ExistsError are the only
    // expected failure, and the keyring is then already usable.
    match engine
        .keyring_manager()
        .create(
            &cipher_cfg.keyring,
            algorithm,
            KeyringCreateOpts {
                rotation_days: cipher_cfg.rotation_days,
                drain_days: cipher_cfg.drain_days,
                ..Default::default()
            },
        )
        .await
    {
        Ok(_) | Err(shroudb_cipher_core::error::CipherError::KeyringExists(_)) => {}
        Err(e) => {
            return Err(anyhow::anyhow!(
                "failed to seed embedded cipher keyring '{}': {e}",
                cipher_cfg.keyring
            ));
        }
    }

    tracing::info!(
        keyring = %cipher_cfg.keyring,
        "embedded Cipher engine initialized on 'cipher' namespace"
    );
    Ok(Some(CipherEmbeddedHandle {
        engine: Arc::new(engine),
        keyring: cipher_cfg.keyring.clone(),
    }))
}

struct CipherEmbeddedHandle {
    engine: Arc<CipherEngine<shroudb_storage::EmbeddedStore>>,
    keyring: String,
}

async fn run_server<S: Store + 'static>(
    cfg: CourierServerConfig,
    store: Arc<S>,
    storage: Option<Arc<shroudb_storage::StorageEngine>>,
    cipher_embedded: Option<CipherEmbeddedHandle>,
) -> anyhow::Result<()> {
    use shroudb_server_bootstrap::Capability;

    // Resolve [audit] and [policy] capabilities. shroudb-engine-bootstrap
    // 0.3.0 defaults both to `mode = "embedded"`, so an omitted section
    // now produces a fully-functional embedded capability rather than
    // failing at startup. Embedded init errors still surface through the
    // `.context(...)` calls below — we do not swallow real failures.
    let audit_cfg = cfg.audit.clone().unwrap_or_default();
    let audit_cap = audit_cfg
        .resolve(storage.clone())
        .await
        .context("failed to resolve [audit] capability")?;
    let policy_cfg = cfg.policy.clone().unwrap_or_default();
    let policy_cap = policy_cfg
        .resolve(storage.clone(), audit_cap.as_ref().cloned())
        .await
        .context("failed to resolve [policy] capability")?;

    // Cipher decryptor: embedded (co-located), remote (external Cipher),
    // or explicit DisabledWithJustification when [cipher] is absent.
    let decryptor: Capability<Arc<dyn shroudb_courier_engine::Decryptor>> =
        match (cfg.cipher.as_ref(), cipher_embedded) {
            (Some(_), Some(handle)) => {
                tracing::info!(
                    keyring = %handle.keyring,
                    "courier: using embedded Cipher for recipient decryption"
                );
                Capability::Enabled(Arc::new(cipher_embedded::EmbeddedDecryptor::new(
                    handle.engine,
                    handle.keyring,
                )))
            }
            (Some(cipher_cfg), None) if cipher_cfg.is_remote() => {
                let addr = cipher_cfg.addr.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("cipher.mode = \"remote\" requires cipher.addr")
                })?;
                let d = cipher_client::CipherDecryptor::new(
                    addr,
                    &cipher_cfg.keyring,
                    cipher_cfg.auth_token.as_deref(),
                )
                .await?;
                Capability::Enabled(Arc::new(d))
            }
            _ => {
                tracing::warn!(
                    "no [cipher] configuration — recipients will be treated as plaintext; \
                     decryption slot is explicit DisabledWithJustification so the server \
                     advertises this posture"
                );
                Capability::disabled(
                    "no [cipher] section in courier config — recipients treated as plaintext",
                )
            }
        };

    // Engine
    let policy_mode = match cfg.policy_mode.as_str() {
        "open" => shroudb_courier_engine::PolicyMode::Open,
        _ => shroudb_courier_engine::PolicyMode::Closed,
    };
    let engine = CourierEngine::new_with_policy_mode(
        Arc::clone(&store),
        decryptor,
        policy_cap,
        audit_cap,
        policy_mode,
    )
    .await
    .context("failed to initialize courier engine")?;
    let engine = Arc::new(engine);

    // Register adapters
    let webhook_adapter = if let Some(ref secret) = cfg.webhook_signing_secret {
        adapters::WebhookAdapter::with_signing_secret(secret.as_bytes().to_vec())
    } else {
        adapters::WebhookAdapter::new()
    };
    engine.register_adapter(ChannelType::Webhook, Arc::new(webhook_adapter));

    // Seed channels from config
    for (name, seed) in &cfg.channels {
        let ct: ChannelType = seed
            .channel_type
            .parse()
            .map_err(|e: String| anyhow::anyhow!(e))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let channel = Channel {
            name: name.clone(),
            channel_type: ct,
            smtp: seed.smtp.clone(),
            webhook: seed.webhook.clone(),
            enabled: true,
            created_at: now,
            default_recipient: None,
        };

        // Register SMTP adapter if this is an email channel with config
        if ct == ChannelType::Email
            && let Some(ref smtp_cfg) = seed.smtp
        {
            engine.register_adapter(
                ChannelType::Email,
                Arc::new(adapters::SmtpAdapter::new(smtp_cfg.clone())),
            );
        }

        engine.seed_channel(channel).await?;
    }

    // Auth
    let token_validator = cfg.auth.build_validator();

    // TCP listener
    let listener = TcpListener::bind(&cfg.server.tcp_bind).await?;
    let tcp_bind = cfg.server.tcp_bind.clone();

    // Shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let tls_acceptor = cfg
        .server
        .tls
        .as_ref()
        .map(shroudb_server_tcp::build_tls_acceptor)
        .transpose()
        .context("failed to build TLS acceptor")?;

    let tcp_engine = Arc::clone(&engine);
    let tcp_tv = token_validator.clone();
    let tcp_handle = tokio::spawn(async move {
        tcp::run_tcp(listener, tcp_engine, tcp_tv, shutdown_rx, tls_acceptor).await;
    });

    // Banner (Courier has extra cipher line)
    let key_mode = if std::env::var("SHROUDB_MASTER_KEY").is_ok()
        || std::env::var("SHROUDB_MASTER_KEY_FILE").is_ok()
    {
        "configured"
    } else {
        "ephemeral (dev mode)"
    };
    eprintln!("Courier v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("├─ tcp:     {tcp_bind}");
    eprintln!("├─ data:    {}", cfg.store.data_dir);
    eprintln!(
        "├─ cipher:  {}",
        match cfg.cipher.as_ref() {
            Some(c) if c.is_embedded() => "embedded",
            Some(c) => c.addr.as_deref().unwrap_or("remote (addr missing)"),
            None => "disabled",
        }
    );
    eprintln!("└─ key:     {key_mode}");
    eprintln!();
    eprintln!("Ready.");

    // Wait for shutdown
    shroudb_server_bootstrap::wait_for_shutdown(shutdown_tx).await?;
    let _ = tcp_handle.await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_debug_asserts() {
        Cli::command().debug_assert();
    }
}
