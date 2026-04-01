use dashmap::DashMap;
use shroudb_chronicle_core::event::{Engine as AuditEngine, Event, EventResult};
use shroudb_chronicle_core::ops::ChronicleOps;
use shroudb_courier_core::{Channel, ChannelType, CourierError, DeliveryReceipt, DeliveryRequest};
use shroudb_store::Store;
use std::sync::Arc;
use std::time::Instant;

use crate::capabilities::{Decryptor, DeliveryAdapter};
use crate::channel_manager::ChannelManager;
use crate::delivery::execute_delivery;

pub struct CourierEngine<S: Store> {
    channel_manager: ChannelManager<S>,
    decryptor: Option<Arc<dyn Decryptor>>,
    adapters: DashMap<ChannelType, Arc<dyn DeliveryAdapter>>,
    chronicle: Option<Arc<dyn ChronicleOps>>,
}

impl<S: Store> CourierEngine<S> {
    pub async fn new(
        store: Arc<S>,
        decryptor: Option<Arc<dyn Decryptor>>,
        chronicle: Option<Arc<dyn ChronicleOps>>,
    ) -> Result<Self, CourierError> {
        let channel_manager = ChannelManager::new(store);
        channel_manager.init().await?;

        Ok(Self {
            channel_manager,
            decryptor,
            adapters: DashMap::new(),
            chronicle,
        })
    }

    /// Emit an audit event to Chronicle. If chronicle is not configured, this
    /// is a no-op. If chronicle is configured but unreachable, returns an error
    /// so security-critical callers can fail closed.
    async fn emit_audit_event(
        &self,
        operation: &str,
        resource: &str,
        result: EventResult,
        actor: Option<&str>,
        start: Instant,
    ) -> Result<(), CourierError> {
        let Some(chronicle) = &self.chronicle else {
            return Ok(());
        };
        let mut event = Event::new(
            AuditEngine::Courier,
            operation.to_string(),
            resource.to_string(),
            result,
            actor.unwrap_or("anonymous").to_string(),
        );
        event.duration_ms = start.elapsed().as_millis() as u64;
        chronicle
            .record(event)
            .await
            .map_err(|e| CourierError::Internal(format!("audit failed: {e}")))?;
        Ok(())
    }

    pub fn register_adapter(&self, channel_type: ChannelType, adapter: Arc<dyn DeliveryAdapter>) {
        self.adapters.insert(channel_type, adapter);
    }

    // --- Channel operations ---

    pub async fn channel_create(&self, channel: Channel) -> Result<(), CourierError> {
        let start = Instant::now();
        self.channel_manager.create(channel.clone())?;
        self.channel_manager.save(&channel).await?;
        self.emit_audit_event(
            "CHANNEL_CREATE",
            &channel.name,
            EventResult::Ok,
            None,
            start,
        )
        .await?;
        tracing::info!(name = %channel.name, channel_type = %channel.channel_type, "channel created");
        Ok(())
    }

    pub fn channel_get(&self, name: &str) -> Result<Arc<Channel>, CourierError> {
        self.channel_manager.get(name)
    }

    pub fn channel_list(&self) -> Vec<String> {
        self.channel_manager.list()
    }

    pub async fn channel_delete(&self, name: &str) -> Result<(), CourierError> {
        let start = Instant::now();
        self.channel_manager.delete(name).await?;
        self.emit_audit_event("CHANNEL_DELETE", name, EventResult::Ok, None, start)
            .await?;
        tracing::info!(name = %name, "channel deleted");
        Ok(())
    }

    // --- Delivery ---

    pub async fn deliver(&self, request: DeliveryRequest) -> Result<DeliveryReceipt, CourierError> {
        let start = Instant::now();
        let channel = self.channel_manager.get(&request.channel)?;
        if !channel.enabled {
            return Err(CourierError::InvalidArgument(format!(
                "channel '{}' is disabled",
                request.channel
            )));
        }

        let adapter = self
            .adapters
            .get(&channel.channel_type)
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| CourierError::AdapterNotConfigured(channel.channel_type.to_string()))?;

        let result =
            execute_delivery(&request, self.decryptor.as_deref(), adapter.as_ref()).await?;

