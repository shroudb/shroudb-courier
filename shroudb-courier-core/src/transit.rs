//! Transit decryptor — connects to a ShrouDB Transit server to decrypt
//! Transit-encrypted recipient ciphertexts.

use std::sync::Arc;

use shroudb_crypto::SecretBytes;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::error::CourierError;

/// A connection to a Transit server over TCP.
struct TransitConnection {
    reader: BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
    writer: BufWriter<Box<dyn tokio::io::AsyncWrite + Unpin + Send>>,
}

impl TransitConnection {
    async fn connect(addr: &str) -> Result<Self, CourierError> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| CourierError::TransitConnectionFailed(format!("{addr}: {e}")))?;
        let (r, w) = tokio::io::split(stream);
        Ok(Self {
            reader: BufReader::new(Box::new(r)),
            writer: BufWriter::new(Box::new(w)),
        })
    }

    async fn connect_tls(addr: &str) -> Result<Self, CourierError> {
        let mut root_store = rustls::RootCertStore::empty();
        let native_certs = rustls_native_certs::load_native_certs();
        for cert in native_certs.certs {
            root_store.add(cert).map_err(|e| {
                CourierError::TransitConnectionFailed(format!("failed to add root cert: {e}"))
            })?;
        }

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| CourierError::TransitConnectionFailed(format!("{addr}: {e}")))?;

        let host = addr
            .split(':')
            .next()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                CourierError::TransitConnectionFailed(format!(
                    "cannot extract hostname from: {addr}"
                ))
            })?;
        let domain = rustls_pki_types::ServerName::try_from(host.to_string()).map_err(|e| {
            CourierError::TransitConnectionFailed(format!("invalid server name: {e}"))
        })?;

        let tls_stream = connector.connect(domain, stream).await.map_err(|e| {
            CourierError::TransitConnectionFailed(format!("TLS handshake failed: {e}"))
        })?;

        let (r, w) = tokio::io::split(tls_stream);
        Ok(Self {
            reader: BufReader::new(Box::new(r)),
            writer: BufWriter::new(Box::new(w)),
        })
    }

    /// Send a command and read the response.
    async fn send_command(&mut self, args: &[&str]) -> Result<String, CourierError> {
        // Write command frame.
        self.writer
            .write_all(format!("*{}\r\n", args.len()).as_bytes())
            .await
            .map_err(|e| CourierError::TransitConnectionFailed(format!("write error: {e}")))?;
        for arg in args {
            let bytes = arg.as_bytes();
            self.writer
                .write_all(format!("${}\r\n", bytes.len()).as_bytes())
                .await
                .map_err(|e| CourierError::TransitConnectionFailed(format!("write error: {e}")))?;
            self.writer
                .write_all(bytes)
                .await
                .map_err(|e| CourierError::TransitConnectionFailed(format!("write error: {e}")))?;
            self.writer
                .write_all(b"\r\n")
                .await
                .map_err(|e| CourierError::TransitConnectionFailed(format!("write error: {e}")))?;
        }
        self.writer
            .flush()
            .await
            .map_err(|e| CourierError::TransitConnectionFailed(format!("flush error: {e}")))?;

        // Read response.
        self.read_response().await
    }

    async fn read_response(&mut self) -> Result<String, CourierError> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|e| CourierError::TransitDecryptFailed(format!("read error: {e}")))?;
        if n == 0 {
            return Err(CourierError::TransitConnectionFailed(
                "connection closed".into(),
            ));
        }
        let line = line.trim_end_matches("\r\n").trim_end_matches('\n');

        if line.is_empty() {
            return Err(CourierError::TransitDecryptFailed("empty response".into()));
        }

        let type_byte = line.as_bytes()[0];
        let payload = &line[1..];

        match type_byte {
            b'+' => Ok(payload.to_string()),
            b'-' => Err(CourierError::TransitDecryptFailed(payload.to_string())),
            b'$' => {
                let len: i64 = payload
                    .parse()
                    .map_err(|e| CourierError::TransitDecryptFailed(format!("bad length: {e}")))?;
                if len < 0 {
                    return Err(CourierError::TransitDecryptFailed("null response".into()));
                }
                let len = len as usize;
                let mut buf = vec![0u8; len + 2];
                self.reader
                    .read_exact(&mut buf)
                    .await
                    .map_err(|e| CourierError::TransitDecryptFailed(format!("read error: {e}")))?;
                let s = String::from_utf8_lossy(&buf[..len]).to_string();
                Ok(s)
            }
            _ => {
                // For map responses, extract the "plaintext" field if present.
                // Fall back to raw payload.
                Ok(payload.to_string())
            }
        }
    }

    /// Send AUTH command.
    async fn auth(&mut self, token: &str) -> Result<(), CourierError> {
        let result = self.send_command(&["AUTH", token]).await?;
        if result.contains("ERR") || result.contains("DENIED") {
            return Err(CourierError::TransitConnectionFailed(format!(
                "auth failed: {result}"
            )));
        }
        Ok(())
    }
}

/// Transit decryptor with lazy persistent connection.
pub struct TransitDecryptor {
    addr: String,
    tls: bool,
    keyring: String,
    auth_token: Option<String>,
    connection: Mutex<Option<TransitConnection>>,
}

impl TransitDecryptor {
    /// Create a new Transit decryptor.
    ///
    /// `uri` format: `host:port` (plain TCP) or parsed from config.
    /// `keyring` — the Transit keyring to use for decryption.
    /// `token` — optional auth token for Transit.
    pub fn new(addr: &str, tls: bool, keyring: &str, token: Option<&str>) -> Self {
        Self {
            addr: addr.to_string(),
            tls,
            keyring: keyring.to_string(),
            auth_token: token.map(String::from),
            connection: Mutex::new(None),
        }
    }

    /// Decrypt a Transit ciphertext. Returns the plaintext wrapped in SecretBytes
    /// for automatic zeroize-on-drop.
    pub async fn decrypt(&self, ciphertext: &str) -> Result<SecretBytes, CourierError> {
        let mut guard = self.connection.lock().await;

        // Try with existing connection first.
        if let Some(ref mut conn) = *guard {
            match conn
                .send_command(&["DECRYPT", &self.keyring, ciphertext])
                .await
            {
                Ok(plaintext) => return Ok(SecretBytes::new(plaintext.into_bytes())),
                Err(_) => {
                    // Connection broken, will reconnect below.
                    *guard = None;
                }
            }
        }

        // Establish new connection.
        let mut conn = if self.tls {
            TransitConnection::connect_tls(&self.addr).await?
        } else {
            TransitConnection::connect(&self.addr).await?
        };

        // Authenticate if needed.
        if let Some(ref token) = self.auth_token {
            conn.auth(token).await?;
        }

        let plaintext = conn
            .send_command(&["DECRYPT", &self.keyring, ciphertext])
            .await?;

        *guard = Some(conn);
        Ok(SecretBytes::new(plaintext.into_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transit_decryptor_construction() {
        let dec = TransitDecryptor::new("127.0.0.1:6399", false, "my-keyring", Some("tok123"));
        assert_eq!(dec.addr, "127.0.0.1:6399");
        assert_eq!(dec.keyring, "my-keyring");
        assert_eq!(dec.auth_token.as_deref(), Some("tok123"));
        assert!(!dec.tls);
    }

    #[test]
    fn transit_decryptor_no_auth() {
        let dec = TransitDecryptor::new("transit.local:6399", true, "ring1", None);
        assert!(dec.auth_token.is_none());
        assert!(dec.tls);
    }
}
