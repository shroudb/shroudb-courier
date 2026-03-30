use shroudb_courier_core::CourierError;
use shroudb_courier_engine::Decryptor;
use std::future::Future;
use std::pin::Pin;

pub struct CipherDecryptor {
    client: tokio::sync::Mutex<shroudb_cipher_client::CipherClient>,
    keyring: String,
}

impl CipherDecryptor {
    pub async fn new(
        addr: &str,
        keyring: &str,
        auth_token: Option<&str>,
    ) -> Result<Self, CourierError> {
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
            client: tokio::sync::Mutex::new(client),
            keyring: keyring.to_string(),
        })
    }
}

impl Decryptor for CipherDecryptor {
    fn decrypt<'a>(
        &'a self,
        ciphertext: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CourierError>> + Send + 'a>> {
        Box::pin(async move {
            let mut client = self.client.lock().await;
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
