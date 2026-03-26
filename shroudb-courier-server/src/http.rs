//! HTTP server for Prometheus metrics.

use std::net::SocketAddr;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;

/// Configuration for the HTTP server.
pub struct HttpConfig {
    pub bind: SocketAddr,
    pub metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
}

/// Start the HTTP server.
pub async fn run_http_server(
    config: HttpConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let bind = config.bind;
    let metrics_handle = config.metrics_handle;

    let app = Router::new().route("/metrics", get(move || get_metrics(metrics_handle.clone())));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %bind, "HTTP server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await?;

    Ok(())
}

/// GET /metrics — Prometheus metrics.
async fn get_metrics(handle: metrics_exporter_prometheus::PrometheusHandle) -> impl IntoResponse {
    let metrics = handle.render();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        metrics,
    )
}
