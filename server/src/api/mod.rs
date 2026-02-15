pub mod handlers;
pub mod models;

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
    Router,
};
use std::sync::Arc;

use crate::state::AppState;

fn api_v1_routes(write_body_limit: usize) -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/refresh", post(handlers::refresh_token))
        .route("/auth/revoke", post(handlers::revoke_token))
        .route(
            "/namespaces",
            post(handlers::create_namespace).get(handlers::list_namespaces),
        )
        .route("/namespaces/{ns}", get(handlers::get_namespace))
        .route("/stat", get(handlers::stat))
        .route("/wstat", post(handlers::wstat))
        .route("/statfs", get(handlers::statfs))
        .route("/open", post(handlers::open))
        .route("/read", post(handlers::read))
        .route(
            "/write",
            post(handlers::write).layer(DefaultBodyLimit::max(write_body_limit)),
        )
        .route("/download", get(handlers::download))
        .route(
            "/upload",
            put(handlers::upload).layer(DefaultBodyLimit::max(write_body_limit)),
        )
        .route("/close", post(handlers::close))
        .route("/readdir", get(handlers::readdir))
        .route("/remove", delete(handlers::remove))
        .route("/capabilities", get(handlers::capabilities))
        .route("/mounts", get(handlers::list_mounts))
}

pub fn create_router(
    state: Arc<AppState>,
    write_body_limit: usize,
    prometheus_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
) -> Router {
    let v1 = api_v1_routes(write_body_limit);

    let mut router = Router::new()
        .route("/health", get(handlers::health))
        .nest("/api/v1", v1.clone())
        .nest("/{tenant_id}/api/v1", v1)
        .with_state(state);

    if let Some(handle) = prometheus_handle {
        router = router.route(
            "/metrics",
            get(fs9_server::metrics::metrics_handler).with_state(handle),
        );
    }

    router
}
