//! Handle registry with sharded locking for high-concurrency scenarios.
//!
//! The registry uses 64 independent shards to reduce lock contention under heavy load.
//! Each shard is a separate `RwLock<HashMap>`, and handles are distributed across shards
//! based on their ID modulo the shard count.

use fs9_sdk::{FileInfo, FsError, FsProvider, FsResult, Handle, OpenFlags};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub type HandleId = u64;

/// Number of shards for the handle registry.
/// 64 provides good balance between concurrency and memory overhead.
const NUM_SHARDS: usize = 64;

pub struct HandleState {
    pub provider: Arc<dyn FsProvider>,
    pub path: String,
    pub flags: OpenFlags,
    pub created_at: Instant,
    pub last_access: RwLock<Instant>,
    pub metadata: FileInfo,
    pub provider_handle: Handle,
}

impl std::fmt::Debug for HandleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandleState")
            .field("path", &self.path)
            .field("flags", &self.flags)
            .field("created_at", &self.created_at)
            .field("metadata", &self.metadata)
            .field("provider_handle", &self.provider_handle)
            .finish_non_exhaustive()
    }
}

/// A single shard containing a subset of handles.
struct Shard {
    handles: RwLock<HashMap<HandleId, HandleState>>,
}

impl Shard {
    fn new() -> Self {
        Self {
            handles: RwLock::new(HashMap::new()),
        }
    }
}

/// Thread-safe handle registry with sharded locking.
///
/// Handles are distributed across 64 shards based on their ID to reduce
/// lock contention under high concurrency. This provides near-linear
/// scalability for read operations.
pub struct HandleRegistry {
    shards: Vec<Shard>,
    next_id: AtomicU64,
    ttl: Duration,
}

impl HandleRegistry {
    /// Create a new handle registry with the specified TTL for stale handle cleanup.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        let shards = (0..NUM_SHARDS).map(|_| Shard::new()).collect();
        Self {
            shards,
            next_id: AtomicU64::new(1),
            ttl,
        }
    }

    /// Get the shard for a given handle ID.
    #[inline]
    fn shard_for(&self, id: HandleId) -> &Shard {
        &self.shards[(id as usize) % NUM_SHARDS]
    }

    /// Register a new handle and return its ID.
    ///
    /// This operation acquires a write lock only on the target shard,
    /// allowing concurrent registrations to different shards.
    pub async fn register(
        &self,
        provider: Arc<dyn FsProvider>,
        path: String,
        flags: OpenFlags,
        metadata: FileInfo,
        provider_handle: Handle,
    ) -> HandleId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();

        let state = HandleState {
            provider,
            path,
            flags,
            created_at: now,
            last_access: RwLock::new(now),
            metadata,
            provider_handle,
        };

        self.shard_for(id).handles.write().await.insert(id, state);
        id
    }

    /// Get a handle reference if it exists.
    pub async fn get(&self, id: HandleId) -> Option<HandleRef<'_>> {
        let shard = self.shard_for(id);
        let handles = shard.handles.read().await;
        if handles.contains_key(&id) {
            Some(HandleRef { registry: self, id })
        } else {
            None
        }
    }

    /// Execute a function with access to a handle's state.
    ///
    /// Updates the handle's last access time.
    pub async fn with_handle<F, R>(&self, id: HandleId, f: F) -> FsResult<R>
    where
        F: FnOnce(&HandleState) -> R,
    {
        let shard = self.shard_for(id);
        let handles = shard.handles.read().await;
        let state = handles.get(&id).ok_or(FsError::invalid_handle(id))?;
        *state.last_access.write().await = Instant::now();
        Ok(f(state))
    }

    /// Close a handle, removing it from the registry.
    ///
    /// Calls the provider's close method before removing.
    pub async fn close(&self, id: HandleId, sync: bool) -> FsResult<()> {
        let shard = self.shard_for(id);
        let state = shard
            .handles
            .write()
            .await
            .remove(&id)
            .ok_or(FsError::invalid_handle(id))?;

        state.provider.close(state.provider_handle, sync).await
    }

    /// Clean up stale handles that have exceeded their TTL.
    ///
    /// This iterates all shards and removes handles that haven't been
    /// accessed within the TTL period. Returns the IDs of closed handles.
    pub async fn cleanup_stale(&self) -> Vec<HandleId> {
        let now = Instant::now();
        let mut all_closed = Vec::new();

        for shard in &self.shards {
            let mut to_close = Vec::new();

            // First pass: identify stale handles (read lock)
            {
                let handles = shard.handles.read().await;
                for (id, state) in handles.iter() {
                    let last_access = *state.last_access.read().await;
                    if now.duration_since(last_access) > self.ttl {
                        to_close.push(*id);
                    }
                }
            }

            // Second pass: remove stale handles (write lock)
            if !to_close.is_empty() {
                let mut handles = shard.handles.write().await;
                for id in to_close {
                    if let Some(state) = handles.remove(&id) {
                        // Close the provider handle (ignore errors during cleanup)
                        let _ = state.provider.close(state.provider_handle, false).await;
                        all_closed.push(id);
                    }
                }
            }
        }

        all_closed
    }

    /// Get the total count of handles across all shards.
    pub async fn count(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.handles.read().await.len();
        }
        total
    }

    /// List information about all handles.
    ///
    /// Acquires read locks on all shards sequentially. O(shards) lock acquisitions.
    pub async fn list_handles(&self) -> Vec<HandleInfo> {
        let mut infos = Vec::new();

        for shard in &self.shards {
            let handles = shard.handles.read().await;
            for (id, state) in handles.iter() {
                infos.push(HandleInfo {
                    id: *id,
                    path: state.path.clone(),
                    flags: state.flags,
                    created_at: state.created_at,
                    last_access: *state.last_access.read().await,
                });
            }
        }

        infos
    }

    /// Get the TTL configured for this registry.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Get the number of shards (for diagnostics).
    #[must_use]
    pub const fn num_shards(&self) -> usize {
        NUM_SHARDS
    }
}

