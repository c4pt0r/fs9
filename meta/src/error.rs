//! Error types for fs9-meta service.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Meta service error type.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Already exists: {0}")]
    AlreadyExists(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for MetaError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::AlreadyExists(msg) => (StatusCode::CONFLICT, msg.clone()),
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            Self::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Self::Database(msg) | Self::Config(msg) | Self::Internal(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
        };

        let body = Json(json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

impl From<sqlx::Error> for MetaError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => Self::NotFound("Record not found".into()),
            sqlx::Error::Database(db_err) => {
                let msg = db_err.message();
                if msg.contains("UNIQUE constraint failed") || msg.contains("duplicate key") {
                    Self::AlreadyExists("Record already exists".into())
                } else {
                    Self::Database(msg.to_string())
                }
            }
            _ => Self::Database(err.to_string()),
        }
    }
}

impl From<jsonwebtoken::errors::Error> for MetaError {
    fn from(err: jsonwebtoken::errors::Error) -> Self {
        Self::Unauthorized(format!("JWT error: {err}"))
    }
}
