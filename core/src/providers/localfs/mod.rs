use async_trait::async_trait;
use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FileType, FsError, FsProvider, FsResult, FsStats, Handle, OpenFlags,
    StatChanges,
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug)]
struct LocalHandle {
    file: Option<File>,
    path: PathBuf,
    flags: OpenFlags,
}

pub struct LocalFs {
    root: PathBuf,
    handles: RwLock<HashMap<u64, LocalHandle>>,
    next_handle: AtomicU64,
}

impl LocalFs {
    pub fn new(root: impl AsRef<Path>) -> FsResult<Self> {
        let root = root.as_ref().to_path_buf();
        if !root.exists() {
            return Err(FsError::not_found(root.display().to_string()));
        }
        if !root.is_dir() {
            return Err(FsError::not_directory(root.display().to_string()));
        }

        // Normalize symlinks to make `starts_with` comparisons reliable (e.g., /var vs /private/var on macOS).
        let root = root
            .canonicalize()
            .map_err(|e| FsError::internal(format!("Failed to canonicalize root: {e}")))?;

        Ok(Self {
            root,
            handles: RwLock::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        })
    }

    fn resolve_path(&self, path: &str) -> FsResult<PathBuf> {
        let path = path.trim_start_matches('/');
        let full_path = if path.is_empty() {
            self.root.clone()
        } else {
            self.root.join(path)
        };

        let canonical = full_path
            .canonicalize()
            .or_else(|_| Ok::<_, std::io::Error>(full_path.clone()))
            .map_err(|e| FsError::internal(e.to_string()))?;

        if !canonical.starts_with(&self.root) && canonical != self.root {
            return Err(FsError::permission_denied("path escapes root"));
        }

        Ok(full_path)
    }

    fn metadata_to_file_info(path: &str, meta: &fs::Metadata, symlink_target: Option<String>) -> FileInfo {
        let file_type = if meta.is_dir() {
            FileType::Directory
        } else if meta.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::Regular
        };

        let atime = meta.accessed().unwrap_or(UNIX_EPOCH);
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        let ctime = UNIX_EPOCH + Duration::from_secs(meta.ctime() as u64);

        FileInfo {
            path: path.to_string(),
            size: meta.len(),
            file_type,
            mode: meta.permissions().mode(),
            uid: meta.uid(),
            gid: meta.gid(),
            atime,
            mtime,
            ctime,
            etag: format!("{:x}-{:x}", meta.ino(), mtime.duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)),
            symlink_target,
        }
    }
}

#[async_trait]
impl FsProvider for LocalFs {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let full_path = self.resolve_path(path)?;

        let symlink_meta = fs::symlink_metadata(&full_path)
            .map_err(|e| map_io_error(e, path))?;

        let symlink_target = if symlink_meta.file_type().is_symlink() {
            fs::read_link(&full_path)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

        let meta = if symlink_meta.file_type().is_symlink() {
            symlink_meta
        } else {
            fs::metadata(&full_path).map_err(|e| map_io_error(e, path))?
        };

        Ok(Self::metadata_to_file_info(path, &meta, symlink_target))
    }

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()> {
        let full_path = self.resolve_path(path)?;

        if let Some(target) = changes.symlink_target {
            std::os::unix::fs::symlink(&target, &full_path)
                .map_err(|e| map_io_error(e, path))?;
            return Ok(());
        }

        if let Some(new_name) = changes.name {
            let new_path = if new_name.starts_with('/') {
                self.resolve_path(&new_name)?
            } else {
                full_path.parent().unwrap_or(&self.root).join(&new_name)
            };
            fs::rename(&full_path, &new_path).map_err(|e| map_io_error(e, path))?;
            return Ok(());
        }

        if let Some(mode) = changes.mode {
            fs::set_permissions(&full_path, fs::Permissions::from_mode(mode))
                .map_err(|e| map_io_error(e, path))?;
        }

        if changes.uid.is_some() || changes.gid.is_some() {
            let meta = fs::metadata(&full_path).map_err(|e| map_io_error(e, path))?;
            let uid = changes.uid.unwrap_or(meta.uid());
            let gid = changes.gid.unwrap_or(meta.gid());

            #[cfg(unix)]
            {
                use std::os::unix::fs::chown;
                chown(&full_path, Some(uid), Some(gid)).map_err(|e| map_io_error(e, path))?;
            }
        }

        if let Some(size) = changes.size {
            let file = File::options()
                .write(true)
                .open(&full_path)
                .map_err(|e| map_io_error(e, path))?;
            file.set_len(size).map_err(|e| map_io_error(e, path))?;
        }

        if changes.atime.is_some() || changes.mtime.is_some() {
            let meta = fs::metadata(&full_path).map_err(|e| map_io_error(e, path))?;
            let atime = changes.atime.unwrap_or_else(|| meta.accessed().unwrap_or(UNIX_EPOCH));
            let mtime = changes.mtime.unwrap_or_else(|| meta.modified().unwrap_or(UNIX_EPOCH));

            filetime::set_file_times(
                &full_path,
                filetime::FileTime::from_system_time(atime),
                filetime::FileTime::from_system_time(mtime),
            )
            .map_err(|e| map_io_error(e, path))?;
        }

        Ok(())
    }

