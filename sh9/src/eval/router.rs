use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fs9_client::{FileInfo, Fs9Client};

use super::local_fs::{
    local_append_file, local_chmod, local_copy, local_mkdir, local_read_file, local_readdir,
    local_remove, local_remove_recursive, local_rename, local_stat, local_truncate,
    local_write_file, safe_resolve, LocalFileInfo,
};
use super::namespace::{normalize_path, MountFlags, Namespace};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFileInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub mode: u32,
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
}

impl From<LocalFileInfo> for RouteFileInfo {
    fn from(value: LocalFileInfo) -> Self {
        Self {
            name: value.name,
            path: String::new(),
            size: value.size,
            is_dir: value.is_dir,
            mode: value.mode,
            mtime: value.mtime,
            uid: value.uid,
            gid: value.gid,
        }
    }
}

impl From<&FileInfo> for RouteFileInfo {
    fn from(value: &FileInfo) -> Self {
        Self {
            name: value.name().to_string(),
            path: value.path.clone(),
            size: value.size,
            is_dir: value.is_dir(),
            mode: value.mode,
            mtime: value.mtime,
            uid: value.uid,
            gid: value.gid,
        }
    }
}

pub struct NamespaceRouter {
    pub namespace: Namespace,
    client: Option<Arc<Fs9Client>>,
}

#[derive(Debug, Clone)]
struct LocalLayerRoute {
    mount_target: String,
    mount_source: PathBuf,
    relative_path: String,
    flags: MountFlags,
}

#[derive(Debug, Clone)]
enum RouteTarget {
    Local {
        mount_target: String,
        mount_source: PathBuf,
        local_path: PathBuf,
    },
    Remote {
        path: String,
    },
}

impl NamespaceRouter {
    pub fn new(client: Option<Arc<Fs9Client>>) -> Self {
        Self {
            namespace: Namespace::new(),
            client,
        }
    }

    pub fn with_namespace(namespace: Namespace, client: Option<Arc<Fs9Client>>) -> Self {
        Self { namespace, client }
    }

    pub async fn stat(&self, path: &str) -> Result<RouteFileInfo, String> {
        let normalized = normalize_path(path);
        let local_layers = self.local_layers_for_path(&normalized);
        if !local_layers.is_empty() {
            let mut last_err = None;
            for layer in &local_layers {
                match self.stat_local_layer(layer, &normalized).await {
                    Ok(info) => return Ok(info),
                    Err(err) => last_err = Some(err),
                }
            }
            return Err(last_err.unwrap_or_else(|| "stat failed".to_string()));
        }

        let client = self.require_client()?;
        client
            .stat(&normalized)
            .await
            .map(|info| RouteFileInfo::from(&info))
            .map_err(|e| e.to_string())
    }

