//! FS9 Metadata Service library.
//!
//! This crate provides metadata management for FS9, including:
//! - Namespace (tenant) management
//! - User management
//! - Role-based access control
//! - API key management
//! - JWT token generation and validation

pub mod api;
pub mod auth;
pub mod db;
pub mod error;

use std::sync::Arc;

pub use db::MetaStore;
pub use error::MetaError;

/// Application state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database store.
    pub store: Arc<MetaStore>,
    /// JWT secret for token signing.
    pub jwt_secret: String,
    /// Optional admin key for protecting management endpoints.
    ///
    /// When set, callers must present it via `Authorization: Bearer ...` or `x-fs9-meta-key`.
    pub admin_key: Option<String>,
}

impl AppState {
    /// Create a new application state.
    #[must_use]
    pub fn new(store: MetaStore, jwt_secret: String, admin_key: Option<String>) -> Self {
        Self {
            store: Arc::new(store),
            jwt_secret,
            admin_key,
        }
    }
}
