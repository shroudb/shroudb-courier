use serde::{Deserialize, Serialize};

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
}
