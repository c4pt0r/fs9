use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use reqwest::Client;
use serde::Serialize;

use crate::error::{Fs9Error, Result};
use crate::types::*;

/// A stream of byte chunks from a download response.
pub struct ByteStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
}

impl Stream for ByteStream {
    type Item = Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner
            .as_mut()
            .poll_next(cx)
            .map(|opt| opt.map(|res| res.map_err(Fs9Error::from)))
    }
}

pub struct Fs9Client {
    client: Client,
    base_url: String,
}

impl Fs9Client {
    pub fn new(base_url: &str) -> Result<Self> {
        Self::builder(base_url).build()
    }

    pub fn builder(base_url: &str) -> Fs9ClientBuilder {
        Fs9ClientBuilder::new(base_url)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn health(&self) -> Result<bool> {
        let resp = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await?;
        Ok(resp.status().is_success())
    }

    pub async fn stat(&self, path: &str) -> Result<FileInfo> {
        let resp = self
            .client
            .get(format!("{}/api/v1/stat", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        self.handle_response::<FileInfoResponse>(resp)
            .await
            .map(Into::into)
    }

    pub async fn wstat(&self, path: &str, changes: StatChanges) -> Result<()> {
        #[derive(Serialize)]
        struct WstatRequest<'a> {
            path: &'a str,
            changes: StatChanges,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/wstat", self.base_url))
            .json(&WstatRequest { path, changes })
            .send()
            .await?;

        self.handle_empty_response(resp).await
    }

    pub async fn statfs(&self, path: &str) -> Result<FsStats> {
        let resp = self
            .client
            .get(format!("{}/api/v1/statfs", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        self.handle_response::<FsStatsResponse>(resp)
            .await
            .map(Into::into)
    }

    pub async fn open(&self, path: &str, flags: OpenFlags) -> Result<FileHandle> {
        #[derive(Serialize)]
        struct OpenRequest<'a> {
            path: &'a str,
            flags: OpenFlags,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/open", self.base_url))
            .json(&OpenRequest { path, flags })
            .send()
            .await?;

        let open_resp: OpenResponse = self.handle_response(resp).await?;
        Ok(FileHandle {
            id: open_resp.handle_id,
            path: path.to_string(),
            metadata: open_resp.metadata.into(),
        })
    }

    pub async fn read(&self, handle: &FileHandle, offset: u64, size: usize) -> Result<Bytes> {
        #[derive(Serialize)]
        struct ReadRequest<'a> {
            handle_id: &'a str,
            offset: u64,
            size: usize,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/read", self.base_url))
            .json(&ReadRequest {
                handle_id: &handle.id,
                offset,
                size,
            })
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(self.extract_error(resp).await);
        }

        Ok(resp.bytes().await?)
    }

    pub async fn write(&self, handle: &FileHandle, offset: u64, data: &[u8]) -> Result<usize> {
        let resp = self
            .client
            .post(format!("{}/api/v1/write", self.base_url))
            .query(&[("handle_id", &handle.id), ("offset", &offset.to_string())])
            .body(data.to_vec())
            .send()
            .await?;

        let write_resp: WriteResponse = self.handle_response(resp).await?;
        Ok(write_resp.bytes_written)
    }

    pub async fn close(&self, handle: FileHandle) -> Result<()> {
        self.close_with_sync(handle, false).await
    }

    pub async fn close_with_sync(&self, handle: FileHandle, sync: bool) -> Result<()> {
        #[derive(Serialize)]
        struct CloseRequest {
            handle_id: String,
            sync: bool,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/close", self.base_url))
            .json(&CloseRequest {
                handle_id: handle.id,
                sync,
            })
            .send()
            .await?;

        self.handle_empty_response(resp).await
    }

    pub async fn readdir(&self, path: &str) -> Result<Vec<FileInfo>> {
        let resp = self
            .client
            .get(format!("{}/api/v1/readdir", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        let entries: Vec<FileInfoResponse> = self.handle_response(resp).await?;
        Ok(entries.into_iter().map(Into::into).collect())
    }

    pub async fn remove(&self, path: &str) -> Result<()> {
        let resp = self
            .client
            .delete(format!("{}/api/v1/remove", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        self.handle_empty_response(resp).await
    }

    pub async fn capabilities(&self, path: &str) -> Result<Capabilities> {
        let resp = self
            .client
            .get(format!("{}/api/v1/capabilities", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        self.handle_response::<CapabilitiesResponse>(resp)
            .await
            .map(Into::into)
    }

    pub async fn list_mounts(&self) -> Result<Vec<MountInfo>> {
        let resp = self
            .client
            .get(format!("{}/api/v1/mounts", self.base_url))
            .send()
            .await?;

        let mounts: Vec<MountResponse> = self.handle_response(resp).await?;
        Ok(mounts.into_iter().map(Into::into).collect())
    }

    pub async fn read_file(&self, path: &str) -> Result<Bytes> {
        let handle = self.open(path, OpenFlags::read()).await?;
        let size = handle.size() as usize;
        let data = self.read(&handle, 0, size.max(1024 * 1024)).await?;
        self.close(handle).await?;
        Ok(data)
    }

    pub async fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        let handle = self.open(path, OpenFlags::create_truncate()).await?;
        self.write(&handle, 0, data).await?;
        self.close(handle).await?;
        Ok(())
    }

    pub async fn download(&self, path: &str) -> Result<Bytes> {
        let resp = self
            .client
            .get(format!("{}/api/v1/download", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(self.extract_error(resp).await);
        }

        Ok(resp.bytes().await?)
    }

    pub async fn download_range(&self, path: &str, start: u64, end: u64) -> Result<Bytes> {
        let resp = self
            .client
            .get(format!("{}/api/v1/download", self.base_url))
            .query(&[("path", path)])
            .header("Range", format!("bytes={start}-{end}"))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(self.extract_error(resp).await);
        }

        Ok(resp.bytes().await?)
    }

    pub async fn download_stream(&self, path: &str) -> Result<ByteStream> {
        let resp = self
            .client
            .get(format!("{}/api/v1/download", self.base_url))
            .query(&[("path", path)])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(self.extract_error(resp).await);
        }

        Ok(ByteStream {
            inner: Box::pin(resp.bytes_stream()),
        })
    }

    pub async fn upload(&self, path: &str, data: impl Into<reqwest::Body>) -> Result<usize> {
        let resp = self
            .client
            .put(format!("{}/api/v1/upload", self.base_url))
            .query(&[("path", path)])
            .body(data)
            .send()
            .await?;

        let upload_resp: UploadResponse = self.handle_response(resp).await?;
        Ok(upload_resp.bytes_written)
    }

    pub async fn upload_stream<S>(&self, path: &str, stream: S) -> Result<usize>
    where
        S: futures_core::Stream<Item = std::result::Result<Bytes, std::io::Error>>
            + Send
            + Sync
            + 'static,
    {
        let body = reqwest::Body::wrap_stream(stream);
        self.upload(path, body).await
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        let handle = self.open(path, OpenFlags::mkdir()).await?;
        self.close(handle).await?;
        Ok(())
    }

    pub async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(Fs9Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn is_dir(&self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(info) => Ok(info.is_dir()),
            Err(Fs9Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn is_file(&self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(info) => Ok(info.is_file()),
            Err(Fs9Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn chmod(&self, path: &str, mode: u32) -> Result<()> {
        self.wstat(path, StatChanges::new().mode(mode)).await
    }

    pub async fn truncate(&self, path: &str, size: u64) -> Result<()> {
        self.wstat(path, StatChanges::new().size(size)).await
    }

    pub async fn rename(&self, path: &str, new_name: &str) -> Result<()> {
        self.wstat(path, StatChanges::new().rename(new_name)).await
    }

    pub async fn load_plugin(&self, name: &str, path: &str) -> Result<PluginInfo> {
        #[derive(Serialize)]
        struct LoadPluginRequest<'a> {
            name: &'a str,
            path: &'a str,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/plugin/load", self.base_url))
            .json(&LoadPluginRequest { name, path })
            .send()
            .await?;

        self.handle_response::<LoadPluginResponse>(resp)
            .await
            .map(Into::into)
    }

    pub async fn unload_plugin(&self, name: &str) -> Result<()> {
        #[derive(Serialize)]
        struct UnloadPluginRequest<'a> {
            name: &'a str,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/plugin/unload", self.base_url))
            .json(&UnloadPluginRequest { name })
            .send()
            .await?;

        self.handle_empty_response(resp).await
    }

    pub async fn list_plugins(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/api/v1/plugin/list", self.base_url))
            .send()
            .await?;

        self.handle_response(resp).await
    }

    pub async fn mount_plugin(
        &self,
        mount_path: &str,
        provider: &str,
        config: Option<serde_json::Value>,
    ) -> Result<MountInfo> {
        #[derive(Serialize)]
        struct MountPluginRequest<'a> {
            path: &'a str,
            provider: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            config: Option<serde_json::Value>,
        }

        let resp = self
            .client
            .post(format!("{}/api/v1/mount", self.base_url))
            .json(&MountPluginRequest {
                path: mount_path,
                provider,
                config,
            })
            .send()
            .await?;

        self.handle_response::<MountResponse>(resp)
            .await
            .map(Into::into)
    }

    pub async fn events(&self, query: &EventsQuery) -> Result<Vec<AuditEvent>> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(limit) = query.limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(offset) = query.offset {
            params.push(("offset", offset.to_string()));
        }
        if let Some(ref path) = query.path {
            params.push(("path", path.clone()));
        }
        if let Some(ref event_type) = query.event_type {
            params.push(("type", event_type.clone()));
        }

        let resp = self
            .client
            .get(format!("{}/api/v1/events", self.base_url))
            .query(&params)
            .send()
            .await?;

        self.handle_response(resp).await
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T> {
        if !resp.status().is_success() {
            return Err(self.extract_error(resp).await);
        }
        Ok(resp.json().await?)
    }

    async fn handle_empty_response(&self, resp: reqwest::Response) -> Result<()> {
        if !resp.status().is_success() {
            return Err(self.extract_error(resp).await);
        }
        Ok(())
    }

    async fn extract_error(&self, resp: reqwest::Response) -> Fs9Error {
        let status = resp.status().as_u16();
        match resp.json::<ErrorResponse>().await {
            Ok(err_resp) => Fs9Error::from_response(status, err_resp.error),
            Err(_) => Fs9Error::Request {
                status,
                message: "unknown error".to_string(),
            },
        }
    }
}

pub struct Fs9ClientBuilder {
    base_url: String,
    timeout: Duration,
    token: Option<String>,
}

impl Fs9ClientBuilder {
    fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(30),
            token: None,
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn build(self) -> Result<Fs9Client> {
        let mut builder = Client::builder().timeout(self.timeout);

        if let Some(token) = &self.token {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token)
                    .parse()
                    .map_err(|_| Fs9Error::InvalidArgument("invalid token".to_string()))?,
            );
            builder = builder.default_headers(headers);
        }

        let client = builder
            .build()
            .map_err(|e| Fs9Error::Connection(e.to_string()))?;

        Ok(Fs9Client {
            client,
            base_url: self.base_url,
        })
    }
}