    pub async fn readdir(&self, path: &str) -> Result<Vec<RouteFileInfo>, String> {
        let normalized = normalize_path(path);
        let local_layers = self.local_layers_for_path(&normalized);
        if !local_layers.is_empty() {
            let mut merged = Vec::new();
            let mut seen = HashSet::new();
            let mut last_err = None;
            let mut had_success = false;

            for layer in &local_layers {
                let local_path = self.resolve_layer_local_path(layer).await?;
                let entries = self.local_readdir_blocking(local_path).await;
                match entries {
                    Ok(entries) => {
                        had_success = true;
                        for entry in entries {
                            if seen.insert(entry.name.clone()) {
                                let mut routed = RouteFileInfo::from(entry);
                                routed.path = join_vfs_path(&normalized, &routed.name);
                                merged.push(routed);
                            }
                        }
                    }
                    Err(err) => {
                        last_err = Some(err);
                    }
                }
            }

            if had_success {
                merged.sort_by(|left, right| left.name.cmp(&right.name));
                // Inject synthetic entries for child mount points
                let child_mounts = self.namespace.child_mount_names(&normalized);
                for name in child_mounts {
                    if seen.insert(name.clone()) {
                        merged.push(RouteFileInfo {
                            name: name.clone(),
                            path: join_vfs_path(&normalized, &name),
                            size: 0,
                            is_dir: true,
                            mode: 0o755,
                            mtime: 0,
                            uid: 0,
                            gid: 0,
                        });
                    }
                }
                merged.sort_by(|left, right| left.name.cmp(&right.name));
                return Ok(merged);
            }

            return Err(last_err.unwrap_or_else(|| "readdir failed".to_string()));
        }

        let client = self.require_client()?;
        let remote_entries = client
            .readdir(&normalized)
            .await
            .map_err(|e| e.to_string())?;
        let mut result: Vec<RouteFileInfo> =
            remote_entries.iter().map(RouteFileInfo::from).collect();

        // Inject synthetic entries for child mount points
        let mut seen: HashSet<String> = result.iter().map(|e| e.name.clone()).collect();
        let child_mounts = self.namespace.child_mount_names(&normalized);
        for name in child_mounts {
            if seen.insert(name.clone()) {
                result.push(RouteFileInfo {
                    name: name.clone(),
                    path: join_vfs_path(&normalized, &name),
                    size: 0,
                    is_dir: true,
                    mode: 0o755,
                    mtime: 0,
                    uid: 0,
                    gid: 0,
                });
            }
        }

        result.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(result)
    }

    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let normalized = normalize_path(path);
        let local_layers = self.local_layers_for_path(&normalized);
        if !local_layers.is_empty() {
            let mut last_err = None;
            for layer in &local_layers {
                let local_path = self.resolve_layer_local_path(layer).await?;
                match self.local_read_file_blocking(local_path).await {
                    Ok(bytes) => return Ok(bytes),
                    Err(err) => last_err = Some(err),
                }
            }
            return Err(last_err.unwrap_or_else(|| "read failed".to_string()));
        }

