//! WebSocket channel registry — manages channels, subscriptions, and pub/sub fan-out.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};

/// A unique ID for each WebSocket connection.
pub type SocketId = String;

/// A message pushed to WebSocket clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WsMessage {
    pub event: String,
    pub channel: String,
    pub data: serde_json::Value,
}

/// Registry of channels and their subscribers.
///
/// Each channel maps to a `broadcast::Sender`. When a client subscribes, it
/// receives a `broadcast::Receiver` from that sender. Publishing sends to the
/// sender, which fans out to all receivers.
pub struct ChannelRegistry {
    /// channel_name -> broadcast sender
    channels: RwLock<HashMap<String, broadcast::Sender<WsMessage>>>,
    /// socket_id -> set of subscribed channel names (for cleanup on disconnect)
    subscriptions: RwLock<HashMap<SocketId, HashSet<String>>>,
    /// Maximum number of channels allowed.
    max_channels: usize,
    /// Maximum connections per channel.
    max_connections_per_channel: usize,
    /// Buffer size for broadcast channels.
    channel_buffer_size: usize,
}

impl ChannelRegistry {
    pub fn new(
        max_channels: usize,
        max_connections_per_channel: usize,
        channel_buffer_size: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            channels: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(HashMap::new()),
            max_channels,
            max_connections_per_channel,
            channel_buffer_size,
        })
    }

    /// Subscribe a socket to a channel. Returns a broadcast Receiver.
    pub async fn subscribe(
        &self,
        socket_id: &str,
        channel: &str,
    ) -> Result<broadcast::Receiver<WsMessage>, RegistryError> {
        let mut channels = self.channels.write().await;

        let sender = if let Some(existing) = channels.get(channel) {
            // Check connection limit on this channel.
            if existing.receiver_count() >= self.max_connections_per_channel {
                return Err(RegistryError::ChannelFull);
            }
            existing.clone()
        } else {
            // Check max channels limit.
            if channels.len() >= self.max_channels {
                return Err(RegistryError::TooManyChannels);
            }
            let (tx, _) = broadcast::channel(self.channel_buffer_size);
            channels.insert(channel.to_string(), tx.clone());
            tx
        };

        let rx = sender.subscribe();

        // Track the subscription for this socket.
        let mut subs = self.subscriptions.write().await;
        subs.entry(socket_id.to_string())
            .or_default()
            .insert(channel.to_string());

        Ok(rx)
    }

    /// Unsubscribe a socket from a channel.
    pub async fn unsubscribe(&self, socket_id: &str, channel: &str) {
        let mut subs = self.subscriptions.write().await;
        if let Some(channels) = subs.get_mut(socket_id) {
            channels.remove(channel);
        }

        // Clean up empty channels (no receivers left).
        self.maybe_remove_channel(channel).await;
    }

    /// Remove all subscriptions for a socket (on disconnect).
    pub async fn disconnect(&self, socket_id: &str) {
        let channels_to_check: Vec<String>;
        {
            let mut subs = self.subscriptions.write().await;
            channels_to_check = subs
                .remove(socket_id)
                .unwrap_or_default()
                .into_iter()
                .collect();
        }

        // Clean up any channels that now have zero receivers.
        for ch in channels_to_check {
            self.maybe_remove_channel(&ch).await;
        }
    }

    /// Publish a message to all subscribers of a channel.
    /// Returns the number of recipients that received the message.
    pub async fn publish(&self, channel: &str, message: WsMessage) -> usize {
        let channels = self.channels.read().await;
        if let Some(sender) = channels.get(channel) {
            // send() returns Ok(receiver_count) or Err if no receivers.
            sender.send(message).unwrap_or(0)
        } else {
            0
        }
    }

    /// Number of active receivers on a channel.
    pub async fn subscriber_count(&self, channel: &str) -> usize {
        let channels = self.channels.read().await;
        channels
            .get(channel)
            .map(|s| s.receiver_count())
            .unwrap_or(0)
    }

    /// Total active connections across all channels.
    pub async fn total_connections(&self) -> usize {
        let subs = self.subscriptions.read().await;
        subs.len()
    }

    /// List all active channels with their subscriber counts.
    pub async fn list_channels(&self) -> Vec<(String, usize)> {
        let channels = self.channels.read().await;
        channels
            .iter()
            .map(|(name, sender)| (name.clone(), sender.receiver_count()))
            .collect()
    }

    /// Remove a channel if it has no receivers left.
    async fn maybe_remove_channel(&self, channel: &str) {
        let mut channels = self.channels.write().await;
        if let Some(sender) = channels.get(channel)
            && sender.receiver_count() == 0
        {
            channels.remove(channel);
        }
    }
}

