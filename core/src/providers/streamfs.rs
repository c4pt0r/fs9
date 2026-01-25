use async_trait::async_trait;
use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FileType, FsError, FsProvider, FsResult, FsStats, Handle, OpenFlags,
    StatChanges,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::broadcast;
use tokio::sync::RwLock as TokioRwLock;

const DEFAULT_RING_SIZE: usize = 100;
const DEFAULT_CHANNEL_SIZE: usize = 100;

const README_CONTENT: &str = r#"StreamFS - Streaming File System

This plugin provides streaming files that support multiple concurrent readers
and writers with real-time data fanout and ring buffer for late joiners.

FEATURES:
  - Multiple writers can append data to a stream concurrently
  - Multiple readers can consume from the stream independently (fanout/broadcast)
  - Ring buffer stores recent data for late-joining readers
  - Memory-based storage with configurable buffer sizes

USAGE:
  Write:  echo "data" > /streamfs/mystream
  Read:   cat /streamfs/mystream (streaming mode recommended)

  In sh9 shell:
    echo "hello" > /streamfs/events
    cat --stream /streamfs/events

NOTES:
  - Streams are append-only (offset is ignored on write)
  - Data is in-memory only (not persistent across restarts)
  - Late-joining readers receive historical data from ring buffer
"#;

struct ReaderState {
    id: u64,
    #[allow(dead_code)]
    registered_at: SystemTime,
    dropped_count: AtomicU64,
}

pub struct StreamFile {
    name: String,
    total_written: AtomicU64,
    closed: RwLock<bool>,
    mtime: RwLock<SystemTime>,
    ring_buffer: RwLock<Vec<Bytes>>,
    ring_size: usize,
    write_index: AtomicU64,
    total_chunks: AtomicU64,
    sender: broadcast::Sender<Bytes>,
    readers: RwLock<HashMap<u64, Arc<ReaderState>>>,
    next_reader_id: AtomicU64,
}

impl StreamFile {
    pub fn new(name: String, ring_size: usize, channel_size: usize) -> Self {
        let (sender, _) = broadcast::channel(channel_size);
        Self {
            name,
            total_written: AtomicU64::new(0),
            closed: RwLock::new(false),
            mtime: RwLock::new(SystemTime::now()),
            ring_buffer: RwLock::new(vec![Bytes::new(); ring_size]),
            ring_size,
            write_index: AtomicU64::new(0),
            total_chunks: AtomicU64::new(0),
            sender,
            readers: RwLock::new(HashMap::new()),
            next_reader_id: AtomicU64::new(1),
        }
    }

    pub fn get_info(&self) -> FileInfo {
        let name = self.name.trim_start_matches('/').to_string();
        FileInfo {
            path: self.name.clone(),
            size: self.total_written.load(Ordering::SeqCst),
            file_type: FileType::Regular,
            mode: 0o644,
            uid: 0,
            gid: 0,
            atime: *self.mtime.read().unwrap(),
            mtime: *self.mtime.read().unwrap(),
            ctime: *self.mtime.read().unwrap(),
            etag: format!("stream-{}", self.total_chunks.load(Ordering::SeqCst)),
            symlink_target: None,
        }
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.read().unwrap()
    }

    pub fn write(&self, data: Bytes) -> FsResult<usize> {
        if self.is_closed() {
            return Err(FsError::internal("stream is closed"));
        }

        let len = data.len();
        if len == 0 {
            return Ok(0);
        }

        {
            let mut ring = self.ring_buffer.write().unwrap();
            let idx = (self.write_index.load(Ordering::SeqCst) as usize) % self.ring_size;
            ring[idx] = data.clone();
        }

        self.write_index.fetch_add(1, Ordering::SeqCst);
        self.total_chunks.fetch_add(1, Ordering::SeqCst);
        self.total_written.fetch_add(len as u64, Ordering::SeqCst);
        *self.mtime.write().unwrap() = SystemTime::now();

        let _ = self.sender.send(data);

        Ok(len)
    }

