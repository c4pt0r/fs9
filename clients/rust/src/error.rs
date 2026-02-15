use thiserror::Error;

#[derive(Debug, Error)]
pub enum Fs9Error {
    #[error("connection error: {0}")]
    Connection(String),

    #[error("request failed: {status} - {message}")]
    Request { status: u16, message: String },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("not a directory: {0}")]
    NotDirectory(String),

    #[error("is a directory: {0}")]
    IsDirectory(String),

    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),

    #[error("invalid handle")]
    InvalidHandle,

    #[error("server error: {0}")]
    Server(String),

    #[error("timeout")]
    Timeout,

    #[error("serialization error: {0}")]
    Serialization(String),
}

impl Fs9Error {
    pub(crate) fn from_response(status: u16, message: String) -> Self {
        let msg = message.trim().to_string();
        match status {
            404 => Self::NotFound(msg.strip_prefix("not found:").map(|s| s.trim().to_string()).unwrap_or(msg)),
            403 => Self::PermissionDenied(msg.strip_prefix("permission denied:").map(|s| s.trim().to_string()).unwrap_or(msg)),
            409 => Self::AlreadyExists(msg.strip_prefix("already exists:").map(|s| s.trim().to_string()).unwrap_or(msg)),
            400 => Self::InvalidArgument(msg.strip_prefix("invalid argument:").map(|s| s.trim().to_string()).unwrap_or(msg)),
            504 => Self::Timeout,
            500..=599 => Self::Server(msg),
            _ => Self::Request { status, message: msg },
        }
    }
}

impl From<reqwest::Error> for Fs9Error {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout
        } else if err.is_connect() {
            Self::Connection(err.to_string())
        } else {
            Self::Connection(err.to_string())
        }
    }
}

impl From<serde_json::Error> for Fs9Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Fs9Error>;