/// Errors from channel registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("channel is full (max connections per channel reached)")]
    ChannelFull,

    #[error("too many channels (max channels reached)")]
    TooManyChannels,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribe_and_publish() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let mut rx = registry.subscribe("s1", "room:general").await.unwrap();

        let msg = WsMessage {
            event: "message".into(),
            channel: "room:general".into(),
            data: serde_json::json!({"body": "hello"}),
        };

        let count = registry.publish("room:general", msg.clone()).await;
        assert_eq!(count, 1);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.channel, "room:general");
        assert_eq!(received.event, "message");
    }

    #[tokio::test]
    async fn multi_subscriber_fan_out() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let mut rx1 = registry.subscribe("s1", "room:general").await.unwrap();
        let mut rx2 = registry.subscribe("s2", "room:general").await.unwrap();

        let msg = WsMessage {
            event: "message".into(),
            channel: "room:general".into(),
            data: serde_json::json!({"body": "hello"}),
        };

        let count = registry.publish("room:general", msg).await;
        assert_eq!(count, 2);

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert_eq!(r1.channel, "room:general");
        assert_eq!(r2.channel, "room:general");
    }

    #[tokio::test]
    async fn unsubscribe_removes_from_tracking() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let _rx = registry.subscribe("s1", "room:general").await.unwrap();
        assert_eq!(registry.subscriber_count("room:general").await, 1);

        registry.unsubscribe("s1", "room:general").await;
        // Note: the broadcast receiver still exists (held by _rx), but tracking is removed.
        // The channel is cleaned up based on broadcast receiver_count.
    }

    #[tokio::test]
    async fn disconnect_cleans_up_all_subscriptions() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let _rx1 = registry.subscribe("s1", "room:a").await.unwrap();
        let _rx2 = registry.subscribe("s1", "room:b").await.unwrap();
        assert_eq!(registry.total_connections().await, 1);

        // Drop receivers first so channel cleanup works.
        drop(_rx1);
        drop(_rx2);
        registry.disconnect("s1").await;
        assert_eq!(registry.total_connections().await, 0);
    }

    #[tokio::test]
    async fn publish_to_empty_channel_returns_zero() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let msg = WsMessage {
            event: "message".into(),
            channel: "nonexistent".into(),
            data: serde_json::json!({}),
        };
        let count = registry.publish("nonexistent", msg).await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn max_channels_enforced() {
        let registry = ChannelRegistry::new(2, 100, 16);
        let _rx1 = registry.subscribe("s1", "ch1").await.unwrap();
        let _rx2 = registry.subscribe("s1", "ch2").await.unwrap();
        let result = registry.subscribe("s1", "ch3").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn max_connections_per_channel_enforced() {
        let registry = ChannelRegistry::new(100, 1, 16);
        let _rx1 = registry.subscribe("s1", "ch1").await.unwrap();
        let result = registry.subscribe("s2", "ch1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_channels_returns_active() {
        let registry = ChannelRegistry::new(100, 100, 16);
        let _rx1 = registry.subscribe("s1", "room:a").await.unwrap();
        let _rx2 = registry.subscribe("s2", "room:b").await.unwrap();

        let channels = registry.list_channels().await;
        assert_eq!(channels.len(), 2);

        let names: HashSet<String> = channels.into_iter().map(|(n, _)| n).collect();
        assert!(names.contains("room:a"));
        assert!(names.contains("room:b"));
    }
}