    async fn statfs(&self, _path: &str) -> FsResult<FsStats> {
        #[cfg(unix)]
        {
            use std::mem::MaybeUninit;
            use std::os::unix::ffi::OsStrExt;

            let path_cstr = std::ffi::CString::new(self.root.as_os_str().as_bytes())
                .map_err(|e| FsError::internal(e.to_string()))?;

            let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
            let result = unsafe { libc::statvfs(path_cstr.as_ptr(), stat.as_mut_ptr()) };

            if result != 0 {
                return Err(FsError::internal("statvfs failed"));
            }

            let stat = unsafe { stat.assume_init() };
            Ok(FsStats {
                total_bytes: (stat.f_blocks as u64) * (stat.f_frsize as u64),
                free_bytes: (stat.f_bavail as u64) * (stat.f_frsize as u64),
                total_inodes: stat.f_files as u64,
                free_inodes: stat.f_favail as u64,
                block_size: stat.f_bsize as u32,
                max_name_len: stat.f_namemax as u32,
            })
        }

        #[cfg(not(unix))]
        {
            Ok(FsStats {
                total_bytes: 0,
                free_bytes: 0,
                total_inodes: 0,
                free_inodes: 0,
                block_size: 4096,
                max_name_len: 255,
            })
        }
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let full_path = self.resolve_path(path)?;

        if flags.create && flags.directory {
            fs::create_dir(&full_path).map_err(|e| map_io_error(e, path))?;
            let meta = fs::metadata(&full_path).map_err(|e| map_io_error(e, path))?;
            let info = Self::metadata_to_file_info(path, &meta, None);

            let handle_id = self.next_handle.fetch_add(1, Ordering::SeqCst);
            self.handles.write().unwrap().insert(
                handle_id,
                LocalHandle {
                    file: None,
                    path: full_path,
                    flags,
                },
            );
            return Ok((Handle::new(handle_id), info));
        }

        let file = OpenOptions::new()
            .read(flags.read)
            .write(flags.write)
            .create(flags.create)
            .truncate(flags.truncate)
            .append(flags.append)
            .open(&full_path)
            .map_err(|e| map_io_error(e, path))?;

        let meta = file.metadata().map_err(|e| map_io_error(e, path))?;
        let symlink_target = if meta.file_type().is_symlink() {
            fs::read_link(&full_path).ok().map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };
        let info = Self::metadata_to_file_info(path, &meta, symlink_target);

        let handle_id = self.next_handle.fetch_add(1, Ordering::SeqCst);
        self.handles.write().unwrap().insert(
            handle_id,
            LocalHandle {
                file: Some(file),
                path: full_path,
                flags,
            },
        );

        Ok((Handle::new(handle_id), info))
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        let mut handles = self.handles.write().unwrap();
        let local_handle = handles
            .get_mut(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let file = local_handle
            .file
            .as_mut()
            .ok_or_else(|| FsError::is_directory(local_handle.path.display().to_string()))?;

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| FsError::internal(e.to_string()))?;

        let mut buf = vec![0u8; size];
        let n = file
            .read(&mut buf)
            .map_err(|e| FsError::internal(e.to_string()))?;
        buf.truncate(n);

        Ok(Bytes::from(buf))
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        let mut handles = self.handles.write().unwrap();
        let local_handle = handles
            .get_mut(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let file = local_handle
            .file
            .as_mut()
            .ok_or_else(|| FsError::is_directory(local_handle.path.display().to_string()))?;

        if !local_handle.flags.append {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| FsError::internal(e.to_string()))?;
        }

        file.write_all(&data)
            .map_err(|e| FsError::internal(e.to_string()))?;

        Ok(data.len())
    }

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()> {
        let local_handle = self
            .handles
            .write()
            .unwrap()
            .remove(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        if let Some(file) = local_handle.file {
            if sync {
                file.sync_all()
                    .map_err(|e| FsError::internal(e.to_string()))?;
            }
        }

        Ok(())
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let full_path = self.resolve_path(path)?;

        let entries = fs::read_dir(&full_path).map_err(|e| map_io_error(e, path))?;

        let mut results = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| FsError::internal(e.to_string()))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let entry_path = format!("{}/{}", path.trim_end_matches('/'), name);

            let symlink_meta = entry.metadata().map_err(|e| FsError::internal(e.to_string()))?;
            let symlink_target = if symlink_meta.file_type().is_symlink() {
                fs::read_link(entry.path())
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            } else {
                None
            };

            results.push(Self::metadata_to_file_info(&entry_path, &symlink_meta, symlink_target));
        }

