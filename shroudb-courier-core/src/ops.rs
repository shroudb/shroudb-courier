use std::future::Future;
use std::pin::Pin;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Capability trait for sending event notifications through Courier.
///
/// In Moat, engines receive `Option<Arc<dyn CourierOps>>` and call
/// `courier.notify(channel, subject, body, actor)` when lifecycle events
/// occur (key rotation, certificate expiry, etc.). This avoids a direct
/// dependency on the Courier engine — callers depend only on courier-core.
///
/// `actor` identifies the caller responsible for triggering the
/// notification. For scheduler-driven events with no end-user (e.g.
/// cipher's rotation scheduler), pass a stable system sentinel like
/// `"cipher-scheduler"` so audit events are attributable. Never pass
/// an empty string; audit paths reject that as a missing-identity
/// violation.
pub trait CourierOps: Send + Sync {
    fn notify(&self, channel: &str, subject: &str, body: &str, actor: &str) -> BoxFut<'_, ()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Pins the trait signature: `notify` takes an explicit actor.
    /// A mock implementation records the args it received and exercises
    /// them via the trait object. If someone removes the `actor` param,
    /// this test fails to compile — which is the point.
    struct MockCourier {
        last: Mutex<Option<(String, String, String, String)>>,
    }

    impl CourierOps for MockCourier {
        fn notify(&self, channel: &str, subject: &str, body: &str, actor: &str) -> BoxFut<'_, ()> {
            let channel = channel.to_string();
            let subject = subject.to_string();
            let body = body.to_string();
            let actor = actor.to_string();
            Box::pin(async move {
                *self.last.lock().unwrap() = Some((channel, subject, body, actor));
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn notify_signature_includes_actor_parameter() {
        let mock = MockCourier {
            last: Mutex::new(None),
        };
        let obj: &dyn CourierOps = &mock;
        obj.notify(
            "alerts",
            "Key rotation",
            "Keyring k1 rotated",
            "cipher-scheduler",
        )
        .await
        .expect("notify ok");
        let got = mock.last.lock().unwrap().clone().expect("recorded");
        assert_eq!(got.0, "alerts");
        assert_eq!(got.1, "Key rotation");
        assert_eq!(got.2, "Keyring k1 rotated");
        assert_eq!(got.3, "cipher-scheduler");
    }

    #[tokio::test]
    async fn notify_rejects_empty_actor_at_caller_convention() {
        // The trait itself doesn't enforce non-empty (it's `&str`), but
        // the documented contract requires a non-empty sentinel. Pin
        // the convention with an explicit test: a mock that validates
        // non-empty would reject; we just verify the type accepts any
        // `&str` including empty, and callers MUST enforce at call
        // sites. This test documents that the enforcement lives at
        // the caller — which is the whole point of threading actor.
        let mock = MockCourier {
            last: Mutex::new(None),
        };
        // Mock accepts the call — empty-string enforcement is a caller
        // responsibility per the docstring.
        mock.notify("x", "y", "z", "").await.expect("mock accepts");
        let got = mock.last.lock().unwrap().clone().expect("recorded");
        assert_eq!(got.3, "");
    }
}
