#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use bytes::Bytes;
use fs9_sdk::{Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FS9_ERR_INVALID_HANDLE, FS9_ERR_IS_DIRECTORY,
    FS9_ERR_NOT_FOUND, FS9_OK, FS9_SDK_VERSION,
};
use libc::{c_char, c_void, size_t};
use serde::Deserialize;
use tokio::sync::broadcast;

const DEFAULT_RING_SIZE: usize = 100;
const DEFAULT_CHANNEL_SIZE: usize = 100;

const README_CONTENT: &str = r#"StreamFS - Streaming File System Plugin

This plugin provides streaming files that support multiple concurrent readers
and writers with real-time data fanout and ring buffer for late joiners.

FEATURES:
  - Multiple writers can append data to a stream concurrently
  - Multiple readers can consume from the stream independently
  - Ring buffer stores recent data for late-joining readers
  - Memory-based storage with configurable buffer sizes

USAGE:
  Write:  echo "data" > /streamfs/mystream
  Read:   cat /streamfs/mystream

NOTES:
  - Streams are append-only (offset is ignored on write)
  - Data is in-memory only (not persistent across restarts)
"#;

#[derive(Debug, Clone, Deserialize)]
struct StreamFsConfig {
    #[serde(default = "default_ring_size")]
    ring_size: usize,
    #[serde(default = "default_channel_size")]
    channel_size: usize,
}

fn default_ring_size() -> usize {
    DEFAULT_RING_SIZE
}

fn default_channel_size() -> usize {
    DEFAULT_CHANNEL_SIZE
}

impl Default for StreamFsConfig {
    fn default() -> Self {
        Self {
            ring_size: DEFAULT_RING_SIZE,
            channel_size: DEFAULT_CHANNEL_SIZE,
        }
    }
}

struct ReaderState {
    #[allow(dead_code)]
    id: u64,
    #[allow(dead_code)]
    registered_at: SystemTime,
}

