use async_trait::async_trait;
use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FileType, FsError, FsProvider, FsResult, FsStats, Handle, OpenFlags,
    StatChanges,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Debug, Clone)]
struct MemFile {
    content: Vec<u8>,
    mode: u32,
    uid: u32,
    gid: u32,
    atime: SystemTime,
    mtime: SystemTime,
    ctime: SystemTime,
}

impl Default for MemFile {
    fn default() -> Self {
        let now = SystemTime::now();
        Self {
            content: Vec::new(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            atime: now,
            mtime: now,
            ctime: now,
        }
    }
}

#[derive(Debug, Clone)]
struct MemDir {
    mode: u32,
    uid: u32,
    gid: u32,
    atime: SystemTime,
    mtime: SystemTime,
    ctime: SystemTime,
}

impl Default for MemDir {
    fn default() -> Self {
        let now = SystemTime::now();
        Self {
            mode: 0o755,
            uid: 0,
            gid: 0,
            atime: now,
            mtime: now,
            ctime: now,
        }
    }
}

#[derive(Debug, Clone)]
struct MemSymlink {
    target: String,
    mode: u32,
    uid: u32,
    gid: u32,
    atime: SystemTime,
    mtime: SystemTime,
    ctime: SystemTime,
}

#[derive(Debug, Clone)]
enum MemEntry {
    File(MemFile),
    Dir(MemDir),
    Symlink(MemSymlink),
}

impl MemEntry {
    fn file_type(&self) -> FileType {
        match self {
            Self::File(_) => FileType::Regular,
            Self::Dir(_) => FileType::Directory,
            Self::Symlink(_) => FileType::Symlink,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Self::File(f) => f.content.len() as u64,
            Self::Dir(_) => 0,
            Self::Symlink(s) => s.target.len() as u64,
        }
    }

    fn mode(&self) -> u32 {
        match self {
            Self::File(f) => f.mode,
            Self::Dir(d) => d.mode,
            Self::Symlink(s) => s.mode,
        }
    }

    fn uid(&self) -> u32 {
        match self {
            Self::File(f) => f.uid,
            Self::Dir(d) => d.uid,
            Self::Symlink(s) => s.uid,
        }
    }

    fn gid(&self) -> u32 {
        match self {
            Self::File(f) => f.gid,
            Self::Dir(d) => d.gid,
            Self::Symlink(s) => s.gid,
        }
    }

    fn atime(&self) -> SystemTime {
        match self {
            Self::File(f) => f.atime,
            Self::Dir(d) => d.atime,
            Self::Symlink(s) => s.atime,
        }
    }

    fn mtime(&self) -> SystemTime {
        match self {
            Self::File(f) => f.mtime,
            Self::Dir(d) => d.mtime,
            Self::Symlink(s) => s.mtime,
        }
    }

    fn ctime(&self) -> SystemTime {
        match self {
            Self::File(f) => f.ctime,
            Self::Dir(d) => d.ctime,
            Self::Symlink(s) => s.ctime,
        }
    }

    fn symlink_target(&self) -> Option<String> {
        match self {
            Self::Symlink(s) => Some(s.target.clone()),
            _ => None,
        }
    }

    fn to_file_info(&self, path: &str) -> FileInfo {
        FileInfo {
            path: path.to_string(),
            size: self.size(),
            file_type: self.file_type(),
            mode: self.mode(),
            uid: self.uid(),
            gid: self.gid(),
            atime: self.atime(),
            mtime: self.mtime(),
            ctime: self.ctime(),
            etag: format!("{:x}", self.mtime().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos()),
            symlink_target: self.symlink_target(),
        }
    }
}

#[derive(Debug)]
struct OpenHandle {
    path: String,
    flags: OpenFlags,
}

pub struct MemoryFs {
    entries: RwLock<HashMap<String, MemEntry>>,
    handles: RwLock<HashMap<u64, OpenHandle>>,
    next_handle: AtomicU64,
}

impl Default for MemoryFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryFs {
    #[must_use]
    pub fn new() -> Self {
        let fs = Self {
            entries: RwLock::new(HashMap::new()),
            handles: RwLock::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        };
        fs.entries
            .write()
            .unwrap()
            .insert("/".to_string(), MemEntry::Dir(MemDir::default()));
        fs
    }

