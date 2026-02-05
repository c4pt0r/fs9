use async_trait::async_trait;
use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FileType, FsError, FsProvider, FsResult, FsStats, Handle, OpenFlags,
    StatChanges,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, UNIX_EPOCH};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HOPS: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
struct FileInfoResponse {
    path: String,
    size: u64,
    file_type: String,
    mode: u32,
    uid: u32,
    gid: u32,
    atime: u64,
    mtime: u64,
    ctime: u64,
    etag: String,
    symlink_target: Option<String>,
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
            atime: UNIX_EPOCH + Duration::from_secs(resp.atime),
            mtime: UNIX_EPOCH + Duration::from_secs(resp.mtime),
            ctime: UNIX_EPOCH + Duration::from_secs(resp.ctime),
            etag: resp.etag,
            symlink_target: resp.symlink_target,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FsStatsResponse {
    total_bytes: u64,
    free_bytes: u64,
    total_inodes: u64,
    free_inodes: u64,
    block_size: u32,
    max_name_len: u32,
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

#[derive(Debug, Serialize)]
struct WstatRequest {
    path: String,
    changes: StatChangesRequest,
}

#[derive(Debug, Serialize, Default)]
struct StatChangesRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    atime: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtime: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symlink_target: Option<String>,
}

impl From<StatChanges> for StatChangesRequest {
    fn from(changes: StatChanges) -> Self {
        Self {
            mode: changes.mode,
            uid: changes.uid,
            gid: changes.gid,
            size: changes.size,
            atime: changes.atime.map(|t| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)),
            mtime: changes.mtime.map(|t| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)),
            name: changes.name,
            symlink_target: changes.symlink_target,
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenRequest {
    path: String,
    flags: OpenFlagsRequest,
}

#[derive(Debug, Serialize, Default)]
struct OpenFlagsRequest {
    read: bool,
    write: bool,
    create: bool,
    truncate: bool,
    append: bool,
    directory: bool,
}

impl From<OpenFlags> for OpenFlagsRequest {
    fn from(flags: OpenFlags) -> Self {
        Self {
            read: flags.read,
            write: flags.write,
            create: flags.create,
            truncate: flags.truncate,
            append: flags.append,
            directory: flags.directory,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenResponse {
    handle_id: String,
    metadata: FileInfoResponse,
}

#[derive(Debug, Serialize)]
struct ReadRequest {
    handle_id: String,
    offset: u64,
    size: usize,
}

#[derive(Debug, Serialize)]
struct CloseRequest {
    handle_id: String,
    sync: bool,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
    #[allow(dead_code)]
    code: u16,
}

pub struct ProxyFs {
    upstream_url: String,
    client: Client,
    jwt_token: Option<String>,
    hop_count: usize,
    max_hops: usize,
    handles: RwLock<HashMap<u64, String>>,
    next_handle: AtomicU64,
    capabilities: Capabilities,
}

impl ProxyFs {
    pub fn new(upstream_url: impl Into<String>) -> Self {
        Self {
            upstream_url: upstream_url.into().trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("Failed to create HTTP client"),
            jwt_token: None,
            hop_count: 0,
            max_hops: MAX_HOPS,
            handles: RwLock::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
            capabilities: Capabilities::all(),
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.jwt_token = Some(token.into());
        self
    }

    pub fn with_hop_count(mut self, hop_count: usize) -> Self {
        self.hop_count = hop_count;
        self
    }

    pub fn with_max_hops(mut self, max_hops: usize) -> Self {
        self.max_hops = max_hops;
        self
    }

    fn check_hop_limit(&self) -> FsResult<()> {
        if self.hop_count >= self.max_hops {
            return Err(FsError::TooManyHops {
                depth: self.hop_count,
                max: self.max_hops,
            });
        }
        Ok(())
    }

    fn build_request(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let url = format!("{}/api/v1{}", self.upstream_url, endpoint);
        let mut req = self.client.request(method, &url);

        if let Some(token) = &self.jwt_token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        req = req.header("X-FS9-Hop-Count", (self.hop_count + 1).to_string());
        req
    }

    async fn handle_error_response(&self, resp: reqwest::Response) -> FsError {
        let status = resp.status().as_u16();
        match resp.json::<ErrorResponse>().await {
            Ok(err) => self.map_error_code(status, &err.error),
            Err(_) => FsError::internal(format!("HTTP {status}")),
        }
    }

    fn map_error_code(&self, status: u16, message: &str) -> FsError {
        match status {
            404 => FsError::not_found(message),
            403 => FsError::permission_denied(message),
            409 => FsError::already_exists(message),
            400 => FsError::invalid_argument(message),
            501 => FsError::not_implemented(message),
            503 => FsError::backend_unavailable(&self.upstream_url),
            504 => FsError::timeout(DEFAULT_TIMEOUT),
            508 => FsError::TooManyHops { depth: self.hop_count + 1, max: self.max_hops },
            _ => FsError::Remote {
                node: self.upstream_url.clone(),
                message: message.to_string(),
            },
        }
    }

    fn map_request_error(&self, err: reqwest::Error) -> FsError {
        if err.is_timeout() {
            FsError::timeout(DEFAULT_TIMEOUT)
        } else if err.is_connect() {
            FsError::backend_unavailable(&self.upstream_url)
        } else {
            FsError::transient(err.to_string())
        }
    }
}

#[async_trait]
impl FsProvider for ProxyFs {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        self.check_hop_limit()?;

        let resp = self
            .build_request(reqwest::Method::GET, "/stat")
            .query(&[("path", path)])
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        let info: FileInfoResponse = resp
            .json()
            .await
            .map_err(|e| FsError::internal(e.to_string()))?;

        Ok(info.into())
    }

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()> {
        self.check_hop_limit()?;

        let req_body = WstatRequest {
            path: path.to_string(),
            changes: changes.into(),
        };

        let resp = self
            .build_request(reqwest::Method::POST, "/wstat")
            .json(&req_body)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        Ok(())
    }

    async fn statfs(&self, path: &str) -> FsResult<FsStats> {
        self.check_hop_limit()?;

        let resp = self
            .build_request(reqwest::Method::GET, "/statfs")
            .query(&[("path", path)])
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        let stats: FsStatsResponse = resp
            .json()
            .await
            .map_err(|e| FsError::internal(e.to_string()))?;

        Ok(stats.into())
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        self.check_hop_limit()?;

        let req_body = OpenRequest {
            path: path.to_string(),
            flags: flags.into(),
        };

        let resp = self
            .build_request(reqwest::Method::POST, "/open")
            .json(&req_body)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        let open_resp: OpenResponse = resp
            .json()
            .await
            .map_err(|e| FsError::internal(e.to_string()))?;

        let local_handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
        self.handles
            .write()
            .unwrap()
            .insert(local_handle, open_resp.handle_id);

        Ok((Handle::new(local_handle), open_resp.metadata.into()))
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        self.check_hop_limit()?;

        let remote_handle = self
            .handles
            .read()
            .unwrap()
            .get(&handle.id())
            .cloned()
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let req_body = ReadRequest {
            handle_id: remote_handle,
            offset,
            size,
        };

        let resp = self
            .build_request(reqwest::Method::POST, "/read")
            .json(&req_body)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        let data = resp
            .bytes()
            .await
            .map_err(|e| FsError::internal(e.to_string()))?;

        Ok(data)
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        self.check_hop_limit()?;

        let remote_handle = self
            .handles
            .read()
            .unwrap()
            .get(&handle.id())
            .cloned()
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let resp = self
            .build_request(reqwest::Method::POST, "/write")
            .query(&[
                ("handle_id", remote_handle.as_str()),
                ("offset", &offset.to_string()),
            ])
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        #[derive(Deserialize)]
        struct WriteResponse {
            bytes_written: usize,
        }

        let write_resp: WriteResponse = resp
            .json()
            .await
            .map_err(|e| FsError::internal(e.to_string()))?;

        Ok(write_resp.bytes_written)
    }

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()> {
        self.check_hop_limit()?;

        let remote_handle = self
            .handles
            .write()
            .unwrap()
            .remove(&handle.id())
            .ok_or_else(|| FsError::invalid_handle(handle.id()))?;

        let req_body = CloseRequest {
            handle_id: remote_handle,
            sync,
        };

        let resp = self
            .build_request(reqwest::Method::POST, "/close")
            .json(&req_body)
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        Ok(())
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        self.check_hop_limit()?;

        let resp = self
            .build_request(reqwest::Method::GET, "/readdir")
            .query(&[("path", path)])
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        let entries: Vec<FileInfoResponse> = resp
            .json()
            .await
            .map_err(|e| FsError::internal(e.to_string()))?;

        Ok(entries.into_iter().map(Into::into).collect())
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        self.check_hop_limit()?;

        let resp = self
            .build_request(reqwest::Method::DELETE, "/remove")
            .query(&[("path", path)])
            .send()
            .await
            .map_err(|e| self.map_request_error(e))?;

        if !resp.status().is_success() {
            return Err(self.handle_error_response(resp).await);
        }

        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_limit_exceeded() {
        let proxy = ProxyFs::new("http://localhost:3000").with_hop_count(10).with_max_hops(8);
        let result = proxy.check_hop_limit();
        assert!(matches!(result, Err(FsError::TooManyHops { depth: 10, max: 8 })));
    }

    #[test]
    fn hop_limit_ok() {
        let proxy = ProxyFs::new("http://localhost:3000").with_hop_count(5).with_max_hops(8);
        assert!(proxy.check_hop_limit().is_ok());
    }

    #[test]
    fn builder_pattern() {
        let proxy = ProxyFs::new("http://localhost:3000")
            .with_token("test-token")
            .with_hop_count(2)
            .with_max_hops(10);

        assert_eq!(proxy.jwt_token, Some("test-token".to_string()));
        assert_eq!(proxy.hop_count, 2);
        assert_eq!(proxy.max_hops, 10);
    }
}
