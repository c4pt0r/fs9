#![allow(missing_docs)]

mod api;

use fs9_server::auth;
use fs9_server::meta_client::MetaClient;
use fs9_server::metrics as fs9_metrics;
use fs9_server::namespace;
use fs9_server::rate_limit::{self, RateLimitState};
use fs9_server::state;
#[cfg(feature = "otel")]
use fs9_server::tracing_otel;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use clap::Parser;
use fs9_config::Fs9Config;
use fs9_core::{default_registry, ProviderConfig};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use auth::{AuthMiddlewareState, AuthState, JwtConfig};
use namespace::DEFAULT_NAMESPACE;

/// FS9 Server - Plan 9-inspired distributed filesystem server.
#[derive(Parser)]
#[command(name = "fs9-server")]
#[command(about = "FS9 distributed filesystem server")]
struct Args {
    /// Path to configuration file
    #[arg(short = 'c', long = "config", env = "FS9_CONFIG")]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let config = match &args.config {
        Some(path) => fs9_config::load_from_file(path).unwrap_or_else(|e| {
            eprintln!("Error: Failed to load config from {path}: {e}");
            std::process::exit(1);
        }),
        None => fs9_config::load().unwrap_or_else(|e| {
            eprintln!("Warning: Failed to load config: {e}, using defaults");
            Fs9Config::default()
        }),
    };

    #[cfg(feature = "otel")]
    let otel_provider = init_logging_with_otel(&config);
    #[cfg(not(feature = "otel"))]
    init_logging(&config);

    // meta_url is required unless FS9_SKIP_META_CHECK is set (for testing)
    let skip_meta_check = std::env::var("FS9_SKIP_META_CHECK").is_ok();
    let has_meta = config.server.meta_url.is_some();
    let meta_client = match config.server.meta_url.as_ref() {
        Some(url) => {
            tracing::info!(meta_url = %url, "Meta service integration enabled");
            Some(MetaClient::new(url, config.server.meta_key.clone()))
        }
        None if skip_meta_check => {
            tracing::warn!("Meta service integration disabled (FS9_SKIP_META_CHECK is set)");
            None
        }
        None => {
            eprintln!("Error: meta_url is required. Set FS9_META_ENDPOINTS or configure server.meta_url in fs9.yaml");
            std::process::exit(1);
        }
    };

    let state = Arc::new(state::AppState::with_meta(meta_client));
    let registry = default_registry();

    load_plugins(&state, &config);
    setup_mounts(&state, &registry, &config).await;

    let jwt_secret = if config.server.auth.jwt_secret.is_empty() {
        let generated = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        tracing::warn!("⚠️  jwt_secret is empty — generated a random secret. Tokens from previous runs will NOT work.");
        generated
    } else {
        config.server.auth.jwt_secret.clone()
    };

    // Store jwt_secret in app state for refresh endpoint
    state.set_jwt_secret(jwt_secret.clone()).await;

    let auth_enabled = config.server.auth.enabled || has_meta;
    let auth_state = AuthState::new(auth_enabled, JwtConfig::new(jwt_secret));
    let auth_middleware_state = AuthMiddlewareState::new(auth_state, Arc::clone(&state));

    let request_timeout = Duration::from_secs(config.server.request_timeout_secs.unwrap_or(30));
    let max_concurrent = config.server.max_concurrent_requests.unwrap_or(1000);
    let default_body_limit = config.server.max_body_size_bytes.unwrap_or(2 * 1024 * 1024);
    let write_body_limit = config.server.max_write_size_bytes.unwrap_or(256 * 1024 * 1024);

    let rate_limit_state = if config.server.rate_limit.enabled {
        RateLimitState::new(
            config.server.rate_limit.namespace_qps,
            config.server.rate_limit.user_qps,
        )
    } else {
        RateLimitState::disabled()
    };

    let prometheus_handle = if config.server.metrics.enabled {
        Some(fs9_metrics::init_metrics())
    } else {
        None
    };

    let mut app = api::create_router(state.clone(), write_body_limit, prometheus_handle.clone());

    if config.server.metrics.enabled {
        app = app.layer(middleware::from_fn(fs9_metrics::metrics_middleware));
    }

    let app = app
        .layer(middleware::from_fn_with_state(
            rate_limit_state,
            rate_limit::rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            auth_middleware_state,
            auth::auth_middleware,
        ))
        .layer(DefaultBodyLimit::max(default_body_limit))
        .layer(TimeoutLayer::new(request_timeout))
        .layer(ConcurrencyLimitLayer::new(max_concurrent))
        .layer(TraceLayer::new_for_http());

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("FS9 Server listening on http://{}", addr);

    let shutdown_state = state.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_state))
        .await
        .unwrap();

    #[cfg(feature = "otel")]
    if let Some(provider) = otel_provider {
        tracing_otel::shutdown_tracer(provider).await;
    }
}