    pub fn register_reader(&self) -> (u64, broadcast::Receiver<Bytes>) {
        let id = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        let receiver = self.sender.subscribe();

        let state = Arc::new(ReaderState {
            id,
            registered_at: SystemTime::now(),
            dropped_count: AtomicU64::new(0),
        });

        self.readers.write().unwrap().insert(id, state);

        (id, receiver)
    }

    pub fn unregister_reader(&self, reader_id: u64) {
        self.readers.write().unwrap().remove(&reader_id);
    }

    pub fn get_historical_chunks(&self, from_index: u64) -> Vec<Bytes> {
        let ring = self.ring_buffer.read().unwrap();
        let total = self.total_chunks.load(Ordering::SeqCst);
        let oldest = total.saturating_sub(self.ring_size as u64);

        let start = from_index.max(oldest);
        let mut chunks = Vec::new();

        for i in start..total {
            let idx = (i as usize) % self.ring_size;
            if !ring[idx].is_empty() {
                chunks.push(ring[idx].clone());
            }
        }

        chunks
    }

    pub fn get_reader_count(&self) -> usize {
        self.readers.read().unwrap().len()
    }

    pub fn close(&self) {
        *self.closed.write().unwrap() = true;
    }
}

struct StreamHandle {
    id: u64,
    path: String,
    flags: OpenFlags,
    stream: Option<Arc<StreamFile>>,
    reader_id: Option<u64>,
    receiver: Option<broadcast::Receiver<Bytes>>,
    read_buffer: Vec<u8>,
    read_base: u64,
    historical_sent: bool,
    historical_index: u64,
}

pub struct StreamFS {
    streams: RwLock<HashMap<String, Arc<StreamFile>>>,
    ring_size: usize,
    channel_size: usize,
    handles: TokioRwLock<HashMap<u64, StreamHandle>>,
    next_handle_id: AtomicU64,
}

impl Default for StreamFS {
    fn default() -> Self {
        Self::new(DEFAULT_RING_SIZE, DEFAULT_CHANNEL_SIZE)
    }
}

impl StreamFS {
    #[must_use]
    pub fn new(ring_size: usize, channel_size: usize) -> Self {
        Self {
            streams: RwLock::new(HashMap::new()),
            ring_size,
            channel_size,
            handles: TokioRwLock::new(HashMap::new()),
            next_handle_id: AtomicU64::new(1),
        }
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

    fn readme_file_info() -> FileInfo {
        FileInfo {
            path: "/README".to_string(),
            size: README_CONTENT.len() as u64,
            file_type: FileType::Regular,
            mode: 0o444,
            uid: 0,
            gid: 0,
            atime: SystemTime::UNIX_EPOCH,
            mtime: SystemTime::UNIX_EPOCH,
            ctime: SystemTime::UNIX_EPOCH,
            etag: "readme".to_string(),
            symlink_target: None,
        }
    }
}

#[async_trait]
impl FsProvider for StreamFS {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path = Self::normalize_path(path);

        if path == "/" {
            return Ok(FileInfo {
                path: "/".to_string(),
                size: 0,
                file_type: FileType::Directory,
                mode: 0o755,
                uid: 0,
                gid: 0,
                atime: SystemTime::now(),
                mtime: SystemTime::now(),
                ctime: SystemTime::now(),
                etag: "root".to_string(),
                symlink_target: None,
            });
        }

        if path == "/README" {
            return Ok(Self::readme_file_info());
        }

        let streams = self.streams.read().unwrap();
        let stream = streams
            .get(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        Ok(stream.get_info())
    }

    async fn wstat(&self, _path: &str, _changes: StatChanges) -> FsResult<()> {
        Err(FsError::not_implemented("streamfs does not support wstat"))
    }

