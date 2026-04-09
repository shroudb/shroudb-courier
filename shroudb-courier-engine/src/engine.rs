use dashmap::DashMap;
use shroudb_acl::{PolicyEffect, PolicyEvaluator, PolicyPrincipal, PolicyRequest, PolicyResource};
use shroudb_chronicle_core::event::{Engine as AuditEngine, Event, EventResult};
use shroudb_chronicle_core::ops::ChronicleOps;
use shroudb_courier_core::{
    Channel, ChannelType, CourierError, DeliveryReceipt, DeliveryRequest, DeliveryStatus,
};
use shroudb_store::Store;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::capabilities::{Decryptor, DeliveryAdapter};
use crate::channel_manager::ChannelManager;
use crate::delivery::{RetryConfig, execute_delivery_with_retry};

/// Policy enforcement mode for engine-level ABAC checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyMode {
    /// Fail-closed: if no PolicyEvaluator is configured, deny all operations.
    /// This is the secure default.
    #[default]
    Closed,
    /// Explicit opt-in to permissive mode: if no PolicyEvaluator is configured,
    /// allow all operations. Only appropriate for development/testing.
    Open,
}

const RECEIPTS_NAMESPACE: &str = "courier.receipts";

/// In-memory delivery metrics. Counters use relaxed ordering since
/// absolute precision is not required for operational metrics.
#[derive(Debug, Default)]
pub struct DeliveryMetrics {
    pub total: AtomicU64,
    pub delivered: AtomicU64,
    pub failed: AtomicU64,
}

pub struct CourierEngine<S: Store> {
    store: Arc<S>,
    channel_manager: ChannelManager<S>,
    decryptor: Option<Arc<dyn Decryptor>>,
    adapters: DashMap<ChannelType, Arc<dyn DeliveryAdapter>>,
    policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
    policy_mode: PolicyMode,
    retry_config: RetryConfig,
    chronicle: Option<Arc<dyn ChronicleOps>>,
    metrics: DeliveryMetrics,
    /// Per-channel delivery counts (channel_name → count).
    channel_metrics: DashMap<String, AtomicU64>,
}

impl<S: Store> CourierEngine<S> {
    pub async fn new(
        store: Arc<S>,
        decryptor: Option<Arc<dyn Decryptor>>,
        policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
        chronicle: Option<Arc<dyn ChronicleOps>>,
    ) -> Result<Self, CourierError> {
        Self::new_with_policy_mode(
            store,
            decryptor,
            policy_evaluator,
            chronicle,
            PolicyMode::default(),
        )
        .await
    }

    pub async fn new_with_policy_mode(
        store: Arc<S>,
        decryptor: Option<Arc<dyn Decryptor>>,
        policy_evaluator: Option<Arc<dyn PolicyEvaluator>>,
        chronicle: Option<Arc<dyn ChronicleOps>>,
        policy_mode: PolicyMode,
    ) -> Result<Self, CourierError> {
        let channel_manager = ChannelManager::new(store.clone());
        channel_manager.init().await?;

        // Create receipts namespace
        store
            .namespace_create(RECEIPTS_NAMESPACE, Default::default())
            .await
            .or_else(|e| match e {
                shroudb_store::StoreError::NamespaceExists(_) => Ok(()),
                other => Err(CourierError::Store(other.to_string())),
            })?;

        Ok(Self {
            store,
            channel_manager,
            decryptor,
            adapters: DashMap::new(),
            policy_evaluator,
            policy_mode,
            retry_config: RetryConfig::default(),
            chronicle,
            metrics: DeliveryMetrics::default(),
            channel_metrics: DashMap::new(),
        })
    }