        let client = self.require_client()?;
        client
            .read_file(&normalized)
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| e.to_string())
    }

    pub async fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let target = self.resolve_write_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => {
                self.local_write_file_blocking(local_path, data.to_vec())
                    .await
            }
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                client
                    .write_file(&path, data)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    pub async fn append_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let target = self.resolve_write_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => {
                self.local_append_file_blocking(local_path, data.to_vec())
                    .await
            }
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                let mut existing = client
                    .read_file(&path)
                    .await
                    .map(|bytes| bytes.to_vec())
                    .unwrap_or_default();
                existing.extend_from_slice(data);
                client
                    .write_file(&path, &existing)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), String> {
        let target = self.resolve_write_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => self.local_mkdir_blocking(local_path).await,
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                client.mkdir(&path).await.map_err(|e| e.to_string())
            }
        }
    }

    pub async fn remove(&self, path: &str) -> Result<(), String> {
        let target = self.resolve_existing_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => self.local_remove_blocking(local_path).await,
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                client.remove(&path).await.map_err(|e| e.to_string())
            }
        }
    }

    pub async fn remove_recursive(&self, path: &str) -> Result<(), String> {
        let target = self.resolve_existing_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => {
                self.local_remove_recursive_blocking(local_path).await
            }
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                self.remove_remote_recursive(client, &path).await
            }
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let source = self.resolve_existing_target(from).await?;
        let destination = self.resolve_write_target(to).await?;

        if !same_provider(&source, &destination) {
            return Err("cross-mount rename".to_string());
        }

        match (source, destination) {
            (
                RouteTarget::Local {
                    local_path: from_local,
                    ..
                },
                RouteTarget::Local {
                    local_path: to_local,
                    ..
                },
            ) => self.local_rename_blocking(from_local, to_local).await,
            (
                RouteTarget::Remote { path: from_remote },
                RouteTarget::Remote { path: to_remote },
            ) => {
                let client = self.require_client()?;
                client
                    .rename(&from_remote, &to_remote)
                    .await
                    .map_err(|e| e.to_string())
            }
            _ => Err("cross-mount rename".to_string()),
        }
    }

    pub async fn copy(&self, from: &str, to: &str) -> Result<(), String> {
        let source = self.resolve_existing_target(from).await?;
        let destination = self.resolve_write_target(to).await?;

        match (source, destination) {
            (
                RouteTarget::Local {
                    local_path: from_local,
                    ..
                },
                RouteTarget::Local {
                    local_path: to_local,
                    ..
                },
            ) => self.local_copy_blocking(from_local, to_local).await,
            _ => {
                let data = self.read_file(from).await?;
                self.write_file(to, &data).await
            }
        }
    }

    pub async fn chmod(&self, path: &str, mode: u32) -> Result<(), String> {
        let target = self.resolve_existing_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => {
                self.local_chmod_blocking(local_path, mode).await
            }
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                client.chmod(&path, mode).await.map_err(|e| e.to_string())
            }
        }
    }

    pub async fn truncate(&self, path: &str, size: u64) -> Result<(), String> {
        let target = self.resolve_existing_target(path).await?;
        match target {
            RouteTarget::Local { local_path, .. } => {
                self.local_truncate_blocking(local_path, size).await
            }
            RouteTarget::Remote { path } => {
                let client = self.require_client()?;
                client
                    .truncate(&path, size)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    pub fn is_local(&self, path: &str) -> bool {
        self.namespace.is_mounted(path)
    }

    pub fn has_client(&self) -> bool {
        self.client.is_some()
    }

    fn require_client(&self) -> Result<&Fs9Client, String> {
        self.client
            .as_deref()
            .ok_or_else(|| "no remote client available".to_string())
    }

    fn local_layers_for_path(&self, path: &str) -> Vec<LocalLayerRoute> {
        let normalized = normalize_path(path);
        let Some(mount_target) = self.longest_mount_target(&normalized) else {
            return Vec::new();
        };

        self.namespace
            .list_mounts()
            .into_iter()
            .filter(|mount| mount.target == mount_target)
            .map(|mount| LocalLayerRoute {
                mount_target: mount.target,
                mount_source: mount.source,
                relative_path: relative_path_for_mount(&normalized, &mount_target),
                flags: mount.flags,
            })
            .collect()
    }

    fn longest_mount_target(&self, path: &str) -> Option<String> {
        let normalized = normalize_path(path);
        self.namespace
            .list_mounts()
            .into_iter()
            .fold(None, |best: Option<String>, mount| {
                if !mount_matches_path(&mount.target, &normalized) {
                    return best;
                }

                match best {
                    Some(current) if current.len() >= mount.target.len() => Some(current),
                    _ => Some(mount.target),
                }
            })
    }

    async fn resolve_layer_local_path(&self, layer: &LocalLayerRoute) -> Result<PathBuf, String> {
        let mount_source = layer.mount_source.clone();
        let relative = layer.relative_path.trim_start_matches('/').to_string();
        tokio::task::spawn_blocking(move || safe_resolve(&mount_source, &relative))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn stat_local_layer(
        &self,
        layer: &LocalLayerRoute,
        normalized_path: &str,
    ) -> Result<RouteFileInfo, String> {
        let local_path = self.resolve_layer_local_path(layer).await?;
        let info = self.local_stat_blocking(local_path).await?;
        let mut routed = RouteFileInfo::from(info);
        routed.path = normalized_path.to_string();
        Ok(routed)
    }

    async fn resolve_existing_target(&self, path: &str) -> Result<RouteTarget, String> {
        let normalized = normalize_path(path);
        let local_layers = self.local_layers_for_path(&normalized);
        if !local_layers.is_empty() {
            let mut last_err = None;
            for layer in &local_layers {
                let local_path = self.resolve_layer_local_path(layer).await?;
                match self.local_stat_blocking(local_path.clone()).await {
                    Ok(_) => {
                        return Ok(RouteTarget::Local {
                            mount_target: layer.mount_target.clone(),
                            mount_source: layer.mount_source.clone(),
                            local_path,
                        });
                    }
                    Err(err) => {
                        last_err = Some(err);
                    }
                }
            }
            return Err(last_err.unwrap_or_else(|| "path not found".to_string()));
        }

        let client = self.require_client()?;
        client.stat(&normalized).await.map_err(|e| e.to_string())?;
        Ok(RouteTarget::Remote { path: normalized })
    }

    async fn resolve_write_target(&self, path: &str) -> Result<RouteTarget, String> {
        let normalized = normalize_path(path);
        let local_layers = self.local_layers_for_path(&normalized);
        if local_layers.is_empty() {
            self.require_client()?;
            return Ok(RouteTarget::Remote { path: normalized });
        }

        let selected = local_layers
            .iter()
            .find(|layer| layer.flags.contains(MountFlags::MCREATE))
            .unwrap_or(&local_layers[0]);
        let local_path = self.resolve_layer_local_path(selected).await?;
        Ok(RouteTarget::Local {
            mount_target: selected.mount_target.clone(),
            mount_source: selected.mount_source.clone(),
            local_path,
        })
    }

    async fn remove_remote_recursive(&self, client: &Fs9Client, path: &str) -> Result<(), String> {
        let mut stack = vec![path.to_string()];
        let mut post_order = Vec::new();

        while let Some(current) = stack.pop() {
            let stat = client.stat(&current).await.map_err(|e| e.to_string())?;
            if stat.is_dir() {
                let entries = client.readdir(&current).await.map_err(|e| e.to_string())?;
                post_order.push(current.clone());
                for entry in entries {
                    stack.push(join_vfs_path(&current, entry.name()));
                }
            } else {
                client.remove(&current).await.map_err(|e| e.to_string())?;
            }
        }

        while let Some(dir) = post_order.pop() {
            client.remove(&dir).await.map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn local_stat_blocking(&self, path: PathBuf) -> Result<LocalFileInfo, String> {
        tokio::task::spawn_blocking(move || local_stat(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_readdir_blocking(&self, path: PathBuf) -> Result<Vec<LocalFileInfo>, String> {
        tokio::task::spawn_blocking(move || local_readdir(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_read_file_blocking(&self, path: PathBuf) -> Result<Vec<u8>, String> {
        tokio::task::spawn_blocking(move || local_read_file(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_write_file_blocking(&self, path: PathBuf, data: Vec<u8>) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_write_file(&path, &data))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_append_file_blocking(&self, path: PathBuf, data: Vec<u8>) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_append_file(&path, &data))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_mkdir_blocking(&self, path: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_mkdir(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_remove_blocking(&self, path: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_remove(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_remove_recursive_blocking(&self, path: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_remove_recursive(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_rename_blocking(&self, from: PathBuf, to: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_rename(&from, &to))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_copy_blocking(&self, from: PathBuf, to: PathBuf) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_copy(&from, &to))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_chmod_blocking(&self, path: PathBuf, mode: u32) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_chmod(&path, mode))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn local_truncate_blocking(&self, path: PathBuf, size: u64) -> Result<(), String> {
        tokio::task::spawn_blocking(move || local_truncate(&path, size))
            .await
            .map_err(|e| e.to_string())?
    }
}

fn mount_matches_path(target: &str, path: &str) -> bool {
    if target == "/" {
        return true;
    }

    path == target || (path.starts_with(target) && path.as_bytes().get(target.len()) == Some(&b'/'))
}

fn relative_path_for_mount(path: &str, target: &str) -> String {
    if path == target {
        "/".to_string()
    } else if target == "/" {
        path.to_string()
    } else {
        path[target.len()..].to_string()
    }
}

fn join_vfs_path(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn same_provider(source: &RouteTarget, destination: &RouteTarget) -> bool {
    match (source, destination) {
        (
            RouteTarget::Local {
                mount_target: source_target,
                mount_source: source_mount,
                ..
            },
            RouteTarget::Local {
                mount_target: destination_target,
                mount_source: destination_mount,
                ..
            },
        ) => source_target == destination_target && source_mount == destination_mount,
        (RouteTarget::Remote { .. }, RouteTarget::Remote { .. }) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::NamespaceRouter;
    use crate::eval::namespace::MountFlags;

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sh9_router_test_{}_{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).expect("failed to create temp test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn router_routes_stat_to_local_when_mounted() {
        let tmp = TempDirGuard::new();
        fs::write(tmp.path().join("hello.txt"), b"hello").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router.namespace.bind(tmp.path(), "/mnt", MountFlags::MREPL);

        let stat = router.stat("/mnt/hello.txt").await.expect("stat failed");
        assert_eq!(stat.name, "hello.txt");
        assert_eq!(stat.path, "/mnt/hello.txt");
        assert_eq!(stat.size, 5);
    }

    #[tokio::test]
    async fn router_returns_error_when_unmounted_without_client() {
        let router = NamespaceRouter::new(None);
        let err = router
            .stat("/remote/only")
            .await
            .expect_err("expected stat error");
        assert!(err.contains("no remote client"));
    }

    #[tokio::test]
    async fn router_local_readdir() {
        let tmp = TempDirGuard::new();
        fs::write(tmp.path().join("b.txt"), b"b").expect("write failed");
        fs::write(tmp.path().join("a.txt"), b"a").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router.namespace.bind(tmp.path(), "/mnt", MountFlags::MREPL);

        let entries = router.readdir("/mnt").await.expect("readdir failed");
        let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }

    #[tokio::test]
    async fn router_local_read_file() {
        let tmp = TempDirGuard::new();
        fs::write(tmp.path().join("read.txt"), b"router").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router.namespace.bind(tmp.path(), "/mnt", MountFlags::MREPL);

        let data = router
            .read_file("/mnt/read.txt")
            .await
            .expect("read failed");
        assert_eq!(data, b"router");
    }

    #[tokio::test]
    async fn router_local_write_file() {
        let tmp = TempDirGuard::new();
        let mut router = NamespaceRouter::new(None);
        router.namespace.bind(tmp.path(), "/mnt", MountFlags::MREPL);

        router
            .write_file("/mnt/write.txt", b"payload")
            .await
            .expect("write failed");

        assert_eq!(
            fs::read(tmp.path().join("write.txt")).expect("read failed"),
            b"payload"
        );
    }

    #[tokio::test]
    async fn router_union_readdir_merges_entries_first_match_wins() {
        let lower = TempDirGuard::new();
        let upper = TempDirGuard::new();
        fs::write(lower.path().join("a.txt"), b"lower").expect("write failed");
        fs::write(lower.path().join("shared.txt"), b"lower").expect("write failed");
        fs::write(upper.path().join("b.txt"), b"upper").expect("write failed");
        fs::write(upper.path().join("shared.txt"), b"upper").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router
            .namespace
            .bind(lower.path(), "/union", MountFlags::MREPL);
        router
            .namespace
            .bind(upper.path(), "/union", MountFlags::MBEFORE);

        let entries = router.readdir("/union").await.expect("readdir failed");
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "shared.txt"]);

        let shared = entries
            .iter()
            .find(|e| e.name == "shared.txt")
            .expect("missing shared entry");
        assert_eq!(shared.path, "/union/shared.txt");
    }

    #[tokio::test]
    async fn router_union_stat_returns_first_match() {
        let lower = TempDirGuard::new();
        let upper = TempDirGuard::new();
        fs::write(lower.path().join("shared.txt"), b"lower").expect("write failed");
        fs::write(upper.path().join("shared.txt"), b"upper-data").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router
            .namespace
            .bind(lower.path(), "/union", MountFlags::MREPL);
        router
            .namespace
            .bind(upper.path(), "/union", MountFlags::MBEFORE);

        let stat = router.stat("/union/shared.txt").await.expect("stat failed");
        assert_eq!(stat.size, 10);
    }

    #[tokio::test]
    async fn router_union_read_file_prefers_first_layer() {
        let lower = TempDirGuard::new();
        let upper = TempDirGuard::new();
        fs::write(lower.path().join("shared.txt"), b"lower").expect("write failed");
        fs::write(upper.path().join("shared.txt"), b"upper").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router
            .namespace
            .bind(lower.path(), "/union", MountFlags::MREPL);
        router
            .namespace
            .bind(upper.path(), "/union", MountFlags::MBEFORE);

        let data = router
            .read_file("/union/shared.txt")
            .await
            .expect("read failed");
        assert_eq!(data, b"upper");
    }

    #[tokio::test]
    async fn router_write_file_prefers_mcreate_layer() {
        let base = TempDirGuard::new();
        let create_layer = TempDirGuard::new();

        let mut router = NamespaceRouter::new(None);
        router
            .namespace
            .bind(base.path(), "/mnt", MountFlags::MREPL);
        router.namespace.bind(
            create_layer.path(),
            "/mnt",
            MountFlags::MAFTER | MountFlags::MCREATE,
        );

        router
            .write_file("/mnt/new.txt", b"from-create")
            .await
            .expect("write failed");

        assert!(
            !base.path().join("new.txt").exists(),
            "base layer should not receive create"
        );
        assert_eq!(
            fs::read(create_layer.path().join("new.txt")).expect("read failed"),
            b"from-create"
        );
    }

    #[tokio::test]
    async fn router_rename_detects_cross_mount() {
        let left = TempDirGuard::new();
        let right = TempDirGuard::new();
        fs::write(left.path().join("file.txt"), b"x").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router
            .namespace
            .bind(left.path(), "/left", MountFlags::MREPL);
        router
            .namespace
            .bind(right.path(), "/right", MountFlags::MREPL);

        let err = router
            .rename("/left/file.txt", "/right/file.txt")
            .await
            .expect_err("expected cross-mount rename to fail");
        assert!(err.contains("cross-mount rename"));
    }

    #[test]
    fn router_is_local_true_for_mounted_false_for_unmounted() {
        let tmp = TempDirGuard::new();
        let mut router = NamespaceRouter::new(None);
        router.namespace.bind(tmp.path(), "/mnt", MountFlags::MREPL);

        assert!(router.is_local("/mnt/file.txt"));
        assert!(!router.is_local("/remote/file.txt"));
    }

    #[tokio::test]
    async fn router_copy_between_local_mounts_works() {
        let src_mount = TempDirGuard::new();
        let dst_mount = TempDirGuard::new();
        fs::write(src_mount.path().join("file.txt"), b"copy-me").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router
            .namespace
            .bind(src_mount.path(), "/src", MountFlags::MREPL);
        router
            .namespace
            .bind(dst_mount.path(), "/dst", MountFlags::MREPL);

        router
            .copy("/src/file.txt", "/dst/file.txt")
            .await
            .expect("copy failed");
        assert_eq!(
            fs::read(dst_mount.path().join("file.txt")).expect("read failed"),
            b"copy-me"
        );
    }

    #[tokio::test]
    async fn router_readdir_synthesizes_mount_points() {
        let tmp = TempDirGuard::new();
        let child = TempDirGuard::new();
        fs::write(tmp.path().join("file.txt"), b"data").expect("write failed");

        let mut router = NamespaceRouter::new(None);
        router.namespace.bind(tmp.path(), "/mnt", MountFlags::MREPL);
        router
            .namespace
            .bind(child.path(), "/mnt/sub", MountFlags::MREPL);

        let entries = router.readdir("/mnt").await.expect("readdir failed");
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"file.txt".to_string()));
        assert!(names.contains(&"sub".to_string()));

        let sub_entry = entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(sub_entry.is_dir);
        assert_eq!(sub_entry.path, "/mnt/sub");
    }
}
