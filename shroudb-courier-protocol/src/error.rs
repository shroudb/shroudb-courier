use thiserror::Error;

/// Errors returned by command execution.
#[derive(Debug, Error)]
pub enum CommandError {
    #[error("bad argument: {message}")]
    BadArg { message: String },

    #[error("template not found: {0}")]
    TemplateNotFound(String),

    #[error("template render failed: {0}")]
    TemplateRenderFailed(String),

    #[error("adapter not found: {0}")]
    AdapterNotFound(String),

    #[error("delivery failed: {0}")]
    DeliveryFailed(String),

    #[error("transit error: {0}")]
    TransitError(String),

    #[error("authentication required")]
    AuthRequired,

    #[error("access denied: {reason}")]
    Denied { reason: String },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl CommandError {
    /// RESP3 error prefix for wire serialization.
    pub fn error_code(&self) -> &'static str {
        match self {
            CommandError::BadArg { .. } => "BADARG",
            CommandError::TemplateNotFound(_) => "NOTFOUND",
            CommandError::TemplateRenderFailed(_) => "RENDERFAIL",
            CommandError::AdapterNotFound(_) => "NOTFOUND",
            CommandError::DeliveryFailed(_) => "DELIVERYFAIL",
            CommandError::TransitError(_) => "TRANSITERR",
            CommandError::AuthRequired => "DENIED",
            CommandError::Denied { .. } => "DENIED",
            CommandError::InvalidRequest(_) => "BADARG",
            CommandError::Internal(_) => "INTERNAL",
        }
    }
}

impl From<shroudb_courier_core::error::CourierError> for CommandError {
    fn from(e: shroudb_courier_core::error::CourierError) -> Self {
        use shroudb_courier_core::error::CourierError as CE;
        match e {
            CE::TemplateNotFound(name) => CommandError::TemplateNotFound(name),
            CE::TemplateRenderFailed(msg) => CommandError::TemplateRenderFailed(msg),
            CE::AdapterNotFound(msg) => CommandError::AdapterNotFound(msg),
            CE::DeliveryFailed(msg) => CommandError::DeliveryFailed(msg),
            CE::TransitConnectionFailed(msg) => CommandError::TransitError(msg),
            CE::TransitDecryptFailed(msg) => CommandError::TransitError(msg),
            CE::InvalidRequest(msg) => CommandError::InvalidRequest(msg),
        }
    }
}
