use thiserror::Error;

#[derive(Debug, Error)]
pub enum CourierError {
    #[error("template not found: {0}")]
    TemplateNotFound(String),

    #[error("template render failed: {0}")]
    TemplateRenderFailed(String),

    #[error("adapter not found for channel: {0}")]
    AdapterNotFound(String),

    #[error("delivery failed: {0}")]
    DeliveryFailed(String),

    #[error("Transit connection failed: {0}")]
    TransitConnectionFailed(String),

    #[error("Transit decrypt failed: {0}")]
    TransitDecryptFailed(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),
}
