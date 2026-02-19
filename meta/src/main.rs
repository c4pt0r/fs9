//! FS9 Metadata Service.
//!
//! A REST API service for managing FS9 metadata including namespaces,
//! users, roles, API keys, and JWT tokens.

use std::net::SocketAddr;
use std::path::Path;

use axum::Router;
use clap::Parser;
use serde::Deserialize;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use fs9_meta::{api, auth, AppState, MetaStore};

/// Configuration file structure for fs9-meta.
#[derive(Debug, Deserialize, Default)]
struct MetaConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    database: DatabaseConfig,
    #[serde(default)]
    auth: AuthConfig,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    9998
}

#[derive(Debug, Deserialize)]
struct DatabaseConfig {
    #[serde(default = "default_dsn")]
    dsn: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self { dsn: default_dsn() }
    }
}

fn default_dsn() -> String {
    "sqlite:fs9-meta.db".to_string()
}

#[derive(Debug, Deserialize, Default)]
struct AuthConfig {
    #[serde(default)]
    jwt_secret: String,
    #[serde(default)]
    admin_key: Option<String>,
}

/// FS9 Metadata Service.
#[derive(Parser)]
#[command(name = "fs9-meta")]
#[command(about = "FS9 Metadata Service - manages namespaces, users, tokens, and mounts")]
struct Args {
    /// Path to configuration file
    #[arg(short = 'c', long = "config")]
    config: Option<String>,

    /// Database DSN (e.g., "sqlite:fs9-meta.db" or "postgres://...")
    #[arg(long, env = "FS9_META_DSN")]
    dsn: Option<String>,

    /// Host to bind to
    #[arg(long, env = "FS9_META_HOST")]
    host: Option<String>,

    /// Port to listen on
    #[arg(long, env = "FS9_META_PORT")]
    port: Option<u16>,

    /// JWT secret for token signing
    #[arg(long, env = "FS9_JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Admin key required for /api/v1/* (recommended when binding non-loopback)
    #[arg(long, env = "FS9_META_KEY")]
    admin_key: Option<String>,
}

fn load_config(path: &str) -> Result<MetaConfig, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: MetaConfig = serde_yaml::from_str(&content)?;
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();

    // Load config file if specified
    let file_config = if let Some(ref path) = args.config {
        if !Path::new(path).exists() {
            eprintln!("Error: Config file not found: {path}");
            std::process::exit(1);
        }
        load_config(path)?
    } else {
        MetaConfig::default()
    };

    // CLI args override config file values
    let dsn = args.dsn.unwrap_or(file_config.database.dsn);
    let host = args.host.unwrap_or(file_config.server.host);
    let port = args.port.unwrap_or(file_config.server.port);
    let jwt_secret = args.jwt_secret.unwrap_or(file_config.auth.jwt_secret);
    let admin_key = args.admin_key.or(file_config.auth.admin_key);

    if jwt_secret.is_empty() {
        eprintln!("Error: JWT secret is required. Set via --jwt-secret, FS9_JWT_SECRET env, or config file.");
        std::process::exit(1);
    }

    // If we're binding to a non-loopback address, require an admin key to avoid exposing
    // management endpoints unauthenticated.
    let is_loopback_host = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if admin_key.is_none() && !is_loopback_host {
        eprintln!(
            "Error: admin key is required when binding to non-loopback host '{host}'. Set via --admin-key, FS9_META_KEY env, or config file."
        );
        std::process::exit(1);
    }
    if admin_key.is_none() {
        tracing::warn!(
            host = %host,
            "fs9-meta admin key is not configured; /api/v1/* is unauthenticated (loopback only is recommended)"
        );
    }

    // Connect to database
    tracing::info!(dsn = %dsn, "Connecting to database");
    let store = MetaStore::connect(&dsn).await?;

    // Run migrations
    tracing::info!("Running migrations");
    store.migrate().await?;

    // Create app state
    let state = AppState::new(store, jwt_secret, admin_key);

    // Build router
    let api_router = api::router().layer(axum::middleware::from_fn_with_state(
        state.clone(),
        auth::require_admin_key,
    ));
    let app = Router::new()
        .nest("/api/v1", api_router)
        .route("/health", axum::routing::get(health))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!(%addr, "Starting fs9-meta server");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Health check endpoint.
async fn health() -> &'static str {
    "ok"
}
