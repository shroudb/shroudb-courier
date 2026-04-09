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

pub struct WebhookAdapter {
    /// Optional HMAC-SHA256 signing secret for webhook request authentication.
    /// When set, each POST includes an `X-ShrouDB-Signature` header containing
    /// the hex-encoded HMAC-SHA256 of the request body.
    signing_secret: Option<Vec<u8>>,
}

impl WebhookAdapter {
    pub fn new() -> Self {
        Self {
            signing_secret: None,
        }
    }

    /// Create a webhook adapter with HMAC-SHA256 request signing.
    pub fn with_signing_secret(secret: Vec<u8>) -> Self {
        Self {
            signing_secret: Some(secret),
        }
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

            let body_bytes = serde_json::to_vec(&body).map_err(|e| {
                CourierError::DeliveryFailed(format!("JSON serialization failed: {e}"))
            })?;

            let mut request = client
                .post(recipient)
                .header("content-type", "application/json");

            // Sign the body if a signing secret is configured
            if let Some(ref secret) = self.signing_secret {
                let signature = shroudb_crypto::hmac_sign(
                    shroudb_crypto::HmacAlgorithm::Sha256,
                    secret,
                    &body_bytes,
                )
                .map_err(|e| CourierError::DeliveryFailed(format!("HMAC signing failed: {e}")))?;
                request = request.header(
                    "X-ShrouDB-Signature",
                    format!("sha256={}", hex::encode(&signature)),
                );
            }

            let response = request.body(body_bytes).send().await.map_err(|e| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_signature_computation() {
        // Verify HMAC-SHA256 signature matches expected value
        let secret = b"test-secret-key";
        let body = serde_json::json!({
            "subject": "Test Subject",
            "body": "Hello, world!",
            "content_type": "text/plain",
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();

        let signature =
            shroudb_crypto::hmac_sign(shroudb_crypto::HmacAlgorithm::Sha256, secret, &body_bytes)
                .unwrap();

        let header_value = format!("sha256={}", hex::encode(&signature));
        assert!(header_value.starts_with("sha256="));
        assert_eq!(header_value.len(), 7 + 64); // "sha256=" + 64 hex chars

        // Verify with the same function the recipient would use
        let verified = shroudb_crypto::hmac_verify(
            shroudb_crypto::HmacAlgorithm::Sha256,
            secret,
            &body_bytes,
            &signature,
        )
        .unwrap();
        assert!(verified, "recipient should be able to verify signature");

        // Tampered body fails verification
        let mut tampered = body_bytes.clone();
        tampered[0] ^= 0xFF;
        let tampered_verify = shroudb_crypto::hmac_verify(
            shroudb_crypto::HmacAlgorithm::Sha256,
            secret,
            &tampered,
            &signature,
        )
        .unwrap();
        assert!(!tampered_verify, "tampered body should fail verification");
    }

    #[test]
    fn webhook_adapter_with_signing_secret_has_secret() {
        let adapter = WebhookAdapter::with_signing_secret(b"my-secret".to_vec());
        assert!(adapter.signing_secret.is_some());
    }

    #[test]
    fn webhook_adapter_new_has_no_secret() {
        let adapter = WebhookAdapter::new();
        assert!(adapter.signing_secret.is_none());
    }
}