        self.emit_audit_event("DELIVER", &request.channel, EventResult::Ok, None, start)
            .await?;
        Ok(result.receipt)
    }

    // --- Event notifications ---

    /// Convenience method for engine schedulers (e.g. Cipher key rotation, Forge cert expiry)
    /// to trigger a notification on a pre-configured channel. The channel must have a
    /// `default_recipient` set; otherwise this returns an error.
    pub async fn notify_event(
        &self,
        channel_name: &str,
        subject: &str,
        body: &str,
    ) -> Result<DeliveryReceipt, CourierError> {
        let channel = self.channel_manager.get(channel_name)?;
        let recipient = channel.default_recipient.as_deref().ok_or_else(|| {
            CourierError::InvalidArgument(format!(
                "channel '{}' has no default_recipient configured for event notifications",
                channel_name
            ))
        })?;

        let request = DeliveryRequest {
            channel: channel_name.to_string(),
            recipient: recipient.to_string(),
            subject: Some(subject.to_string()),
            body: Some(body.to_string()),
            body_encrypted: None,
            content_type: None,
        };

        self.deliver(request).await
    }

    // --- Seeding ---

    pub async fn seed_channel(&self, channel: Channel) -> Result<(), CourierError> {
        self.channel_manager.seed_if_absent(channel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroudb_courier_core::{DeliveryStatus, RenderedMessage, SmtpConfig, WebhookConfig};
    use shroudb_storage::EmbeddedStore;
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

    struct MockAdapter {
        channel_type: ChannelType,
    }
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
            let ct = self.channel_type;
            Box::pin(async move {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Ok(DeliveryReceipt {
                    delivery_id: uuid::Uuid::new_v4().to_string(),
                    channel: ct.to_string(),
                    status: DeliveryStatus::Delivered,
                    delivered_at: now,
                    error: None,
                })
            })
        }
    }

    async fn create_engine() -> CourierEngine<EmbeddedStore> {
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let engine = CourierEngine::new(store, Some(Arc::new(MockDecryptor)), None)
            .await
            .unwrap();
        engine.register_adapter(
            ChannelType::Email,
            Arc::new(MockAdapter {
                channel_type: ChannelType::Email,
            }),
        );
        engine.register_adapter(
            ChannelType::Webhook,
            Arc::new(MockAdapter {
                channel_type: ChannelType::Webhook,
            }),
        );
        engine
    }

    #[tokio::test]
    async fn test_channel_lifecycle() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "email-prod".into(),
            channel_type: ChannelType::Email,
            smtp: Some(SmtpConfig {
                host: "smtp.test.com".into(),
                port: 587,
                username: None,
                password: None,
                from_address: "test@test.com".into(),
                starttls: true,
            }),
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        engine.channel_create(ch).await.unwrap();

        let got = engine.channel_get("email-prod").unwrap();
        assert_eq!(got.channel_type, ChannelType::Email);

        let list = engine.channel_list();
        assert_eq!(list.len(), 1);

        engine.channel_delete("email-prod").await.unwrap();
        assert!(engine.channel_get("email-prod").is_err());
    }

    #[tokio::test]
    async fn test_deliver_full_flow() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "test-email".into(),
            channel_type: ChannelType::Email,
            smtp: Some(SmtpConfig {
                host: "smtp.test.com".into(),
                port: 587,
                username: None,
                password: None,
                from_address: "test@test.com".into(),
                starttls: true,
            }),
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        engine.channel_create(ch).await.unwrap();

        let req = DeliveryRequest {
            channel: "test-email".into(),
            recipient: "enc:alice@example.com".into(),
            subject: Some("Hello".into()),
            body: Some("Welcome Alice".into()),
            body_encrypted: None,
            content_type: None,
        };

        let receipt = engine.deliver(req).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_deliver_disabled_channel() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "disabled".into(),
            channel_type: ChannelType::Email,
            smtp: None,
            webhook: None,
            enabled: false,
            created_at: 1000,
            default_recipient: None,
        };
        engine.channel_create(ch).await.unwrap();

        let req = DeliveryRequest {
            channel: "disabled".into(),
            recipient: "enc:x".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };

        assert!(engine.deliver(req).await.is_err());
    }

    #[tokio::test]
    async fn test_deliver_nonexistent_channel() {
        let engine = create_engine().await;

        let req = DeliveryRequest {
            channel: "ghost".into(),
            recipient: "enc:x".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };

        assert!(engine.deliver(req).await.is_err());
    }

    #[tokio::test]
    async fn test_notify_event_delivers_to_channel() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "rotation-alerts".into(),
            channel_type: ChannelType::Webhook,
            smtp: None,
            webhook: Some(WebhookConfig {
                default_method: None,
                default_headers: None,
                timeout_secs: None,
            }),
            enabled: true,
            created_at: 1000,
            default_recipient: Some("https://ops.example.com/alerts".into()),
        };
        engine.channel_create(ch).await.unwrap();

        let receipt = engine
            .notify_event(
                "rotation-alerts",
                "Key rotation approaching",
                "Key 'master-key-1' expires in 24 hours",
            )
            .await
            .unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_notify_event_no_default_recipient() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "no-recipient".into(),
            channel_type: ChannelType::Webhook,
            smtp: None,
            webhook: Some(WebhookConfig {
                default_method: None,
                default_headers: None,
                timeout_secs: None,
            }),
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        engine.channel_create(ch).await.unwrap();

        let result = engine.notify_event("no-recipient", "subject", "body").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deliver_webhook() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "events".into(),
            channel_type: ChannelType::Webhook,
            smtp: None,
            webhook: Some(WebhookConfig {
                default_method: None,
                default_headers: None,
                timeout_secs: None,
            }),
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        engine.channel_create(ch).await.unwrap();

        let req = DeliveryRequest {
            channel: "events".into(),
            recipient: "enc:https://example.com/hook".into(),
            subject: None,
            body: Some("{\"event\": \"test\"}".into()),
            body_encrypted: None,
            content_type: None,
        };

        let receipt = engine.deliver(req).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);
    }
}
