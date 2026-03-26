//! `shroudb-courier-client` — typed Rust client library for ShrouDB Courier.
//!
//! Provides a high-level async API for interacting with a Courier server over TCP.
//! The wire protocol is handled internally — callers never deal with raw frames.

pub mod connection;
pub mod error;
pub mod response;

pub use error::ClientError;
pub use response::{DeliverResult, HealthResult, Response, TemplateInfoResult, TemplateListResult};

use connection::Connection;

/// Default Courier server port.
const DEFAULT_PORT: u16 = 6999;

/// Parsed components of a Courier connection URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub auth_token: Option<String>,
}

/// Parse a Courier connection URI.
///
/// Format: `shroudb-courier://[token@]host[:port]`
///         `shroudb-courier+tls://[token@]host[:port]`
///
/// # Examples
///
/// ```
/// use shroudb_courier_client::parse_uri;
///
/// let cfg = parse_uri("shroudb-courier://localhost").unwrap();
/// assert_eq!(cfg.host, "localhost");
/// assert_eq!(cfg.port, 6999);
/// assert!(!cfg.tls);
///
/// let cfg = parse_uri("shroudb-courier+tls://mytoken@prod.example.com:7100").unwrap();
/// assert!(cfg.tls);
/// assert_eq!(cfg.auth_token.as_deref(), Some("mytoken"));
/// assert_eq!(cfg.host, "prod.example.com");
/// assert_eq!(cfg.port, 7100);
/// ```
pub fn parse_uri(uri: &str) -> Result<ConnectionConfig, ClientError> {
    let (tls, rest) = if let Some(rest) = uri.strip_prefix("shroudb-courier+tls://") {
        (true, rest)
    } else if let Some(rest) = uri.strip_prefix("shroudb-courier://") {
        (false, rest)
    } else {
        return Err(ClientError::Protocol(format!("invalid URI scheme: {uri}")));
    };

    let (auth_token, hostport) = if let Some(at_pos) = rest.find('@') {
        (Some(rest[..at_pos].to_string()), &rest[at_pos + 1..])
    } else {
        (None, rest)
    };

    // Strip trailing path if present
    let hostport = hostport.split('/').next().unwrap_or(hostport);

    let (host, port) = if let Some(colon_pos) = hostport.rfind(':') {
        let port_str = &hostport[colon_pos + 1..];
        match port_str.parse::<u16>() {
            Ok(p) => (hostport[..colon_pos].to_string(), p),
            Err(_) => (hostport.to_string(), DEFAULT_PORT),
        }
    } else {
        (hostport.to_string(), DEFAULT_PORT)
    };

    Ok(ConnectionConfig {
        host,
        port,
        tls,
        auth_token,
    })
}

/// A client for interacting with a ShrouDB Courier server.
pub struct CourierClient {
    connection: Connection,
}

impl CourierClient {
    /// Connect to a Courier server at the given address (e.g. `"127.0.0.1:6999"`).
    pub async fn connect(addr: &str) -> Result<Self, ClientError> {
        let connection = Connection::connect(addr).await?;
        Ok(Self { connection })
    }

    /// Connect to a Courier server over TLS.
    pub async fn connect_tls(addr: &str) -> Result<Self, ClientError> {
        let connection = Connection::connect_tls(addr).await?;
        Ok(Self { connection })
    }

    /// Connect using a URI string.
    ///
    /// Format: `shroudb-courier://[token@]host[:port]`
    ///         `shroudb-courier+tls://[token@]host[:port]`
    pub async fn from_uri(uri: &str) -> Result<Self, ClientError> {
        let config = parse_uri(uri)?;
        let addr = format!("{}:{}", config.host, config.port);
        let mut client = if config.tls {
            Self::connect_tls(&addr).await?
        } else {
            Self::connect(&addr).await?
        };
        if let Some(token) = &config.auth_token {
            client.auth(token).await?;
        }
        Ok(client)
    }

