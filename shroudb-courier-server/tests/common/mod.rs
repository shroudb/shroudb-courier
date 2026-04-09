use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};

pub struct TestServer {
    child: Child,
    pub tcp_addr: String,
    _data_dir: tempfile::TempDir,
    _config_dir: tempfile::TempDir,
}

pub struct TestToken {
    pub raw: String,
    pub tenant: String,
    pub actor: String,
    pub platform: bool,
    pub grants: Vec<TestGrant>,
}

pub struct TestGrant {
    pub namespace: String,
    pub scopes: Vec<String>,
}

#[derive(Default)]
pub struct TestServerConfig {
    pub tokens: Vec<TestToken>,
    pub channels: Vec<TestChannel>,
}

pub struct TestChannel {
    pub name: String,
    pub channel_type: String,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with_config(TestServerConfig::default()).await
    }

    pub async fn start_with_config(config: TestServerConfig) -> Self {
        let binary = find_binary();
        let port = free_port();
        let tcp_addr = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();

        let config_content = generate_config(&tcp_addr, &config);
        let config_path = config_dir.path().join("config.toml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(config_content.as_bytes()).unwrap();

        let child = Command::new(&binary)
            .arg("--config")
            .arg(config_path.to_str().unwrap())
            .arg("--data-dir")
            .arg(data_dir.path().to_str().unwrap())
            .arg("--log-level")
            .arg("warn")
            .env("SHROUDB_MASTER_KEY", "42".repeat(32))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", binary.display()));

        let mut server = TestServer {
            child,
            tcp_addr: tcp_addr.clone(),
            _data_dir: data_dir,
            _config_dir: config_dir,
        };

        // Poll for readiness
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if std::time::Instant::now() > deadline {
                server.kill();
                panic!("server failed to start within 10 seconds");
            }

            match shroudb_courier_client::CourierClient::connect(&tcp_addr).await {
                Ok(mut client) => match client.health().await {
                    Ok(_) => break,
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
                },
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }

        server
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.kill();
    }
}

fn find_binary() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let binary = PathBuf::from(manifest_dir).join("../target/debug/shroudb-courier");
    if binary.exists() {
        return binary;
    }
    panic!(
        "shroudb-courier binary not found at {}. Run `cargo build -p shroudb-courier-server` first.",
        binary.display()
    );
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn generate_config(tcp_addr: &str, config: &TestServerConfig) -> String {
    let mut toml = String::new();

    // Root-level keys must come before any [section] header
    toml.push_str(&format!(
        r#"policy_mode = "open"

[server]
tcp_bind = "{tcp_addr}"
log_level = "warn"

[store]
mode = "embedded"
"#
    ));

    if config.tokens.is_empty() {
        toml.push_str("\n[auth]\n");
    } else {
        toml.push_str("\n[auth]\nmethod = \"token\"\n\n");
        for token in &config.tokens {
            toml.push_str(&format!(
                "[auth.tokens.\"{}\"]\ntenant = \"{}\"\nactor = \"{}\"\nplatform = {}\n",
                token.raw, token.tenant, token.actor, token.platform
            ));
            if !token.grants.is_empty() {
                toml.push_str("grants = [\n");
                for grant in &token.grants {
                    let scopes: Vec<String> =
                        grant.scopes.iter().map(|s| format!("\"{s}\"")).collect();
                    toml.push_str(&format!(
                        "    {{ namespace = \"{}\", scopes = [{}] }},\n",
                        grant.namespace,
                        scopes.join(", ")
                    ));
                }
                toml.push_str("]\n");
            }
            toml.push('\n');
        }
    }

    for ch in &config.channels {
        toml.push_str(&format!(
            "[channels.{}]\nchannel_type = \"{}\"\n\n",
            ch.name, ch.channel_type
        ));
        if ch.channel_type == "webhook" {
            toml.push_str(&format!("[channels.{}.webhook]\n\n", ch.name));
        }
    }

    toml
}