struct StreamFile {
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
    fn new(name: String, ring_size: usize, channel_size: usize) -> Self {
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

    fn get_info(&self) -> FileInfo {
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

    fn is_closed(&self) -> bool {
        *self.closed.read().unwrap()
    }

    fn write(&self, data: Bytes) -> FsResult<usize> {
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

    fn register_reader(&self) -> (u64, broadcast::Receiver<Bytes>) {
        let id = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        let receiver = self.sender.subscribe();

        let state = Arc::new(ReaderState {
            id,
            registered_at: SystemTime::now(),
        });

        self.readers.write().unwrap().insert(id, state);
        (id, receiver)
    }

    fn unregister_reader(&self, reader_id: u64) {
        self.readers.write().unwrap().remove(&reader_id);
    }

    fn get_historical_chunks(&self, from_index: u64) -> Vec<Bytes> {
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

    fn close(&self) {
        *self.closed.write().unwrap() = true;
    }
}

struct StreamHandle {
    #[allow(dead_code)]
    id: u64,
    path: String,
    #[allow(dead_code)]
    flags: OpenFlags,
    stream: Option<Arc<StreamFile>>,
    reader_id: Option<u64>,
    receiver: Option<broadcast::Receiver<Bytes>>,
    read_buffer: Vec<u8>,
    read_base: u64,
    historical_sent: bool,
    historical_index: u64,
}

struct StreamFsProvider {
    streams: RwLock<HashMap<String, Arc<StreamFile>>>,
    ring_size: usize,
    channel_size: usize,
    handles: Mutex<HashMap<u64, StreamHandle>>,
    next_handle_id: AtomicU64,
}

impl StreamFsProvider {
    fn new(config: StreamFsConfig) -> Self {
        Self {
            streams: RwLock::new(HashMap::new()),
            ring_size: config.ring_size,
            channel_size: config.channel_size,
            handles: Mutex::new(HashMap::new()),
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

    fn stat(&self, path: &str) -> FsResult<FileInfo> {
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

    fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let path = Self::normalize_path(path);

        if path == "/README" {
            let info = self.stat(&path)?;
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
            self.handles.lock().unwrap().insert(handle_id, handle);
            return Ok((Handle::new(handle_id), info));
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

        self.handles.lock().unwrap().insert(handle_id, handle);

        let info = self.stat(&path)?;
        Ok((Handle::new(handle_id), info))
    }

    fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let mut handles = self.handles.lock().unwrap();
        let h = handles
            .get_mut(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

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
                    Err(broadcast::error::TryRecvError::Lagged(_)) => break,
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

        Ok(Bytes::new())
    }

    fn write(&self, handle: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let h = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

        if h.path == "/README" {
            return Err(FsError::permission_denied("README is read-only"));
        }

        let stream = h
            .stream
            .as_ref()
            .ok_or_else(|| FsError::internal("no stream"))?;

        stream.write(Bytes::copy_from_slice(data))
    }

    fn close(&self, handle: u64) -> FsResult<()> {
        let mut handles = self.handles.lock().unwrap();

        if let Some(h) = handles.remove(&handle) {
            if let (Some(reader_id), Some(stream)) = (h.reader_id, h.stream.as_ref()) {
                stream.unregister_reader(reader_id);
            }
        }

        Ok(())
    }

    fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
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

    fn remove(&self, path: &str) -> FsResult<()> {
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
}

fn systemtime_to_timestamp(time: SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn make_cresult_err(code: i32) -> CResult {
    CResult {
        code,
        error_msg: ptr::null(),
        error_msg_len: 0,
    }
}

fn fserror_to_code(err: &FsError) -> i32 {
    match err {
        FsError::NotFound(_) => FS9_ERR_NOT_FOUND,
        FsError::IsDirectory(_) => FS9_ERR_IS_DIRECTORY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        FsError::PermissionDenied(_) => fs9_sdk_ffi::FS9_ERR_PERMISSION_DENIED,
        _ => fs9_sdk_ffi::FS9_ERR_INTERNAL,
    }
}

unsafe extern "C" fn create_provider(config: *const c_char, config_len: size_t) -> *mut c_void {
    let config: StreamFsConfig = if config.is_null() || config_len == 0 {
        StreamFsConfig::default()
    } else {
        let slice = std::slice::from_raw_parts(config as *const u8, config_len);
        serde_json::from_slice(slice).unwrap_or_default()
    };

    let provider = Box::new(StreamFsProvider::new(config));
    Box::into_raw(provider) as *mut c_void
}

unsafe extern "C" fn destroy_provider(provider: *mut c_void) {
    if !provider.is_null() {
        drop(Box::from_raw(provider as *mut StreamFsProvider));
    }
}

unsafe extern "C" fn get_capabilities(_provider: *mut c_void) -> u64 {
    (Capabilities::READ
        | Capabilities::WRITE
        | Capabilities::CREATE
        | Capabilities::DELETE
        | Capabilities::DIRECTORY)
        .bits()
}

unsafe extern "C" fn stat_fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    out_info: *mut CFileInfo,
) -> CResult {
    if provider.is_null() || out_info.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);
    let path =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(path as *const u8, path_len));

    match provider.stat(path) {
        Ok(info) => {
            (*out_info).size = info.size;
            (*out_info).file_type = if info.file_type == FileType::Directory {
                FILE_TYPE_DIRECTORY
            } else {
                FILE_TYPE_REGULAR
            };
            (*out_info).mode = info.mode;
            (*out_info).mtime = systemtime_to_timestamp(info.mtime);
            (*out_info).atime = systemtime_to_timestamp(info.atime);
            (*out_info).ctime = systemtime_to_timestamp(info.ctime);
            CResult {
                code: FS9_OK,
                error_msg: ptr::null(),
                error_msg_len: 0,
            }
        }
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn wstat_fn(
    _provider: *mut c_void,
    _path: *const c_char,
    _path_len: size_t,
    _changes: *const CStatChanges,
) -> CResult {
    make_cresult_err(fs9_sdk_ffi::FS9_ERR_NOT_IMPLEMENTED)
}

unsafe extern "C" fn statfs_fn(
    _provider: *mut c_void,
    _path: *const c_char,
    _path_len: size_t,
    out_stats: *mut CFsStats,
) -> CResult {
    if out_stats.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    (*out_stats).total_bytes = u64::MAX;
    (*out_stats).free_bytes = u64::MAX;
    (*out_stats).total_inodes = u64::MAX;
    (*out_stats).free_inodes = u64::MAX;
    (*out_stats).block_size = 4096;
    (*out_stats).max_name_len = 255;

    CResult {
        code: FS9_OK,
        error_msg: ptr::null(),
        error_msg_len: 0,
    }
}

unsafe extern "C" fn open_fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    flags: *const COpenFlags,
    out_handle: *mut u64,
    out_info: *mut CFileInfo,
) -> CResult {
    if provider.is_null() || flags.is_null() || out_handle.is_null() || out_info.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);
    let path =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(path as *const u8, path_len));
    let flags = &*flags;

    let open_flags = OpenFlags {
        read: flags.read != 0,
        write: flags.write != 0,
        create: flags.create != 0,
        truncate: flags.truncate != 0,
        append: flags.append != 0,
        directory: flags.directory != 0,
    };

    match provider.open(path, open_flags) {
        Ok((handle, info)) => {
            *out_handle = handle.id();
            (*out_info).size = info.size;
            (*out_info).file_type = if info.file_type == FileType::Directory {
                FILE_TYPE_DIRECTORY
            } else {
                FILE_TYPE_REGULAR
            };
            (*out_info).mode = info.mode;
            (*out_info).uid = info.uid;
            (*out_info).gid = info.gid;
            (*out_info).mtime = systemtime_to_timestamp(info.mtime);
            (*out_info).atime = systemtime_to_timestamp(info.atime);
            (*out_info).ctime = systemtime_to_timestamp(info.ctime);
            CResult {
                code: FS9_OK,
                error_msg: ptr::null(),
                error_msg_len: 0,
            }
        }
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn read_fn(
    provider: *mut c_void,
    handle: u64,
    offset: u64,
    size: size_t,
    out_data: *mut CBytes,
) -> CResult {
    if provider.is_null() || out_data.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);

    match provider.read(handle, offset, size) {
        Ok(data) => {
            *out_data = fs9_sdk_ffi::vec_to_cbytes(data.to_vec());
            CResult {
                code: FS9_OK,
                error_msg: ptr::null(),
                error_msg_len: 0,
            }
        }
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn write_fn(
    provider: *mut c_void,
    handle: u64,
    _offset: u64,
    data: *const u8,
    data_len: size_t,
    out_written: *mut size_t,
) -> CResult {
    if provider.is_null() || out_written.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);
    let data = if data.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(data, data_len)
    };

    match provider.write(handle, data) {
        Ok(written) => {
            *out_written = written;
            CResult {
                code: FS9_OK,
                error_msg: ptr::null(),
                error_msg_len: 0,
            }
        }
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn close_fn(provider: *mut c_void, handle: u64, _sync: u8) -> CResult {
    if provider.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);

    match provider.close(handle) {
        Ok(()) => CResult {
            code: FS9_OK,
            error_msg: ptr::null(),
            error_msg_len: 0,
        },
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn readdir_fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    callback: fs9_sdk_ffi::ReaddirCallback,
    user_data: *mut c_void,
) -> CResult {
    if provider.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);
    let path =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(path as *const u8, path_len));

    match provider.readdir(path) {
        Ok(entries) => {
            for entry in entries {
                let path_bytes = entry.path.as_bytes();
                let info = CFileInfo {
                    path: path_bytes.as_ptr() as *const c_char,
                    path_len: path_bytes.len(),
                    size: entry.size,
                    file_type: if entry.file_type == FileType::Directory {
                        FILE_TYPE_DIRECTORY
                    } else {
                        FILE_TYPE_REGULAR
                    },
                    mode: entry.mode,
                    uid: 0,
                    gid: 0,
                    atime: systemtime_to_timestamp(entry.atime),
                    mtime: systemtime_to_timestamp(entry.mtime),
                    ctime: systemtime_to_timestamp(entry.ctime),
                };
                if callback(&info, user_data) != 0 {
                    break;
                }
            }
            CResult {
                code: FS9_OK,
                error_msg: ptr::null(),
                error_msg_len: 0,
            }
        }
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn remove_fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
) -> CResult {
    if provider.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const StreamFsProvider);
    let path =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(path as *const u8, path_len));

    match provider.remove(path) {
        Ok(()) => CResult {
            code: FS9_OK,
            error_msg: ptr::null(),
            error_msg_len: 0,
        },
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

static PLUGIN_NAME: &[u8] = b"streamfs";
static PLUGIN_VERSION: &[u8] = b"0.1.0";

static VTABLE: PluginVTable = PluginVTable {
    sdk_version: FS9_SDK_VERSION,
    name: PLUGIN_NAME.as_ptr() as *const libc::c_char,
    name_len: PLUGIN_NAME.len(),
    version: PLUGIN_VERSION.as_ptr() as *const libc::c_char,
    version_len: PLUGIN_VERSION.len(),
    create: create_provider,
    destroy: destroy_provider,
    get_capabilities: get_capabilities,
    stat: stat_fn,
    wstat: wstat_fn,
    statfs: statfs_fn,
    open: open_fn,
    read: read_fn,
    write: write_fn,
    close: close_fn,
    readdir: readdir_fn,
    remove: remove_fn,
};

#[no_mangle]
pub extern "C" fn fs9_plugin_version() -> u32 {
    FS9_SDK_VERSION
}

#[no_mangle]
pub extern "C" fn fs9_plugin_vtable() -> *const PluginVTable {
    &VTABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_sdk() {
        assert_eq!(fs9_plugin_version(), FS9_SDK_VERSION);
    }

    #[test]
    fn vtable_not_null() {
        let vtable = fs9_plugin_vtable();
        assert!(!vtable.is_null());
        unsafe {
            assert_eq!((*vtable).sdk_version, FS9_SDK_VERSION);
        }
    }

    #[test]
    fn provider_lifecycle() {
        unsafe {
            let provider = create_provider(ptr::null(), 0);
            assert!(!provider.is_null());
            destroy_provider(provider);
        }
    }

    #[test]
    fn create_and_write_stream() {
        let provider = StreamFsProvider::new(StreamFsConfig::default());

        let handle = provider
            .open(
                "/test",
                OpenFlags {
                    read: false,
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let written = provider.write(handle.id(), b"hello").unwrap();
        assert_eq!(written, 5);

        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn read_stream() {
        let provider = StreamFsProvider::new(StreamFsConfig::default());

        let wh = provider
            .open(
                "/test",
                OpenFlags {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();

        provider.write(wh.id(), b"hello").unwrap();
        provider.write(wh.id(), b"world").unwrap();

        let rh = provider
            .open(
                "/test",
                OpenFlags {
                    read: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let data = provider.read(rh.id(), 0, 1024).unwrap();
        assert_eq!(&data[..], b"helloworld");

        provider.close(wh.id()).unwrap();
        provider.close(rh.id()).unwrap();
    }

    #[test]
    fn list_streams() {
        let provider = StreamFsProvider::new(StreamFsConfig::default());

        let h1 = provider
            .open(
                "/stream1",
                OpenFlags {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let h2 = provider
            .open(
                "/stream2",
                OpenFlags {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let entries = provider.readdir("/").unwrap();
        assert_eq!(entries.len(), 3);

        provider.close(h1.id()).unwrap();
        provider.close(h2.id()).unwrap();
    }

    #[test]
    fn remove_stream() {
        let provider = StreamFsProvider::new(StreamFsConfig::default());

        let h = provider
            .open(
                "/test",
                OpenFlags {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        provider.close(h.id()).unwrap();

        provider.remove("/test").unwrap();

        let result = provider.stat("/test");
        assert!(result.is_err());
    }
}