    async fn statfs(&self, _path: &str) -> FsResult<FsStats> {
        Ok(FsStats {
            total_bytes: u64::MAX,
            free_bytes: u64::MAX,
            total_inodes: u64::MAX,
            free_inodes: u64::MAX,
            block_size: 4096,
            max_name_len: 255,
        })
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        let path = Self::normalize_path(path);

        if path == "/README" {
            let handle_id = self.next_handle_id.fetch_add(1, Ordering::SeqCst);
            let handle = StreamHandle {
                id: handle_id,
                path: path.clone(),
                flags,
                stream: None,
                reader_id: None,
                receiver: None,
                read_buffer: README_CONTENT.as_bytes().to_vec(),
                read_base: 0,
                historical_sent: true,
                historical_index: 0,
            };
            self.handles.write().await.insert(handle_id, handle);
            return Ok(Handle::new(handle_id));
        }

        let stream = {
            let mut streams = self.streams.write().unwrap();
            if let Some(s) = streams.get(&path) {
                s.clone()
            } else {
                if !flags.create && !flags.write {
                    return Err(FsError::not_found(&path));
                }
                let s = Arc::new(StreamFile::new(
                    path.clone(),
                    self.ring_size,
                    self.channel_size,
                ));
                streams.insert(path.clone(), s.clone());
                s
            }
        };

        let handle_id = self.next_handle_id.fetch_add(1, Ordering::SeqCst);

        let (reader_id, receiver) = if flags.read {
            let (id, rx) = stream.register_reader();
            (Some(id), Some(rx))
        } else {
            (None, None)
        };

        let total_chunks = stream.total_chunks.load(Ordering::SeqCst);
        let oldest = total_chunks.saturating_sub(stream.ring_size as u64);

        let handle = StreamHandle {
            id: handle_id,
            path: path.clone(),
            flags,
            stream: Some(stream),
            reader_id,
            receiver,
            read_buffer: Vec::new(),
            read_base: 0,
            historical_sent: false,
            historical_index: oldest,
        };

        self.handles.write().await.insert(handle_id, handle);

        Ok(Handle::new(handle_id))
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        let mut handles = self.handles.write().await;
        let h = handles
            .get_mut(&handle.id())
            .ok_or_else(|| FsError::invalid_argument("invalid handle"))?;

        if h.path == "/README" {
            let start = offset as usize;
            if start >= h.read_buffer.len() {
                return Ok(Bytes::new());
            }
            let end = (start + size).min(h.read_buffer.len());
            return Ok(Bytes::copy_from_slice(&h.read_buffer[start..end]));
        }

        let stream = h
            .stream
            .as_ref()
            .ok_or_else(|| FsError::internal("no stream"))?;

        if !h.historical_sent {
            let historical = stream.get_historical_chunks(h.historical_index);
            for chunk in historical {
                h.read_buffer.extend_from_slice(&chunk);
            }
            h.historical_sent = true;
        }

        if let Some(ref mut receiver) = h.receiver {
            loop {
                match receiver.try_recv() {
                    Ok(chunk) => {
                        h.read_buffer.extend_from_slice(&chunk);
                    }
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!("Reader lagged by {} chunks", n);
                        break;
                    }
                    Err(broadcast::error::TryRecvError::Closed) => break,
                }
            }
        }

        let rel_offset = offset.saturating_sub(h.read_base) as usize;
        if rel_offset < h.read_buffer.len() {
            let end = (rel_offset + size).min(h.read_buffer.len());
            let data = Bytes::copy_from_slice(&h.read_buffer[rel_offset..end]);

            if h.read_buffer.len() > 1024 * 1024 && rel_offset > 64 * 1024 {
                let trim = rel_offset - 64 * 1024;
                h.read_buffer.drain(..trim);
                h.read_base += trim as u64;
            }

            return Ok(data);
        }

        if stream.is_closed() {
            return Ok(Bytes::new());
        }

        if let Some(ref mut receiver) = h.receiver {
            let timeout_result = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                receiver.recv(),
            )
            .await;

            match timeout_result {
                Ok(Ok(chunk)) => {
                    h.read_buffer.extend_from_slice(&chunk);
                    let rel_offset = offset.saturating_sub(h.read_base) as usize;
                    let end = (rel_offset + size).min(h.read_buffer.len());
                    if rel_offset < h.read_buffer.len() {
                        return Ok(Bytes::copy_from_slice(&h.read_buffer[rel_offset..end]));
                    }
                }
                Ok(Err(_)) => {
                    return Ok(Bytes::new());
                }
                Err(_) => {
                    return Ok(Bytes::new());
                }
            }
        }

