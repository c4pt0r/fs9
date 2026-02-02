#![allow(missing_docs)]

mod api;
mod auth;
mod state;

use axum::middleware;
use fs9_config::Fs9Config;
use fs9_core::{default_registry, ProviderConfig};
use std::path::Path;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use auth::{AuthState, JwtConfig};

#[tokio::main]
async fn main() {
    let config = fs9_config::load().unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load config: {e}, using defaults");
        Fs9Config::default()
    });

    init_logging(&config);

    let state = Arc::new(state::AppState::new());
    let registry = default_registry();

    load_plugins(&state, &config);
    setup_mounts(&state, &registry, &config).await;

    let auth_state = if config.server.auth.enabled && !config.server.auth.jwt_secret.is_empty() {
        AuthState::new(JwtConfig::new(config.server.auth.jwt_secret.clone()))
    } else {
        if config.server.auth.enabled {
            tracing::warn!("Auth enabled but jwt_secret is empty, disabling auth");
        }
        AuthState::disabled()
    };

    let app = api::create_router(state)
        .layer(middleware::from_fn_with_state(auth_state, auth::auth_middleware))
        .layer(TraceLayer::new_for_http());

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("FS9 Server listening on http://{}", addr);

    axum::serve(listener, app).await.unwrap();
}

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
    for mount in &config.mounts {
        let config_json = mount
            .config
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default())
            .unwrap_or_default();

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
                if let Err(e) = state.mount_table.mount(&mount.path, &mount.provider, p).await {
                    tracing::error!(path = %mount.path, error = %e, "Failed to mount");
                } else {
                    tracing::info!(path = %mount.path, provider = %mount.provider, "Mounted");
                }
            }
            Err(e) => {
                tracing::error!(path = %mount.path, provider = %mount.provider, error = %e, "Failed to create provider");
            }
        }
    }
}
