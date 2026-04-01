use std::future::Future;
use std::pin::Pin;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Capability trait for sending event notifications through Courier.
///
/// In Moat, engines receive `Option<Arc<dyn CourierOps>>` and call
/// `courier.notify(channel, subject, body)` when lifecycle events occur
/// (key rotation, certificate expiry, etc.). This avoids a direct
/// dependency on the Courier engine — callers depend only on courier-core.
pub trait CourierOps: Send + Sync {
    fn notify(&self, channel: &str, subject: &str, body: &str) -> BoxFut<'_, ()>;
}
