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
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Email => write!(f, "email"),
            Channel::Sms => write!(f, "sms"),
            Channel::Webhook => write!(f, "webhook"),
            Channel::Push => write!(f, "push"),
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
}
