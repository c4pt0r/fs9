use fs9_sdk::{Capabilities, FsError, FsProvider, FsResult};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub path: String,
    pub provider_name: String,
}

pub struct MountEntry {
    pub mount_point: MountPoint,
    pub provider: Arc<dyn FsProvider>,
}

impl std::fmt::Debug for MountEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountEntry")
            .field("mount_point", &self.mount_point)
            .finish_non_exhaustive()
    }
}

pub struct MountTable {
    mounts: RwLock<BTreeMap<String, MountEntry>>,
}

impl Default for MountTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MountTable {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mounts: RwLock::new(BTreeMap::new()),
        }
    }

    fn normalize_mount_path(path: &str) -> String {
        let path = path.trim();
        let path = if path.is_empty() { "/" } else { path };
        let path = if !path.starts_with('/') {
            format!("/{path}")
        } else {
            path.to_string()
        };
        if path.len() > 1 && path.ends_with('/') {
            path.trim_end_matches('/').to_string()
        } else {
            path
        }
    }

    pub async fn mount(
        &self,
        path: &str,
        provider_name: &str,
        provider: Arc<dyn FsProvider>,
    ) -> FsResult<()> {
        let path = Self::normalize_mount_path(path);
        let mut mounts = self.mounts.write().await;

        if mounts.contains_key(&path) {
            return Err(FsError::already_exists(&path));
        }

        mounts.insert(
            path.clone(),
            MountEntry {
                mount_point: MountPoint {
                    path,
                    provider_name: provider_name.to_string(),
                },
                provider,
            },
        );

        Ok(())
    }

    pub async fn unmount(&self, path: &str) -> FsResult<Arc<dyn FsProvider>> {
        let path = Self::normalize_mount_path(path);
        let mut mounts = self.mounts.write().await;

        mounts
            .remove(&path)
            .map(|e| e.provider)
            .ok_or_else(|| FsError::not_found(&path))
    }

    pub async fn resolve(&self, path: &str) -> FsResult<(Arc<dyn FsProvider>, String)> {
        let path = Self::normalize_mount_path(path);
        let mounts = self.mounts.read().await;

        // O(log n) resolution using BTreeMap ordering.
        // Iterate keys <= path in reverse to find longest prefix match first.
        for (mount_path, entry) in mounts.range(..=path.clone()).rev() {
            if path == *mount_path {
                return Ok((entry.provider.clone(), "/".to_string()));
            }
            if mount_path == "/" {
                return Ok((entry.provider.clone(), path));
            }
            if path.starts_with(mount_path) && path.as_bytes().get(mount_path.len()) == Some(&b'/')
            {
                let relative_path = path[mount_path.len()..].to_string();
                return Ok((entry.provider.clone(), relative_path));
            }
        }

        if let Some(entry) = mounts.get("/") {
            return Ok((entry.provider.clone(), path));
        }

        Err(FsError::not_found(&path))
    }

    pub async fn list_mounts(&self) -> Vec<MountPoint> {
        self.mounts
            .read()
            .await
            .values()
            .map(|e| e.mount_point.clone())
            .collect()
    }

    pub async fn get_mount_info(&self, path: &str) -> Option<(MountPoint, Capabilities)> {
        let path = Self::normalize_mount_path(path);
        let mounts = self.mounts.read().await;

        mounts
            .get(&path)
            .map(|e| (e.mount_point.clone(), e.provider.capabilities()))
    }

    pub async fn count(&self) -> usize {
        self.mounts.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFs;

    #[tokio::test]
    async fn mount_and_resolve() {
        let table = MountTable::new();
        let fs = Arc::new(MemoryFs::new());

        table.mount("/", "root", fs.clone()).await.unwrap();

        let (provider, relative) = table.resolve("/test/file.txt").await.unwrap();
        assert_eq!(relative, "/test/file.txt");
        assert!(Arc::ptr_eq(&provider, &(fs.clone() as Arc<dyn FsProvider>)));
    }

    #[tokio::test]
    async fn nested_mounts() {
        let table = MountTable::new();
        let root_fs = Arc::new(MemoryFs::new());
        let data_fs = Arc::new(MemoryFs::new());

        table.mount("/", "root", root_fs.clone()).await.unwrap();
        table.mount("/data", "data", data_fs.clone()).await.unwrap();

        let (_, relative) = table.resolve("/config.txt").await.unwrap();
        assert_eq!(relative, "/config.txt");

        let (provider, relative) = table.resolve("/data/file.txt").await.unwrap();
        assert_eq!(relative, "/file.txt");
        assert!(Arc::ptr_eq(&provider, &(data_fs as Arc<dyn FsProvider>)));
    }

    #[tokio::test]
    async fn mount_at_exact_path() {
        let table = MountTable::new();
        let fs = Arc::new(MemoryFs::new());

        table.mount("/data", "data", fs.clone()).await.unwrap();

        let (_, relative) = table.resolve("/data").await.unwrap();
        assert_eq!(relative, "/");
    }

    #[tokio::test]
    async fn unmount() {
        let table = MountTable::new();
        let fs = Arc::new(MemoryFs::new());

        table.mount("/data", "data", fs).await.unwrap();
        assert_eq!(table.count().await, 1);

        table.unmount("/data").await.unwrap();
        assert_eq!(table.count().await, 0);
    }

    #[tokio::test]
    async fn cannot_mount_duplicate() {
        let table = MountTable::new();
        let fs1 = Arc::new(MemoryFs::new());
        let fs2 = Arc::new(MemoryFs::new());

        table.mount("/data", "data1", fs1).await.unwrap();
        let result = table.mount("/data", "data2", fs2).await;

        assert!(matches!(result, Err(FsError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn list_mounts() {
        let table = MountTable::new();

        table
            .mount("/", "root", Arc::new(MemoryFs::new()))
            .await
            .unwrap();
        table
            .mount("/data", "data", Arc::new(MemoryFs::new()))
            .await
            .unwrap();
        table
            .mount("/cache", "cache", Arc::new(MemoryFs::new()))
            .await
            .unwrap();

        let mounts = table.list_mounts().await;
        assert_eq!(mounts.len(), 3);
    }

    #[tokio::test]
    async fn resolve_without_root_mount() {
        let table = MountTable::new();
        let fs = Arc::new(MemoryFs::new());

        table.mount("/data", "data", fs).await.unwrap();

        let result = table.resolve("/other/file.txt").await;
        assert!(matches!(result, Err(FsError::NotFound(_))));
    }

    #[tokio::test]
    async fn deeply_nested_mounts() {
        let table = MountTable::new();

        table
            .mount("/", "root", Arc::new(MemoryFs::new()))
            .await
            .unwrap();
        table
            .mount("/a", "a", Arc::new(MemoryFs::new()))
            .await
            .unwrap();
        table
            .mount("/a/b", "b", Arc::new(MemoryFs::new()))
            .await
            .unwrap();
        table
            .mount("/a/b/c", "c", Arc::new(MemoryFs::new()))
            .await
            .unwrap();

        let (_, relative) = table.resolve("/a/b/c/file.txt").await.unwrap();
        assert_eq!(relative, "/file.txt");

        let (_, relative) = table.resolve("/a/b/file.txt").await.unwrap();
        assert_eq!(relative, "/file.txt");

        let (_, relative) = table.resolve("/a/file.txt").await.unwrap();
        assert_eq!(relative, "/file.txt");
    }
}
