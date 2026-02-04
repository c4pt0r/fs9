use fs9_core::PluginManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::namespace::{Namespace, NamespaceManager, DEFAULT_NAMESPACE};

pub struct AppState {
    pub namespace_manager: Arc<NamespaceManager>,
    pub plugin_manager: Arc<PluginManager>,
}

pub struct HandleMap {
    uuid_to_id: HashMap<String, u64>,
    id_to_uuid: HashMap<u64, String>,
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
            uuid_to_id: HashMap::new(),
            id_to_uuid: HashMap::new(),
        }
    }

    pub fn insert(&mut self, uuid: String, id: u64) {
        self.uuid_to_id.insert(uuid.clone(), id);
        self.id_to_uuid.insert(id, uuid);
    }

    pub fn get_id(&self, uuid: &str) -> Option<u64> {
        self.uuid_to_id.get(uuid).copied()
    }

    #[allow(dead_code)]
    pub fn get_uuid(&self, id: u64) -> Option<&String> {
        self.id_to_uuid.get(&id)
    }

    pub fn remove_by_uuid(&mut self, uuid: &str) -> Option<u64> {
        if let Some(id) = self.uuid_to_id.remove(uuid) {
            self.id_to_uuid.remove(&id);
            Some(id)
        } else {
            None
        }
    }
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::with_handle_ttl(Duration::from_secs(300))
    }

    #[must_use]
    pub fn with_handle_ttl(ttl: Duration) -> Self {
        let namespace_manager = Arc::new(NamespaceManager::new(ttl));
        let plugin_manager = Arc::new(PluginManager::new());

        Self {
            namespace_manager,
            plugin_manager,
        }
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