    async fn check_policy(
        &self,
        resource_id: &str,
        action: &str,
        actor: Option<&str>,
    ) -> Result<(), CourierError> {
        let Some(evaluator) = &self.policy_evaluator else {
            // Fail-closed: no evaluator configured means deny unless explicitly open
            if self.policy_mode == PolicyMode::Open {
                return Ok(());
            }
            return Err(CourierError::PolicyDenied {
                action: action.to_string(),
                resource: resource_id.to_string(),
                policy: "no policy evaluator configured (fail-closed)".to_string(),
            });
        };
        let request = PolicyRequest {
            principal: PolicyPrincipal {
                id: actor.unwrap_or("").to_string(),
                roles: vec![],
                claims: Default::default(),
            },
            resource: PolicyResource {
                id: resource_id.to_string(),
                resource_type: "channel".to_string(),
                attributes: Default::default(),
            },
            action: action.to_string(),
        };
        let decision = evaluator
            .evaluate(&request)
            .await
            .map_err(|e| CourierError::Internal(format!("policy evaluation: {e}")))?;
        if decision.effect == PolicyEffect::Deny {
            return Err(CourierError::PolicyDenied {
                action: action.to_string(),
                resource: resource_id.to_string(),
                policy: decision.matched_policy.unwrap_or_default(),
            });
        }
        Ok(())
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
        self.check_policy(&channel.name, "channel_create", None)
            .await?;
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
        self.check_policy(name, "channel_delete", None).await?;
        self.channel_manager.delete(name).await?;
        self.emit_audit_event("CHANNEL_DELETE", name, EventResult::Ok, None, start)
            .await?;
        tracing::info!(name = %name, "channel deleted");
        Ok(())
    }

    // --- Delivery ---

