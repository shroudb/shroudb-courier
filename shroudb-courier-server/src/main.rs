mod adapters;
mod cipher_client;
mod config;
mod tcp;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use shroudb_courier_core::{Channel, ChannelType};
use shroudb_courier_engine::CourierEngine;
use shroudb_storage::{
    ChainedMasterKeySource, EnvMasterKey, EphemeralKey, FileMasterKey, MasterKeySource,
    StorageEngineConfig,
};
use tokio::net::TcpListener;

use crate::config::{build_token_validator, load_config};

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

    // Store mode validation
    if cfg.store.mode == "remote" {
        anyhow::bail!(
            "remote store mode not yet implemented (uri: {:?})",
            cfg.store.uri
        );
    }

    // Logging
    let filter = tracing_subscriber::EnvFilter::try_new(&cfg.server.log_level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();

    shroudb_crypto::disable_core_dumps();

    // Master key
    let key_source: Box<dyn MasterKeySource> = Box::new(ChainedMasterKeySource::new(vec![
        Box::new(EnvMasterKey::new()),
        Box::new(FileMasterKey::new()),
        Box::new(EphemeralKey),
    ]));

    let key_mode = if std::env::var("SHROUDB_MASTER_KEY").is_ok()
        || std::env::var("SHROUDB_MASTER_KEY_FILE").is_ok()
    {
        "configured"
    } else {
        "ephemeral (dev mode)"
    };

    // Storage
    let storage_config = StorageEngineConfig {
        data_dir: std::path::PathBuf::from(&cfg.store.data_dir),
        ..Default::default()
    };
    let storage_engine = shroudb_storage::StorageEngine::open(storage_config, key_source.as_ref())
        .await
        .context("failed to open storage engine")?;
    let store = Arc::new(shroudb_storage::EmbeddedStore::new(
        Arc::new(storage_engine),
        "courier",
    ));

    // Cipher decryptor
    let decryptor: Option<Arc<dyn shroudb_courier_engine::Decryptor>> =
        if let Some(ref cipher_cfg) = cfg.cipher {
            let d = cipher_client::CipherDecryptor::new(
                &cipher_cfg.addr,
                &cipher_cfg.keyring,
                cipher_cfg.auth_token.as_deref(),
            )
            .await?;
            Some(Arc::new(d))
        } else {
            tracing::warn!("no cipher configuration — recipients will be treated as plaintext");
            None
        };

    // Engine
    let engine = CourierEngine::new(Arc::clone(&store), decryptor, None)
        .await
        .context("failed to initialize courier engine")?;
    let engine = Arc::new(engine);

    // Register adapters
    engine.register_adapter(
        ChannelType::Webhook,
        Arc::new(adapters::WebhookAdapter::new()),
    );

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
    let token_validator = build_token_validator(&cfg.auth);

    // TCP listener
    let listener = TcpListener::bind(&cfg.server.tcp_bind).await?;
    let tcp_bind = cfg.server.tcp_bind.clone();

    // Shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let tcp_engine = Arc::clone(&engine);
    let tcp_tv = token_validator.clone();
    let tcp_handle = tokio::spawn(async move {
        tcp::run_tcp(listener, tcp_engine, tcp_tv, shutdown_rx).await;
    });

    // Banner
    eprintln!("Courier v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("├─ tcp:     {tcp_bind}");
    eprintln!("├─ data:    {}", cfg.store.data_dir);
    eprintln!(
        "├─ cipher:  {}",
        cfg.cipher
            .as_ref()
            .map(|c| c.addr.as_str())
            .unwrap_or("disabled")
    );
    eprintln!("└─ key:     {key_mode}");
    eprintln!();
    eprintln!("Ready.");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown signal received");
    let _ = shutdown_tx.send(true);
    let _ = tcp_handle.await;

    Ok(())
}
