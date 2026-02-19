use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory)
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::Regular)
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink)
    }
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub file_type: FileType,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub etag: String,
    pub symlink_target: Option<String>,
}

impl FileInfo {
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    pub fn is_file(&self) -> bool {
        self.file_type.is_file()
    }

    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone)]
pub struct FsStats {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub block_size: u32,
    pub max_name_len: u32,
}

impl FsStats {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    pub fn usage_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes() as f64 / self.total_bytes as f64) * 100.0
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct OpenFlags {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub create: bool,
    #[serde(default)]
    pub truncate: bool,
    #[serde(default)]
    pub append: bool,
    #[serde(default)]
    pub directory: bool,
}

impl OpenFlags {
    pub fn read() -> Self {
        Self {
            read: true,
            ..Default::default()
        }
    }

    pub fn write() -> Self {
        Self {
            write: true,
            ..Default::default()
        }
    }

    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            ..Default::default()
        }
    }

    pub fn create() -> Self {
        Self {
            read: true,
            write: true,
            create: true,
            ..Default::default()
        }
    }

    pub fn create_truncate() -> Self {
        Self {
            read: true,
            write: true,
            create: true,
            truncate: true,
            ..Default::default()
        }
    }

    pub fn append() -> Self {
        Self {
            write: true,
            append: true,
            ..Default::default()
        }
    }

    pub fn mkdir() -> Self {
        Self {
            create: true,
            directory: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatChanges {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atime: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

impl StatChanges {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(mut self, mode: u32) -> Self {
        self.mode = Some(mode);
        self
    }

    pub fn uid(mut self, uid: u32) -> Self {
        self.uid = Some(uid);
        self
    }

    pub fn gid(mut self, gid: u32) -> Self {
        self.gid = Some(gid);
        self
    }

    pub fn size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn rename(mut self, new_name: impl Into<String>) -> Self {
        self.name = Some(new_name.into());
        self
    }

    pub fn symlink(mut self, target: impl Into<String>) -> Self {
        self.symlink_target = Some(target.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct FileHandle {
    pub id: String,
    pub path: String,
    pub metadata: FileInfo,
}

impl FileHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn metadata(&self) -> &FileInfo {
        &self.metadata
    }

    pub fn size(&self) -> u64 {
        self.metadata.size
    }
}

#[derive(Debug, Clone)]
pub struct MountInfo {
    pub path: String,
    pub provider_name: String,
}

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub capabilities: Vec<String>,
    pub provider_type: String,
}

impl Capabilities {
    pub fn can_read(&self) -> bool {
        self.capabilities.iter().any(|c| c == "read")
    }

    pub fn can_write(&self) -> bool {
        self.capabilities.iter().any(|c| c == "write")
    }

    pub fn can_create(&self) -> bool {
        self.capabilities.iter().any(|c| c == "create")
    }

    pub fn can_delete(&self) -> bool {
        self.capabilities.iter().any(|c| c == "delete")
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileInfoResponse {
    pub path: String,
    pub size: u64,
    pub file_type: String,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub etag: String,
    pub symlink_target: Option<String>,
}

impl From<FileInfoResponse> for FileInfo {
    fn from(resp: FileInfoResponse) -> Self {
        let file_type = match resp.file_type.as_str() {
            "directory" => FileType::Directory,
            "symlink" => FileType::Symlink,
            _ => FileType::Regular,
        };
        Self {
            path: resp.path,
            size: resp.size,
            file_type,
            mode: resp.mode,
            uid: resp.uid,
            gid: resp.gid,
            atime: resp.atime,
            mtime: resp.mtime,
            ctime: resp.ctime,
            etag: resp.etag,
            symlink_target: resp.symlink_target,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct FsStatsResponse {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub block_size: u32,
    pub max_name_len: u32,
}

impl From<FsStatsResponse> for FsStats {
    fn from(resp: FsStatsResponse) -> Self {
        Self {
            total_bytes: resp.total_bytes,
            free_bytes: resp.free_bytes,
            total_inodes: resp.total_inodes,
            free_inodes: resp.free_inodes,
            block_size: resp.block_size,
            max_name_len: resp.max_name_len,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenResponse {
    pub handle_id: String,
    pub metadata: FileInfoResponse,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WriteResponse {
    pub bytes_written: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MountResponse {
    pub path: String,
    pub provider_name: String,
}

impl From<MountResponse> for MountInfo {
    fn from(resp: MountResponse) -> Self {
        Self {
            path: resp.path,
            provider_name: resp.provider_name,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CapabilitiesResponse {
    pub capabilities: Vec<String>,
    pub provider_type: String,
}

impl From<CapabilitiesResponse> for Capabilities {
    fn from(resp: CapabilitiesResponse) -> Self {
        Self {
            capabilities: resp.capabilities,
            provider_type: resp.provider_type,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct UploadResponse {
    #[allow(dead_code)]
    pub path: String,
    pub bytes_written: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorResponse {
    pub error: String,
    #[allow(dead_code)]
    pub code: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: u64,
    pub event_type: String,
    pub path: String,
    pub user: String,
    pub count: u64,
}

#[derive(Debug, Default)]
pub struct EventsQuery {
    pub limit: Option<usize>,
    pub path: Option<String>,
    pub event_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LoadPluginResponse {
    pub name: String,
    pub status: String,
}

impl From<LoadPluginResponse> for PluginInfo {
    fn from(resp: LoadPluginResponse) -> Self {
        Self {
            name: resp.name,
            status: resp.status,
        }
    }
}