    fn normalize_path(path: &str) -> String {
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

    fn parent_path(path: &str) -> Option<String> {
        let path = Self::normalize_path(path);
        if path == "/" {
            return None;
        }
        let parts: Vec<&str> = path.rsplitn(2, '/').collect();
        if parts.len() == 2 {
            let parent = if parts[1].is_empty() {
                "/".to_string()
            } else {
                parts[1].to_string()
            };
            Some(parent)
        } else {
            Some("/".to_string())
        }
    }

}

#[async_trait]
impl FsProvider for MemoryFs {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path = Self::normalize_path(path);
        let entries = self.entries.read().unwrap();
        entries
            .get(&path)
            .map(|e| e.to_file_info(&path))
            .ok_or_else(|| FsError::not_found(&path))
    }

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()> {
        let path = Self::normalize_path(path);
        let mut entries = self.entries.write().unwrap();

        if changes.symlink_target.is_some() {
            let target = changes.symlink_target.unwrap();
            if entries.contains_key(&path) {
                return Err(FsError::already_exists(&path));
            }
            let parent = Self::parent_path(&path).ok_or_else(|| FsError::invalid_argument("cannot create symlink at root"))?;
            if !entries.contains_key(&parent) {
                return Err(FsError::not_found(&parent));
            }
            let now = SystemTime::now();
            entries.insert(
                path,
                MemEntry::Symlink(MemSymlink {
                    target,
                    mode: 0o777,
                    uid: 0,
                    gid: 0,
                    atime: now,
                    mtime: now,
                    ctime: now,
                }),
            );
            return Ok(());
        }

        if let Some(new_name) = &changes.name {
            let entry = entries.remove(&path).ok_or_else(|| FsError::not_found(&path))?;
            let new_path = if new_name.starts_with('/') {
                Self::normalize_path(new_name)
            } else {
                let parent = Self::parent_path(&path).unwrap_or_else(|| "/".to_string());
                if parent == "/" {
                    Self::normalize_path(&format!("/{new_name}"))
                } else {
                    Self::normalize_path(&format!("{parent}/{new_name}"))
                }
            };
            if entries.contains_key(&new_path) {
                entries.insert(path, entry);
                return Err(FsError::already_exists(&new_path));
            }
            entries.insert(new_path, entry);
            return Ok(());
        }

        let entry = entries.get_mut(&path).ok_or_else(|| FsError::not_found(&path))?;
        let now = SystemTime::now();

        match entry {
            MemEntry::File(f) => {
                if let Some(mode) = changes.mode {
                    f.mode = mode;
                }
                if let Some(uid) = changes.uid {
                    f.uid = uid;
                }
                if let Some(gid) = changes.gid {
                    f.gid = gid;
                }
                if let Some(size) = changes.size {
                    f.content.resize(size as usize, 0);
                    f.mtime = now;
                }
                if let Some(atime) = changes.atime {
                    f.atime = atime;
                }
                if let Some(mtime) = changes.mtime {
                    f.mtime = mtime;
                }
                f.ctime = now;
            }
            MemEntry::Dir(d) => {
                if changes.size.is_some() {
                    return Err(FsError::is_directory(&path));
                }
                if let Some(mode) = changes.mode {
                    d.mode = mode;
                }
                if let Some(uid) = changes.uid {
                    d.uid = uid;
                }
                if let Some(gid) = changes.gid {
                    d.gid = gid;
                }
                if let Some(atime) = changes.atime {
                    d.atime = atime;
                }
                if let Some(mtime) = changes.mtime {
                    d.mtime = mtime;
                }
                d.ctime = now;
            }
            MemEntry::Symlink(s) => {
                if let Some(mode) = changes.mode {
                    s.mode = mode;
                }
                if let Some(uid) = changes.uid {
                    s.uid = uid;
                }
                if let Some(gid) = changes.gid {
                    s.gid = gid;
                }
                if let Some(atime) = changes.atime {
                    s.atime = atime;
                }
                if let Some(mtime) = changes.mtime {
                    s.mtime = mtime;
                }
                s.ctime = now;
            }
        }

        Ok(())
    }

