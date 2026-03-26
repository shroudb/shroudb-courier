//! HTTP API server for delivery, templates, adapters, and health.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use tower_http::cors::{AllowOrigin, CorsLayer};

use shroudb_courier_core::adapter::AdapterRegistry;
use shroudb_courier_core::template::TemplateEngine;
use shroudb_courier_core::transit::TransitDecryptor;
use shroudb_courier_protocol::auth::AuthRegistry;

use tokio::sync::RwLock;

#[derive(Clone)]
struct HttpState {
    template_engine: Arc<RwLock<TemplateEngine>>,
    adapters: Arc<AdapterRegistry>,
    transit: Arc<TransitDecryptor>,
    auth_registry: Arc<AuthRegistry>,
}

/// Configuration for the HTTP API server.
pub struct HttpConfig {
    pub bind: SocketAddr,
    pub template_engine: Arc<RwLock<TemplateEngine>>,
    pub adapters: Arc<AdapterRegistry>,
    pub transit: Arc<TransitDecryptor>,
    pub auth_registry: Arc<AuthRegistry>,
    pub cors_origins: Option<Vec<String>>,
}

/// Start the HTTP API server.
pub async fn run_http_server(
    config: HttpConfig,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let cors_origins = config.cors_origins;
    let bind = config.bind;
    let state = HttpState {
        template_engine: config.template_engine,
        adapters: config.adapters,
        transit: config.transit,
        auth_registry: config.auth_registry,
    };

    let cors_layer = build_cors_layer(cors_origins);

    let app = Router::new()
        .route("/deliver", post(post_deliver))
        .route("/batch-deliver", post(post_batch_deliver))
        .route("/templates", get(get_templates))
        .route("/templates/{name}", get(get_template))
        .route("/adapters", get(get_adapters))
        .route("/health", get(get_health))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            bearer_auth_middleware,
        ))
        .layer(cors_layer)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %bind, "HTTP API listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await?;

    Ok(())
}

/// POST /deliver — deliver a notification.
async fn post_deliver(State(state): State<HttpState>, body: String) -> impl IntoResponse {
    use shroudb_courier_core::delivery::{ContentType, DeliveryRequest, RenderedMessage};
    use zeroize::Zeroize;

    let request: DeliveryRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": format!("invalid request: {e}") }).to_string(),
            )
                .into_response();
        }
    };

    // Decrypt recipient.
    let plaintext_secret = match state.transit.decrypt(&request.recipient).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": format!("transit decrypt: {e}") }).to_string(),
            )
                .into_response();
        }
    };

    let mut plaintext_recipient = match String::from_utf8(plaintext_secret.as_bytes().to_vec()) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": "decrypted recipient is not valid UTF-8" })
                    .to_string(),
            )
                .into_response();
        }
    };

    // Render message.
    let engine = state.template_engine.read().await;
    let message = if let Some(ref template_name) = request.template {
        let vars = request.vars.as_ref().cloned().unwrap_or_default();
        match engine.render(template_name, &vars) {
            Ok(m) => m,
            Err(e) => {
                plaintext_recipient.zeroize();
                return (
                    StatusCode::BAD_REQUEST,
                    [(header::CONTENT_TYPE, "application/json")],
                    serde_json::json!({ "error": format!("template: {e}") }).to_string(),
                )
                    .into_response();
            }
        }
    } else if let Some(ref body_text) = request.body {
        RenderedMessage {
            subject: request.subject.clone(),
            body: body_text.clone(),
            content_type: ContentType::Plain,
        }
    } else {
        plaintext_recipient.zeroize();
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "error": "either 'template' or 'body' must be provided" })
                .to_string(),
        )
            .into_response();
    };
    drop(engine);

    // Find adapter.
    let adapter = match state.adapters.get(request.channel) {
        Some(a) => a,
        None => {
            plaintext_recipient.zeroize();
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": format!("no adapter for channel: {}", request.channel) }).to_string(),
            )
                .into_response();
        }
    };

    // Deliver.
    let receipt = match adapter.deliver(&plaintext_recipient, &message).await {
        Ok(r) => r,
        Err(e) => {
            plaintext_recipient.zeroize();
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": format!("delivery: {e}") }).to_string(),
            )
                .into_response();
        }
    };

    // Zeroize plaintext.
    plaintext_recipient.zeroize();

    let response = serde_json::json!({
        "delivery_id": receipt.delivery_id,
        "channel": receipt.channel,
        "adapter": receipt.adapter,
        "status": receipt.status,
        "delivered_at": receipt.delivered_at,
        "error": receipt.error,
    });

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        response.to_string(),
    )
        .into_response()
}