pub struct HandleRef<'a> {
    registry: &'a HandleRegistry,
    id: HandleId,
}

impl<'a> HandleRef<'a> {
    pub async fn provider(&self) -> FsResult<Arc<dyn FsProvider>> {
        self.registry
            .with_handle(self.id, |s| s.provider.clone())
            .await
    }

    pub async fn provider_handle(&self) -> FsResult<Handle> {
        self.registry
            .with_handle(self.id, |s| s.provider_handle)
            .await
    }

    pub async fn path(&self) -> FsResult<String> {
        self.registry.with_handle(self.id, |s| s.path.clone()).await
    }

    pub async fn flags(&self) -> FsResult<OpenFlags> {
        self.registry.with_handle(self.id, |s| s.flags).await
    }
}

#[derive(Debug, Clone)]
pub struct HandleInfo {
    pub id: HandleId,
    pub path: String,
    pub flags: OpenFlags,
    pub created_at: Instant,
    pub last_access: Instant,
}

/// Start a background task that periodically cleans up stale handles.
///
/// Returns a `JoinHandle` that can be used to cancel the task.
pub fn start_cleanup_task(
    registry: Arc<HandleRegistry>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let closed = registry.cleanup_stale().await;
            if !closed.is_empty() {
                tracing::debug!(count = closed.len(), "Cleaned up stale handles");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFs;

    #[tokio::test]
    async fn register_and_get_handle() {
        let registry = HandleRegistry::new(Duration::from_secs(300));
        let fs = Arc::new(MemoryFs::new());

        let (provider_handle, metadata) = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();

        let id = registry
            .register(
                fs.clone(),
                "/test.txt".to_string(),
                OpenFlags::create_file(),
                metadata,
                provider_handle,
            )
            .await;

        assert!(registry.get(id).await.is_some());
        assert!(registry.get(999).await.is_none());
        assert_eq!(registry.count().await, 1);
    }

    #[tokio::test]
    async fn close_handle() {
        let registry = HandleRegistry::new(Duration::from_secs(300));
        let fs = Arc::new(MemoryFs::new());

        let (provider_handle, metadata) = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();

        let id = registry
            .register(
                fs.clone(),
                "/test.txt".to_string(),
                OpenFlags::create_file(),
                metadata,
                provider_handle,
            )
            .await;

        registry.close(id, false).await.unwrap();
        assert!(registry.get(id).await.is_none());
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn cleanup_stale_handles() {
        let registry = HandleRegistry::new(Duration::from_millis(10));
        let fs = Arc::new(MemoryFs::new());

        let (provider_handle, metadata) = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();

        let id = registry
            .register(
                fs.clone(),
                "/test.txt".to_string(),
                OpenFlags::create_file(),
                metadata,
                provider_handle,
            )
            .await;

        tokio::time::sleep(Duration::from_millis(20)).await;

        let closed = registry.cleanup_stale().await;
        assert_eq!(closed, vec![id]);
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn list_handles() {
        let registry = HandleRegistry::new(Duration::from_secs(300));
        let fs = Arc::new(MemoryFs::new());

        let (h1, m1) = fs.open("/file1.txt", OpenFlags::create_file()).await.unwrap();
        registry
            .register(fs.clone(), "/file1.txt".to_string(), OpenFlags::create_file(), m1, h1)
            .await;

        let (h2, m2) = fs.open("/file2.txt", OpenFlags::create_file()).await.unwrap();
        registry
            .register(fs.clone(), "/file2.txt".to_string(), OpenFlags::create_file(), m2, h2)
            .await;

        let handles = registry.list_handles().await;
        assert_eq!(handles.len(), 2);
    }

    #[tokio::test]
    async fn sharding_distributes_handles() {
        let registry = HandleRegistry::new(Duration::from_secs(300));
        let fs = Arc::new(MemoryFs::new());

        // Register many handles to test distribution
        let mut ids = Vec::new();
        for i in 0..128 {
            let path = format!("/file{i}.txt");
            let (h, m) = fs.open(&path, OpenFlags::create_file()).await.unwrap();
            let id = registry.register(fs.clone(), path, OpenFlags::create_file(), m, h).await;
            ids.push(id);
        }

        assert_eq!(registry.count().await, 128);

        // Verify all handles are accessible
        for id in &ids {
            assert!(registry.get(*id).await.is_some());
        }

        // Verify handles are distributed across shards (not all in one)
        let mut non_empty_shards = 0;
        for shard in &registry.shards {
            if !shard.handles.read().await.is_empty() {
                non_empty_shards += 1;
            }
        }
        // With 128 handles and 64 shards, we should have at least 2 non-empty shards
        assert!(non_empty_shards >= 2, "Handles should be distributed across shards");
    }

    #[tokio::test]
    async fn concurrent_operations() {
        use std::sync::Arc;
        use tokio::task::JoinSet;

        let registry = Arc::new(HandleRegistry::new(Duration::from_secs(300)));
        let fs = Arc::new(MemoryFs::new());

        // Prepare files
        for i in 0..100 {
            let path = format!("/concurrent{i}.txt");
            let (h, _) = fs.open(&path, OpenFlags::create_file()).await.unwrap();
            fs.close(h, false).await.unwrap();
        }

        let mut tasks = JoinSet::new();

        // Spawn concurrent registrations
        for i in 0..100 {
            let reg = registry.clone();
            let fs_clone = fs.clone();
            tasks.spawn(async move {
                let path = format!("/concurrent{i}.txt");
                let (h, m) = fs_clone.open(&path, OpenFlags::read()).await.unwrap();
                reg.register(fs_clone, path, OpenFlags::read(), m, h).await
            });
        }

        let mut ids = Vec::new();
        while let Some(result) = tasks.join_next().await {
            ids.push(result.unwrap());
        }

        assert_eq!(registry.count().await, 100);

        // Concurrent reads
        let mut read_tasks = JoinSet::new();
        for id in ids.clone() {
            let reg = registry.clone();
            read_tasks.spawn(async move { reg.get(id).await.is_some() });
        }

        while let Some(result) = read_tasks.join_next().await {
            assert!(result.unwrap());
        }

        // Concurrent closes
        let mut close_tasks = JoinSet::new();
        for id in ids {
            let reg = registry.clone();
            close_tasks.spawn(async move { reg.close(id, false).await.is_ok() });
        }

        while let Some(result) = close_tasks.join_next().await {
            assert!(result.unwrap());
        }

        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn handle_ref_operations() {
        let registry = HandleRegistry::new(Duration::from_secs(300));
        let fs = Arc::new(MemoryFs::new());

        let (provider_handle, metadata) = fs.open("/ref_test.txt", OpenFlags::create_file()).await.unwrap();

        let id = registry
            .register(
                fs.clone(),
                "/ref_test.txt".to_string(),
                OpenFlags::create_file(),
                metadata,
                provider_handle,
            )
            .await;

        let handle_ref = registry.get(id).await.unwrap();
        
        let path = handle_ref.path().await.unwrap();
        assert_eq!(path, "/ref_test.txt");
        
        let flags = handle_ref.flags().await.unwrap();
        assert!(flags.create);
        
        let _ = handle_ref.provider().await.unwrap();
        let _ = handle_ref.provider_handle().await.unwrap();
    }
}
