use fs9_sdk::{FileInfo, FileType, FsStats, OpenFlags};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct StatRequest {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileInfoResponse {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

impl From<FileInfo> for FileInfoResponse {
    fn from(info: FileInfo) -> Self {
        Self {
            path: info.path,
            size: info.size,
            file_type: match info.file_type {
                FileType::Regular => "regular".to_string(),
                FileType::Directory => "directory".to_string(),
                FileType::Symlink => "symlink".to_string(),
            },
            mode: info.mode,
            uid: info.uid,
            gid: info.gid,
            atime: info
                .atime
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            mtime: info
                .mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            ctime: info
                .ctime
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            etag: info.etag,
            symlink_target: info.symlink_target,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WstatRequest {
    pub path: String,
    pub changes: StatChangesRequest,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StatChangesRequest {
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

impl From<StatChangesRequest> for fs9_sdk::StatChanges {
    fn from(req: StatChangesRequest) -> Self {
        Self {
            mode: req.mode,
            uid: req.uid,
            gid: req.gid,
            size: req.size,
            atime: req
                .atime
                .map(|t| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t)),
            mtime: req
                .mtime
                .map(|t| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t)),
            name: req.name,
            symlink_target: req.symlink_target,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatfsRequest {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FsStatsResponse {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub block_size: u32,
    pub max_name_len: u32,
}

impl From<FsStats> for FsStatsResponse {
    fn from(stats: FsStats) -> Self {
        Self {
            total_bytes: stats.total_bytes,
            free_bytes: stats.free_bytes,
            total_inodes: stats.total_inodes,
            free_inodes: stats.free_inodes,
            block_size: stats.block_size,
            max_name_len: stats.max_name_len,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenRequest {
    pub path: String,
    pub flags: OpenFlagsRequest,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct OpenFlagsRequest {
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

impl From<OpenFlagsRequest> for OpenFlags {
    fn from(req: OpenFlagsRequest) -> Self {
        Self {
            read: req.read,
            write: req.write,
            create: req.create,
            truncate: req.truncate,
            append: req.append,
            directory: req.directory,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenResponse {
    pub handle_id: String,
    pub metadata: FileInfoResponse,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadRequest {
    pub handle_id: String,
    pub offset: u64,
    pub size: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WriteRequest {
    pub handle_id: String,
    pub offset: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WriteResponse {
    pub bytes_written: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CloseRequest {
    pub handle_id: String,
    #[serde(default)]
    pub sync: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReaddirRequest {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveRequest {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesRequest {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub capabilities: Vec<String>,
    pub provider_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MountRequest {
    pub path: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MountResponse {
    pub path: String,
    pub provider_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoadPluginRequest {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoadPluginResponse {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnloadPluginRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MountPluginRequest {
    pub path: String,
    pub provider: String,
    #[serde(default)]
    pub config: serde_json::Value,
}

// ============================================================================
// Namespace management models
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateNamespaceRequest {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NamespaceInfoResponse {
    pub name: String,
    pub created_at: String,
    pub created_by: String,
    pub status: String,
}

// ============================================================================
// Auth models
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct RefreshTokenResponse {
    pub token: String,
    pub expires_in: u64,
}
