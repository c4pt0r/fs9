use async_trait::async_trait;
use bytes::Bytes;

use crate::capabilities::Capabilities;
use crate::error::FsResult;
use crate::types::{FileInfo, FsStats, Handle, OpenFlags, StatChanges};

#[async_trait]
pub trait FsProvider: Send + Sync {
    async fn stat(&self, path: &str) -> FsResult<FileInfo>;

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()>;

    async fn statfs(&self, path: &str) -> FsResult<FsStats>;

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle>;

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes>;

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize>;

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()>;

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>>;

    async fn remove(&self, path: &str) -> FsResult<()>;

    fn capabilities(&self) -> Capabilities;
}

#[async_trait]
impl<P: FsProvider + ?Sized> FsProvider for Box<P> {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        (**self).stat(path).await
    }

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()> {
        (**self).wstat(path, changes).await
    }

    async fn statfs(&self, path: &str) -> FsResult<FsStats> {
        (**self).statfs(path).await
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        (**self).open(path, flags).await
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        (**self).read(handle, offset, size).await
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        (**self).write(handle, offset, data).await
    }

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()> {
        (**self).close(handle, sync).await
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        (**self).readdir(path).await
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        (**self).remove(path).await
    }

    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
}

#[async_trait]
impl<P: FsProvider + ?Sized> FsProvider for std::sync::Arc<P> {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        (**self).stat(path).await
    }

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()> {
        (**self).wstat(path, changes).await
    }

    async fn statfs(&self, path: &str) -> FsResult<FsStats> {
        (**self).statfs(path).await
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        (**self).open(path, flags).await
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        (**self).read(handle, offset, size).await
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        (**self).write(handle, offset, data).await
    }

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()> {
        (**self).close(handle, sync).await
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        (**self).readdir(path).await
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        (**self).remove(path).await
    }

    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
}
