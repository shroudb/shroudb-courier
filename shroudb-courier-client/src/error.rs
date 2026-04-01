use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("connection error: {0}")]
    Connection(String),

    #[error("server error: {0}")]
    Server(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("response format error: {0}")]
    ResponseFormat(String),
}

impl From<shroudb_client_common::ConnectionError> for ClientError {
    fn from(err: shroudb_client_common::ConnectionError) -> Self {
        match err {
            shroudb_client_common::ConnectionError::Io(e) => Self::Connection(e.to_string()),
            shroudb_client_common::ConnectionError::Protocol(s) => Self::Protocol(s),
            shroudb_client_common::ConnectionError::Server(s) => Self::Server(s),
        }
    }
}