    /// Authenticate the connection with a bearer token.
    pub async fn auth(&mut self, token: &str) -> Result<(), ClientError> {
        let resp = self.connection.send_command_strs(&["AUTH", token]).await?;
        check_ok_status(resp)
    }

    /// Deliver a notification via JSON payload.
    pub async fn deliver(&mut self, json: &str) -> Result<DeliverResult, ClientError> {
        let resp = self
            .connection
            .send_command_strs(&["DELIVER", json])
            .await?;
        DeliverResult::from_response(resp)
    }

    /// List all loaded templates.
    pub async fn template_list(&mut self) -> Result<TemplateListResult, ClientError> {
        let resp = self
            .connection
            .send_command_strs(&["TEMPLATE_LIST"])
            .await?;
        TemplateListResult::from_response(resp)
    }

    /// Get information about a specific template.
    pub async fn template_info(&mut self, name: &str) -> Result<TemplateInfoResult, ClientError> {
        let resp = self
            .connection
            .send_command_strs(&["TEMPLATE_INFO", name])
            .await?;
        TemplateInfoResult::from_response(resp)
    }

    /// Reload all templates from disk.
    pub async fn template_reload(&mut self) -> Result<(), ClientError> {
        let resp = self
            .connection
            .send_command_strs(&["TEMPLATE_RELOAD"])
            .await?;
        check_ok_status(resp)
    }

    /// Check server health.
    pub async fn health(&mut self) -> Result<HealthResult, ClientError> {
        let resp = self.connection.send_command_strs(&["HEALTH"]).await?;
        HealthResult::from_response(resp)
    }

    /// Send an arbitrary command and return the raw server response.
    pub async fn raw_command(&mut self, args: &[&str]) -> Result<Response, ClientError> {
        self.connection.send_command_strs(args).await
    }
}

/// Check that a response indicates success (must be a Map, not an error or other type).
fn check_ok_status(resp: Response) -> Result<(), ClientError> {
    match &resp {
        Response::Error(e) => {
            if e.contains("DENIED") {
                Err(ClientError::AuthRequired)
            } else {
                Err(ClientError::Server(e.clone()))
            }
        }
        Response::Map(_) => Ok(()),
        other => Err(ClientError::Protocol(format!(
            "expected Map response, got {}",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uri_plain_host() {
        let cfg = parse_uri("shroudb-courier://localhost").unwrap();
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 6999);
        assert!(!cfg.tls);
        assert!(cfg.auth_token.is_none());
    }

    #[test]
    fn parse_uri_with_port() {
        let cfg = parse_uri("shroudb-courier://localhost:7100").unwrap();
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 7100);
    }

    #[test]
    fn parse_uri_tls() {
        let cfg = parse_uri("shroudb-courier+tls://prod.example.com").unwrap();
        assert!(cfg.tls);
        assert_eq!(cfg.host, "prod.example.com");
        assert_eq!(cfg.port, 6999);
    }

    #[test]
    fn parse_uri_with_auth() {
        let cfg = parse_uri("shroudb-courier://mytoken@localhost:6999").unwrap();
        assert_eq!(cfg.auth_token.as_deref(), Some("mytoken"));
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 6999);
    }

    #[test]
    fn parse_uri_full_form() {
        let cfg = parse_uri("shroudb-courier+tls://tok@host:7100").unwrap();
        assert!(cfg.tls);
        assert_eq!(cfg.auth_token.as_deref(), Some("tok"));
        assert_eq!(cfg.host, "host");
        assert_eq!(cfg.port, 7100);
    }

    #[test]
    fn parse_uri_invalid_scheme() {
        assert!(parse_uri("redis://localhost").is_err());
        assert!(parse_uri("http://localhost").is_err());
        assert!(parse_uri("shroudb-sentry://localhost").is_err());
    }

    #[test]
    fn parse_uri_default_port_on_invalid_port() {
        let cfg = parse_uri("shroudb-courier://localhost:notaport").unwrap();
        assert_eq!(cfg.port, 6999);
    }
}