/// POST /batch-deliver — deliver multiple notifications.
async fn post_batch_deliver(State(state): State<HttpState>, body: String) -> impl IntoResponse {
    use shroudb_courier_core::delivery::{ContentType, DeliveryRequest, RenderedMessage};
    use zeroize::Zeroize;

    #[derive(serde::Deserialize)]
    struct BatchRequest {
        deliveries: Vec<DeliveryRequest>,
    }

    let batch: BatchRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "error": format!("invalid request: {e}") }).to_string(),
            )
                .into_response();
        }
    };

    let mut results = Vec::with_capacity(batch.deliveries.len());

    for request in &batch.deliveries {
        // Decrypt.
        let plaintext_secret = match state.transit.decrypt(&request.recipient).await {
            Ok(s) => s,
            Err(e) => {
                results.push(serde_json::json!({ "error": format!("transit decrypt: {e}") }));
                continue;
            }
        };

        let mut plaintext_recipient = match String::from_utf8(plaintext_secret.as_bytes().to_vec())
        {
            Ok(s) => s,
            Err(_) => {
                results
                    .push(serde_json::json!({ "error": "decrypted recipient is not valid UTF-8" }));
                continue;
            }
        };

        // Render.
        let engine = state.template_engine.read().await;
        let message = if let Some(ref template_name) = request.template {
            let vars = request.vars.as_ref().cloned().unwrap_or_default();
            match engine.render(template_name, &vars) {
                Ok(m) => m,
                Err(e) => {
                    plaintext_recipient.zeroize();
                    results.push(serde_json::json!({ "error": format!("template: {e}") }));
                    continue;
                }
            }
        } else if let Some(ref body_text) = request.body {
            RenderedMessage {
                subject: request.subject.clone(),
                body: body_text.clone(),
                content_type: ContentType::Plain,
            }
        } else {
            plaintext_recipient.zeroize();
            results.push(
                serde_json::json!({ "error": "either 'template' or 'body' must be provided" }),
            );
            continue;
        };
        drop(engine);

        // Adapter.
        let adapter = match state.adapters.get(request.channel) {
            Some(a) => a,
            None => {
                plaintext_recipient.zeroize();
                results.push(serde_json::json!({ "error": format!("no adapter for channel: {}", request.channel) }));
                continue;
            }
        };

        // Deliver.
        match adapter.deliver(&plaintext_recipient, &message).await {
            Ok(receipt) => {
                results.push(serde_json::json!({
                    "delivery_id": receipt.delivery_id,
                    "channel": receipt.channel,
                    "adapter": receipt.adapter,
                    "status": receipt.status,
                    "delivered_at": receipt.delivered_at,
                    "error": receipt.error,
                }));
            }
            Err(e) => {
                results.push(serde_json::json!({ "error": format!("delivery: {e}") }));
            }
        }

        plaintext_recipient.zeroize();
    }

    let response = serde_json::json!({ "results": results });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        response.to_string(),
    )
        .into_response()
}

/// GET /templates — list all loaded templates.
async fn get_templates(State(state): State<HttpState>) -> impl IntoResponse {
    let engine = state.template_engine.read().await;
    let templates: Vec<serde_json::Value> = engine
        .list()
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "has_subject": t.has_subject,
                "has_html_body": t.has_html_body,
                "has_text_body": t.has_text_body,
            })
        })
        .collect();

    let response = serde_json::json!({
        "count": templates.len(),
        "templates": templates,
    });

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        response.to_string(),
    )
}

/// GET /templates/{name} — get details of a specific template.
async fn get_template(
    State(state): State<HttpState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let engine = state.template_engine.read().await;
    match engine.get(&name) {
        Some(info) => {
            let response = serde_json::json!({
                "name": info.name,
                "has_subject": info.has_subject,
                "has_html_body": info.has_html_body,
                "has_text_body": info.has_text_body,
            });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                response.to_string(),
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "error": "template not found" }).to_string(),
        )
            .into_response(),
    }
}

/// GET /adapters — list registered adapters.
async fn get_adapters(State(state): State<HttpState>) -> impl IntoResponse {
    let list = state.adapters.list();
    let adapters: Vec<serde_json::Value> = list
        .iter()
        .map(|(ch, name)| {
            serde_json::json!({
                "channel": ch.to_string(),
                "name": name,
            })
        })
        .collect();

    let response = serde_json::json!({
        "count": adapters.len(),
        "adapters": adapters,
    });

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        response.to_string(),
    )
}

/// GET /health — health check.
async fn get_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"ok"}"#,
    )
}

/// Bearer token auth middleware for HTTP API.
async fn bearer_auth_middleware(
    State(state): State<HttpState>,
    req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    // Skip auth for health endpoint.
    let path = req.uri().path();
    if path == "/health" {
        return next.run(req).await;
    }

    // If auth is not required, pass through.
    if !state.auth_registry.is_required() {
        return next.run(req).await;
    }

    // Check Authorization header.
    let auth_header = req.headers().get(header::AUTHORIZATION);
    match auth_header.and_then(|v| v.to_str().ok()) {
        Some(value) if value.starts_with("Bearer ") => {
            let token = &value[7..];
            match state.auth_registry.authenticate(token) {
                Ok(_policy) => next.run(req).await,
                Err(_) => (
                    StatusCode::UNAUTHORIZED,
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"error":"invalid token"}"#,
                )
                    .into_response(),
            }
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"authorization required"}"#,
        )
            .into_response(),
    }
}

/// Build CORS layer from config.
fn build_cors_layer(origins: Option<Vec<String>>) -> CorsLayer {
    let allow_origin = match origins {
        Some(ref origins) if !origins.is_empty() => {
            let origins: Vec<axum::http::HeaderValue> =
                origins.iter().filter_map(|o| o.parse().ok()).collect();
            AllowOrigin::list(origins)
        }
        _ => AllowOrigin::any(),
    };

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
}
