use shroudb_courier_core::{CourierError, DeliveryReceipt, RenderedMessage};
use std::future::Future;
use std::pin::Pin;

/// Decrypts Cipher-encrypted ciphertexts at delivery time.
///
/// Production: connects to a Cipher server over TCP.
/// Testing: passthrough that returns the input unchanged.
pub trait Decryptor: Send + Sync {
    fn decrypt<'a>(
        &'a self,
        ciphertext: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CourierError>> + Send + 'a>>;
}

/// Delivers a rendered message to a decrypted recipient.
///
/// Implementations: SMTP for email, HTTP POST for webhooks.
pub trait DeliveryAdapter: Send + Sync {
    fn deliver<'a>(
        &'a self,
        recipient: &'a str,
        message: &'a RenderedMessage,
    ) -> Pin<Box<dyn Future<Output = Result<DeliveryReceipt, CourierError>> + Send + 'a>>;
}
