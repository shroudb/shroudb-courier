use shroudb_courier_core::{
    ContentType, CourierError, DeliveryReceipt, DeliveryStatus, RenderedMessage, SmtpConfig,
};
use shroudb_courier_engine::DeliveryAdapter;
use std::future::Future;
use std::pin::Pin;

pub struct SmtpAdapter {
    config: SmtpConfig,
}

impl SmtpAdapter {
    pub fn new(config: SmtpConfig) -> Self {
        Self { config }
    }
}

impl DeliveryAdapter for SmtpAdapter {
    fn deliver<'a>(
        &'a self,
        recipient: &'a str,
        message: &'a RenderedMessage,
    ) -> Pin<Box<dyn Future<Output = Result<DeliveryReceipt, CourierError>> + Send + 'a>> {
        Box::pin(async move {
            use lettre::message::{Mailbox, SinglePart, header};
            use lettre::transport::smtp::authentication::Credentials;
            use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

            let from: Mailbox =
                self.config.from_address.parse().map_err(|e| {
                    CourierError::DeliveryFailed(format!("invalid from address: {e}"))
                })?;

            let to: Mailbox = recipient
                .parse()
                .map_err(|e| CourierError::DeliveryFailed(format!("invalid recipient: {e}")))?;

            let subject = message.subject.as_deref().unwrap_or("(no subject)");

            let body_part = match message.content_type {
                ContentType::Html => SinglePart::builder()
                    .header(header::ContentType::TEXT_HTML)
                    .body(message.body.clone()),
                ContentType::Plain => SinglePart::builder()
                    .header(header::ContentType::TEXT_PLAIN)
                    .body(message.body.clone()),
            };

            let email = Message::builder()
                .from(from)
                .to(to)
                .subject(subject)
                .singlepart(body_part)
                .map_err(|e| CourierError::DeliveryFailed(format!("email build failed: {e}")))?;

            let mut transport_builder = if self.config.starttls {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.config.host)
                    .map_err(|e| CourierError::DeliveryFailed(format!("SMTP relay error: {e}")))?
            } else {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&self.config.host)
                    .map_err(|e| CourierError::DeliveryFailed(format!("SMTP relay error: {e}")))?
            };

            transport_builder = transport_builder.port(self.config.port);

            if let (Some(username), Some(password)) = (&self.config.username, &self.config.password)
            {
                transport_builder = transport_builder
                    .credentials(Credentials::new(username.clone(), password.clone()));
            }

            let transport = transport_builder.build();

            transport
                .send(email)
                .await
                .map_err(|e| CourierError::DeliveryFailed(format!("SMTP send failed: {e}")))?;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            Ok(DeliveryReceipt {
                delivery_id: uuid::Uuid::new_v4().to_string(),
                channel: "email".into(),
                status: DeliveryStatus::Delivered,
                delivered_at: now,
                error: None,
            })
        })
    }
}

pub struct WebhookAdapter;

impl WebhookAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl DeliveryAdapter for WebhookAdapter {
    fn deliver<'a>(
        &'a self,
        recipient: &'a str,
        message: &'a RenderedMessage,
    ) -> Pin<Box<dyn Future<Output = Result<DeliveryReceipt, CourierError>> + Send + 'a>> {
        Box::pin(async move {
            let client = reqwest::Client::new();

            let body = serde_json::json!({
                "subject": message.subject,
                "body": message.body,
                "content_type": message.content_type.to_string(),
            });

            let response = client
                .post(recipient)
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    CourierError::DeliveryFailed(format!("webhook request failed: {e}"))
                })?;

            let status = response.status();
            if !status.is_success() {
                return Err(CourierError::DeliveryFailed(format!(
                    "webhook returned HTTP {status}"
                )));
            }

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            Ok(DeliveryReceipt {
                delivery_id: uuid::Uuid::new_v4().to_string(),
                channel: "webhook".into(),
                status: DeliveryStatus::Delivered,
                delivered_at: now,
                error: None,
            })
        })
    }
}
