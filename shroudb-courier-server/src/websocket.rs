//! WebSocket server — accepts persistent connections for real-time push delivery.
//!
//! Runs on its own port (default 7001), separate from the RESP3 and HTTP servers.
//! Clients subscribe to channels and receive messages when Courier delivers via
//! the `ws` channel adapter.

use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tracing::Instrument;

use shroudb_courier_core::ws::{ChannelRegistry, WsMessage};

/// Run the WebSocket server, accepting connections until `shutdown_rx` fires.
pub async fn run_websocket_server(
    bind: SocketAddr,
    registry: Arc<ChannelRegistry>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let listener = match TcpListener::bind(bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(addr = %bind, error = %e, "WebSocket server failed to bind");
            return;
        }
    };
    tracing::info!(addr = %bind, "WebSocket server listening");

    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("WebSocket server shutting down");
                    break;
                }
            }

            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        let reg = Arc::clone(&registry);
                        let span = tracing::info_span!("ws_conn", peer = %peer_addr);
                        tokio::spawn(
                            handle_ws_connection(stream, reg, peer_addr)
                                .instrument(span)
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "WebSocket TCP accept error");
                    }
                }
            }
        }
    }
}

/// Handle a single WebSocket connection lifecycle.
async fn handle_ws_connection(
    stream: TcpStream,
    registry: Arc<ChannelRegistry>,
    peer_addr: SocketAddr,
) {
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!(%peer_addr, error = %e, "WebSocket handshake failed");
            return;
        }
    };

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let socket_id = uuid::Uuid::new_v4().to_string();

    tracing::debug!(%peer_addr, %socket_id, "WebSocket connected");

    // Send "connected" event.
    let connected_msg = serde_json::json!({
        "event": "connected",
        "data": { "socket_id": &socket_id }
    });
    if let Err(e) = ws_sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            connected_msg.to_string(),
        ))
        .await
    {
        tracing::warn!(%socket_id, error = %e, "failed to send connected event");
        return;
    }

    // Single mpsc channel that all broadcast forwarders send to.
    // The writer task reads from this and forwards to the WebSocket sender.
    let (forward_tx, mut forward_rx) = mpsc::unbounded_channel::<WsMessage>();

    // Reader task: reads client messages, manages subscriptions.
    let reader_registry = Arc::clone(&registry);
    let reader_socket_id = socket_id.clone();
    let reader_forward_tx = forward_tx.clone();

    let reader_task = tokio::spawn(async move {
        while let Some(msg_result) = ws_receiver.next().await {
            let msg = match msg_result {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(socket_id = %reader_socket_id, error = %e, "WebSocket read error");
                    break;
                }
            };

            match msg {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    handle_client_message(
                        &text,
                        &reader_socket_id,
                        &reader_registry,
                        &reader_forward_tx,
                    )
                    .await;
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => {
                    tracing::debug!(socket_id = %reader_socket_id, "WebSocket close frame received");
                    break;
                }
                tokio_tungstenite::tungstenite::Message::Ping(data) => {
                    // Pong is sent automatically by tungstenite, but we track it.
                    tracing::trace!(socket_id = %reader_socket_id, len = data.len(), "ping received");
                }
                _ => {}
            }
        }
    });

    // Writer task: reads from forward_rx and sends to WS sender.
    let writer_task = tokio::spawn(async move {
        while let Some(ws_msg) = forward_rx.recv().await {
            let json = match serde_json::to_string(&ws_msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to serialize WsMessage");
                    continue;
                }
            };
            if let Err(e) = ws_sender
                .send(tokio_tungstenite::tungstenite::Message::Text(json))
                .await
            {
                tracing::debug!(error = %e, "failed to send to WebSocket, closing");
                break;
            }
        }
    });

    // Wait for either task to finish (disconnect).
    tokio::select! {
        _ = reader_task => {}
        _ = writer_task => {}
    }

    // Clean up all subscriptions for this socket.
    registry.disconnect(&socket_id).await;
    tracing::debug!(%peer_addr, %socket_id, "WebSocket disconnected, subscriptions cleaned up");
}

/// Handle a parsed client message (subscribe, unsubscribe, ping).
async fn handle_client_message(
    text: &str,
    socket_id: &str,
    registry: &Arc<ChannelRegistry>,
    forward_tx: &mpsc::UnboundedSender<WsMessage>,
) {
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            let _ = forward_tx.send(WsMessage {
                event: "error".into(),
                channel: String::new(),
                data: serde_json::json!({"message": "invalid JSON"}),
            });
            return;
        }
    };

    let event = parsed.get("event").and_then(|v| v.as_str()).unwrap_or("");

    match event {
        "subscribe" => {
            let channel = match parsed.get("channel").and_then(|v| v.as_str()) {
                Some(ch) => ch,
                None => {
                    let _ = forward_tx.send(WsMessage {
                        event: "error".into(),
                        channel: String::new(),
                        data: serde_json::json!({"message": "subscribe requires 'channel' field"}),
                    });
                    return;
                }
            };

            match registry.subscribe(socket_id, channel).await {
                Ok(mut rx) => {
                    tracing::debug!(%socket_id, %channel, "subscribed");

                    // Send confirmation.
                    let _ = forward_tx.send(WsMessage {
                        event: "subscribed".into(),
                        channel: channel.to_string(),
                        data: serde_json::Value::Null,
                    });

                    // Spawn a forwarder that reads from the broadcast receiver
                    // and sends to the unified forward_tx.
                    let fwd = forward_tx.clone();
                    tokio::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(msg) => {
                                    if fwd.send(msg).is_err() {
                                        // Client disconnected.
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!(
                                        lagged = n,
                                        "broadcast receiver lagged, messages dropped"
                                    );
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    break;
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    let _ = forward_tx.send(WsMessage {
                        event: "error".into(),
                        channel: channel.to_string(),
                        data: serde_json::json!({"message": e.to_string()}),
                    });
                }
            }
        }

        "unsubscribe" => {
            let channel = match parsed.get("channel").and_then(|v| v.as_str()) {
                Some(ch) => ch,
                None => {
                    let _ = forward_tx.send(WsMessage {
                        event: "error".into(),
                        channel: String::new(),
                        data: serde_json::json!({"message": "unsubscribe requires 'channel' field"}),
                    });
                    return;
                }
            };

            registry.unsubscribe(socket_id, channel).await;
            tracing::debug!(%socket_id, %channel, "unsubscribed");
        }

        "ping" => {
            let _ = forward_tx.send(WsMessage {
                event: "pong".into(),
                channel: String::new(),
                data: serde_json::Value::Null,
            });
        }

        _ => {
            let _ = forward_tx.send(WsMessage {
                event: "error".into(),
                channel: String::new(),
                data: serde_json::json!({"message": format!("unknown event: {event}")}),
            });
        }
    }
}
