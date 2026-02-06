use fs9_core::{PluginManager, ProviderRegistry};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::meta_client::MetaClient;
use crate::namespace::{Namespace, NamespaceManager, DEFAULT_NAMESPACE};
use crate::token_cache::TokenCache;

pub struct AppState {
    pub namespace_manager: Arc<NamespaceManager>,
    pub plugin_manager: Arc<PluginManager>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub jwt_secret: RwLock<String>,
    /// Optional client for fs9-meta service integration.
    pub meta_client: Option<MetaClient>,
    /// Cache for validated tokens to reduce meta service load.
    pub token_cache: TokenCache,
}

pub struct HandleMap {
    active_handles: HashSet<u64>,
}

impl Default for HandleMap {
    fn default() -> Self {
        Self::new()
    }
}

impl HandleMap {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active_handles: HashSet::new(),
        }
    }

    pub fn insert(&mut self, id: u64) {
        self.active_handles.insert(id);
    }

    pub fn get_id(&self, handle_str: &str) -> Option<u64> {
        let id: u64 = handle_str.parse().ok()?;
        if self.active_handles.contains(&id) {
            Some(id)
        } else {
            None
        }
    }

    pub fn remove(&mut self, handle_str: &str) -> Option<u64> {
        let id: u64 = handle_str.parse().ok()?;
        if self.active_handles.remove(&id) {
            Some(id)
        } else {
            None
        }
    }
}

/// Default token cache TTL: 5 minutes.
const DEFAULT_TOKEN_CACHE_TTL: Duration = Duration::from_secs(300);

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(Duration::from_secs(300), None)
    }

    #[must_use]
    pub fn with_handle_ttl(ttl: Duration) -> Self {
        Self::with_options(ttl, None)
    }

    /// Create AppState with meta client integration.
    #[must_use]
    pub fn with_meta(meta_client: Option<MetaClient>) -> Self {
        Self::with_options(Duration::from_secs(300), meta_client)
    }

    /// Create AppState with all options.
    #[must_use]
    pub fn with_options(handle_ttl: Duration, meta_client: Option<MetaClient>) -> Self {
        let namespace_manager = Arc::new(NamespaceManager::new(handle_ttl));
        let plugin_manager = Arc::new(PluginManager::new());
        let provider_registry = Arc::new(fs9_core::default_registry());
        let token_cache = TokenCache::new(DEFAULT_TOKEN_CACHE_TTL);

        Self {
            namespace_manager,
            plugin_manager,
            provider_registry,
            jwt_secret: RwLock::new(String::new()),
            meta_client,
            token_cache,
        }
    }

    /// Set the JWT secret for token refresh
    pub async fn set_jwt_secret(&self, secret: String) {
        *self.jwt_secret.write().await = secret;
    }

    /// Get the default namespace, creating it if needed.
    pub async fn default_namespace(&self) -> Arc<Namespace> {
        self.namespace_manager.get_or_create(DEFAULT_NAMESPACE).await
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
