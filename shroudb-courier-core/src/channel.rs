use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelType {
    Email,
    Webhook,
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelType::Email => write!(f, "email"),
            ChannelType::Webhook => write!(f, "webhook"),
        }
    }
}

impl std::str::FromStr for ChannelType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "email" => Ok(ChannelType::Email),
            "webhook" => Ok(ChannelType::Webhook),
            other => Err(format!("unknown channel type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: String,
    pub starttls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub default_method: Option<String>,
    pub default_headers: Option<std::collections::HashMap<String, String>>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub channel_type: ChannelType,
    pub smtp: Option<SmtpConfig>,
    pub webhook: Option<WebhookConfig>,
    pub enabled: bool,
    pub created_at: u64,
    /// Default recipient for event notifications (e.g. rotation/expiry alerts).
    /// When set, `notify_event` uses this instead of requiring a per-request recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_recipient: Option<String>,
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    if name.len() > 255 {
        return Err("name cannot exceed 255 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "name must contain only alphanumeric characters, hyphens, or underscores".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_type_display() {
        assert_eq!(ChannelType::Email.to_string(), "email");
        assert_eq!(ChannelType::Webhook.to_string(), "webhook");
    }

    #[test]
    fn test_channel_type_parse() {
        assert_eq!("email".parse::<ChannelType>().unwrap(), ChannelType::Email);
        assert_eq!(
            "WEBHOOK".parse::<ChannelType>().unwrap(),
            ChannelType::Webhook
        );
        assert!("unknown".parse::<ChannelType>().is_err());
    }

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_name("my-channel").is_ok());
        assert!(validate_name("email_prod").is_ok());
        assert!(validate_name("a").is_ok());
        assert!(validate_name("Channel123").is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        assert!(validate_name("").is_err());
        assert!(validate_name("has spaces").is_err());
        assert!(validate_name("has.dots").is_err());
        assert!(validate_name("has/slash").is_err());
        let long = "a".repeat(256);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn test_channel_serialization() {
        let channel = Channel {
            name: "test".into(),
            channel_type: ChannelType::Email,
            smtp: Some(SmtpConfig {
                host: "smtp.example.com".into(),
                port: 587,
                username: None,
                password: None,
                from_address: "noreply@example.com".into(),
                starttls: true,
            }),
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        let json = serde_json::to_string(&channel).unwrap();
        let deserialized: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.channel_type, ChannelType::Email);
        assert!(deserialized.smtp.is_some());
    }

    #[test]
    fn test_validate_name_boundary_255() {
        let name_255 = "a".repeat(255);
        assert!(validate_name(&name_255).is_ok());
        let name_256 = "a".repeat(256);
        assert!(validate_name(&name_256).is_err());
    }

    #[test]
    fn test_validate_name_special_chars() {
        assert!(validate_name("has@sign").is_err());
        assert!(validate_name("has!bang").is_err());
        assert!(validate_name("has:colon").is_err());
        assert!(validate_name("has#hash").is_err());
    }

    #[test]
    fn test_channel_type_serde_roundtrip() {
        for ct in [ChannelType::Email, ChannelType::Webhook] {
            let json = serde_json::to_string(&ct).unwrap();
            let parsed: ChannelType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, ct);
        }
    }

    #[test]
    fn test_channel_type_display_parse_roundtrip() {
        for ct in [ChannelType::Email, ChannelType::Webhook] {
            let display = ct.to_string();
            let parsed: ChannelType = display.parse().unwrap();
            assert_eq!(parsed, ct);
        }
    }

    #[test]
    fn test_webhook_channel_serialization() {
        let channel = Channel {
            name: "alerts-webhook".into(),
            channel_type: ChannelType::Webhook,
            smtp: None,
            webhook: Some(WebhookConfig {
                default_method: Some("POST".into()),
                default_headers: Some(
                    [("Content-Type".into(), "application/json".into())]
                        .into_iter()
                        .collect(),
                ),
                timeout_secs: Some(30),
            }),
            enabled: true,
            created_at: 2000,
            default_recipient: None,
        };
        let json = serde_json::to_string(&channel).unwrap();
        let deserialized: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "alerts-webhook");
        assert_eq!(deserialized.channel_type, ChannelType::Webhook);
        assert!(deserialized.webhook.is_some());
        let wh = deserialized.webhook.unwrap();
        assert_eq!(wh.default_method.as_deref(), Some("POST"));
        assert_eq!(wh.timeout_secs, Some(30));
    }

    #[test]
    fn test_channel_disabled() {
        let channel = Channel {
            name: "disabled-channel".into(),
            channel_type: ChannelType::Email,
            smtp: None,
            webhook: None,
            enabled: false,
            created_at: 0,
            default_recipient: None,
        };
        let json = serde_json::to_string(&channel).unwrap();
        let deserialized: Channel = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.enabled);
    }
}
