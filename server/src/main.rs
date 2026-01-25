#![allow(missing_docs)]

mod api;
mod auth;
mod state;

use axum::middleware;
use fs9_core::{MemoryFs, StreamFS};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use auth::{AuthState, JwtConfig};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let state = Arc::new(state::AppState::new());

    state
        .mount_table
        .mount("/", "memfs", Arc::new(MemoryFs::new()))
        .await
        .expect("Failed to mount root filesystem");

    tracing::info!("Mounted MemoryFs at /");

    state
        .mount_table
        .mount("/streamfs", "streamfs", Arc::new(StreamFS::default()))
        .await
        .expect("Failed to mount streamfs");

    tracing::info!("Mounted StreamFS at /streamfs");

    let jwt_secret = std::env::var("FS9_JWT_SECRET").unwrap_or_else(|_| {
        tracing::warn!("FS9_JWT_SECRET not set, authentication disabled");
        String::new()
    });

    let auth_state = if jwt_secret.is_empty() {
        AuthState::disabled()
    } else {
        AuthState::new(JwtConfig::new(jwt_secret))
    };

    let app = api::create_router(state)
        .layer(middleware::from_fn_with_state(auth_state, auth::auth_middleware))
        .layer(TraceLayer::new_for_http());

    let host = std::env::var("FS9_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("FS9_PORT").unwrap_or_else(|_| "9999".to_string());
    let addr = format!("{}:{}", host, port);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("FS9 Server listening on http://{}", addr);

    axum::serve(listener, app).await.unwrap();
}