    async fn statfs(&self, _path: &str) -> FsResult<FsStats> {
        let entries = self.entries.read().unwrap();
        let total_size: u64 = entries
            .values()
            .filter_map(|e| match e {
                MemEntry::File(f) => Some(f.content.len() as u64),
                _ => None,
            })
            .sum();

        Ok(FsStats {
            total_bytes: 1024 * 1024 * 1024,
            free_bytes: 1024 * 1024 * 1024 - total_size,
            total_inodes: 1_000_000,
            free_inodes: 1_000_000 - entries.len() as u64,
            block_size: 4096,
            max_name_len: 255,
        })
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        let path = Self::normalize_path(path);

        {
            let mut entries = self.entries.write().unwrap();

            if flags.create && flags.directory {
                if entries.contains_key(&path) {
                    return Err(FsError::already_exists(&path));
                }
                let parent = Self::parent_path(&path)
                    .ok_or_else(|| FsError::invalid_argument("cannot create directory at root"))?;
                if !entries.contains_key(&parent) {
                    return Err(FsError::not_found(&parent));
                }
                entries.insert(path.clone(), MemEntry::Dir(MemDir::default()));
            } else if flags.create {
                if !entries.contains_key(&path) {
                    let parent = Self::parent_path(&path)
                        .ok_or_else(|| FsError::invalid_argument("cannot create file at root"))?;
                    if !entries.contains_key(&parent) {
                        return Err(FsError::not_found(&parent));
                    }
                    entries.insert(path.clone(), MemEntry::File(MemFile::default()));
                } else if flags.truncate {
                    if let Some(MemEntry::File(f)) = entries.get_mut(&path) {
                        f.content.clear();
                        f.mtime = SystemTime::now();
                    }
                }
            } else if !entries.contains_key(&path) {
                return Err(FsError::not_found(&path));
            }
        }

        let handle_id = self.next_handle.fetch_add(1, Ordering::SeqCst);
        self.handles.write().unwrap().insert(
            handle_id,
            OpenHandle {
                path,
                flags,
            },
        );

        Ok(Handle::new(handle_id))
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        let handles = self.handles.read().unwrap();
        let open_handle = handles
            .get(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        if !open_handle.flags.read {
            return Err(FsError::permission_denied("file not opened for reading"));
        }

        let entries = self.entries.read().unwrap();
        let entry = entries
            .get(&open_handle.path)
            .ok_or_else(|| FsError::not_found(&open_handle.path))?;

        match entry {
            MemEntry::File(f) => {
                let start = (offset as usize).min(f.content.len());
                let end = (start + size).min(f.content.len());
                Ok(Bytes::copy_from_slice(&f.content[start..end]))
            }
            MemEntry::Dir(_) => Err(FsError::is_directory(&open_handle.path)),
            MemEntry::Symlink(_) => Err(FsError::invalid_argument("cannot read symlink as file")),
        }
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        let handles = self.handles.read().unwrap();
        let open_handle = handles
            .get(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        if !open_handle.flags.write {
            return Err(FsError::permission_denied("file not opened for writing"));
        }

        let path = open_handle.path.clone();
        let is_append = open_handle.flags.append;
        drop(handles);

        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .get_mut(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        match entry {
            MemEntry::File(f) => {
                let write_offset = if is_append {
                    f.content.len()
                } else {
                    offset as usize
                };

                let required_len = write_offset + data.len();
                if f.content.len() < required_len {
                    f.content.resize(required_len, 0);
                }

                f.content[write_offset..write_offset + data.len()].copy_from_slice(&data);
                f.mtime = SystemTime::now();

                Ok(data.len())
            }
            MemEntry::Dir(_) => Err(FsError::is_directory(&path)),
            MemEntry::Symlink(_) => Err(FsError::invalid_argument("cannot write to symlink")),
        }
    }

    async fn close(&self, handle: Handle, _sync: bool) -> FsResult<()> {
        self.handles
            .write()
            .unwrap()
            .remove(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;
        Ok(())
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = Self::normalize_path(path);
        let entries = self.entries.read().unwrap();

        let entry = entries
            .get(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        if !matches!(entry, MemEntry::Dir(_)) {
            return Err(FsError::not_directory(&path));
        }

        let prefix = if path == "/" {
            "/".to_string()
        } else {
            format!("{path}/")
        };

        let mut results = Vec::new();
        for (entry_path, entry) in entries.iter() {
            if entry_path == &path {
                continue;
            }
            if entry_path.starts_with(&prefix) {
                let remainder = &entry_path[prefix.len()..];
                if !remainder.contains('/') {
                    results.push(entry.to_file_info(entry_path));
                }
            }
        }

        results.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(results)
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        let path = Self::normalize_path(path);

        if path == "/" {
            return Err(FsError::permission_denied("cannot remove root"));
        }

        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .get(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        if let MemEntry::Dir(_) = entry {
            let prefix = format!("{path}/");
            let has_children = entries.keys().any(|k| k.starts_with(&prefix));
            if has_children {
                return Err(FsError::directory_not_empty(&path));
            }
        }

        entries.remove(&path);
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::POSIX_LIKE | Capabilities::ETAG | Capabilities::ATOMIC_RENAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_and_read_file() {
        let fs = MemoryFs::new();

        let handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("hello world")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let handle = fs.open("/test.txt", OpenFlags::read()).await.unwrap();
        let data = fs.read(&handle, 0, 1024).await.unwrap();
        assert_eq!(&data[..], b"hello world");
        fs.close(handle, false).await.unwrap();
    }

    #[tokio::test]
    async fn create_directory_and_list() {
        let fs = MemoryFs::new();

        fs.open("/mydir", OpenFlags::create_dir()).await.unwrap();

        let handle = fs.open("/mydir/file1.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("content1")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let handle = fs.open("/mydir/file2.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("content2")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let entries = fs.readdir("/mydir").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.path == "/mydir/file1.txt"));
        assert!(entries.iter().any(|e| e.path == "/mydir/file2.txt"));
    }

    #[tokio::test]
    async fn stat_file() {
        let fs = MemoryFs::new();

        let handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("hello")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let info = fs.stat("/test.txt").await.unwrap();
        assert_eq!(info.size, 5);
        assert_eq!(info.file_type, FileType::Regular);
    }

    #[tokio::test]
    async fn rename_file() {
        let fs = MemoryFs::new();

        let handle = fs.open("/old.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("content")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        fs.wstat("/old.txt", StatChanges::rename("new.txt")).await.unwrap();

        assert!(fs.stat("/old.txt").await.is_err());
        let info = fs.stat("/new.txt").await.unwrap();
        assert_eq!(info.size, 7);
    }

    #[tokio::test]
    async fn truncate_file() {
        let fs = MemoryFs::new();

        let handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("hello world")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        fs.wstat("/test.txt", StatChanges::truncate(5)).await.unwrap();

        let handle = fs.open("/test.txt", OpenFlags::read()).await.unwrap();
        let data = fs.read(&handle, 0, 1024).await.unwrap();
        assert_eq!(&data[..], b"hello");
    }

    #[tokio::test]
    async fn create_symlink() {
        let fs = MemoryFs::new();

        let handle = fs.open("/target.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("target content")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        fs.wstat("/link", StatChanges::symlink("/target.txt")).await.unwrap();

        let info = fs.stat("/link").await.unwrap();
        assert_eq!(info.file_type, FileType::Symlink);
        assert_eq!(info.symlink_target, Some("/target.txt".to_string()));
    }

    #[tokio::test]
    async fn remove_empty_directory() {
        let fs = MemoryFs::new();

        fs.open("/mydir", OpenFlags::create_dir()).await.unwrap();
        fs.remove("/mydir").await.unwrap();

        assert!(fs.stat("/mydir").await.is_err());
    }

    #[tokio::test]
    async fn cannot_remove_non_empty_directory() {
        let fs = MemoryFs::new();

        fs.open("/mydir", OpenFlags::create_dir()).await.unwrap();
        let handle = fs.open("/mydir/file.txt", OpenFlags::create_file()).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let result = fs.remove("/mydir").await;
        assert!(matches!(result, Err(FsError::DirectoryNotEmpty(_))));
    }

    #[tokio::test]
    async fn statfs_reports_usage() {
        let fs = MemoryFs::new();

        let stats = fs.statfs("/").await.unwrap();
        assert!(stats.free_bytes > 0);
        assert!(stats.total_inodes > 0);
    }

    #[tokio::test]
    async fn append_mode() {
        let fs = MemoryFs::new();

        let handle = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("hello")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let handle = fs.open("/test.txt", OpenFlags::append()).await.unwrap();
        fs.write(&handle, 0, Bytes::from(" world")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        let handle = fs.open("/test.txt", OpenFlags::read()).await.unwrap();
        let data = fs.read(&handle, 0, 1024).await.unwrap();
        assert_eq!(&data[..], b"hello world");
    }
}
