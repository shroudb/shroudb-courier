use thiserror::Error;

#[derive(Debug, Error)]
pub enum CourierError {
    #[error("channel not found: {0}")]
    ChannelNotFound(String),

    #[error("channel already exists: {0}")]
    ChannelExists(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("invalid name: {0}")]
    InvalidName(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("delivery failed: {0}")]
    DeliveryFailed(String),

    #[error("adapter not configured for channel type: {0}")]
    AdapterNotConfigured(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("internal error: {0}")]
    Internal(String),
}
