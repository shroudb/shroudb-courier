mod common;

use common::*;
use shroudb_courier_client::CourierClient;

#[tokio::test]
async fn test_health() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();
    let resp = client.health().await.unwrap();
    assert_eq!(resp["status"], "ok");
}

#[tokio::test]
async fn test_channel_lifecycle() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    // Create
    let resp = client
        .channel_create("test-webhook", "webhook", "{}")
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");
    assert_eq!(resp["name"], "test-webhook");

    // Get
    let resp = client.channel_get("test-webhook").await.unwrap();
    assert_eq!(resp["name"], "test-webhook");
    assert_eq!(resp["channel_type"], "webhook");
    assert_eq!(resp["enabled"], true);

    // List
    let resp = client.channel_list().await.unwrap();
    assert_eq!(resp["count"], 1);

    // Delete
    let resp = client.channel_delete("test-webhook").await.unwrap();
    assert_eq!(resp["status"], "ok");

    // Verify deleted
    let resp = client.channel_list().await.unwrap();
    assert_eq!(resp["count"], 0);
}

#[tokio::test]
async fn test_channel_create_email() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let smtp_config =
        r#"{"host":"smtp.test.com","port":587,"from_address":"test@test.com","starttls":true}"#;
    let resp = client
        .channel_create("email-prod", "email", smtp_config)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");
    assert_eq!(resp["channel_type"], "email");

    let got = client.channel_get("email-prod").await.unwrap();
    assert!(got["smtp"].is_object());
    assert_eq!(got["smtp"]["host"], "smtp.test.com");
}

#[tokio::test]
async fn test_channel_duplicate_rejected() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    client.channel_create("dup", "webhook", "{}").await.unwrap();
    let result = client.channel_create("dup", "webhook", "{}").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_channel_invalid_name() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let result = client.channel_create("has spaces", "webhook", "{}").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_channel_get_nonexistent() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let result = client.channel_get("ghost").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_channel_delete_nonexistent() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let result = client.channel_delete("ghost").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_deliver_nonexistent_channel() {
    let server = TestServer::start().await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let result = client
        .deliver(r#"{"channel":"ghost","recipient":"x","body":"test"}"#)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_config_seeded_channel() {
    let config = TestServerConfig {
        channels: vec![TestChannel {
            name: "seeded-hook".into(),
            channel_type: "webhook".into(),
        }],
        ..Default::default()
    };
    let server = TestServer::start_with_config(config).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let resp = client.channel_get("seeded-hook").await.unwrap();
    assert_eq!(resp["name"], "seeded-hook");
    assert_eq!(resp["channel_type"], "webhook");
}

// --- ACL tests ---

fn acl_config() -> TestServerConfig {
    TestServerConfig {
        tokens: vec![
            TestToken {
                raw: "admin-token".into(),
                tenant: "platform".into(),
                actor: "admin".into(),
                platform: true,
                grants: Vec::new(),
            },
            TestToken {
                raw: "app-token".into(),
                tenant: "tenant-a".into(),
                actor: "my-app".into(),
                platform: false,
                grants: vec![TestGrant {
                    namespace: "courier.test-hook.*".into(),
                    scopes: vec!["read".into(), "write".into()],
                }],
            },
            TestToken {
                raw: "readonly-token".into(),
                tenant: "tenant-a".into(),
                actor: "reader".into(),
                platform: false,
                grants: vec![TestGrant {
                    namespace: "courier.test-hook.*".into(),
                    scopes: vec!["read".into()],
                }],
            },
        ],
        channels: vec![TestChannel {
            name: "test-hook".into(),
            channel_type: "webhook".into(),
        }],
    }
}

#[tokio::test]
async fn test_acl_unauthenticated_public() {
    let server = TestServer::start_with_config(acl_config()).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let resp = client.health().await.unwrap();
    assert_eq!(resp["status"], "ok");

    let resp = client.channel_list().await.unwrap();
    assert!(resp["count"].is_number());
}

#[tokio::test]
async fn test_acl_unauthenticated_rejected() {
    let server = TestServer::start_with_config(acl_config()).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let result = client.channel_create("x", "webhook", "{}").await;
    assert!(result.is_err());

    let result = client.channel_get("test-hook").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_acl_admin_full_access() {
    let server = TestServer::start_with_config(acl_config()).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();
    client.auth("admin-token").await.unwrap();

    let resp = client.channel_get("test-hook").await.unwrap();
    assert_eq!(resp["name"], "test-hook");

    let resp = client
        .channel_create("new-hook", "webhook", "{}")
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    client.channel_delete("new-hook").await.unwrap();
}

#[tokio::test]
async fn test_acl_scoped_token() {
    let server = TestServer::start_with_config(acl_config()).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();
    client.auth("app-token").await.unwrap();

    let resp = client.channel_get("test-hook").await.unwrap();
    assert_eq!(resp["name"], "test-hook");

    let result = client.channel_create("x", "webhook", "{}").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_acl_wrong_token() {
    let server = TestServer::start_with_config(acl_config()).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();

    let result = client.auth("invalid-token").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_acl_readonly_token() {
    let server = TestServer::start_with_config(acl_config()).await;
    let mut client = CourierClient::connect(&server.tcp_addr).await.unwrap();
    client.auth("readonly-token").await.unwrap();

    let resp = client.channel_get("test-hook").await.unwrap();
    assert_eq!(resp["name"], "test-hook");

    let result = client
        .deliver(r#"{"channel":"test-hook","recipient":"x","body":"test"}"#)
        .await;
    assert!(result.is_err());
}
