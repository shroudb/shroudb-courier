//! Delivery adapter trait and implementations (SMTP, webhook, SendGrid).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::delivery::{Channel, ContentType, DeliveryReceipt, DeliveryStatus, RenderedMessage};
use crate::error::CourierError;

/// Trait for delivery adapters. Each adapter handles one channel type.
#[async_trait]
pub trait DeliveryAdapter: Send + Sync {
    /// Human-readable name of this adapter.
    fn name(&self) -> &str;

    /// The channel this adapter handles.
    fn channel(&self) -> Channel;

    /// Deliver a rendered message to the given plaintext recipient.
    async fn deliver(
        &self,
        recipient: &str,
        message: &RenderedMessage,
    ) -> Result<DeliveryReceipt, CourierError>;
}

/// Registry of delivery adapters keyed by channel.
pub struct AdapterRegistry {
    adapters: HashMap<Channel, Box<dyn DeliveryAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
        }
    }

    pub fn register(&mut self, adapter: Box<dyn DeliveryAdapter>) {
        let channel = adapter.channel();
        self.adapters.insert(channel, adapter);
    }

    pub fn get(&self, channel: Channel) -> Option<&dyn DeliveryAdapter> {
        self.adapters.get(&channel).map(AsRef::as_ref)
    }

    pub fn list(&self) -> Vec<(Channel, &str)> {
        self.adapters
            .iter()
            .map(|(ch, adapter)| (*ch, adapter.name()))
            .collect()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper: generate a delivery receipt.
fn make_receipt(
    adapter_name: &str,
    channel: Channel,
    status: DeliveryStatus,
    error: Option<String>,
) -> DeliveryReceipt {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    DeliveryReceipt {
        delivery_id: uuid::Uuid::new_v4().to_string(),
        channel,
        adapter: adapter_name.to_string(),
        status,
        delivered_at: now,
        error,
    }
}

// ---------------------------------------------------------------------------
// SMTP adapter
// ---------------------------------------------------------------------------

/// SMTP delivery adapter using `lettre`.
pub struct SmtpAdapter {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from_address: String,
}

impl SmtpAdapter {
    /// Create a new SMTP adapter.
    ///
    /// `host` — SMTP server hostname.
    /// `port` — SMTP server port (e.g. 587 for STARTTLS).
    /// `username` / `password` — SMTP credentials (optional for relay).
    /// `from_address` — The From: address to use.
    /// `starttls` — Whether to use STARTTLS.
    pub fn new(
        host: &str,
        port: u16,
        username: Option<&str>,
        password: Option<&str>,
        from_address: &str,
        starttls: bool,
    ) -> Result<Self, CourierError> {
        use lettre::transport::smtp::authentication::Credentials;

        let builder = if starttls {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(host)
                .map_err(|e| {
                    CourierError::DeliveryFailed(format!("SMTP STARTTLS relay error: {e}"))
                })?
                .port(port)
        } else {
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(host)
                .map_err(|e| CourierError::DeliveryFailed(format!("SMTP relay error: {e}")))?
                .port(port)
        };

        let builder = if let (Some(user), Some(pass)) = (username, password) {
            builder.credentials(Credentials::new(user.to_string(), pass.to_string()))
        } else {
            builder
        };

        let transport = builder.build();

        Ok(Self {
            transport,
            from_address: from_address.to_string(),
        })
    }
}

#[async_trait]
impl DeliveryAdapter for SmtpAdapter {
    fn name(&self) -> &str {
        "smtp"
    }

    fn channel(&self) -> Channel {
        Channel::Email
    }

    async fn deliver(
        &self,
        recipient: &str,
        message: &RenderedMessage,
    ) -> Result<DeliveryReceipt, CourierError> {
        use lettre::AsyncTransport;
        use lettre::message::{Message, SinglePart, header::ContentType as LettreContentType};

        let subject = message.subject.as_deref().unwrap_or("(no subject)");

        let content_type = match message.content_type {
            ContentType::Html => LettreContentType::TEXT_HTML,
            ContentType::Plain => LettreContentType::TEXT_PLAIN,
        };

        let email =
            Message::builder()
                .from(self.from_address.parse().map_err(|e| {
                    CourierError::DeliveryFailed(format!("invalid from address: {e}"))
                })?)
                .to(recipient
                    .parse()
                    .map_err(|e| CourierError::DeliveryFailed(format!("invalid recipient: {e}")))?)
                .subject(subject)
                .singlepart(
                    SinglePart::builder()
                        .content_type(content_type)
                        .body(message.body.clone()),
                )
                .map_err(|e| CourierError::DeliveryFailed(format!("failed to build email: {e}")))?;

        match self.transport.send(email).await {
            Ok(_) => Ok(make_receipt(
                "smtp",
                Channel::Email,
                DeliveryStatus::Delivered,
                None,
            )),
            Err(e) => Ok(make_receipt(
                "smtp",
                Channel::Email,
                DeliveryStatus::Failed,
                Some(e.to_string()),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Webhook adapter
// ---------------------------------------------------------------------------

/// Webhook delivery adapter. The decrypted recipient IS the webhook URL.
pub struct WebhookAdapter {
    client: reqwest::Client,
}

impl WebhookAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for WebhookAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeliveryAdapter for WebhookAdapter {
    fn name(&self) -> &str {
        "webhook"
    }

    fn channel(&self) -> Channel {
        Channel::Webhook
    }

    async fn deliver(
        &self,
        recipient: &str,
        message: &RenderedMessage,
    ) -> Result<DeliveryReceipt, CourierError> {
        // recipient is the webhook URL
        let payload = serde_json::json!({
            "recipient": recipient,
            "subject": message.subject,
            "body": message.body,
            "channel": "webhook",
        });

        match self.client.post(recipient).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => Ok(make_receipt(
                "webhook",
                Channel::Webhook,
                DeliveryStatus::Delivered,
                None,
            )),
            Ok(resp) => Ok(make_receipt(
                "webhook",
                Channel::Webhook,
                DeliveryStatus::Failed,
                Some(format!("HTTP {}", resp.status())),
            )),
            Err(e) => Ok(make_receipt(
                "webhook",
                Channel::Webhook,
                DeliveryStatus::Failed,
                Some(e.to_string()),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// SendGrid adapter
// ---------------------------------------------------------------------------

/// SendGrid email delivery adapter.
pub struct SendGridAdapter {
    client: reqwest::Client,
    api_key: String,
    from_email: String,
    from_name: Option<String>,
}

impl SendGridAdapter {
    pub fn new(api_key: &str, from_email: &str, from_name: Option<&str>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            from_email: from_email.to_string(),
            from_name: from_name.map(String::from),
        }
    }
}

#[async_trait]
impl DeliveryAdapter for SendGridAdapter {
    fn name(&self) -> &str {
        "sendgrid"
    }

    fn channel(&self) -> Channel {
        Channel::Email
    }

    async fn deliver(
        &self,
        recipient: &str,
        message: &RenderedMessage,
    ) -> Result<DeliveryReceipt, CourierError> {
        let subject = message.subject.as_deref().unwrap_or("(no subject)");

        let content_type = match message.content_type {
            ContentType::Html => "text/html",
            ContentType::Plain => "text/plain",
        };

        let from = if let Some(ref name) = self.from_name {
            serde_json::json!({ "email": self.from_email, "name": name })
        } else {
            serde_json::json!({ "email": self.from_email })
        };

        let payload = serde_json::json!({
            "personalizations": [{
                "to": [{ "email": recipient }],
            }],
            "from": from,
            "subject": subject,
            "content": [{
                "type": content_type,
                "value": message.body,
            }],
        });

        match self
            .client
            .post("https://api.sendgrid.com/v3/mail/send")
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => Ok(make_receipt(
                "sendgrid",
                Channel::Email,
                DeliveryStatus::Delivered,
                None,
            )),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Ok(make_receipt(
                    "sendgrid",
                    Channel::Email,
                    DeliveryStatus::Failed,
                    Some(format!("HTTP {status}: {body}")),
                ))
            }
            Err(e) => Ok(make_receipt(
                "sendgrid",
                Channel::Email,
                DeliveryStatus::Failed,
                Some(e.to_string()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockAdapter {
        should_fail: bool,
    }

    #[async_trait]
    impl DeliveryAdapter for MockAdapter {
        fn name(&self) -> &str {
            "mock"
        }

        fn channel(&self) -> Channel {
            Channel::Email
        }

        async fn deliver(
            &self,
            _recipient: &str,
            _message: &RenderedMessage,
        ) -> Result<DeliveryReceipt, CourierError> {
            if self.should_fail {
                Ok(make_receipt(
                    "mock",
                    Channel::Email,
                    DeliveryStatus::Failed,
                    Some("mock failure".into()),
                ))
            } else {
                Ok(make_receipt(
                    "mock",
                    Channel::Email,
                    DeliveryStatus::Delivered,
                    None,
                ))
            }
        }
    }

    #[tokio::test]
    async fn mock_adapter_delivers() {
        let adapter = MockAdapter { should_fail: false };
        let msg = RenderedMessage {
            subject: Some("Test".into()),
            body: "Hello".into(),
            content_type: ContentType::Plain,
        };
        let receipt = adapter.deliver("test@example.com", &msg).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);
        assert!(receipt.error.is_none());
        assert_eq!(receipt.adapter, "mock");
    }

    #[tokio::test]
    async fn mock_adapter_fails() {
        let adapter = MockAdapter { should_fail: true };
        let msg = RenderedMessage {
            subject: None,
            body: "Hello".into(),
            content_type: ContentType::Plain,
        };
        let receipt = adapter.deliver("test@example.com", &msg).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Failed);
        assert!(receipt.error.is_some());
    }

    #[test]
    fn adapter_registry_register_and_lookup() {
        let mut registry = AdapterRegistry::new();
        registry.register(Box::new(MockAdapter { should_fail: false }));
        assert!(registry.get(Channel::Email).is_some());
        assert!(registry.get(Channel::Webhook).is_none());

        let list = registry.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, "mock");
    }

    #[test]
    fn webhook_adapter_builds_request() {
        // Just verify the adapter can be constructed — actual HTTP not tested here.
        let adapter = WebhookAdapter::new();
        assert_eq!(adapter.name(), "webhook");
        assert_eq!(adapter.channel(), Channel::Webhook);
    }
}
