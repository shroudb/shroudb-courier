use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Plain,
    Html,
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentType::Plain => write!(f, "plain"),
            ContentType::Html => write!(f, "html"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRequest {
    pub channel: String,
    pub recipient: String,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub body_encrypted: Option<String>,
    pub content_type: Option<ContentType>,
}

impl DeliveryRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.channel.is_empty() {
            return Err("channel is required".into());
        }
        if self.recipient.is_empty() {
            return Err("recipient is required".into());
        }
        if self.body.is_none() && self.body_encrypted.is_none() {
            return Err("body or body_encrypted is required".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedMessage {
    pub subject: Option<String>,
    pub body: String,
    pub content_type: ContentType,
}

impl Drop for RenderedMessage {
    fn drop(&mut self) {
        self.body.zeroize();
        if let Some(ref mut s) = self.subject {
            s.zeroize();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    Delivered,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub delivery_id: String,
    pub channel: String,
    pub status: DeliveryStatus,
    pub delivered_at: u64,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delivery_request_validate_valid() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "encrypted-recipient".into(),
            subject: Some("Hello".into()),
            body: Some("world".into()),
            body_encrypted: None,
            content_type: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_delivery_request_validate_encrypted_body() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "encrypted-url".into(),
            subject: None,
            body: None,
            body_encrypted: Some("cipher:abc123".into()),
            content_type: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_delivery_request_validate_missing_channel() {
        let req = DeliveryRequest {
            channel: String::new(),
            recipient: "x".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_delivery_request_validate_missing_recipient() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: String::new(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_delivery_request_validate_no_body() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "x".into(),
            subject: None,
            body: None,
            body_encrypted: None,
            content_type: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_delivery_receipt_serialization() {
        let receipt = DeliveryReceipt {
            delivery_id: "abc-123".into(),
            channel: "email".into(),
            status: DeliveryStatus::Delivered,
            delivered_at: 1000,
            error: None,
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let deserialized: DeliveryReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.delivery_id, "abc-123");
        assert_eq!(deserialized.status, DeliveryStatus::Delivered);
    }

    #[test]
    fn test_content_type_display() {
        assert_eq!(ContentType::Plain.to_string(), "plain");
        assert_eq!(ContentType::Html.to_string(), "html");
    }

    #[test]
    fn test_content_type_serde_roundtrip() {
        for ct in [ContentType::Plain, ContentType::Html] {
            let json = serde_json::to_string(&ct).unwrap();
            let parsed: ContentType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, ct);
        }
    }

    #[test]
    fn test_delivery_status_serde_roundtrip() {
        for status in [DeliveryStatus::Delivered, DeliveryStatus::Failed] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: DeliveryStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_delivery_status_json_values() {
        assert_eq!(
            serde_json::to_string(&DeliveryStatus::Delivered).unwrap(),
            "\"delivered\""
        );
        assert_eq!(
            serde_json::to_string(&DeliveryStatus::Failed).unwrap(),
            "\"failed\""
        );
    }

    #[test]
    fn test_delivery_receipt_failed_with_error() {
        let receipt = DeliveryReceipt {
            delivery_id: "fail-001".into(),
            channel: "email".into(),
            status: DeliveryStatus::Failed,
            delivered_at: 2000,
            error: Some("SMTP connection refused".into()),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let parsed: DeliveryReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, DeliveryStatus::Failed);
        assert_eq!(parsed.error.as_deref(), Some("SMTP connection refused"));
    }

    #[test]
    fn test_delivery_request_with_content_type() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "user@example.com".into(),
            subject: Some("Alert".into()),
            body: Some("<h1>Alert</h1>".into()),
            body_encrypted: None,
            content_type: Some(ContentType::Html),
        };
        assert!(req.validate().is_ok());
        let json = serde_json::to_string(&req).unwrap();
        let parsed: DeliveryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_type, Some(ContentType::Html));
    }

    #[test]
    fn test_delivery_request_serde_roundtrip() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "https://example.com/hook".into(),
            subject: None,
            body: Some(r#"{"event":"test"}"#.into()),
            body_encrypted: None,
            content_type: Some(ContentType::Plain),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: DeliveryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.channel, "webhook");
        assert_eq!(parsed.recipient, "https://example.com/hook");
        assert!(parsed.subject.is_none());
        assert_eq!(parsed.body.as_deref(), Some(r#"{"event":"test"}"#));
    }

    #[test]
    fn test_rendered_message_serde() {
        let msg = RenderedMessage {
            subject: Some("Test Subject".into()),
            body: "Hello, world!".into(),
            content_type: ContentType::Plain,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: RenderedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.subject.as_deref(), Some("Test Subject"));
        assert_eq!(parsed.body, "Hello, world!");
        assert_eq!(parsed.content_type, ContentType::Plain);
    }
}
