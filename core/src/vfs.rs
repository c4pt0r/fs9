use async_trait::async_trait;
use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FsError, FsProvider, FsResult, FsStats, Handle, OpenFlags, StatChanges,
};
use std::sync::Arc;

use crate::handle::HandleRegistry;
use crate::mount::MountTable;

pub struct VfsRouter {
    mount_table: Arc<MountTable>,
    handle_registry: Arc<HandleRegistry>,
}

impl VfsRouter {
    #[must_use]
    pub fn new(mount_table: Arc<MountTable>, handle_registry: Arc<HandleRegistry>) -> Self {
        Self {
            mount_table,
            handle_registry,
        }
    }

    pub fn mount_table(&self) -> &Arc<MountTable> {
        &self.mount_table
    }

    pub fn handle_registry(&self) -> &Arc<HandleRegistry> {
        &self.handle_registry
    }

    async fn resolve(&self, path: &str) -> FsResult<(Arc<dyn FsProvider>, String)> {
        self.mount_table.resolve(path).await
    }
}

#[async_trait]
impl FsProvider for VfsRouter {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let (provider, relative_path) = self.resolve(path).await?;
        let mut info = provider.stat(&relative_path).await?;
        info.path = path.to_string();
        Ok(info)
    }

    async fn wstat(&self, path: &str, mut changes: StatChanges) -> FsResult<()> {
        let (provider, relative_path) = self.resolve(path).await?;
        let caps = provider.capabilities();

        if changes.mode.is_some() && !caps.contains(Capabilities::CHMOD) {
            return Err(FsError::not_implemented("chmod"));
        }
        if (changes.uid.is_some() || changes.gid.is_some()) && !caps.contains(Capabilities::CHOWN) {
            return Err(FsError::not_implemented("chown"));
        }
        if changes.size.is_some() && !caps.contains(Capabilities::TRUNCATE) {
            return Err(FsError::not_implemented("truncate"));
        }
        if (changes.atime.is_some() || changes.mtime.is_some()) && !caps.contains(Capabilities::UTIME) {
            return Err(FsError::not_implemented("utime"));
        }
        if changes.name.is_some() && !caps.contains(Capabilities::RENAME) {
            return Err(FsError::not_implemented("rename"));
        }
        if changes.symlink_target.is_some() && !caps.contains(Capabilities::SYMLINK) {
            return Err(FsError::not_implemented("symlink"));
        }

        // Translate absolute VFS rename target to mount-relative path
        if let Some(ref new_name) = changes.name {
            let (target_provider, target_relative) = self.resolve(new_name).await?;
            if !Arc::ptr_eq(&provider, &target_provider) {
                return Err(FsError::invalid_argument("cannot rename across mount points"));
            }
            changes.name = Some(target_relative);
        }

        provider.wstat(&relative_path, changes).await
    }

    async fn statfs(&self, path: &str) -> FsResult<FsStats> {
        let (provider, relative_path) = self.resolve(path).await?;
        provider.statfs(&relative_path).await
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let (provider, relative_path) = self.resolve(path).await?;
        let caps = provider.capabilities();

        if flags.read && !caps.contains(Capabilities::READ) {
            return Err(FsError::not_implemented("read"));
        }
        if flags.write && !caps.contains(Capabilities::WRITE) {
            return Err(FsError::not_implemented("write"));
        }
        if flags.create && !caps.contains(Capabilities::CREATE) {
            return Err(FsError::not_implemented("create"));
        }

        let (provider_handle, mut metadata) = provider.open(&relative_path, flags).await?;

        // Rewrite path to absolute VFS path
        metadata.path = path.to_string();

        let handle_id = self
            .handle_registry
            .register(provider.clone(), path.to_string(), flags, metadata.clone(), provider_handle)
            .await;

        Ok((Handle::new(handle_id), metadata))
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        let handle_ref = self
            .handle_registry
            .get(handle.id())
            .await
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let provider = handle_ref.provider().await?;
        let provider_handle = handle_ref.provider_handle().await?;

        provider.read(&provider_handle, offset, size).await
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        let handle_ref = self
            .handle_registry
            .get(handle.id())
            .await
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let provider = handle_ref.provider().await?;
        let provider_handle = handle_ref.provider_handle().await?;

        provider.write(&provider_handle, offset, data).await
    }

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()> {
        self.handle_registry.close(handle.id(), sync).await
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let (provider, relative_path) = self.resolve(path).await?;
        let entries = provider.readdir(&relative_path).await?;

        let base_path = if path == "/" { "" } else { path };
        let entries = entries
            .into_iter()
            .map(|mut info| {
                let name = info.path.rsplit('/').next().unwrap_or(&info.path);
                info.path = format!("{base_path}/{name}");
                info
            })
            .collect();

        Ok(entries)
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        let (provider, relative_path) = self.resolve(path).await?;
        let caps = provider.capabilities();

        if !caps.contains(Capabilities::DELETE) {
            return Err(FsError::not_implemented("delete"));
        }

        provider.remove(&relative_path).await
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFs;
    use std::time::Duration;

    fn create_vfs() -> VfsRouter {
        VfsRouter::new(
            Arc::new(MountTable::new()),
            Arc::new(HandleRegistry::new(Duration::from_secs(300))),
        )
    }

    #[tokio::test]
    async fn basic_file_operations() {
        let vfs = create_vfs();
        let fs = Arc::new(MemoryFs::new());

        vfs.mount_table().mount("/", "root", fs).await.unwrap();

        let (handle, _) = vfs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        vfs.write(&handle, 0, Bytes::from("hello")).await.unwrap();
        vfs.close(handle, false).await.unwrap();

        let info = vfs.stat("/test.txt").await.unwrap();
        assert_eq!(info.size, 5);
        assert_eq!(info.path, "/test.txt");
    }

    #[tokio::test]
    async fn nested_mount_operations() {
        let vfs = create_vfs();

        vfs.mount_table()
            .mount("/", "root", Arc::new(MemoryFs::new()))
            .await
            .unwrap();
        vfs.mount_table()
            .mount("/data", "data", Arc::new(MemoryFs::new()))
            .await
            .unwrap();

        let (handle, _) = vfs.open("/data/file.txt", OpenFlags::create_file()).await.unwrap();
        vfs.write(&handle, 0, Bytes::from("data content")).await.unwrap();
        vfs.close(handle, false).await.unwrap();

        let info = vfs.stat("/data/file.txt").await.unwrap();
        assert_eq!(info.size, 12);
        assert_eq!(info.path, "/data/file.txt");

        let (handle, _) = vfs.open("/root.txt", OpenFlags::create_file()).await.unwrap();
        vfs.write(&handle, 0, Bytes::from("root")).await.unwrap();
        vfs.close(handle, false).await.unwrap();

        let info = vfs.stat("/root.txt").await.unwrap();
        assert_eq!(info.size, 4);
    }

    #[tokio::test]
    async fn readdir_with_correct_paths() {
        let vfs = create_vfs();
        let fs = Arc::new(MemoryFs::new());

        vfs.mount_table().mount("/data", "data", fs).await.unwrap();

        vfs.open("/data/dir", OpenFlags::create_dir()).await.unwrap();
        let (h1, _) = vfs.open("/data/dir/a.txt", OpenFlags::create_file()).await.unwrap();
        vfs.close(h1, false).await.unwrap();
        let (h2, _) = vfs.open("/data/dir/b.txt", OpenFlags::create_file()).await.unwrap();
        vfs.close(h2, false).await.unwrap();

        let entries = vfs.readdir("/data/dir").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.path == "/data/dir/a.txt"));
        assert!(entries.iter().any(|e| e.path == "/data/dir/b.txt"));
    }

    #[tokio::test]
    async fn handle_isolation() {
        let vfs = create_vfs();
        let fs = Arc::new(MemoryFs::new());

        vfs.mount_table().mount("/", "root", fs).await.unwrap();

        let (h1, _) = vfs.open("/file1.txt", OpenFlags::create_file()).await.unwrap();
        let (h2, _) = vfs.open("/file2.txt", OpenFlags::create_file()).await.unwrap();

        vfs.write(&h1, 0, Bytes::from("content1")).await.unwrap();
        vfs.write(&h2, 0, Bytes::from("content2")).await.unwrap();

        let data1 = vfs.read(&h1, 0, 100).await.unwrap();
        let data2 = vfs.read(&h2, 0, 100).await.unwrap();

        assert_eq!(&data1[..], b"content1");
        assert_eq!(&data2[..], b"content2");

        vfs.close(h1, false).await.unwrap();
        vfs.close(h2, false).await.unwrap();
    }

    #[tokio::test]
    async fn wstat_operations() {
        let vfs = create_vfs();
        let fs = Arc::new(MemoryFs::new());

        vfs.mount_table().mount("/", "root", fs).await.unwrap();

        let (handle, _) = vfs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        vfs.write(&handle, 0, Bytes::from("hello world")).await.unwrap();
        vfs.close(handle, false).await.unwrap();

        vfs.wstat("/test.txt", StatChanges::truncate(5)).await.unwrap();

        let info = vfs.stat("/test.txt").await.unwrap();
        assert_eq!(info.size, 5);

        vfs.wstat("/test.txt", StatChanges::rename("renamed.txt")).await.unwrap();

        let info = vfs.stat("/renamed.txt").await.unwrap();
        assert_eq!(info.size, 5);
    }
}
