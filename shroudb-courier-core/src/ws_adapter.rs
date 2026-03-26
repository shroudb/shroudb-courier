//! WebSocket delivery adapter — pushes rendered messages to connected WebSocket clients.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::adapter::DeliveryAdapter;
use crate::delivery::{Channel, DeliveryReceipt, DeliveryStatus, RenderedMessage};
use crate::error::CourierError;
use crate::ws::{ChannelRegistry, WsMessage};

/// Adapter that delivers messages to WebSocket clients subscribed to a channel.
///
/// The decrypted recipient is the channel name (e.g., "user:alice"). The adapter
/// publishes the rendered message to all clients subscribed to that channel.
pub struct WsAdapter {
    registry: Arc<ChannelRegistry>,
}

impl WsAdapter {
    pub fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl DeliveryAdapter for WsAdapter {
    fn name(&self) -> &str {
        "websocket"
    }

    fn channel(&self) -> Channel {
        Channel::Ws
    }

    async fn deliver(
        &self,
        recipient: &str,
        message: &RenderedMessage,
    ) -> Result<DeliveryReceipt, CourierError> {
        let ws_msg = WsMessage {
            event: "message".into(),
            channel: recipient.into(),
            data: serde_json::json!({
                "subject": message.subject,
                "body": message.body,
            }),
        };

        let count = self.registry.publish(recipient, ws_msg).await;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(DeliveryReceipt {
            delivery_id: uuid::Uuid::new_v4().to_string(),
            channel: Channel::Ws,
            adapter: "websocket".into(),
            // Delivered even with 0 subscribers — the message was accepted and broadcast.
            status: DeliveryStatus::Delivered,
            delivered_at: now,
            error: None,
            recipients: Some(count),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::ContentType;

    #[tokio::test]
    async fn deliver_to_channel_with_subscribers() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let mut rx = registry.subscribe("s1", "user:alice").await.unwrap();

        let adapter = WsAdapter::new(Arc::clone(&registry));
        let msg = RenderedMessage {
            subject: Some("New message".into()),
            body: "Hello Alice".into(),
            content_type: ContentType::Plain,
        };

        let receipt = adapter.deliver("user:alice", &msg).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);
        assert_eq!(receipt.channel, Channel::Ws);
        assert_eq!(receipt.adapter, "websocket");
        assert_eq!(receipt.recipients, Some(1));

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event, "message");
        assert_eq!(received.channel, "user:alice");
    }

    #[tokio::test]
    async fn deliver_with_zero_subscribers() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let adapter = WsAdapter::new(registry);
        let msg = RenderedMessage {
            subject: None,
            body: "Hello".into(),
            content_type: ContentType::Plain,
        };

        let receipt = adapter.deliver("user:nobody", &msg).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);
        assert_eq!(receipt.recipients, Some(0));
    }

    #[tokio::test]
    async fn deliver_multi_subscriber() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let _rx1 = registry.subscribe("s1", "room:general").await.unwrap();
        let _rx2 = registry.subscribe("s2", "room:general").await.unwrap();

        let adapter = WsAdapter::new(Arc::clone(&registry));
        let msg = RenderedMessage {
            subject: Some("Announcement".into()),
            body: "Hello everyone".into(),
            content_type: ContentType::Plain,
        };

        let receipt = adapter.deliver("room:general", &msg).await.unwrap();
        assert_eq!(receipt.recipients, Some(2));
    }
}
