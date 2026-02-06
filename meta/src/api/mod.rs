//! REST API handlers for fs9-meta service.

mod apikey;
mod mount;
mod namespace;
mod token;
mod user;

use axum::{
    routing::{delete, get, post},
    Router,
};

use crate::AppState;

/// Create the API router.
pub fn router() -> Router<AppState> {
    Router::new()
        // Namespace routes
        .route("/namespaces", post(namespace::create))
        .route("/namespaces", get(namespace::list))
        .route("/namespaces/:name", get(namespace::get))
        .route("/namespaces/:name", delete(namespace::delete))
        // Mount routes
        .route("/namespaces/:namespace/mounts", post(mount::create))
        .route("/namespaces/:namespace/mounts", get(mount::list))
        .route("/namespaces/:namespace/mounts/*path", get(mount::get))
        .route("/namespaces/:namespace/mounts/*path", delete(mount::delete))
        // User routes - use :user_id consistently
        .route("/users", post(user::create))
        .route("/users", get(user::list))
        .route("/users/by-name/:username", get(user::get))
        .route("/users/:user_id", delete(user::delete))
        .route("/users/:user_id/roles", post(user::assign_role))
        .route("/users/:user_id/roles", get(user::get_roles))
        .route(
            "/users/:user_id/roles/:namespace/:role",
            delete(user::revoke_role),
        )
        // Token routes
        .route("/tokens/generate", post(token::generate))
        .route("/tokens/validate", post(token::validate))
        .route("/tokens/refresh", post(token::refresh))
        // API Key routes
        .route("/apikeys", post(apikey::create))
        .route("/apikeys", get(apikey::list))
        .route("/apikeys/validate", post(apikey::validate))
        .route("/apikeys/:id", delete(apikey::revoke))
}
