use shroudb_acl::AuthContext;
use shroudb_courier_engine::CourierEngine;
use shroudb_protocol_wire::WIRE_PROTOCOL;
use shroudb_store::Store;

use crate::commands::CourierCommand;
use crate::response::CourierResponse;

const SUPPORTED_COMMANDS: &[&str] = &[
    "AUTH",
    "CHANNEL CREATE",
    "CHANNEL GET",
    "CHANNEL LIST",
    "CHANNEL DELETE",
    "DELIVER",
    "DELIVERY GET",
    "DELIVERY LIST",
    "NOTIFY_EVENT",
    "METRICS",
    "HEALTH",
    "PING",
    "COMMAND LIST",
    "HELLO",
];

pub async fn dispatch<S: Store>(
    engine: &CourierEngine<S>,
    cmd: CourierCommand,
    auth_context: Option<&AuthContext>,
) -> CourierResponse {
    if let Err(e) = shroudb_acl::check_dispatch_acl(auth_context, &cmd.acl_requirement()) {
        return CourierResponse::error(e);
    }

    match cmd {
        CourierCommand::Auth { .. } => {
            CourierResponse::error("AUTH is handled at the connection layer")
        }

        CourierCommand::ChannelCreate {
            name,
            channel_type,
            config_json,
        } => handle_channel_create(engine, &name, &channel_type, &config_json).await,

        CourierCommand::ChannelGet { name } => handle_channel_get(engine, &name),

        CourierCommand::ChannelList => handle_channel_list(engine),

        CourierCommand::ChannelDelete { name } => handle_channel_delete(engine, &name).await,

        CourierCommand::NotifyEvent {
            channel,
            subject,
            body,
        } => handle_notify_event(engine, &channel, &subject, &body).await,

        CourierCommand::Deliver { request_json } => handle_deliver(engine, &request_json).await,

        CourierCommand::DeliveryGet { id } => handle_delivery_get(engine, &id).await,

        CourierCommand::DeliveryList { channel, limit } => {
            handle_delivery_list(engine, channel.as_deref(), limit).await
        }

        CourierCommand::Metrics => handle_metrics(engine),

        CourierCommand::Health => {
            let channels = engine.channel_list();
            CourierResponse::ok(serde_json::json!({
                "status": "ok",
                "channels": channels.len(),
            }))
        }

        CourierCommand::Ping => CourierResponse::ok(serde_json::json!("PONG")),

        CourierCommand::CommandList => CourierResponse::ok(serde_json::json!({
            "commands": SUPPORTED_COMMANDS,
            "count": SUPPORTED_COMMANDS.len(),
        })),

        CourierCommand::Hello => CourierResponse::ok(serde_json::json!({
            "engine": "courier",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": WIRE_PROTOCOL,
            "commands": SUPPORTED_COMMANDS,
            "capabilities": Vec::<&str>::new(),
        })),
    }
}

