use shroudb_acl::AuthContext;
use shroudb_courier_engine::CourierEngine;
use shroudb_store::Store;

use crate::commands::CourierCommand;
use crate::response::CourierResponse;

pub async fn dispatch<S: Store>(
    engine: &CourierEngine<S>,
    cmd: CourierCommand,
    auth_context: Option<&AuthContext>,
) -> CourierResponse {
    let requirement = cmd.acl_requirement();
    if let Some(ctx) = auth_context
        && let Err(e) = ctx.check(&requirement)
    {
        return CourierResponse::error(format!("access denied: {e}"));
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

        CourierCommand::Deliver { request_json } => handle_deliver(engine, &request_json).await,

        CourierCommand::Health => {
            let channels = engine.channel_list();
            CourierResponse::ok(serde_json::json!({
                "status": "ok",
                "channels": channels.len(),
            }))
        }

        CourierCommand::Ping => CourierResponse::ok(serde_json::json!("PONG")),

        CourierCommand::CommandList => CourierResponse::ok(serde_json::json!({
            "commands": [
                "AUTH", "CHANNEL CREATE", "CHANNEL GET", "CHANNEL LIST", "CHANNEL DELETE",
                "DELIVER", "HEALTH", "PING", "COMMAND LIST"
            ],
            "count": 9
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
        let store = shroudb_storage::test_util::create_test_store("courier-test").await;
        let engine = CourierEngine::new(store, Some(Arc::new(MockDecryptor)), None)
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
            assert_eq!(v["count"], 9);
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
}
