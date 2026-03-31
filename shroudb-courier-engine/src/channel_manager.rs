use dashmap::DashMap;
use shroudb_courier_core::{Channel, CourierError};
use shroudb_store::{Store, StoreError};
use std::sync::Arc;

fn store_err(e: StoreError) -> CourierError {
    CourierError::Store(e.to_string())
}

const CHANNELS_NAMESPACE: &str = "courier.channels";

pub struct ChannelManager<S: Store> {
    store: Arc<S>,
    cache: DashMap<String, Arc<Channel>>,
}

impl<S: Store> ChannelManager<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            cache: DashMap::new(),
        }
    }

    pub async fn init(&self) -> Result<(), CourierError> {
        self.store
            .namespace_create(CHANNELS_NAMESPACE, Default::default())
            .await
            .or_else(|e| match e {
                shroudb_store::StoreError::NamespaceExists(_) => Ok(()),
                other => Err(CourierError::Store(other.to_string())),
            })?;

        let mut cursor = None;
        loop {
            let page = self
                .store
                .list(CHANNELS_NAMESPACE, None, cursor.as_deref(), 100)
                .await
                .map_err(store_err)?;

            for key in &page.keys {
                let entry = self
                    .store
                    .get(CHANNELS_NAMESPACE, key, None)
                    .await
                    .map_err(store_err)?;
                let channel: Channel = serde_json::from_slice(&entry.value)
                    .map_err(|e| CourierError::Internal(format!("corrupt channel data: {e}")))?;
                self.cache.insert(channel.name.clone(), Arc::new(channel));
            }

            match page.cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        tracing::info!(count = self.cache.len(), "loaded channels into cache");
        Ok(())
    }

    pub fn create(&self, channel: Channel) -> Result<(), CourierError> {
        shroudb_courier_core::channel::validate_name(&channel.name)
            .map_err(CourierError::InvalidName)?;

        if self.cache.contains_key(&channel.name) {
            return Err(CourierError::ChannelExists(channel.name.clone()));
        }

        self.cache.insert(channel.name.clone(), Arc::new(channel));
        Ok(())
    }

    pub async fn save(&self, channel: &Channel) -> Result<(), CourierError> {
        let value =
            serde_json::to_vec(channel).map_err(|e| CourierError::Internal(e.to_string()))?;
        self.store
            .put(CHANNELS_NAMESPACE, channel.name.as_bytes(), &value, None)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Arc<Channel>, CourierError> {
        self.cache
            .get(name)
            .map(|entry| Arc::clone(entry.value()))
            .ok_or_else(|| CourierError::ChannelNotFound(name.to_string()))
    }

    pub fn list(&self) -> Vec<String> {
        self.cache.iter().map(|e| e.key().clone()).collect()
    }

    pub async fn delete(&self, name: &str) -> Result<(), CourierError> {
        if self.cache.remove(name).is_none() {
            return Err(CourierError::ChannelNotFound(name.to_string()));
        }
        self.store
            .delete(CHANNELS_NAMESPACE, name.as_bytes())
            .await
            .map_err(store_err)?;
        Ok(())
    }

    pub async fn seed_if_absent(&self, channel: Channel) -> Result<(), CourierError> {
        if self.cache.contains_key(&channel.name) {
            tracing::debug!(name = %channel.name, "channel already exists, skipping seed");
            return Ok(());
        }
        self.create(channel.clone())?;
        self.save(&channel).await?;
        tracing::info!(name = %channel.name, "seeded channel from config");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroudb_courier_core::{ChannelType, SmtpConfig};
    use shroudb_storage::{EmbeddedStore, MasterKeySource, StorageEngine, StorageEngineConfig};

    /// Fixed-key source for persistence tests that reopen the same directory.
    struct FixedTestKey;
    impl MasterKeySource for FixedTestKey {
        fn source_name(&self) -> &str {
            "fixed-test"
        }
        fn load(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<shroudb_crypto::SecretBytes, shroudb_storage::StorageError>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(shroudb_crypto::SecretBytes::new(vec![0x42u8; 32])) })
        }
    }

    fn test_channel(name: &str) -> Channel {
        Channel {
            name: name.into(),
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
        }
    }

    #[tokio::test]
    async fn test_channel_crud() {
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let mgr = ChannelManager::new(store);
        mgr.init().await.unwrap();

        let ch = test_channel("email-prod");
        mgr.create(ch.clone()).unwrap();
        mgr.save(&ch).await.unwrap();

        let got = mgr.get("email-prod").unwrap();
        assert_eq!(got.name, "email-prod");
        assert_eq!(got.channel_type, ChannelType::Email);

        let names = mgr.list();
        assert_eq!(names.len(), 1);

        mgr.delete("email-prod").await.unwrap();
        assert!(mgr.get("email-prod").is_err());
    }

    #[tokio::test]
    async fn test_channel_duplicate_rejected() {
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let mgr = ChannelManager::new(store);
        mgr.init().await.unwrap();

        let ch = test_channel("dup");
        mgr.create(ch.clone()).unwrap();
        assert!(mgr.create(ch).is_err());
    }

    #[tokio::test]
    async fn test_channel_invalid_name() {
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let mgr = ChannelManager::new(store);
        mgr.init().await.unwrap();

        let mut ch = test_channel("valid");
        ch.name = "has spaces".into();
        assert!(mgr.create(ch).is_err());
    }

    #[tokio::test]
    async fn test_channel_persistence() {
        let dir = tempfile::tempdir().unwrap().keep();
        let config = StorageEngineConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let engine = StorageEngine::open(config, &FixedTestKey).await.unwrap();
        let store = Arc::new(EmbeddedStore::new(Arc::new(engine), "courier-test"));

        let mgr = ChannelManager::new(store);
        mgr.init().await.unwrap();
        let ch = test_channel("persist-me");
        mgr.create(ch.clone()).unwrap();
        mgr.save(&ch).await.unwrap();
        drop(mgr);

        let config2 = StorageEngineConfig {
            data_dir: dir,
            ..Default::default()
        };
        let engine2 = StorageEngine::open(config2, &FixedTestKey).await.unwrap();
        let store2 = Arc::new(EmbeddedStore::new(Arc::new(engine2), "courier-test"));
        let mgr2 = ChannelManager::new(store2);
        mgr2.init().await.unwrap();

        let got = mgr2.get("persist-me").unwrap();
        assert_eq!(got.name, "persist-me");
    }

    #[tokio::test]
    async fn test_seed_if_absent() {
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let mgr = ChannelManager::new(store);
        mgr.init().await.unwrap();

        let ch = test_channel("seeded");
        mgr.seed_if_absent(ch.clone()).await.unwrap();
        assert!(mgr.get("seeded").is_ok());

        // Second seed is a no-op
        mgr.seed_if_absent(ch).await.unwrap();
        assert_eq!(mgr.list().len(), 1);
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let mgr = ChannelManager::new(store);
        mgr.init().await.unwrap();

        assert!(mgr.delete("ghost").await.is_err());
    }
}
