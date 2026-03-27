//! Integration tests for ShrouDB Courier — notification delivery.
//!
//! Handler-level tests: exercises every command through the CommandDispatcher
//! without network overhead. Tests TEMPLATE_RELOAD/LIST/INFO, HEALTH,
//! CONFIG GET/SET/LIST, CHANNEL_LIST/INFO/CONNECTIONS (WS disabled + enabled),
//! plus auth enforcement.
//!
//! Note: DELIVER is not tested here because it requires a real Transit connection
//! for recipient decryption. DELIVER tests belong in e2e with Moat.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use shroudb_courier_core::adapter::AdapterRegistry;
use shroudb_courier_core::template::TemplateEngine;
use shroudb_courier_core::transit::TransitDecryptor;
use shroudb_courier_core::ws::ChannelRegistry;

use shroudb_courier_protocol::auth::{AuthPolicy, AuthRegistry};
use shroudb_courier_protocol::{Command, CommandDispatcher, CommandResponse, ResponseValue};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn setup_dispatcher(templates_dir: &std::path::Path) -> CommandDispatcher {
    let template_engine = TemplateEngine::load_dir(templates_dir).unwrap();
    let adapters = AdapterRegistry::new();
    // TransitDecryptor pointing to a dummy address — we won't actually call DELIVER
    let transit = TransitDecryptor::new("127.0.0.1:0", false, "test", None);

    CommandDispatcher::new(
        Arc::new(RwLock::new(template_engine)),
        Arc::new(adapters),
        Arc::new(transit),
        Arc::new(AuthRegistry::permissive()),
        templates_dir.to_path_buf(),
    )
}

fn setup_dispatcher_with_ws(templates_dir: &std::path::Path) -> CommandDispatcher {
    let mut dispatcher = setup_dispatcher(templates_dir);
    let ws_registry = ChannelRegistry::new(100, 50, 256);
    dispatcher.set_ws_registry(ws_registry);
    dispatcher
}

fn is_success(resp: &CommandResponse) -> bool {
    matches!(resp, CommandResponse::Success(_))
}

fn is_error(resp: &CommandResponse) -> bool {
    matches!(resp, CommandResponse::Error(_))
}

fn error_code(resp: &CommandResponse) -> &'static str {
    match resp {
        CommandResponse::Error(e) => e.error_code(),
        _ => panic!("expected error, got success"),
    }
}

fn field_str(resp: &CommandResponse, key: &str) -> String {
    match resp {
        CommandResponse::Success(map) => map
            .fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| match v {
                ResponseValue::String(s) => s.clone(),
                ResponseValue::Integer(n) => n.to_string(),
                ResponseValue::Boolean(b) => b.to_string(),
                other => format!("{other:?}"),
            })
            .unwrap_or_else(|| {
                let keys: Vec<&str> = map.fields.iter().map(|(k, _)| k.as_str()).collect();
                panic!("field '{key}' not found, available: {keys:?}")
            }),
        other => panic!("expected Success, got: {other:?}"),
    }
}

fn field_int(resp: &CommandResponse, key: &str) -> i64 {
    match resp {
        CommandResponse::Success(map) => map
            .fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| match v {
                ResponseValue::Integer(n) => *n,
                other => panic!("field '{key}' is not an integer: {other:?}"),
            })
            .unwrap_or_else(|| panic!("field '{key}' not found")),
        other => panic!("expected Success, got: {other:?}"),
    }
}

/// Write a test template to the given directory.
fn write_template(dir: &std::path::Path, name: &str, subject: &str, body: &str) {
    std::fs::write(
        dir.join(format!("{name}.subject.txt")),
        subject,
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("{name}.body.html")),
        body,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// HEALTH
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::Health, None).await;
    assert!(is_success(&resp), "HEALTH should succeed: {resp:?}");
    assert_eq!(field_str(&resp, "status"), "OK");
}

// ---------------------------------------------------------------------------
// PING
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ping_returns_pong() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::Ping, None).await;
    assert!(is_success(&resp), "PING should succeed: {resp:?}");
    assert_eq!(field_str(&resp, "message"), "PONG");
}

// ---------------------------------------------------------------------------
// COMMAND LIST
// ---------------------------------------------------------------------------

#[tokio::test]
async fn command_list_returns_all_verbs() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::CommandList, None).await;
    assert!(is_success(&resp), "COMMAND LIST should succeed: {resp:?}");
    let count = field_int(&resp, "count");
    assert!(count >= 10, "should have at least 10 commands, got {count}");
}

// ---------------------------------------------------------------------------
// TEMPLATE_LIST / TEMPLATE_INFO / TEMPLATE_RELOAD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_list_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::TemplateList, None).await;
    assert!(is_success(&resp), "TEMPLATE_LIST should succeed: {resp:?}");
    assert_eq!(field_int(&resp, "count"), 0);
}

#[tokio::test]
async fn template_list_returns_loaded_templates() {
    let tmp = tempfile::tempdir().unwrap();
    write_template(tmp.path(), "welcome", "Welcome!", "<h1>Hello</h1>");
    write_template(tmp.path(), "password-reset", "Reset", "<p>Reset link</p>");

    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::TemplateList, None).await;
    assert!(is_success(&resp), "TEMPLATE_LIST should succeed: {resp:?}");
    assert_eq!(field_int(&resp, "count"), 2);
}

