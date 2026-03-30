use dashmap::DashMap;
use shroudb_courier_core::{Channel, ChannelType, CourierError, DeliveryReceipt, DeliveryRequest};
use shroudb_store::Store;
use std::sync::Arc;

use crate::capabilities::{Decryptor, DeliveryAdapter};
use crate::channel_manager::ChannelManager;
use crate::delivery::execute_delivery;

#[derive(Default)]
pub struct CourierConfig {}

pub struct CourierEngine<S: Store> {
    channel_manager: ChannelManager<S>,
    decryptor: Option<Arc<dyn Decryptor>>,
    adapters: DashMap<ChannelType, Arc<dyn DeliveryAdapter>>,
}

impl<S: Store> CourierEngine<S> {
    pub async fn new(
        store: Arc<S>,
        _config: CourierConfig,
        decryptor: Option<Arc<dyn Decryptor>>,
    ) -> Result<Self, CourierError> {
        let channel_manager = ChannelManager::new(store);
        channel_manager.init().await?;

        Ok(Self {
            channel_manager,
            decryptor,
            adapters: DashMap::new(),
        })
    }

    pub fn register_adapter(&self, channel_type: ChannelType, adapter: Arc<dyn DeliveryAdapter>) {
        self.adapters.insert(channel_type, adapter);
    }

    // --- Channel operations ---

    pub async fn channel_create(&self, channel: Channel) -> Result<(), CourierError> {
        self.channel_manager.create(channel.clone())?;
        self.channel_manager.save(&channel).await?;
        tracing::info!(name = %channel.name, channel_type = %channel.channel_type, "channel created");
        Ok(())
    }

    pub fn channel_get(&self, name: &str) -> Result<Channel, CourierError> {
        self.channel_manager.get(name)
    }

    pub fn channel_list(&self) -> Vec<String> {
        self.channel_manager.list()
    }

    pub async fn channel_delete(&self, name: &str) -> Result<(), CourierError> {
        self.channel_manager.delete(name).await?;
        tracing::info!(name = %name, "channel deleted");
        Ok(())
    }

    // --- Delivery ---

    pub async fn deliver(&self, request: DeliveryRequest) -> Result<DeliveryReceipt, CourierError> {
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

        Ok(result.receipt)
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
    use shroudb_storage::{EmbeddedStore, StorageEngine, StorageEngineConfig};
    use std::pin::Pin;

    struct EphemeralKey;
    impl shroudb_storage::MasterKeySource for EphemeralKey {
        fn source_name(&self) -> &str {
            "ephemeral"
        }
        fn load<'a>(
            &'a self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<shroudb_crypto::SecretBytes, shroudb_storage::StorageError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(shroudb_crypto::SecretBytes::new(vec![0x42u8; 32])) })
        }
    }

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
        let dir = tempfile::tempdir().unwrap().keep();
        let config = StorageEngineConfig {
            data_dir: dir,
            ..Default::default()
        };
        let se = StorageEngine::open(config, &EphemeralKey).await.unwrap();
        let store = Arc::new(EmbeddedStore::new(Arc::new(se), "courier-test"));
        let engine = CourierEngine::new(
            store,
            CourierConfig::default(),
            Some(Arc::new(MockDecryptor)),
        )
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
