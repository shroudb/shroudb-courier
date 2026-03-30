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
        };
        let json = serde_json::to_string(&channel).unwrap();
        let deserialized: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.channel_type, ChannelType::Email);
        assert!(deserialized.smtp.is_some());
    }
}