async fn handle_channel_create<S: Store>(
    engine: &CourierEngine<S>,
    name: &str,
    channel_type: &str,
    config_json: &str,
) -> CourierResponse {
    let ct: shroudb_courier_core::ChannelType = match channel_type.parse() {
        Ok(ct) => ct,
        Err(e) => return CourierResponse::error(e),
    };

    let config: serde_json::Value = match serde_json::from_str(config_json) {
        Ok(v) => v,
        Err(e) => return CourierResponse::error(format!("invalid config JSON: {e}")),
    };

    let smtp = if ct == shroudb_courier_core::ChannelType::Email {
        match serde_json::from_value(config.clone()) {
            Ok(s) => Some(s),
            Err(e) => return CourierResponse::error(format!("invalid SMTP config: {e}")),
        }
    } else {
        None
    };

    let webhook = if ct == shroudb_courier_core::ChannelType::Webhook {
        Some(serde_json::from_value(config.clone()).unwrap_or(
            shroudb_courier_core::WebhookConfig {
                default_method: None,
                default_headers: None,
                timeout_secs: None,
            },
        ))
    } else {
        None
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let channel = shroudb_courier_core::Channel {
        name: name.to_string(),
        channel_type: ct,
        smtp,
        webhook,
        enabled: true,
        created_at: now,
        default_recipient: None,
    };

    match engine.channel_create(channel).await {
        Ok(()) => CourierResponse::ok(serde_json::json!({
            "status": "ok",
            "name": name,
            "channel_type": channel_type.to_lowercase(),
        })),
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

fn handle_channel_get<S: Store>(engine: &CourierEngine<S>, name: &str) -> CourierResponse {
    match engine.channel_get(name) {
        Ok(ch) => match serde_json::to_value(&*ch) {
            Ok(v) => CourierResponse::ok(v),
            Err(e) => CourierResponse::error(format!("serialization error: {e}")),
        },
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

fn handle_channel_list<S: Store>(engine: &CourierEngine<S>) -> CourierResponse {
    let names = engine.channel_list();
    CourierResponse::ok(serde_json::json!({
        "status": "ok",
        "count": names.len(),
        "channels": names,
    }))
}

async fn handle_channel_delete<S: Store>(engine: &CourierEngine<S>, name: &str) -> CourierResponse {
    match engine.channel_delete(name).await {
        Ok(()) => CourierResponse::ok(serde_json::json!({
            "status": "ok",
            "name": name,
        })),
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

async fn handle_notify_event<S: Store>(
    engine: &CourierEngine<S>,
    channel: &str,
    subject: &str,
    body: &str,
) -> CourierResponse {
    match engine.notify_event(channel, subject, body).await {
        Ok(receipt) => match serde_json::to_value(&receipt) {
            Ok(v) => CourierResponse::ok(v),
            Err(e) => CourierResponse::error(format!("serialization error: {e}")),
        },
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

async fn handle_deliver<S: Store>(
    engine: &CourierEngine<S>,
    request_json: &str,
) -> CourierResponse {
    let request: shroudb_courier_core::DeliveryRequest = match serde_json::from_str(request_json) {
        Ok(r) => r,
        Err(e) => return CourierResponse::error(format!("invalid delivery request JSON: {e}")),
    };

    match engine.deliver(request).await {
        Ok(receipt) => match serde_json::to_value(&receipt) {
            Ok(v) => CourierResponse::ok(v),
            Err(e) => CourierResponse::error(format!("serialization error: {e}")),
        },
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

async fn handle_delivery_get<S: Store>(engine: &CourierEngine<S>, id: &str) -> CourierResponse {
    match engine.delivery_get(id).await {
        Ok(receipt) => match serde_json::to_value(&receipt) {
            Ok(v) => CourierResponse::ok(v),
            Err(e) => CourierResponse::error(format!("serialization error: {e}")),
        },
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

async fn handle_delivery_list<S: Store>(
    engine: &CourierEngine<S>,
    channel: Option<&str>,
    limit: usize,
) -> CourierResponse {
    match engine.delivery_list(channel, limit).await {
        Ok(receipts) => {
            let entries: Vec<serde_json::Value> = receipts
                .iter()
                .filter_map(|r| serde_json::to_value(r).ok())
                .collect();
            CourierResponse::ok(serde_json::json!({
                "status": "ok",
                "count": entries.len(),
                "receipts": entries,
            }))
        }
        Err(e) => CourierResponse::error(e.to_string()),
    }
}

fn handle_metrics<S: Store>(engine: &CourierEngine<S>) -> CourierResponse {
    CourierResponse::ok(engine.metrics())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroudb_courier_core::{CourierError, DeliveryReceipt, DeliveryStatus, RenderedMessage};
    use shroudb_courier_engine::{Decryptor, DeliveryAdapter};
    use shroudb_storage::EmbeddedStore;
    use std::pin::Pin;
    use std::sync::Arc;

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
                    delivery_id: "test-id".into(),
                    channel: "mock".into(),
                    status: DeliveryStatus::Delivered,
                    delivered_at: now,
                    error: None,
                })
            })
        }
    }

    async fn create_engine() -> CourierEngine<EmbeddedStore> {
        use shroudb_server_bootstrap::Capability;
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let engine = CourierEngine::new_with_policy_mode(
            store,
            Capability::Enabled(Arc::new(MockDecryptor)),
            Capability::DisabledForTests,
            Capability::DisabledForTests,
            shroudb_courier_engine::PolicyMode::Open,
        )
        .await
        .unwrap();
        engine.register_adapter(
            shroudb_courier_core::ChannelType::Email,
            Arc::new(MockAdapter),
        );
        engine.register_adapter(
            shroudb_courier_core::ChannelType::Webhook,
            Arc::new(MockAdapter),
        );
        engine
    }

    #[tokio::test]
    async fn test_dispatch_health() {
        let engine = create_engine().await;
        let resp = dispatch(&engine, CourierCommand::Health, None).await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn test_dispatch_ping() {
        let engine = create_engine().await;
        let resp = dispatch(&engine, CourierCommand::Ping, None).await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn test_dispatch_command_list() {
        let engine = create_engine().await;
        let resp = dispatch(&engine, CourierCommand::CommandList, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = resp {
            assert_eq!(v["count"], 14);
        }
    }

    #[tokio::test]
    async fn test_dispatch_channel_lifecycle() {
        let engine = create_engine().await;

        let create = CourierCommand::ChannelCreate {
            name: "test-email".into(),
            channel_type: "email".into(),
            config_json: r#"{"host":"smtp.test.com","port":587,"from_address":"test@test.com","starttls":true}"#.into(),
        };
        let resp = dispatch(&engine, create, None).await;
        assert!(resp.is_ok());

        let get = CourierCommand::ChannelGet {
            name: "test-email".into(),
        };
        let resp = dispatch(&engine, get, None).await;
        assert!(resp.is_ok());

        let list = CourierCommand::ChannelList;
        let resp = dispatch(&engine, list, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = resp {
            assert_eq!(v["count"], 1);
        }

        let del = CourierCommand::ChannelDelete {
            name: "test-email".into(),
        };
        let resp = dispatch(&engine, del, None).await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn test_dispatch_deliver() {
        let engine = create_engine().await;

        let create_ch = CourierCommand::ChannelCreate {
            name: "test-hook".into(),
            channel_type: "webhook".into(),
            config_json: "{}".into(),
        };
        dispatch(&engine, create_ch, None).await;

        let deliver = CourierCommand::Deliver {
            request_json:
                r#"{"channel":"test-hook","recipient":"enc:https://example.com","body":"test payload"}"#
                    .into(),
        };
        let resp = dispatch(&engine, deliver, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = resp {
            assert_eq!(v["status"], "delivered");
        }
    }

    #[tokio::test]
    async fn test_dispatch_notify_event() {
        let engine = create_engine().await;

        // Create a webhook channel with a default_recipient
        let create_ch = CourierCommand::ChannelCreate {
            name: "rotation-alerts".into(),
            channel_type: "webhook".into(),
            config_json: "{}".into(),
        };
        dispatch(&engine, create_ch, None).await;

        // Manually set default_recipient by creating via engine directly
        // since CHANNEL CREATE doesn't expose default_recipient yet.
        // We'll delete and re-create through the engine.
        engine.channel_delete("rotation-alerts").await.unwrap();
        let ch = shroudb_courier_core::Channel {
            name: "rotation-alerts".into(),
            channel_type: shroudb_courier_core::ChannelType::Webhook,
            smtp: None,
            webhook: Some(shroudb_courier_core::WebhookConfig {
                default_method: None,
                default_headers: None,
                timeout_secs: None,
            }),
            enabled: true,
            created_at: 1000,
            default_recipient: Some("https://ops.example.com/alerts".into()),
        };
        engine.channel_create(ch).await.unwrap();

        let cmd = CourierCommand::NotifyEvent {
            channel: "rotation-alerts".into(),
            subject: "Cert expiry warning".into(),
            body: "Certificate 'api-tls' expires in 7 days".into(),
        };
        let resp = dispatch(&engine, cmd, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = resp {
            assert_eq!(v["status"], "delivered");
        }
    }

    #[tokio::test]
    async fn test_dispatch_notify_event_no_default_recipient() {
        let engine = create_engine().await;

        let create_ch = CourierCommand::ChannelCreate {
            name: "no-default".into(),
            channel_type: "webhook".into(),
            config_json: "{}".into(),
        };
        dispatch(&engine, create_ch, None).await;

        let cmd = CourierCommand::NotifyEvent {
            channel: "no-default".into(),
            subject: "test".into(),
            body: "test".into(),
        };
        let resp = dispatch(&engine, cmd, None).await;
        assert!(!resp.is_ok());
    }

    #[tokio::test]
    async fn test_dispatch_deliver_nonexistent_channel() {
        let engine = create_engine().await;

        let deliver = CourierCommand::Deliver {
            request_json: r#"{"channel":"ghost","recipient":"enc:x","body":"test"}"#.into(),
        };
        let resp = dispatch(&engine, deliver, None).await;
        assert!(!resp.is_ok());
    }

    #[tokio::test]
    async fn test_dispatch_channel_get_nonexistent() {
        let engine = create_engine().await;

        let get = CourierCommand::ChannelGet {
            name: "ghost".into(),
        };
        let resp = dispatch(&engine, get, None).await;
        assert!(!resp.is_ok());
    }

    // ── ACL tests ─────────────────────────────────────────────────────

    fn read_only_context() -> AuthContext {
        use shroudb_acl::{Grant, Scope};
        AuthContext::tenant(
            "tenant-a",
            "read-user",
            vec![Grant {
                namespace: "courier.test-hook.*".into(),
                scopes: vec![Scope::Read],
            }],
            None,
        )
    }

    fn write_context() -> AuthContext {
        use shroudb_acl::{Grant, Scope};
        AuthContext::tenant(
            "tenant-a",
            "write-user",
            vec![Grant {
                namespace: "courier.test-hook.*".into(),
                scopes: vec![Scope::Read, Scope::Write],
            }],
            None,
        )
    }

    #[tokio::test]
    async fn test_unauthorized_write_rejected() {
        let engine = create_engine().await;
        let ctx = read_only_context();

        // DELIVER requires Write scope on courier.<channel>.*
        let cmd = CourierCommand::Deliver {
            request_json: r#"{"channel":"test-hook","recipient":"enc:x","body":"test"}"#.into(),
        };
        let resp = dispatch(&engine, cmd, Some(&ctx)).await;
        assert!(
            !resp.is_ok(),
            "read-only context should not be able to deliver"
        );

        match resp {
            CourierResponse::Error(msg) => assert!(
                msg.contains("access denied"),
                "error should mention access denied, got: {msg}"
            ),
            _ => panic!("expected error response"),
        }
    }

    #[tokio::test]
    async fn test_unauthorized_admin_rejected() {
        let engine = create_engine().await;
        let ctx = write_context();

        // CHANNEL CREATE requires Admin scope
        let cmd = CourierCommand::ChannelCreate {
            name: "new-channel".into(),
            channel_type: "webhook".into(),
            config_json: "{}".into(),
        };
        let resp = dispatch(&engine, cmd, Some(&ctx)).await;
        assert!(
            !resp.is_ok(),
            "non-admin context should not be able to create channels"
        );

        match resp {
            CourierResponse::Error(msg) => assert!(
                msg.contains("access denied"),
                "error should mention access denied, got: {msg}"
            ),
            _ => panic!("expected error response"),
        }
    }

    // ── Delivery persistence (LOW-23) ─────────────────────────────

    #[tokio::test]
    async fn test_dispatch_delivery_list_after_deliver() {
        let engine = create_engine().await;

        let create_ch = CourierCommand::ChannelCreate {
            name: "dl-hook".into(),
            channel_type: "webhook".into(),
            config_json: "{}".into(),
        };
        dispatch(&engine, create_ch, None).await;

        let deliver = CourierCommand::Deliver {
            request_json:
                r#"{"channel":"dl-hook","recipient":"enc:https://example.com","body":"test"}"#
                    .into(),
        };
        let resp = dispatch(&engine, deliver, None).await;
        assert!(resp.is_ok());

        // Extract delivery_id from the receipt
        let delivery_id = if let CourierResponse::Ok(v) = &resp {
            v["delivery_id"].as_str().unwrap().to_string()
        } else {
            panic!("expected Ok response");
        };

        // DELIVERY GET should return the receipt
        let get_cmd = CourierCommand::DeliveryGet {
            id: delivery_id.clone(),
        };
        let resp = dispatch(&engine, get_cmd, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = &resp {
            assert_eq!(v["delivery_id"].as_str().unwrap(), delivery_id);
            assert_eq!(v["status"].as_str().unwrap(), "delivered");
        }

        // DELIVERY LIST should include the receipt
        let list_cmd = CourierCommand::DeliveryList {
            channel: None,
            limit: 100,
        };
        let resp = dispatch(&engine, list_cmd, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = &resp {
            assert!(v["count"].as_u64().unwrap() >= 1);
        }
    }

    #[tokio::test]
    async fn test_dispatch_delivery_list_filtered_by_channel() {
        let engine = create_engine().await;

        // Create two channels
        for name in ["ch-a", "ch-b"] {
            let cmd = CourierCommand::ChannelCreate {
                name: name.into(),
                channel_type: "webhook".into(),
                config_json: "{}".into(),
            };
            dispatch(&engine, cmd, None).await;
        }

        // Deliver to ch-a
        let deliver = CourierCommand::Deliver {
            request_json:
                r#"{"channel":"ch-a","recipient":"enc:https://example.com","body":"hello"}"#.into(),
        };
        dispatch(&engine, deliver, None).await;

        // Deliver to ch-b
        let deliver = CourierCommand::Deliver {
            request_json:
                r#"{"channel":"ch-b","recipient":"enc:https://example.com","body":"world"}"#.into(),
        };
        dispatch(&engine, deliver, None).await;

        // List filtered by ch-a
        let list_cmd = CourierCommand::DeliveryList {
            channel: Some("ch-a".into()),
            limit: 100,
        };
        let resp = dispatch(&engine, list_cmd, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = &resp {
            let receipts = v["receipts"].as_array().unwrap();
            for r in receipts {
                assert_eq!(r["channel"].as_str().unwrap(), "mock");
            }
        }
    }

    #[tokio::test]
    async fn test_dispatch_delivery_get_nonexistent() {
        let engine = create_engine().await;
        let cmd = CourierCommand::DeliveryGet {
            id: "nonexistent-id".into(),
        };
        let resp = dispatch(&engine, cmd, None).await;
        assert!(!resp.is_ok());
    }

    // ── Metrics (LOW-24) ──────────────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_metrics() {
        let engine = create_engine().await;

        // Initial metrics should be zero
        let resp = dispatch(&engine, CourierCommand::Metrics, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = &resp {
            assert_eq!(v["total_deliveries"].as_u64().unwrap(), 0);
            assert_eq!(v["delivered"].as_u64().unwrap(), 0);
            assert_eq!(v["failed"].as_u64().unwrap(), 0);
        }

        // Create a channel and deliver
        let create_ch = CourierCommand::ChannelCreate {
            name: "metrics-test".into(),
            channel_type: "webhook".into(),
            config_json: "{}".into(),
        };
        dispatch(&engine, create_ch, None).await;

        let deliver = CourierCommand::Deliver {
            request_json:
                r#"{"channel":"metrics-test","recipient":"enc:https://example.com","body":"test"}"#
                    .into(),
        };
        dispatch(&engine, deliver, None).await;

        // Metrics should now reflect the delivery
        let resp = dispatch(&engine, CourierCommand::Metrics, None).await;
        assert!(resp.is_ok());
        if let CourierResponse::Ok(v) = &resp {
            assert_eq!(v["total_deliveries"].as_u64().unwrap(), 1);
            assert_eq!(v["delivered"].as_u64().unwrap(), 1);
            assert_eq!(v["failed"].as_u64().unwrap(), 0);
            assert!(v["per_channel"].as_object().is_some());
        }
    }
}
