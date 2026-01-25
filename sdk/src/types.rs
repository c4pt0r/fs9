use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub file_type: FileType,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub etag: String,
    pub symlink_target: Option<String>,
}

impl FileInfo {
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }

    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.file_type == FileType::Symlink
    }

    #[must_use]
    pub fn is_regular(&self) -> bool {
        self.file_type == FileType::Regular
    }
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StatChanges {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub atime: Option<SystemTime>,
    pub mtime: Option<SystemTime>,
    pub name: Option<String>,
    pub symlink_target: Option<String>,
}

impl StatChanges {
    #[must_use]
    pub fn chmod(mode: u32) -> Self {
        Self {
            mode: Some(mode),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn chown(uid: u32, gid: u32) -> Self {
        Self {
            uid: Some(uid),
            gid: Some(gid),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn truncate(size: u64) -> Self {
        Self {
            size: Some(size),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn rename(new_name: impl Into<String>) -> Self {
        Self {
            name: Some(new_name.into()),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn symlink(target: impl Into<String>) -> Self {
        Self {
            symlink_target: Some(target.into()),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn utime(atime: SystemTime, mtime: SystemTime) -> Self {
        Self {
            atime: Some(atime),
            mtime: Some(mtime),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mode.is_none()
            && self.uid.is_none()
            && self.gid.is_none()
            && self.size.is_none()
            && self.atime.is_none()
            && self.mtime.is_none()
            && self.name.is_none()
            && self.symlink_target.is_none()
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FsStats {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub block_size: u32,
    pub max_name_len: u32,
}

impl FsStats {
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    #[must_use]
    pub fn used_inodes(&self) -> u64 {
        self.total_inodes.saturating_sub(self.free_inodes)
    }

    #[must_use]
    pub fn usage_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes() as f64 / self.total_bytes as f64) * 100.0
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
    pub directory: bool,
}

impl OpenFlags {
    #[must_use]
    pub fn read() -> Self {
        Self {
            read: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn write() -> Self {
        Self {
            write: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn create_file() -> Self {
        Self {
            read: true,
            write: true,
            create: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn create_dir() -> Self {
        Self {
            create: true,
            directory: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn append() -> Self {
        Self {
            write: true,
            append: true,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn create_truncate() -> Self {
        Self {
            read: true,
            write: true,
            create: true,
            truncate: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Handle(pub u64);

impl Handle {
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn id(&self) -> u64 {
        self.0
    }
}

impl From<u64> for Handle {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<Handle> for u64 {
    fn from(handle: Handle) -> Self {
        handle.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_info_type_checks() {
        let dir = FileInfo {
            path: "/test".into(),
            size: 0,
            file_type: FileType::Directory,
            mode: 0o755,
            uid: 1000,
            gid: 1000,
            atime: SystemTime::UNIX_EPOCH,
            mtime: SystemTime::UNIX_EPOCH,
            ctime: SystemTime::UNIX_EPOCH,
            etag: String::new(),
            symlink_target: None,
        };
        assert!(dir.is_dir());
        assert!(!dir.is_symlink());
        assert!(!dir.is_regular());

        let symlink = FileInfo {
            file_type: FileType::Symlink,
            symlink_target: Some("/target".into()),
            ..dir.clone()
        };
        assert!(symlink.is_symlink());
        assert!(!symlink.is_dir());

        let file = FileInfo {
            file_type: FileType::Regular,
            size: 1024,
            ..dir
        };
        assert!(file.is_regular());
        assert!(!file.is_dir());
    }

    #[test]
    fn stat_changes_constructors() {
        let chmod = StatChanges::chmod(0o644);
        assert_eq!(chmod.mode, Some(0o644));
        assert!(chmod.uid.is_none());

        let chown = StatChanges::chown(1000, 1000);
        assert_eq!(chown.uid, Some(1000));
        assert_eq!(chown.gid, Some(1000));

        let truncate = StatChanges::truncate(0);
        assert_eq!(truncate.size, Some(0));

        let rename = StatChanges::rename("newname");
        assert_eq!(rename.name.as_deref(), Some("newname"));

        let symlink = StatChanges::symlink("/target");
        assert_eq!(symlink.symlink_target.as_deref(), Some("/target"));

        let empty = StatChanges::default();
        assert!(empty.is_empty());
        assert!(!chmod.is_empty());
    }

    #[test]
    fn fs_stats_calculations() {
        let stats = FsStats {
            total_bytes: 1000,
            free_bytes: 400,
            total_inodes: 100,
            free_inodes: 50,
            block_size: 4096,
            max_name_len: 255,
        };
        assert_eq!(stats.used_bytes(), 600);
        assert_eq!(stats.used_inodes(), 50);
        assert!((stats.usage_percent() - 60.0).abs() < 0.001);

        let empty = FsStats {
            total_bytes: 0,
            free_bytes: 0,
            total_inodes: 0,
            free_inodes: 0,
            block_size: 4096,
            max_name_len: 255,
        };
        assert!((empty.usage_percent() - 0.0).abs() < 0.001);
    }

    #[test]
    fn open_flags_constructors() {
        let read = OpenFlags::read();
        assert!(read.read);
        assert!(!read.write);

        let write = OpenFlags::write();
        assert!(!write.read);
        assert!(write.write);

        let rw = OpenFlags::read_write();
        assert!(rw.read);
        assert!(rw.write);

        let create_file = OpenFlags::create_file();
        assert!(create_file.read);
        assert!(create_file.write);
        assert!(create_file.create);
        assert!(!create_file.directory);

        let create_dir = OpenFlags::create_dir();
        assert!(create_dir.create);
        assert!(create_dir.directory);

        let append = OpenFlags::append();
        assert!(append.write);
        assert!(append.append);

        let create_truncate = OpenFlags::create_truncate();
        assert!(create_truncate.create);
        assert!(create_truncate.truncate);
    }

    #[test]
    fn handle_conversions() {
        let handle = Handle::new(42);
        assert_eq!(handle.id(), 42);

        let from_u64: Handle = 123u64.into();
        assert_eq!(from_u64.0, 123);

        let to_u64: u64 = handle.into();
        assert_eq!(to_u64, 42);
    }
}