    pub async fn deliver(&self, request: DeliveryRequest) -> Result<DeliveryReceipt, CourierError> {
        let start = Instant::now();
        self.check_policy(&request.channel, "deliver", None).await?;
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

        let result = execute_delivery_with_retry(
            &request,
            self.decryptor.as_deref(),
            adapter.as_ref(),
            &self.retry_config,
        )
        .await?;

        let receipt = result.receipt;
        self.record_metrics(&receipt);
        self.persist_receipt(&receipt).await?;

        self.emit_audit_event("DELIVER", &request.channel, EventResult::Ok, None, start)
            .await?;
        Ok(receipt)
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

    // --- Receipt persistence ---

    async fn persist_receipt(&self, receipt: &DeliveryReceipt) -> Result<(), CourierError> {
        let data =
            serde_json::to_vec(receipt).map_err(|e| CourierError::Internal(e.to_string()))?;
        self.store
            .put(
                RECEIPTS_NAMESPACE,
                receipt.delivery_id.as_bytes(),
                &data,
                None,
            )
            .await
            .map_err(|e| CourierError::Store(e.to_string()))?;
        Ok(())
    }

    fn record_metrics(&self, receipt: &DeliveryReceipt) {
        self.metrics.total.fetch_add(1, Ordering::Relaxed);
        match receipt.status {
            DeliveryStatus::Delivered => {
                self.metrics.delivered.fetch_add(1, Ordering::Relaxed);
            }
            DeliveryStatus::Failed => {
                self.metrics.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.channel_metrics
            .entry(receipt.channel.clone())
            .or_default()
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Retrieve a delivery receipt by ID.
    pub async fn delivery_get(&self, id: &str) -> Result<DeliveryReceipt, CourierError> {
        let entry = self
            .store
            .get(RECEIPTS_NAMESPACE, id.as_bytes(), None)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("not found") {
                    CourierError::InvalidArgument(format!("delivery not found: {id}"))
                } else {
                    CourierError::Store(msg)
                }
            })?;
        serde_json::from_slice(&entry.value)
            .map_err(|e| CourierError::Internal(format!("corrupt receipt data: {e}")))
    }

    /// List recent delivery receipts, optionally filtered by channel.
    pub async fn delivery_list(
        &self,
        channel: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DeliveryReceipt>, CourierError> {
        let mut receipts = Vec::new();
        let mut cursor: Option<String> = None;

        'outer: loop {
            let page = self
                .store
                .list(RECEIPTS_NAMESPACE, None, cursor.as_deref(), 100)
                .await
                .map_err(|e| CourierError::Store(e.to_string()))?;

            for key in &page.keys {
                let entry = self
                    .store
                    .get(RECEIPTS_NAMESPACE, key, None)
                    .await
                    .map_err(|e| CourierError::Store(e.to_string()))?;
                if let Ok(receipt) = serde_json::from_slice::<DeliveryReceipt>(&entry.value)
                    && (channel.is_none() || channel == Some(receipt.channel.as_str()))
                {
                    receipts.push(receipt);
                    if receipts.len() >= limit {
                        break 'outer;
                    }
                }
            }

            match page.cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }

        Ok(receipts)
    }

    /// Get current delivery metrics.
    pub fn metrics(&self) -> serde_json::Value {
        let mut per_channel = serde_json::Map::new();
        for entry in self.channel_metrics.iter() {
            per_channel.insert(
                entry.key().clone(),
                serde_json::json!(entry.value().load(Ordering::Relaxed)),
            );
        }

        serde_json::json!({
            "total_deliveries": self.metrics.total.load(Ordering::Relaxed),
            "delivered": self.metrics.delivered.load(Ordering::Relaxed),
            "failed": self.metrics.failed.load(Ordering::Relaxed),
            "per_channel": per_channel,
        })
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
        let engine = CourierEngine::new_with_policy_mode(
            store,
            Some(Arc::new(MockDecryptor)),
            None,
            None,
            PolicyMode::Open,
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

    #[tokio::test]
    async fn test_policy_denied_blocks_channel_create() {
        use shroudb_acl::{
            PolicyDecision, PolicyEffect, PolicyEvaluator, PolicyRequest as AclPolicyRequest,
            error::AclError,
        };
        use std::pin::Pin;

        struct DenyAll;
        impl PolicyEvaluator for DenyAll {
            fn evaluate(
                &self,
                _request: &AclPolicyRequest,
            ) -> Pin<
                Box<dyn std::future::Future<Output = Result<PolicyDecision, AclError>> + Send + '_>,
            > {
                Box::pin(async {
                    Ok(PolicyDecision {
                        effect: PolicyEffect::Deny,
                        matched_policy: Some("deny-all".to_string()),
                        token: None,
                        cache_until: None,
                    })
                })
            }
        }

        let store = shroudb_storage::test_util::create_test_store("courier-policy-test").await;
        let engine = CourierEngine::new(
            store,
            Some(Arc::new(MockDecryptor)),
            Some(Arc::new(DenyAll)),
            None,
        )
        .await
        .unwrap();

        let ch = Channel {
            name: "blocked".into(),
            channel_type: ChannelType::Webhook,
            smtp: None,
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        let err = engine.channel_create(ch).await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("policy denied"),
            "expected policy denied error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn no_evaluator_default_closed_denies() {
        let store = shroudb_storage::test_util::create_test_store("courier-closed-test").await;
        // Default PolicyMode::Closed, no evaluator
        let engine = CourierEngine::new(store, Some(Arc::new(MockDecryptor)), None, None)
            .await
            .unwrap();

        let ch = Channel {
            name: "test".into(),
            channel_type: ChannelType::Email,
            smtp: None,
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        let err = engine.channel_create(ch).await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("no policy evaluator configured"),
            "expected fail-closed message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn explicit_open_mode_permits_without_evaluator() {
        let store = shroudb_storage::test_util::create_test_store("courier-open-test").await;
        let engine = CourierEngine::new_with_policy_mode(
            store,
            Some(Arc::new(MockDecryptor)),
            None,
            None,
            PolicyMode::Open,
        )
        .await
        .unwrap();
        engine.register_adapter(
            ChannelType::Email,
            Arc::new(MockAdapter {
                channel_type: ChannelType::Email,
            }),
        );

        let ch = Channel {
            name: "allowed".into(),
            channel_type: ChannelType::Email,
            smtp: None,
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        let result = engine.channel_create(ch).await;
        assert!(result.is_ok(), "open mode should allow without evaluator");
    }

    #[tokio::test]
    async fn evaluator_present_evaluates_normally() {
        use shroudb_acl::{
            AclError, PolicyDecision, PolicyEffect, PolicyRequest as AclPolicyRequest,
        };

        struct PermitAll;
        impl PolicyEvaluator for PermitAll {
            fn evaluate(
                &self,
                _request: &AclPolicyRequest,
            ) -> Pin<
                Box<dyn std::future::Future<Output = Result<PolicyDecision, AclError>> + Send + '_>,
            > {
                Box::pin(async {
                    Ok(PolicyDecision {
                        effect: PolicyEffect::Permit,
                        matched_policy: None,
                        token: None,
                        cache_until: None,
                    })
                })
            }
        }

        let store = shroudb_storage::test_util::create_test_store("courier-eval-test").await;
        // Default closed mode, but evaluator IS present — should evaluate normally
        let engine = CourierEngine::new(
            store,
            Some(Arc::new(MockDecryptor)),
            Some(Arc::new(PermitAll)),
            None,
        )
        .await
        .unwrap();
        engine.register_adapter(
            ChannelType::Webhook,
            Arc::new(MockAdapter {
                channel_type: ChannelType::Webhook,
            }),
        );

        let ch = Channel {
            name: "eval-test".into(),
            channel_type: ChannelType::Webhook,
            smtp: None,
            webhook: None,
            enabled: true,
            created_at: 1000,
            default_recipient: None,
        };
        let result = engine.channel_create(ch).await;
        assert!(result.is_ok(), "present evaluator should permit");
    }

    // ── Delivery persistence (LOW-23) ─────────────────────────────

    #[tokio::test]
    async fn test_delivery_receipt_persisted() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "persist-ch".into(),
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
            channel: "persist-ch".into(),
            recipient: "enc:https://example.com/hook".into(),
            subject: None,
            body: Some("test payload".into()),
            body_encrypted: None,
            content_type: None,
        };
        let receipt = engine.deliver(req).await.unwrap();
        assert_eq!(receipt.status, DeliveryStatus::Delivered);

        // Should be retrievable by ID
        let fetched = engine.delivery_get(&receipt.delivery_id).await.unwrap();
        assert_eq!(fetched.delivery_id, receipt.delivery_id);
        assert_eq!(fetched.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn test_delivery_list_returns_receipts() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "list-ch".into(),
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

        // Deliver twice
        for _ in 0..2 {
            let req = DeliveryRequest {
                channel: "list-ch".into(),
                recipient: "enc:user@example.com".into(),
                subject: Some("Hello".into()),
                body: Some("World".into()),
                body_encrypted: None,
                content_type: None,
            };
            engine.deliver(req).await.unwrap();
        }

        let receipts = engine.delivery_list(None, 100).await.unwrap();
        assert!(receipts.len() >= 2);
    }

    #[tokio::test]
    async fn test_delivery_get_nonexistent() {
        let engine = create_engine().await;
        let result = engine.delivery_get("no-such-id").await;
        assert!(result.is_err());
    }

    // ── Metrics (LOW-24) ──────────────────────────────────────────

    #[tokio::test]
    async fn test_metrics_increment_on_delivery() {
        let engine = create_engine().await;

        let ch = Channel {
            name: "metrics-ch".into(),
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

        // Initial metrics
        let m = engine.metrics();
        assert_eq!(m["total_deliveries"].as_u64().unwrap(), 0);

        // Deliver
        let req = DeliveryRequest {
            channel: "metrics-ch".into(),
            recipient: "enc:https://example.com".into(),
            subject: None,
            body: Some("test".into()),
            body_encrypted: None,
            content_type: None,
        };
        engine.deliver(req).await.unwrap();

        let m = engine.metrics();
        assert_eq!(m["total_deliveries"].as_u64().unwrap(), 1);
        assert_eq!(m["delivered"].as_u64().unwrap(), 1);
        assert_eq!(m["failed"].as_u64().unwrap(), 0);
        assert!(m["per_channel"]["webhook"].as_u64().unwrap() >= 1);
    }
}
