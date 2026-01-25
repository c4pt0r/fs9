use fs9_sdk::{FileInfo, FsError, FsProvider, FsResult, Handle, OpenFlags};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

pub type HandleId = u64;

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

pub struct HandleRegistry {
    handles: RwLock<HashMap<HandleId, HandleState>>,
    next_id: AtomicU64,
    ttl: Duration,
}

impl HandleRegistry {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            handles: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            ttl,
        }
    }

    pub async fn register(
        &self,
        provider: Arc<dyn FsProvider>,
        path: String,
        flags: OpenFlags,
        metadata: FileInfo,
        provider_handle: Handle,
    ) -> HandleId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
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

        self.handles.write().await.insert(id, state);
        id
    }

    pub async fn get(&self, id: HandleId) -> Option<HandleRef<'_>> {
        let handles = self.handles.read().await;
        if handles.contains_key(&id) {
            Some(HandleRef {
                registry: self,
                id,
            })
        } else {
            None
        }
    }

    pub async fn with_handle<F, R>(&self, id: HandleId, f: F) -> FsResult<R>
    where
        F: FnOnce(&HandleState) -> R,
    {
        let handles = self.handles.read().await;
        let state = handles.get(&id).ok_or(FsError::invalid_handle(id))?;
        *state.last_access.write().await = Instant::now();
        Ok(f(state))
    }

    pub async fn close(&self, id: HandleId, sync: bool) -> FsResult<()> {
        let state = self
            .handles
            .write()
            .await
            .remove(&id)
            .ok_or(FsError::invalid_handle(id))?;

        state.provider.close(state.provider_handle, sync).await
    }

    pub async fn cleanup_stale(&self) -> Vec<HandleId> {
        let now = Instant::now();
        let mut to_close = Vec::new();
        let mut closed_ids = Vec::new();

        {
            let mut handles = self.handles.write().await;
            let stale_ids: Vec<HandleId> = {
                let mut stale = Vec::new();
                for (id, state) in handles.iter() {
                    let last_access = *state.last_access.read().await;
                    if now.duration_since(last_access) > self.ttl {
                        stale.push(*id);
                    }
                }
                stale
            };

            for id in stale_ids {
                if let Some(state) = handles.remove(&id) {
                    to_close.push((id, state));
                }
            }
        }

        for (id, state) in to_close {
            let _ = state.provider.close(state.provider_handle, false).await;
            closed_ids.push(id);
        }

        closed_ids
    }

    pub async fn count(&self) -> usize {
        self.handles.read().await.len()
    }

    pub async fn list_handles(&self) -> Vec<HandleInfo> {
        let handles = self.handles.read().await;
        let mut infos = Vec::with_capacity(handles.len());

        for (id, state) in handles.iter() {
            infos.push(HandleInfo {
                id: *id,
                path: state.path.clone(),
                flags: state.flags,
                created_at: state.created_at,
                last_access: *state.last_access.read().await,
            });
        }

        infos
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFs;

    #[tokio::test]
    async fn register_and_get_handle() {
        let registry = HandleRegistry::new(Duration::from_secs(300));
        let fs = Arc::new(MemoryFs::new());

        let provider_handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        let metadata = fs.stat("/test.txt").await.unwrap();

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

        let provider_handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        let metadata = fs.stat("/test.txt").await.unwrap();

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

        let provider_handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        let metadata = fs.stat("/test.txt").await.unwrap();

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

        let h1 = fs.open("/file1.txt", OpenFlags::create_file()).await.unwrap();
        let m1 = fs.stat("/file1.txt").await.unwrap();
        registry
            .register(fs.clone(), "/file1.txt".to_string(), OpenFlags::create_file(), m1, h1)
            .await;

        let h2 = fs.open("/file2.txt", OpenFlags::create_file()).await.unwrap();
        let m2 = fs.stat("/file2.txt").await.unwrap();
        registry
            .register(fs.clone(), "/file2.txt".to_string(), OpenFlags::create_file(), m2, h2)
            .await;

        let handles = registry.list_handles().await;
        assert_eq!(handles.len(), 2);
    }
}
