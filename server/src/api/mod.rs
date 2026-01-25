pub mod handlers;
pub mod models;

use axum::{
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;

use crate::state::AppState;

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/api/v1/stat", get(handlers::stat))
        .route("/api/v1/wstat", post(handlers::wstat))
        .route("/api/v1/statfs", get(handlers::statfs))
        .route("/api/v1/open", post(handlers::open))
        .route("/api/v1/read", post(handlers::read))
        .route("/api/v1/write", post(handlers::write))
        .route("/api/v1/close", post(handlers::close))
        .route("/api/v1/readdir", get(handlers::readdir))
        .route("/api/v1/remove", delete(handlers::remove))
        .route("/api/v1/capabilities", get(handlers::capabilities))
        .route("/api/v1/mounts", get(handlers::list_mounts))
        .with_state(state)
}
