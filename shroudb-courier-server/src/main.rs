//! ShrouDB Courier — secure notification delivery pipeline.
//!
//! Binary entry point: CLI argument parsing, config loading, and server startup.

mod config;
mod connection;
mod http;
mod server;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::RwLock;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use shroudb_courier_core::adapter::{
    AdapterRegistry, SendGridAdapter, SmtpAdapter, WebhookAdapter,
};
use shroudb_courier_core::template::TemplateEngine;
use shroudb_courier_core::transit::TransitDecryptor;

#[derive(Parser)]
#[command(
    name = "shroudb-courier",
    about = "Secure notification delivery pipeline",
    version
)]
struct Cli {
    /// Path to the TOML configuration file.
    #[arg(long, default_value = "courier.toml")]
    config: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 0. Disable core dumps to prevent leaking plaintext (Linux only).
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0);
    }

    // 1. Parse CLI arguments.
    let cli = Cli::parse();

    // 2. Load configuration (or use defaults if no config file).
    let cfg = match config::load(&cli.config)? {
        Some(cfg) => {
            init_logging()?;
            tracing::info!(config = %cli.config.display(), "configuration loaded");
            cfg
        }
        None => {
            init_logging()?;
            tracing::info!("no config file found, starting with defaults");
            config::CourierConfig::default()
        }
    };

    // 3. Load templates from directory.
    let templates_dir = cfg.templates.dir.clone();
    std::fs::create_dir_all(&templates_dir).ok();
    let template_engine = TemplateEngine::load_dir(&templates_dir)?;
    let template_count = template_engine.list().len();
    let template_engine = Arc::new(RwLock::new(template_engine));
    tracing::info!(
        count = template_count,
        dir = %templates_dir.display(),
        watch = cfg.templates.watch,
        "templates loaded"
    );

    // 4. Build adapter registry.
    let mut adapters = AdapterRegistry::new();

    if let Some(ref smtp_cfg) = cfg.adapters.smtp {
        match SmtpAdapter::new(
            &smtp_cfg.host,
            smtp_cfg.port,
            smtp_cfg.username.as_deref(),
            smtp_cfg.password.as_deref(),
            &smtp_cfg.from_address,
            smtp_cfg.starttls,
        ) {
            Ok(adapter) => {
                adapters.register(Box::new(adapter));
                tracing::info!(
                    host = %smtp_cfg.host,
                    port = smtp_cfg.port,
                    "SMTP adapter registered"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to create SMTP adapter");
            }
        }
    }

    if cfg.adapters.webhook.as_ref().is_some_and(|w| w.enabled) || cfg.adapters.webhook.is_none() {
        adapters.register(Box::new(WebhookAdapter::new()));
        tracing::info!("webhook adapter registered");
    }

    if let Some(ref sg_cfg) = cfg.adapters.sendgrid {
        adapters.register(Box::new(SendGridAdapter::new(
            &sg_cfg.api_key,
            &sg_cfg.from_email,
            sg_cfg.from_name.as_deref(),
        )));
        tracing::info!("SendGrid adapter registered");
    }

    let adapters = Arc::new(adapters);
    tracing::info!(count = adapters.list().len(), "adapters ready");

    // 5. Create Transit decryptor.
    let transit = Arc::new(TransitDecryptor::new(
        &cfg.transit.addr,
        cfg.transit.tls,
        &cfg.transit.keyring,
        cfg.transit.auth_token.as_deref(),
    ));
    tracing::info!(
        addr = %cfg.transit.addr,
        keyring = %cfg.transit.keyring,
        tls = cfg.transit.tls,
        "Transit decryptor configured"
    );

    // 6. Build auth registry from config.
    let auth_registry = Arc::new(config::build_auth_registry(&cfg));

    // 7. Create CommandDispatcher.
    let dispatcher = Arc::new(shroudb_courier_protocol::CommandDispatcher::new(
        Arc::clone(&template_engine),
        Arc::clone(&adapters),
        Arc::clone(&transit),
        Arc::clone(&auth_registry),
        templates_dir.clone(),
    ));

    // 8. Install Prometheus metrics recorder.
    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install metrics recorder");

    // 9. Set up shutdown signal (SIGTERM + SIGINT).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    // 10. Start file watcher for hot template reload.
    if cfg.templates.watch {
        let watch_disp = Arc::clone(&dispatcher);
        let watch_dir = templates_dir.clone();
        let watch_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            run_template_watcher(watch_dir, watch_disp, watch_rx).await;
        });
        tracing::info!(dir = %templates_dir.display(), "template file watcher started");
    }

    // 11. Start HTTP server (metrics only).
    {
        let http_config = http::HttpConfig {
            bind: cfg.server.http_bind,
            metrics_handle: metrics_handle.clone(),
        };
        let http_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = http::run_http_server(http_config, http_rx).await {
                tracing::error!(error = %e, "HTTP server failed");
            }
        });
    }

    // 12. Run RESP3 server (blocks until shutdown).
    tracing::info!(bind = %cfg.server.bind, "shroudb-courier ready");
    server::run(&cfg.server, dispatcher, metrics_handle, shutdown_rx).await?;

    tracing::info!("shroudb-courier shut down cleanly");
    Ok(())
}

fn init_logging() -> anyhow::Result<()> {
    let env_filter = resolve_log_filter();
    let console_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_filter(env_filter);

    tracing_subscriber::registry().with(console_layer).init();

    Ok(())
}

fn resolve_log_filter() -> tracing_subscriber::EnvFilter {
    if let Ok(level) = std::env::var("LOG_LEVEL") {
        return tracing_subscriber::EnvFilter::new(level);
    }
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

async fn run_template_watcher(
    dir: PathBuf,
    dispatcher: Arc<shroudb_courier_protocol::CommandDispatcher>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    use notify::{RecursiveMode, Watcher};

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);

    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let has_template = event.paths.iter().any(|p| {
                    let name = p.to_string_lossy();
                    name.ends_with(".txt") || name.ends_with(".html")
                });
                if has_template {
                    let _ = tx.blocking_send(());
                }
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, "failed to create file watcher");
                return;
            }
        };

    if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        tracing::error!(error = %e, dir = %dir.display(), "failed to watch templates directory");
        return;
    }

    let debounce = tokio::time::Duration::from_secs(1);
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            Some(()) = rx.recv() => {
                // Debounce: drain any events that arrive within 1 second.
                tokio::time::sleep(debounce).await;
                while rx.try_recv().is_ok() {}

                match dispatcher.reload_templates().await {
                    Ok(count) => {
                        tracing::info!(count, "templates hot-reloaded by file watcher");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "template hot-reload failed");
                    }
                }
            }
        }
    }

    drop(watcher);
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to listen for ctrl+c");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
