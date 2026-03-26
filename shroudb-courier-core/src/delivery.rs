//! Core delivery types for Courier.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Notification channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Email,
    Sms,
    Webhook,
    Push,
    Ws,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Email => write!(f, "email"),
            Channel::Sms => write!(f, "sms"),
            Channel::Webhook => write!(f, "webhook"),
            Channel::Push => write!(f, "push"),
            Channel::Ws => write!(f, "ws"),
        }
    }
}

/// Content type of a rendered message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Plain,
    Html,
}

/// A rendered message ready for delivery.
#[derive(Debug)]
pub struct RenderedMessage {
    pub subject: Option<String>,
    pub body: String,
    pub content_type: ContentType,
}

/// An incoming delivery request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRequest {
    pub channel: Channel,
    /// Transit-encrypted recipient (ciphertext).
    pub recipient: String,
    /// Template name to render (optional if body is pre-rendered).
    pub template: Option<String>,
    /// Template variables.
    pub vars: Option<HashMap<String, serde_json::Value>>,
    /// Pre-rendered subject (used when no template is specified).
    pub subject: Option<String>,
    /// Pre-rendered body (used when no template is specified).
    pub body: Option<String>,
}

/// Status of a completed delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    Delivered,
    Failed,
}

impl std::fmt::Display for DeliveryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryStatus::Delivered => write!(f, "delivered"),
            DeliveryStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Receipt returned after a delivery attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub delivery_id: String,
    pub channel: Channel,
    pub adapter: String,
    pub status: DeliveryStatus,
    pub delivered_at: u64,
    pub error: Option<String>,
    /// Number of recipients that received the message (WebSocket fan-out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipients: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_ws_serde_roundtrip() {
        let json = serde_json::to_string(&Channel::Ws).unwrap();
        assert_eq!(json, "\"ws\"");

        let parsed: Channel = serde_json::from_str("\"ws\"").unwrap();
        assert_eq!(parsed, Channel::Ws);
    }

    #[test]
    fn channel_ws_display() {
        assert_eq!(Channel::Ws.to_string(), "ws");
    }

    #[test]
    fn delivery_receipt_with_recipients() {
        let receipt = DeliveryReceipt {
            delivery_id: "test-id".into(),
            channel: Channel::Ws,
            adapter: "websocket".into(),
            status: DeliveryStatus::Delivered,
            delivered_at: 1234567890,
            error: None,
            recipients: Some(5),
        };

        let json = serde_json::to_string(&receipt).unwrap();
        assert!(json.contains("\"recipients\":5"));

        let parsed: DeliveryReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.recipients, Some(5));
        assert_eq!(parsed.channel, Channel::Ws);
    }

    #[test]
    fn delivery_receipt_without_recipients_omits_field() {
        let receipt = DeliveryReceipt {
            delivery_id: "test-id".into(),
            channel: Channel::Email,
            adapter: "smtp".into(),
            status: DeliveryStatus::Delivered,
            delivered_at: 1234567890,
            error: None,
            recipients: None,
        };

        let json = serde_json::to_string(&receipt).unwrap();
        assert!(!json.contains("recipients"));
    }

    #[test]
    fn delivery_request_ws_channel() {
        let json = r#"{"channel":"ws","recipient":"enc...","body":"hello"}"#;
        let req: DeliveryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.channel, Channel::Ws);
    }
}
