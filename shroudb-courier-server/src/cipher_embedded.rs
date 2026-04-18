//! Embedded Cipher-backed Decryptor for the standalone Courier server.
//!
//! When `[cipher] mode = "embedded"` is set, Courier runs an in-process
//! `CipherEngine` on the same `StorageEngine` (distinct namespace) and
//! decrypts recipient ciphertexts directly, without a separate Cipher
//! deployment.
//!
//! Mirrors the `EmbeddedDecryptor` adapter that Moat uses when Cipher
//! is co-located — the wiring pattern is identical so operators get
//! the same security posture whether they deploy via Moat or as a
//! standalone Courier process.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use shroudb_cipher_engine::engine::CipherEngine;
use shroudb_courier_core::CourierError;
use shroudb_courier_engine::Decryptor;
use shroudb_store::Store;

pub struct EmbeddedDecryptor<S: Store> {
    engine: Arc<CipherEngine<S>>,
    keyring: String,
}

impl<S: Store> EmbeddedDecryptor<S> {
    pub fn new(engine: Arc<CipherEngine<S>>, keyring: impl Into<String>) -> Self {
        Self {
            engine,
            keyring: keyring.into(),
        }
    }
}

impl<S: Store + 'static> Decryptor for EmbeddedDecryptor<S> {
    fn decrypt<'a>(
        &'a self,
        ciphertext: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CourierError>> + Send + 'a>> {
        Box::pin(async move {
            let result = self
                .engine
                .decrypt(&self.keyring, ciphertext, None)
                .await
                .map_err(|e| CourierError::DecryptionFailed(format!("cipher decrypt: {e}")))?;
            String::from_utf8(result.plaintext.into_vec())
                .map_err(|e| CourierError::DecryptionFailed(format!("invalid utf8: {e}")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use shroudb_cipher_core::keyring::KeyringAlgorithm;
    use shroudb_cipher_engine::engine::CipherConfig;
    use shroudb_cipher_engine::keyring_manager::KeyringCreateOpts;
    use shroudb_server_bootstrap::Capability;

    async fn build_engine() -> Arc<CipherEngine<shroudb_storage::EmbeddedStore>> {
        let store =
            shroudb_storage::test_util::create_test_store("courier-embedded-cipher-test").await;
        let engine = CipherEngine::new(
            store,
            CipherConfig::default(),
            Capability::DisabledForTests,
            Capability::DisabledForTests,
        )
        .await
        .expect("cipher engine init");
        engine
            .keyring_manager()
            .create(
                "courier-recipients",
                KeyringAlgorithm::Aes256Gcm,
                KeyringCreateOpts::default(),
            )
            .await
            .expect("create keyring");
        Arc::new(engine)
    }

    #[tokio::test]
    async fn embedded_decryptor_round_trips_recipient() {
        let engine = build_engine().await;
        let keyring = "courier-recipients";
        let recipient = "alice@example.com";
        let plaintext_b64 = base64::engine::general_purpose::STANDARD.encode(recipient);

        let encrypted = engine
            .encrypt(keyring, &plaintext_b64, None, None, false)
            .await
            .expect("encrypt");

        let decryptor = EmbeddedDecryptor::new(engine, keyring);
        let decrypted = decryptor
            .decrypt(&encrypted.ciphertext)
            .await
            .expect("decrypt ok");
        assert_eq!(
            decrypted, recipient,
            "round-trip yields original recipient string"
        );
    }

    #[tokio::test]
    async fn embedded_decryptor_errors_on_garbage_ciphertext() {
        let engine = build_engine().await;
        let decryptor = EmbeddedDecryptor::new(engine, "courier-recipients");
        let result = decryptor.decrypt("not-a-real-ciphertext").await;
        assert!(
            result.is_err(),
            "garbage ciphertext must fail-closed, not return plaintext"
        );
    }
}