        Ok(Bytes::new())
    }

    async fn write(&self, handle: &Handle, _offset: u64, data: Bytes) -> FsResult<usize> {
        let handles = self.handles.read().await;
        let h = handles
            .get(&handle.id())
            .ok_or_else(|| FsError::invalid_argument("invalid handle"))?;

        if h.path == "/README" {
            return Err(FsError::permission_denied("README is read-only"));
        }

        let stream = h
            .stream
            .as_ref()
            .ok_or_else(|| FsError::internal("no stream"))?;

        stream.write(data)
    }

    async fn close(&self, handle: Handle, _sync: bool) -> FsResult<()> {
        let mut handles = self.handles.write().await;

        if let Some(h) = handles.remove(&handle.id()) {
            if let (Some(reader_id), Some(stream)) = (h.reader_id, h.stream.as_ref()) {
                stream.unregister_reader(reader_id);
            }
        }

        Ok(())
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = Self::normalize_path(path);

        if path != "/" {
            return Err(FsError::not_found(&path));
        }

        let mut entries = vec![Self::readme_file_info()];

        let streams = self.streams.read().unwrap();
        for stream in streams.values() {
            entries.push(stream.get_info());
        }

        Ok(entries)
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        let path = Self::normalize_path(path);

        if path == "/" || path == "/README" {
            return Err(FsError::permission_denied("cannot remove"));
        }

        let mut streams = self.streams.write().unwrap();
        if let Some(stream) = streams.remove(&path) {
            stream.close();
            Ok(())
        } else {
            Err(FsError::not_found(&path))
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ | Capabilities::WRITE | Capabilities::CREATE | Capabilities::DELETE | Capabilities::DIRECTORY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_stream() {
        let fs = StreamFS::new(10, 10);
        
        let handle = fs.open("/test", OpenFlags {
            read: false,
            write: true,
            create: true,
            ..Default::default()
        }).await.unwrap();
        
        let written = fs.write(&handle, 0, Bytes::from("hello")).await.unwrap();
        assert_eq!(written, 5);
        
        fs.close(handle, false).await.unwrap();
    }

    #[tokio::test]
    async fn test_read_stream() {
        let fs = StreamFS::new(10, 10);
        
        let wh = fs.open("/test", OpenFlags {
            read: false,
            write: true,
            create: true,
            ..Default::default()
        }).await.unwrap();
        
        fs.write(&wh, 0, Bytes::from("hello")).await.unwrap();
        fs.write(&wh, 0, Bytes::from("world")).await.unwrap();
        
        let rh = fs.open("/test", OpenFlags {
            read: true,
            write: false,
            ..Default::default()
        }).await.unwrap();
        
        let data = fs.read(&rh, 0, 1024).await.unwrap();
        assert_eq!(&data[..], b"helloworld");
        
        fs.close(wh, false).await.unwrap();
        fs.close(rh, false).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_streams() {
        let fs = StreamFS::new(10, 10);
        
        let h1 = fs.open("/stream1", OpenFlags {
            write: true,
            create: true,
            ..Default::default()
        }).await.unwrap();
        
        let h2 = fs.open("/stream2", OpenFlags {
            write: true,
            create: true,
            ..Default::default()
        }).await.unwrap();
        
        let entries = fs.readdir("/").await.unwrap();
        assert_eq!(entries.len(), 3);
        
        fs.close(h1, false).await.unwrap();
        fs.close(h2, false).await.unwrap();
    }

    #[tokio::test]
    async fn test_remove_stream() {
        let fs = StreamFS::new(10, 10);
        
        let h = fs.open("/test", OpenFlags {
            write: true,
            create: true,
            ..Default::default()
        }).await.unwrap();
        fs.close(h, false).await.unwrap();
        
        fs.remove("/test").await.unwrap();
        
        let result = fs.stat("/test").await;
        assert!(result.is_err());
    }
}