        results.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(results)
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        let full_path = self.resolve_path(path)?;

        let meta = fs::symlink_metadata(&full_path).map_err(|e| map_io_error(e, path))?;

        if meta.is_dir() {
            fs::remove_dir(&full_path).map_err(|e| map_io_error(e, path))?;
        } else {
            fs::remove_file(&full_path).map_err(|e| map_io_error(e, path))?;
        }

        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::POSIX_LIKE
            | Capabilities::ETAG
            | Capabilities::ATOMIC_RENAME
            | Capabilities::HARDLINK
    }
}

fn map_io_error(err: std::io::Error, path: &str) -> FsError {
    match err.kind() {
        std::io::ErrorKind::NotFound => FsError::not_found(path),
        std::io::ErrorKind::PermissionDenied => FsError::permission_denied(path),
        std::io::ErrorKind::AlreadyExists => FsError::already_exists(path),
        std::io::ErrorKind::NotADirectory => FsError::not_directory(path),
        std::io::ErrorKind::IsADirectory => FsError::is_directory(path),
        std::io::ErrorKind::DirectoryNotEmpty => FsError::directory_not_empty(path),
        _ => FsError::internal(format!("{}: {}", path, err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn setup() -> (TempDir, LocalFs) {
        let temp = TempDir::new().unwrap();
        let fs = LocalFs::new(temp.path()).unwrap();
        (temp, fs)
    }

    #[tokio::test]
    async fn create_and_read_file() {
        let (_temp, fs) = setup().await;

        let (handle, _) = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("hello world")).await.unwrap();
        fs.close(handle, true).await.unwrap();

        let (handle, _) = fs.open("/test.txt", OpenFlags::read()).await.unwrap();
        let data = fs.read(&handle, 0, 1024).await.unwrap();
        assert_eq!(&data[..], b"hello world");
        fs.close(handle, false).await.unwrap();
    }

    #[tokio::test]
    async fn create_directory() {
        let (_temp, fs) = setup().await;

        let _ = fs.open("/mydir", OpenFlags::create_dir()).await.unwrap();

        let info = fs.stat("/mydir").await.unwrap();
        assert!(info.is_dir());
    }

    #[tokio::test]
    async fn readdir_contents() {
        let (_temp, fs) = setup().await;

        let _ = fs.open("/dir", OpenFlags::create_dir()).await.unwrap();
        let (h1, _) = fs.open("/dir/a.txt", OpenFlags::create_file()).await.unwrap();
        fs.close(h1, false).await.unwrap();
        let (h2, _) = fs.open("/dir/b.txt", OpenFlags::create_file()).await.unwrap();
        fs.close(h2, false).await.unwrap();

        let entries = fs.readdir("/dir").await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn remove_file() {
        let (_temp, fs) = setup().await;

        let (handle, _) = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.close(handle, false).await.unwrap();

        fs.remove("/test.txt").await.unwrap();
        assert!(fs.stat("/test.txt").await.is_err());
    }

    #[tokio::test]
    async fn rename_file() {
        let (_temp, fs) = setup().await;

        let (handle, _) = fs.open("/old.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("content")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        fs.wstat("/old.txt", StatChanges::rename("new.txt")).await.unwrap();

        assert!(fs.stat("/old.txt").await.is_err());
        let info = fs.stat("/new.txt").await.unwrap();
        assert_eq!(info.size, 7);
    }

    #[tokio::test]
    async fn truncate_file() {
        let (_temp, fs) = setup().await;

        let (handle, _) = fs.open("/test.txt", OpenFlags::create_file()).await.unwrap();
        fs.write(&handle, 0, Bytes::from("hello world")).await.unwrap();
        fs.close(handle, false).await.unwrap();

        fs.wstat("/test.txt", StatChanges::truncate(5)).await.unwrap();

        let info = fs.stat("/test.txt").await.unwrap();
        assert_eq!(info.size, 5);
    }

    #[tokio::test]
    async fn statfs_works() {
        let (_temp, fs) = setup().await;

        let stats = fs.statfs("/").await.unwrap();
        assert!(stats.total_bytes > 0);
        assert!(stats.block_size > 0);
    }

    #[tokio::test]
    async fn path_escape_blocked() {
        let (_temp, fs) = setup().await;

        let result = fs.stat("/../../../etc/passwd").await;
        assert!(result.is_err());
    }
}
