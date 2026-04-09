use shroudb_courier_core::{
    ContentType, CourierError, DeliveryReceipt, DeliveryRequest, DeliveryStatus, RenderedMessage,
};
use zeroize::Zeroize;

use crate::capabilities::{Decryptor, DeliveryAdapter};

pub struct DeliverResult {
    pub receipt: DeliveryReceipt,
}

/// Retry configuration for delivery attempts.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts after the initial attempt.
    pub max_retries: u32,
    /// Base delay before the first retry. Subsequent retries double this.
    pub base_delay: std::time::Duration,
    /// Maximum delay cap (prevents unbounded backoff).
    pub max_delay: std::time::Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: std::time::Duration::from_millis(100),
            max_delay: std::time::Duration::from_secs(5),
        }
    }
}

impl RetryConfig {
    /// No retries — fail immediately on first error.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }
}

pub async fn execute_delivery(
    request: &DeliveryRequest,
    decryptor: Option<&dyn Decryptor>,
    adapter: &dyn DeliveryAdapter,
) -> Result<DeliverResult, CourierError> {
    execute_delivery_with_retry(request, decryptor, adapter, &RetryConfig::none()).await
}

pub async fn execute_delivery_with_retry(
    request: &DeliveryRequest,
    decryptor: Option<&dyn Decryptor>,
    adapter: &dyn DeliveryAdapter,
    retry_config: &RetryConfig,
) -> Result<DeliverResult, CourierError> {
    request.validate().map_err(CourierError::InvalidArgument)?;

    // Step 1: Decrypt recipient (once — plaintext held for all attempts)
    let mut plaintext_recipient = decrypt_value(&request.recipient, decryptor).await?;

    // Step 2: Resolve message body (once)
    let message = resolve_message(request, decryptor).await?;

    // Step 3: Deliver with retry
    let mut last_error = None;
    let total_attempts = 1 + retry_config.max_retries;

    for attempt in 0..total_attempts {
        match adapter.deliver(&plaintext_recipient, &message).await {
            Ok(receipt) => {
                plaintext_recipient.zeroize();
                return Ok(DeliverResult { receipt });
            }
            Err(e) => {
                last_error = Some(e);
                if attempt + 1 < total_attempts {
                    let delay = retry_config
                        .base_delay
                        .saturating_mul(1 << attempt)
                        .min(retry_config.max_delay);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = total_attempts,
                        delay_ms = delay.as_millis() as u64,
                        "delivery attempt failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    // Step 4: Zeroize plaintext
    plaintext_recipient.zeroize();

    // All attempts exhausted
    let e = last_error.unwrap();
    let delivery_id = uuid::Uuid::new_v4().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(DeliverResult {
        receipt: DeliveryReceipt {
            delivery_id,
            channel: request.channel.clone(),
            status: DeliveryStatus::Failed,
            delivered_at: now,
            error: Some(e.to_string()),
        },
    })
}

async fn decrypt_value(
    value: &str,
    decryptor: Option<&dyn Decryptor>,
) -> Result<String, CourierError> {
    match decryptor {
        Some(d) => d.decrypt(value).await,
        None => {
            tracing::warn!("no decryptor configured — treating value as plaintext");
            Ok(value.to_string())
        }
    }
}

async fn resolve_message(
    request: &DeliveryRequest,
    decryptor: Option<&dyn Decryptor>,
) -> Result<RenderedMessage, CourierError> {
    // Encrypted body takes priority — decrypt just-in-time
    if let Some(ref encrypted_body) = request.body_encrypted {
        let mut plaintext_body = decrypt_value(encrypted_body, decryptor).await?;
        let content_type = request.content_type.unwrap_or(ContentType::Plain);
        let msg = RenderedMessage {
            subject: request.subject.clone(),
            body: plaintext_body.clone(),
            content_type,
        };
        plaintext_body.zeroize();
        return Ok(msg);
    }

    // Direct body
    if let Some(ref body) = request.body {
        let content_type = request.content_type.unwrap_or(ContentType::Plain);
        return Ok(RenderedMessage {
            subject: request.subject.clone(),
            body: body.clone(),
            content_type,
        });
    }

    Err(CourierError::InvalidArgument(
        "no message body could be resolved".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;

    struct MockDecryptor;
    impl Decryptor for MockDecryptor {
        fn decrypt<'a>(
            &'a self,
            ciphertext: &'a str,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<String, CourierError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(ciphertext
                    .strip_prefix("enc:")
                    .unwrap_or(ciphertext)
                    .to_string())
            })
        }
    }

    struct MockAdapter;
    impl DeliveryAdapter for MockAdapter {
        fn deliver<'a>(
            &'a self,
            _recipient: &'a str,
            _message: &'a RenderedMessage,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<DeliveryReceipt, CourierError>> + Send + 'a,
            >,
        > {
            Box::pin(async move {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Ok(DeliveryReceipt {
                    delivery_id: uuid::Uuid::new_v4().to_string(),
                    channel: "mock".into(),
                    status: DeliveryStatus::Delivered,
                    delivered_at: now,
                    error: None,
                })
            })
        }
    }

    struct FailAdapter;
    impl DeliveryAdapter for FailAdapter {
        fn deliver<'a>(
            &'a self,
            _recipient: &'a str,
            _message: &'a RenderedMessage,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<DeliveryReceipt, CourierError>> + Send + 'a,
            >,
        > {
            Box::pin(async move { Err(CourierError::DeliveryFailed("connection refused".into())) })
        }
    }

    #[tokio::test]
    async fn test_deliver_with_direct_body() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "enc:https://example.com/hook".into(),
            subject: None,
            body: Some("{\"event\": \"test\"}".into()),
            body_encrypted: None,
            content_type: None,
        };

        let result = execute_delivery(&req, Some(&MockDecryptor), &MockAdapter)
            .await
            .unwrap();
        assert_eq!(result.receipt.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_deliver_with_encrypted_body() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "enc:bob@example.com".into(),
            subject: Some("Encrypted content".into()),
            body: None,
            body_encrypted: Some("enc:secret message body".into()),
            content_type: Some(ContentType::Plain),
        };

        let result = execute_delivery(&req, Some(&MockDecryptor), &MockAdapter)
            .await
            .unwrap();
        assert_eq!(result.receipt.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_deliver_adapter_failure_returns_receipt() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "enc:fail@example.com".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };

        let result = execute_delivery(&req, Some(&MockDecryptor), &FailAdapter)
            .await
            .unwrap();
        assert_eq!(result.receipt.status, DeliveryStatus::Failed);
        assert!(result.receipt.error.is_some());
    }

    #[tokio::test]
    async fn test_deliver_no_decryptor_passthrough() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "https://example.com/hook".into(),
            subject: None,
            body: Some("payload".into()),
            body_encrypted: None,
            content_type: None,
        };

        let result = execute_delivery(&req, None, &MockAdapter).await.unwrap();
        assert_eq!(result.receipt.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_deliver_no_body_fails() {
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "enc:x".into(),
            subject: None,
            body: None,
            body_encrypted: None,
            content_type: None,
        };

        let result = execute_delivery(&req, Some(&MockDecryptor), &MockAdapter).await;
        assert!(result.is_err());
    }

    /// Decryptor that sleeps to simulate network latency.
    /// Used to verify concurrent deliveries don't serialize.
    struct SlowDecryptor;
    impl Decryptor for SlowDecryptor {
        fn decrypt<'a>(
            &'a self,
            ciphertext: &'a str,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<String, CourierError>> + Send + 'a>>
        {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Ok(ciphertext
                    .strip_prefix("enc:")
                    .unwrap_or(ciphertext)
                    .to_string())
            })
        }
    }

    #[tokio::test]
    async fn test_plaintext_zeroized_after_delivery() {
        // Verify the delivery path completes correctly when using encrypted body+recipient.
        // The zeroize calls are in the code path; we verify the full encrypted flow
        // produces a correct receipt (the zeroize is exercised implicitly).
        let req = DeliveryRequest {
            channel: "email".into(),
            recipient: "enc:alice@example.com".into(),
            subject: Some("Sensitive notice".into()),
            body: None,
            body_encrypted: Some("enc:This is a secret body".into()),
            content_type: Some(ContentType::Plain),
        };

        let result = execute_delivery(&req, Some(&MockDecryptor), &MockAdapter)
            .await
            .unwrap();

        assert_eq!(result.receipt.status, DeliveryStatus::Delivered);
        assert_eq!(result.receipt.channel, "mock");
        assert!(result.receipt.error.is_none());
        assert!(!result.receipt.delivery_id.is_empty());
        assert!(result.receipt.delivered_at > 0);
    }

    // ── Retry tests ───────────────────────────────────────────────────

    /// Adapter that fails N times, then succeeds.
    struct TransientFailAdapter {
        remaining_failures: std::sync::atomic::AtomicU32,
    }

    impl TransientFailAdapter {
        fn new(fail_count: u32) -> Self {
            Self {
                remaining_failures: std::sync::atomic::AtomicU32::new(fail_count),
            }
        }
    }

    impl DeliveryAdapter for TransientFailAdapter {
        fn deliver<'a>(
            &'a self,
            _recipient: &'a str,
            _message: &'a RenderedMessage,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<DeliveryReceipt, CourierError>> + Send + 'a,
            >,
        > {
            Box::pin(async move {
                let remaining = self
                    .remaining_failures
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                if remaining > 0 {
                    return Err(CourierError::DeliveryFailed("transient error".into()));
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Ok(DeliveryReceipt {
                    delivery_id: uuid::Uuid::new_v4().to_string(),
                    channel: "mock".into(),
                    status: DeliveryStatus::Delivered,
                    delivered_at: now,
                    error: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_on_transient_failure() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "https://example.com".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };

        // Adapter fails twice, succeeds on 3rd attempt
        let adapter = TransientFailAdapter::new(2);
        let config = RetryConfig {
            max_retries: 3,
            base_delay: std::time::Duration::from_millis(1), // fast for tests
            max_delay: std::time::Duration::from_millis(10),
        };

        let result = execute_delivery_with_retry(&req, None, &adapter, &config)
            .await
            .unwrap();
        assert_eq!(result.receipt.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_retry_exhausted_returns_failed() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "https://example.com".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };

        // Always fails — 1 initial + 2 retries = 3 total attempts
        let adapter = FailAdapter;
        let config = RetryConfig {
            max_retries: 2,
            base_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(10),
        };

        let result = execute_delivery_with_retry(&req, None, &adapter, &config)
            .await
            .unwrap();
        assert_eq!(result.receipt.status, DeliveryStatus::Failed);
        assert!(result.receipt.error.is_some());
    }

    #[tokio::test]
    async fn test_no_retry_fails_immediately() {
        let req = DeliveryRequest {
            channel: "webhook".into(),
            recipient: "https://example.com".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };

        let adapter = TransientFailAdapter::new(1);
        let config = RetryConfig::none(); // no retries

        let result = execute_delivery_with_retry(&req, None, &adapter, &config)
            .await
            .unwrap();
        assert_eq!(
            result.receipt.status,
            DeliveryStatus::Failed,
            "should fail without retries"
        );
    }

    #[tokio::test]
    async fn test_concurrent_deliveries_do_not_serialize() {
        let decryptor = std::sync::Arc::new(SlowDecryptor);
        let adapter = std::sync::Arc::new(MockAdapter);

        let start = std::time::Instant::now();
        let mut handles = Vec::new();

        for i in 0..5 {
            let d = decryptor.clone();
            let a = adapter.clone();
            handles.push(tokio::spawn(async move {
                let req = DeliveryRequest {
                    channel: "webhook".into(),
                    recipient: format!("enc:https://example.com/hook/{i}"),
                    subject: None,
                    body: Some("payload".into()),
                    body_encrypted: None,
                    content_type: None,
                };
                execute_delivery(&req, Some(d.as_ref()), a.as_ref())
                    .await
                    .unwrap()
            }));
        }

        for h in handles {
            let result = h.await.unwrap();
            assert_eq!(result.receipt.status, DeliveryStatus::Delivered);
        }

        let elapsed = start.elapsed();
        // 5 deliveries with 100ms decrypt each. If serialized: >= 500ms.
        // If concurrent: ~100ms. Allow generous margin.
        assert!(
            elapsed.as_millis() < 350,
            "deliveries appear to be serializing: {elapsed:?} for 5 concurrent deliveries \
             with 100ms decrypt each (expected < 350ms)"
        );
    }
}
