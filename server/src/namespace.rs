use fs9_core::{HandleRegistry, MountTable, PluginManager, VfsRouter};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::state::HandleMap;

/// Per-namespace isolated state: each namespace gets its own VFS, mounts, handles.
pub struct Namespace {
    pub name: String,
    pub vfs: Arc<VfsRouter>,
    pub mount_table: Arc<MountTable>,
    pub handle_registry: Arc<HandleRegistry>,
    pub handle_map: Arc<RwLock<HandleMap>>,
}

impl Namespace {
    #[must_use]
    pub fn new(name: &str, handle_ttl: Duration) -> Self {
        let mount_table = Arc::new(MountTable::new());
        let handle_registry = Arc::new(HandleRegistry::new(handle_ttl));
        let vfs = Arc::new(VfsRouter::new(mount_table.clone(), handle_registry.clone()));

        Self {
            name: name.to_string(),
            vfs,
            mount_table,
            handle_registry,
            handle_map: Arc::new(RwLock::new(HandleMap::new())),
        }
    }
}

/// Default namespace name used when auth is disabled or JWT has no `ns` field.
pub const DEFAULT_NAMESPACE: &str = "default";

/// Manages all namespaces. Provides get-or-create semantics for lazy initialization.
pub struct NamespaceManager {
    namespaces: RwLock<HashMap<String, Arc<Namespace>>>,
    handle_ttl: Duration,
}

impl NamespaceManager {
    #[must_use]
    pub fn new(handle_ttl: Duration) -> Self {
        Self {
            namespaces: RwLock::new(HashMap::new()),
            handle_ttl,
        }
    }

    /// Get an existing namespace or create a new empty one.
    pub async fn get_or_create(&self, name: &str) -> Arc<Namespace> {
        // Fast path: read lock
        {
            let namespaces = self.namespaces.read().await;
            if let Some(ns) = namespaces.get(name) {
                return ns.clone();
            }
        }

        // Slow path: write lock + create
        let mut namespaces = self.namespaces.write().await;
        // Double-check after acquiring write lock
        if let Some(ns) = namespaces.get(name) {
            return ns.clone();
        }

        let ns = Arc::new(Namespace::new(name, self.handle_ttl));
        namespaces.insert(name.to_string(), ns.clone());
        tracing::info!(namespace = %name, "Created new namespace");
        ns
    }

    /// Get a namespace if it exists (no creation).
    pub async fn get(&self, name: &str) -> Option<Arc<Namespace>> {
        self.namespaces.read().await.get(name).cloned()
    }

    /// Insert a pre-built namespace (used during startup for config-defined namespaces).
    pub async fn insert(&self, ns: Arc<Namespace>) {
        self.namespaces
            .write()
            .await
            .insert(ns.name.clone(), ns);
    }

    /// List all namespace names.
    pub async fn list(&self) -> Vec<String> {
        self.namespaces.read().await.keys().cloned().collect()
    }
}
