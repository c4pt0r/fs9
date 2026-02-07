use dashmap::DashMap;
use fs9_core::{start_cleanup_task, HandleRegistry, MountTable, VfsRouter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::state::HandleMap;

const HANDLE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Per-namespace isolated state: each namespace gets its own VFS, mounts, handles.
pub struct Namespace {
    pub name: String,
    pub vfs: Arc<VfsRouter>,
    pub mount_table: Arc<MountTable>,
    pub handle_registry: Arc<HandleRegistry>,
    pub handle_map: Arc<RwLock<HandleMap>>,
    #[allow(dead_code)]
    cleanup_task: tokio::task::JoinHandle<()>,
}

impl Namespace {
    #[must_use]
    pub fn new(name: &str, handle_ttl: Duration) -> Self {
        let mount_table = Arc::new(MountTable::new());
        let handle_registry = Arc::new(HandleRegistry::new(handle_ttl));
        let vfs = Arc::new(VfsRouter::new(mount_table.clone(), handle_registry.clone()));
        let cleanup_task = start_cleanup_task(handle_registry.clone(), HANDLE_CLEANUP_INTERVAL);

        Self {
            name: name.to_string(),
            vfs,
            mount_table,
            handle_registry,
            handle_map: Arc::new(RwLock::new(HandleMap::new())),
            cleanup_task,
        }
    }
}

/// Default namespace name used when auth is disabled or JWT has no `ns` field.
pub const DEFAULT_NAMESPACE: &str = "default";

/// Metadata about a namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceInfo {
    pub name: String,
    pub created_at: String,
    pub created_by: String,
    pub status: String,
}

/// Validate a namespace name: `[a-z0-9][a-z0-9_-]*`, length 1-64.
pub fn validate_namespace_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("Namespace name must be 1-64 characters".to_string());
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err("Namespace name must start with a lowercase letter or digit".to_string());
    }
    for &b in &bytes[1..] {
        if !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'_' && b != b'-' {
            return Err(
                "Namespace name may only contain lowercase letters, digits, underscores, and hyphens"
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// Generate a simple ISO 8601 UTC timestamp.
fn iso8601_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Convert Unix timestamp to civil date using Howard Hinnant's algorithm
    let z = (secs / 86400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1461 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let min = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
}

/// Manages all namespaces. Provides get-or-create semantics for lazy initialization.
pub struct NamespaceManager {
    namespaces: DashMap<String, (Arc<Namespace>, NamespaceInfo)>,
    handle_ttl: Duration,
}

impl NamespaceManager {
    #[must_use]
    pub fn new(handle_ttl: Duration) -> Self {
        Self {
            namespaces: DashMap::new(),
            handle_ttl,
        }
    }

    pub async fn create(&self, name: &str, created_by: &str) -> Result<Arc<Namespace>, String> {
        validate_namespace_name(name)?;

        if self.namespaces.contains_key(name) {
            return Err(format!("Namespace '{}' already exists", name));
        }

        let ns = Arc::new(Namespace::new(name, self.handle_ttl));
        let info = NamespaceInfo {
            name: name.to_string(),
            created_at: iso8601_now(),
            created_by: created_by.to_string(),
            status: "active".to_string(),
        };

        // Use entry API for atomicity
        match self.namespaces.entry(name.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(_) => {
                Err(format!("Namespace '{}' already exists", name))
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert((ns.clone(), info));
                tracing::info!(namespace = %name, created_by = %created_by, "Created namespace");
                Ok(ns)
            }
        }
    }

    pub async fn get_or_create(&self, name: &str) -> Arc<Namespace> {
        if let Some(entry) = self.namespaces.get(name) {
            return entry.value().0.clone();
        }

        let ns = Arc::new(Namespace::new(name, self.handle_ttl));
        let info = NamespaceInfo {
            name: name.to_string(),
            created_at: iso8601_now(),
            created_by: "system".to_string(),
            status: "active".to_string(),
        };

        let entry = self
            .namespaces
            .entry(name.to_string())
            .or_insert((ns, info));
        entry.value().0.clone()
    }

    pub async fn get(&self, name: &str) -> Option<Arc<Namespace>> {
        self.namespaces.get(name).map(|r| r.value().0.clone())
    }

    pub async fn exists(&self, name: &str) -> bool {
        self.namespaces.contains_key(name)
    }

    pub async fn insert(&self, ns: Arc<Namespace>) {
        let info = NamespaceInfo {
            name: ns.name.clone(),
            created_at: iso8601_now(),
            created_by: "system".to_string(),
            status: "active".to_string(),
        };
        self.namespaces.insert(ns.name.clone(), (ns, info));
    }

    pub async fn list(&self) -> Vec<String> {
        self.namespaces.iter().map(|r| r.key().clone()).collect()
    }

    pub async fn list_info(&self) -> Vec<NamespaceInfo> {
        self.namespaces
            .iter()
            .map(|r| r.value().1.clone())
            .collect()
    }

    pub async fn get_info(&self, name: &str) -> Option<NamespaceInfo> {
        self.namespaces.get(name).map(|r| r.value().1.clone())
    }

    pub async fn drain_all(&self) {
        let namespaces: Vec<Arc<Namespace>> = self
            .namespaces
            .iter()
            .map(|r| r.value().0.clone())
            .collect();
        for ns in namespaces {
            let count = ns.handle_registry.close_all().await;
            if count > 0 {
                tracing::info!(namespace = %ns.name, closed = count, "Drained handles");
            }
        }
    }
}