async fn shutdown_signal(state: Arc<state::AppState>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("Received Ctrl+C, shutting down"),
        () = terminate => tracing::info!("Received SIGTERM, shutting down"),
    }

    tracing::info!("Draining open handles...");
    state.namespace_manager.drain_all().await;
    tracing::info!("Shutdown complete");
}

#[cfg(not(feature = "otel"))]
fn init_logging(config: &Fs9Config) {
    let filter = if config.logging.filter.is_empty() {
        config.logging.level.as_str().to_string()
    } else {
        config.logging.filter.clone()
    };

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::new(filter))
        .init();
}

#[cfg(feature = "otel")]
fn init_logging_with_otel(config: &Fs9Config) -> Option<opentelemetry_sdk::trace::TracerProvider> {
    let filter = if config.logging.filter.is_empty() {
        config.logging.level.as_str().to_string()
    } else {
        config.logging.filter.clone()
    };

    let otel_enabled = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();

    if otel_enabled {
        match tracing_otel::init_tracer("fs9-server") {
            Ok(provider) => {
                let tracer = tracing_otel::otel_tracer(&provider);
                tracing_subscriber::registry()
                    .with(tracing_subscriber::EnvFilter::new(filter))
                    .with(tracing_subscriber::fmt::layer())
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .init();
                tracing::info!("OpenTelemetry tracing enabled");
                return Some(provider);
            }
            Err(e) => {
                eprintln!("Warning: Failed to initialize OpenTelemetry: {e}, falling back to stdout tracing");
            }
        }
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::new(filter))
        .init();

    None
}

fn load_plugins(state: &state::AppState, config: &Fs9Config) {
    let mut total_loaded = 0;

    for dir in &config.server.plugins.directories {
        let path = Path::new(dir);
        if path.is_dir() {
            let count = state.plugin_manager.load_from_directory(path);
            if count > 0 {
                tracing::info!(dir = %dir, count = count, "Loaded plugins");
                total_loaded += count;
            }
        }
    }

    for entry in &config.server.plugins.preload {
        let path = Path::new(&entry.path);
        if path.exists() {
            match state.plugin_manager.load(&entry.name, path) {
                Ok(()) => {
                    tracing::info!(name = %entry.name, path = %entry.path, "Preloaded plugin");
                    total_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(name = %entry.name, error = %e, "Failed to preload plugin");
                }
            }
        }
    }

    if total_loaded > 0 {
        let plugins = state.plugin_manager.loaded_plugins();
        tracing::info!(plugins = ?plugins, "Available plugins");
    }
}

async fn setup_mounts(
    state: &Arc<state::AppState>,
    registry: &fs9_core::ProviderRegistry,
    config: &Fs9Config,
) {
    // All config-defined mounts go into the default namespace.
    let default_ns = match state.namespace_manager.create(DEFAULT_NAMESPACE, "system").await {
        Ok(ns) => ns,
        Err(_) => state.namespace_manager.get(DEFAULT_NAMESPACE).await.unwrap(),
    };

    for mount in &config.mounts {
        let config_json = {
            let mut cfg = mount.config.clone().unwrap_or(serde_json::Value::Object(Default::default()));
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert("ns".to_string(), serde_json::Value::String(DEFAULT_NAMESPACE.to_string()));
            }
            serde_json::to_string(&cfg).unwrap_or_default()
        };

        let provider_config = match &mount.config {
            Some(json) => {
                let mut pc = ProviderConfig::new();
                if let Some(obj) = json.as_object() {
                    for (k, v) in obj {
                        pc.options.insert(k.clone(), v.clone());
                    }
                }
                pc
            }
            None => ProviderConfig::new(),
        };

        let provider: Result<Arc<dyn fs9_sdk::FsProvider>, _> = if registry.has(&mount.provider) {
            registry.create(&mount.provider, provider_config)
        } else {
            match state.plugin_manager.create_provider(&mount.provider, &config_json) {
                Ok(p) => Ok(Arc::new(p) as Arc<dyn fs9_sdk::FsProvider>),
                Err(e) => {
                    tracing::error!(path = %mount.path, provider = %mount.provider, error = %e, "Unknown provider or creation failed");
                    continue;
                }
            }
        };

        match provider {
            Ok(p) => {
                if let Err(e) = default_ns.mount_table.mount(&mount.path, &mount.provider, p).await {
                    tracing::error!(path = %mount.path, error = %e, "Failed to mount");
                } else {
                    tracing::info!(path = %mount.path, provider = %mount.provider, ns = DEFAULT_NAMESPACE, "Mounted");
                }
            }
            Err(e) => {
                tracing::error!(path = %mount.path, provider = %mount.provider, error = %e, "Failed to create provider");
            }
        }
    }
}