#[tokio::test]
async fn template_info_returns_details() {
    let tmp = tempfile::tempdir().unwrap();
    write_template(tmp.path(), "welcome", "Welcome {{name}}", "<h1>Hello {{name}}</h1>");

    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher
        .execute(Command::TemplateInfo { name: "welcome".into() }, None)
        .await;
    assert!(is_success(&resp), "TEMPLATE_INFO should succeed: {resp:?}");
    assert_eq!(field_str(&resp, "name"), "welcome");
}

#[tokio::test]
async fn template_info_nonexistent_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher
        .execute(Command::TemplateInfo { name: "nonexistent".into() }, None)
        .await;
    assert!(is_error(&resp), "nonexistent template should error: {resp:?}");
    assert_eq!(error_code(&resp), "NOTFOUND");
}

#[tokio::test]
async fn template_reload_picks_up_new_templates() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    // Initially empty
    let resp = dispatcher.execute(Command::TemplateList, None).await;
    assert_eq!(field_int(&resp, "count"), 0);

    // Write a template
    write_template(tmp.path(), "new-template", "Subject", "<p>Body</p>");

    // Reload
    let reload_resp = dispatcher.execute(Command::TemplateReload, None).await;
    assert!(is_success(&reload_resp), "TEMPLATE_RELOAD should succeed: {reload_resp:?}");
    assert_eq!(field_int(&reload_resp, "templates_loaded"), 1);

    // List should now show it
    let list_resp = dispatcher.execute(Command::TemplateList, None).await;
    assert_eq!(field_int(&list_resp, "count"), 1);
}

// ---------------------------------------------------------------------------
// CONFIG
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_get_returns_value() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher
        .execute(Command::ConfigGet { key: "templates.dir".into() }, None)
        .await;
    assert!(is_success(&resp), "CONFIG GET should succeed: {resp:?}");
    // The value should be the templates dir path
    let value = field_str(&resp, "value");
    assert!(!value.is_empty());
}

#[tokio::test]
async fn config_get_unknown_key_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher
        .execute(Command::ConfigGet { key: "nonexistent".into() }, None)
        .await;
    assert!(is_error(&resp), "unknown config key should error: {resp:?}");
}

#[tokio::test]
async fn config_list_returns_all() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::ConfigList, None).await;
    match &resp {
        CommandResponse::Success(map) => {
            assert!(!map.fields.is_empty(), "should have at least 1 config entry");
        }
        other => panic!("CONFIG LIST should succeed: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// CHANNEL_LIST / CHANNEL_INFO / CONNECTIONS (WS disabled)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_list_without_ws_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::ChannelList, None).await;
    assert!(is_error(&resp), "CHANNEL_LIST without WS should error: {resp:?}");
}

#[tokio::test]
async fn connections_without_ws_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher(tmp.path());

    let resp = dispatcher.execute(Command::Connections, None).await;
    assert!(is_error(&resp), "CONNECTIONS without WS should error: {resp:?}");
}

// ---------------------------------------------------------------------------
// CHANNEL_LIST / CHANNEL_INFO / CONNECTIONS (WS enabled)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_list_with_ws_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher_with_ws(tmp.path());

    let resp = dispatcher.execute(Command::ChannelList, None).await;
    assert!(is_success(&resp), "CHANNEL_LIST with WS should succeed: {resp:?}");
}

#[tokio::test]
async fn connections_with_ws_returns_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher_with_ws(tmp.path());

    let resp = dispatcher.execute(Command::Connections, None).await;
    assert!(is_success(&resp), "CONNECTIONS with WS should succeed: {resp:?}");
    assert_eq!(field_int(&resp, "connections"), 0);
}

#[tokio::test]
async fn channel_info_with_ws_nonexistent_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = setup_dispatcher_with_ws(tmp.path());

    let resp = dispatcher
        .execute(Command::ChannelInfo { channel: "nonexistent".into() }, None)
        .await;
    assert!(is_success(&resp), "CHANNEL_INFO should succeed for unknown channel: {resp:?}");
    assert_eq!(field_int(&resp, "subscribers"), 0);
}

// ---------------------------------------------------------------------------
// AUTH ENFORCEMENT
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_required_without_policy_returns_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let template_engine = TemplateEngine::load_dir(tmp.path()).unwrap();
    let adapters = AdapterRegistry::new();
    let transit = TransitDecryptor::new("127.0.0.1:0", false, "test", None);

    let mut policies = HashMap::new();
    policies.insert("valid-token".to_string(), AuthPolicy::system());
    let auth = AuthRegistry::new(policies, true);

    let dispatcher = CommandDispatcher::new(
        Arc::new(RwLock::new(template_engine)),
        Arc::new(adapters),
        Arc::new(transit),
        Arc::new(auth),
        tmp.path().to_path_buf(),
    );

    let resp = dispatcher.execute(Command::TemplateList, None).await;
    assert!(is_error(&resp));
    assert_eq!(error_code(&resp), "DENIED");
}

#[tokio::test]
async fn auth_health_always_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let template_engine = TemplateEngine::load_dir(tmp.path()).unwrap();
    let adapters = AdapterRegistry::new();
    let transit = TransitDecryptor::new("127.0.0.1:0", false, "test", None);
    let auth = AuthRegistry::new(HashMap::new(), true);

    let dispatcher = CommandDispatcher::new(
        Arc::new(RwLock::new(template_engine)),
        Arc::new(adapters),
        Arc::new(transit),
        Arc::new(auth),
        tmp.path().to_path_buf(),
    );

    let resp = dispatcher.execute(Command::Health, None).await;
    assert!(is_success(&resp), "HEALTH should always be allowed");
}
