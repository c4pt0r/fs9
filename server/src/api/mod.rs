pub mod handlers;
pub mod models;

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
    Router,
};
use std::sync::Arc;

use crate::state::AppState;

pub fn create_router(
    state: Arc<AppState>,
    write_body_limit: usize,
    prometheus_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
) -> Router {
    let mut router = Router::new()
        .route("/health", get(handlers::health))
        .route("/api/v1/auth/refresh", post(handlers::refresh_token))
        .route("/api/v1/auth/revoke", post(handlers::revoke_token))
        .route(
            "/api/v1/namespaces",
            post(handlers::create_namespace).get(handlers::list_namespaces),
        )
        .route("/api/v1/namespaces/{ns}", get(handlers::get_namespace))
        .route("/api/v1/stat", get(handlers::stat))
        .route("/api/v1/wstat", post(handlers::wstat))
        .route("/api/v1/statfs", get(handlers::statfs))
        .route("/api/v1/open", post(handlers::open))
        .route("/api/v1/read", post(handlers::read))
        .route(
            "/api/v1/write",
            post(handlers::write).layer(DefaultBodyLimit::max(write_body_limit)),
        )
        .route("/api/v1/download", get(handlers::download))
        .route(
            "/api/v1/upload",
            put(handlers::upload).layer(DefaultBodyLimit::max(write_body_limit)),
        )
        .route("/api/v1/close", post(handlers::close))
        .route("/api/v1/readdir", get(handlers::readdir))
        .route("/api/v1/remove", delete(handlers::remove))
        .route("/api/v1/capabilities", get(handlers::capabilities))
        .route("/api/v1/mounts", get(handlers::list_mounts))
        .route("/api/v1/plugin/load", post(handlers::load_plugin))
        .route("/api/v1/plugin/unload", post(handlers::unload_plugin))
        .route("/api/v1/plugin/list", get(handlers::list_plugins))
        .route("/api/v1/mount", post(handlers::mount_plugin))
        .with_state(state);

    if let Some(handle) = prometheus_handle {
        router = router.route(
            "/metrics",
            get(fs9_server::metrics::metrics_handler).with_state(handle),
        );
    }

    router
}
