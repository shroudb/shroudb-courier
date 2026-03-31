use shroudb_courier_core::CourierError;
use shroudb_courier_engine::Decryptor;
use std::future::Future;
use std::pin::Pin;

/// Decrypts ciphertexts via a remote Cipher server.
///
/// Creates a fresh TCP connection per decrypt call to allow concurrent
/// decryption without serializing through a single connection. Connection
/// overhead (~2-5ms) is negligible relative to delivery latency (100ms+).
pub struct CipherDecryptor {
    addr: String,
    keyring: String,
    auth_token: Option<String>,
}

impl CipherDecryptor {
    /// Connect to the Cipher server and verify connectivity.
    ///
    /// Establishes an initial connection to validate the address and auth
    /// token, then stores the parameters for per-request connections.
    pub async fn new(
        addr: &str,
        keyring: &str,
        auth_token: Option<&str>,
    ) -> Result<Self, CourierError> {
        // Verify connectivity and auth on startup so misconfig fails fast.
        let mut client = shroudb_cipher_client::CipherClient::connect(addr)
            .await
            .map_err(|e| CourierError::Internal(format!("cipher connection failed: {e}")))?;

        if let Some(token) = auth_token {
            client
                .auth(token)
                .await
                .map_err(|e| CourierError::Internal(format!("cipher auth failed: {e}")))?;
        }

        Ok(Self {
            addr: addr.to_string(),
            keyring: keyring.to_string(),
            auth_token: auth_token.map(String::from),
        })
    }
}

impl Decryptor for CipherDecryptor {
    fn decrypt<'a>(
        &'a self,
        ciphertext: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CourierError>> + Send + 'a>> {
        Box::pin(async move {
            let mut client = shroudb_cipher_client::CipherClient::connect(&self.addr)
                .await
                .map_err(|e| CourierError::DecryptionFailed(format!("cipher connect: {e}")))?;

            if let Some(ref token) = self.auth_token {
                client
                    .auth(token)
                    .await
                    .map_err(|e| CourierError::DecryptionFailed(format!("cipher auth: {e}")))?;
            }

            let result = client
                .decrypt(&self.keyring, ciphertext, None)
                .await
                .map_err(|e| CourierError::DecryptionFailed(e.to_string()))?;

            // result.plaintext is base64-encoded — decode to string
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&result.plaintext)
                .map_err(|e| {
                    CourierError::DecryptionFailed(format!("base64 decode failed: {e}"))
                })?;

            String::from_utf8(bytes)
                .map_err(|e| CourierError::DecryptionFailed(format!("invalid UTF-8: {e}")))
        })
    }
}
