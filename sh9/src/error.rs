//! Error types for sh9

use thiserror::Error;

/// Result type alias for sh9 operations
pub type Sh9Result<T> = Result<T, Sh9Error>;

/// Error types for sh9 shell operations
#[derive(Error, Debug)]
pub enum Sh9Error {
    /// Parse error in sh9script
    #[error("Parse error: {0}")]
    Parse(String),

    /// Runtime error during script execution
    #[error("Runtime error: {0}")]
    Runtime(String),

    /// IO error (file operations, etc.)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// FS9 client error
    #[error("FS9 error: {0}")]
    Fs9(String),

    /// Command not found
    #[error("Command not found: {0}")]
    CommandNotFound(String),

    /// Invalid argument
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    /// Variable not found
    #[error("Variable not found: {0}")]
    VariableNotFound(String),

    /// Function not found
    #[error("Function not found: {0}")]
    FunctionNotFound(String),

    /// Exit requested (not really an error)
    #[error("Exit with code {0}")]
    Exit(i32),
}

impl From<fs9_client::Fs9Error> for Sh9Error {
    fn from(err: fs9_client::Fs9Error) -> Self {
        Sh9Error::Fs9(err.to_string())
    }
}
