#![allow(clippy::missing_safety_doc)]

use std::collections::{HashMap, VecDeque};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use bytes::Bytes;
use fs9_sdk::{Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FS9_ERR_INVALID_ARGUMENT, FS9_ERR_INVALID_HANDLE,
    FS9_ERR_IS_DIRECTORY, FS9_ERR_NOT_FOUND, FS9_OK, FS9_SDK_VERSION,
};
use libc::{c_char, c_void, size_t};
use serde::Deserialize;
use tokio::sync::broadcast;

const DEFAULT_RING_SIZE: usize = 100;
const DEFAULT_CHANNEL_SIZE: usize = 100;
const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB

const README_CONTENT: &str = r#"PubSubFS - Publish/Subscribe File System Plugin

A topic-based pub/sub system inspired by Unix pipes: everything is a file.

FILE STRUCTURE:
  /pubsub/
    README            # This documentation
    chat              # Topic file: read=subscribe, write=publish
    chat.info         # Topic metadata (subscribers, messages, etc)
    logs              # Another topic
    logs.info         # Its metadata
    events            # Yet another topic
    events.info       # Its metadata

USAGE:
  # Create a topic (auto-created on first write)
  echo "hello" > /pubsub/chat

  # List all topics
  ls /pubsub

  # Publish messages (write mode)
  echo "hello world" > /pubsub/chat
  echo '{"level":"info"}' > /pubsub/logs

  # Subscribe to messages (read mode - streaming)
  cat /pubsub/chat
  tail -f /pubsub/chat           # Recommended: last N + follow

  # View topic info
  cat /pubsub/chat.info

  # Delete a topic
  rm /pubsub/chat

  # Subscribe with processing
  tail -f /pubsub/logs | grep ERROR > /errors.log &

FEATURES:
  - Simple pipe-like interface: read=subscribe, write=publish
  - Multiple publishers and subscribers per topic
  - Ring buffer for late joiners (configurable size)
  - Real-time message broadcast
  - Topic statistics via .info files
  - Auto-create topics on first write

NOTES:
  - Messages are in-memory only (not persistent)
  - Each message is broadcast to all active subscribers
  - Ring buffer stores recent messages for new subscribers
  - Path is short and intuitive: /pubsub/chat vs /pubsub/topics/chat/pub
"#;

#[derive(Debug, Clone, Deserialize)]
struct PubSubFsConfig {
    #[serde(default = "default_ring_size")]
    default_ring_size: usize,
    #[serde(default = "default_channel_size")]
    default_channel_size: usize,
}

fn default_ring_size() -> usize {
    DEFAULT_RING_SIZE
}

fn default_channel_size() -> usize {
    DEFAULT_CHANNEL_SIZE
}

impl Default for PubSubFsConfig {
    fn default() -> Self {
        Self {
            default_ring_size: DEFAULT_RING_SIZE,
            default_channel_size: DEFAULT_CHANNEL_SIZE,
        }
    }
}

#[derive(Debug, Clone)]
struct Message {
    timestamp: SystemTime,
    data: Bytes,
}

impl Message {
    fn new(mut data: Bytes) -> Self {
        // Remove trailing newline to avoid double newlines when formatting
        if data.ends_with(b"\n") {
            data = data.slice(..data.len() - 1);
        }

        Self {
            timestamp: SystemTime::now(),
            data,
        }
    }

    fn format(&self) -> Bytes {
        // Return the original data with a newline
        // No timestamp prefix - faithful to input
        let mut result = Vec::with_capacity(self.data.len() + 1);
        result.extend_from_slice(&self.data);
        result.push(b'\n');
        Bytes::from(result)
    }
}

struct Topic {
    name: String,
    created_at: SystemTime,
    mtime: RwLock<SystemTime>,
    ring_buffer: RwLock<VecDeque<Message>>,
    ring_size: usize,
    total_messages: AtomicU64,
    sender: broadcast::Sender<Message>,
    subscribers: RwLock<HashMap<u64, SubscriberInfo>>,
    next_subscriber_id: AtomicU64,
}

struct SubscriberInfo {
    id: u64,
    subscribed_at: SystemTime,
}

impl Topic {
    fn new(name: String, ring_size: usize, channel_size: usize) -> Self {
        let (sender, _) = broadcast::channel(channel_size);
        Self {
            name,
            created_at: SystemTime::now(),
            mtime: RwLock::new(SystemTime::now()),
            ring_buffer: RwLock::new(VecDeque::with_capacity(ring_size)),
            ring_size,
            total_messages: AtomicU64::new(0),
            sender,
            subscribers: RwLock::new(HashMap::new()),
            next_subscriber_id: AtomicU64::new(1),
        }
    }

    fn publish(&self, data: Bytes) -> FsResult<usize> {
        if data.len() > MAX_MESSAGE_SIZE {
            return Err(FsError::invalid_argument(
                format!("message too large: {} > {}", data.len(), MAX_MESSAGE_SIZE)
            ));
        }

        let msg = Message::new(data);
        let len = msg.data.len();

        // Add to ring buffer
        {
            let mut ring = self.ring_buffer.write().unwrap();
            if ring.len() >= self.ring_size {
                ring.pop_front();
            }
            ring.push_back(msg.clone());
        }

        self.total_messages.fetch_add(1, Ordering::SeqCst);
        *self.mtime.write().unwrap() = SystemTime::now();

        // Broadcast to all subscribers
        let _ = self.sender.send(msg);

        Ok(len)
    }

    fn subscribe(&self) -> (u64, broadcast::Receiver<Message>, Vec<Message>) {
        let id = self.next_subscriber_id.fetch_add(1, Ordering::SeqCst);
        let receiver = self.sender.subscribe();

        let info = SubscriberInfo {
            id,
            subscribed_at: SystemTime::now(),
        };

        self.subscribers.write().unwrap().insert(id, info);

        // Get historical messages from ring buffer
        let historical = self.ring_buffer.read().unwrap()
            .iter()
            .cloned()
            .collect();

        (id, receiver, historical)
    }

    fn unsubscribe(&self, subscriber_id: u64) {
        self.subscribers.write().unwrap().remove(&subscriber_id);
    }

    fn get_info(&self) -> String {
        let subscriber_count = self.subscribers.read().unwrap().len();
        let message_count = self.total_messages.load(Ordering::SeqCst);
        let created = self.created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let modified = self.mtime.read().unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        format!(
            "name: {}\nsubscribers: {}\nmessages: {}\nring_size: {}\ncreated: {}\nmodified: {}\n",
            self.name,
            subscriber_count,
            message_count,
            self.ring_size,
            chrono::DateTime::<chrono::Utc>::from(self.created_at).format("%Y-%m-%d %H:%M:%S"),
            chrono::DateTime::<chrono::Utc>::from(*self.mtime.read().unwrap()).format("%Y-%m-%d %H:%M:%S"),
        )
    }
}

enum HandleType {
    ReadmeFile(Vec<u8>),
    TopicInfo(Arc<Topic>),
    TopicPublish(Arc<Topic>),
    TopicSubscribe {
        topic: Arc<Topic>,
        subscriber_id: u64,
        receiver: broadcast::Receiver<Message>,
        buffer: Vec<u8>,
        buffer_offset: u64,
        historical_sent: bool,
        historical: Vec<Message>,
        historical_index: usize,
    },
}

struct PubSubHandle {
    id: u64,
    path: String,
    handle_type: HandleType,
}

struct PubSubFsProvider {
    topics: RwLock<HashMap<String, Arc<Topic>>>,
    default_ring_size: usize,
    default_channel_size: usize,
    handles: Mutex<HashMap<u64, PubSubHandle>>,
    next_handle_id: AtomicU64,
}

impl PubSubFsProvider {
    fn new(config: PubSubFsConfig) -> Self {
        Self {
            topics: RwLock::new(HashMap::new()),
            default_ring_size: config.default_ring_size,
            default_channel_size: config.default_channel_size,
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

    fn create_topic_if_needed(&self, name: &str) -> Arc<Topic> {
        let mut topics = self.topics.write().unwrap();
        if let Some(topic) = topics.get(name) {
            topic.clone()
        } else {
            let topic = Arc::new(Topic::new(
                name.to_string(),
                self.default_ring_size,
                self.default_channel_size,
            ));
            topics.insert(name.to_string(), topic.clone());
            topic
        }
    }

    fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path = Self::normalize_path(path);

        // Root directory
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

        // README file
        if path == "/README" {
            return Ok(FileInfo {
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
            });
        }

        // Check if it's a .info file
        if let Some(topic_name) = path.strip_prefix('/').and_then(|p| p.strip_suffix(".info")) {
            let topics = self.topics.read().unwrap();
            if let Some(topic) = topics.get(topic_name) {
                let mtime = *topic.mtime.read().unwrap();
                return Ok(FileInfo {
                    path: path.clone(),
                    size: topic.get_info().len() as u64,
                    file_type: FileType::Regular,
                    mode: 0o444, // read-only
                    uid: 0,
                    gid: 0,
                    atime: mtime,
                    mtime,
                    ctime: topic.created_at,
                    etag: format!("info-{}", topic_name),
                    symlink_target: None,
                });
            } else {
                return Err(FsError::not_found(&path));
            }
        }

        // Check if it's a topic file
        if let Some(topic_name) = path.strip_prefix('/') {
            if !topic_name.is_empty() && !topic_name.contains('/') {
                let topics = self.topics.read().unwrap();
                if let Some(topic) = topics.get(topic_name) {
                    let mtime = *topic.mtime.read().unwrap();
                    return Ok(FileInfo {
                        path: path.clone(),
                        size: 0,
                        file_type: FileType::Regular,
                        mode: 0o600, // read-write
                        uid: 0,
                        gid: 0,
                        atime: mtime,
                        mtime,
                        ctime: topic.created_at,
                        etag: format!("topic-{}", topic_name),
                        symlink_target: None,
                    });
                } else {
                    // Topic doesn't exist yet, but could be created
                    return Err(FsError::not_found(&path));
                }
            }
        }

        Err(FsError::not_found(&path))
    }

    fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        let path = Self::normalize_path(path);
        let handle_id = self.next_handle_id.fetch_add(1, Ordering::SeqCst);

        let handle_type = if path == "/README" {
            if flags.write {
                return Err(FsError::permission_denied("README is read-only"));
            }
            HandleType::ReadmeFile(README_CONTENT.as_bytes().to_vec())
        } else if let Some(topic_name) = path.strip_prefix('/').and_then(|p| p.strip_suffix(".info")) {
            // Opening .info file
            if flags.write {
                return Err(FsError::permission_denied(".info files are read-only"));
            }
            let topics = self.topics.read().unwrap();
            let topic = topics.get(topic_name)
                .ok_or_else(|| FsError::not_found(topic_name))?
                .clone();
            HandleType::TopicInfo(topic)
        } else if let Some(topic_name) = path.strip_prefix('/') {
            // Opening topic file (e.g., /pubsub/chat)
            if topic_name.is_empty() || topic_name.contains('/') || topic_name == "README" {
                return Err(FsError::not_found(&path));
            }

            // Determine mode based on flags
            // For PubSubFS: write = publish, read = subscribe
            // If both read and write are set (e.g., from create_truncate),
            // prioritize write (publish) mode
            if flags.write {
                // Write mode = Publish
                let topic = self.create_topic_if_needed(topic_name);
                HandleType::TopicPublish(topic)
            } else if flags.read {
                // Read mode = Subscribe
                let topics = self.topics.read().unwrap();
                let topic = topics.get(topic_name)
                    .ok_or_else(|| FsError::not_found(topic_name))?
                    .clone();

                let (subscriber_id, receiver, historical) = topic.subscribe();
                HandleType::TopicSubscribe {
                    topic,
                    subscriber_id,
                    receiver,
                    buffer: Vec::new(),
                    buffer_offset: 0,
                    historical_sent: false,
                    historical,
                    historical_index: 0,
                }
            } else {
                return Err(FsError::invalid_argument("must specify read or write mode"));
            }
        } else {
            return Err(FsError::not_found(&path));
        };

        let handle = PubSubHandle {
            id: handle_id,
            path: path.clone(),
            handle_type,
        };

        self.handles.lock().unwrap().insert(handle_id, handle);
        Ok(Handle::new(handle_id))
    }

    fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let mut handles = self.handles.lock().unwrap();
        let h = handles.get_mut(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

        match &mut h.handle_type {
            HandleType::ReadmeFile(data) => {
                let start = offset as usize;
                if start >= data.len() {
                    return Ok(Bytes::new());
                }
                let end = (start + size).min(data.len());
                Ok(Bytes::copy_from_slice(&data[start..end]))
            }
            HandleType::TopicInfo(topic) => {
                let info = topic.get_info();
                let start = offset as usize;
                if start >= info.len() {
                    return Ok(Bytes::new());
                }
                let end = (start + size).min(info.len());
                Ok(Bytes::copy_from_slice(&info.as_bytes()[start..end]))
            }
            HandleType::TopicSubscribe {
                receiver,
                buffer,
                buffer_offset,
                historical_sent,
                historical,
                historical_index,
                ..
            } => {
                // First, send historical messages
                if !*historical_sent {
                    for msg in &historical[*historical_index..] {
                        let formatted = msg.format();
                        buffer.extend_from_slice(&formatted);
                    }
                    *historical_sent = true;
                }

                // Then try to receive new messages (non-blocking)
                loop {
                    match receiver.try_recv() {
                        Ok(msg) => {
                            let formatted = msg.format();
                            buffer.extend_from_slice(&formatted);
                        }
                        Err(broadcast::error::TryRecvError::Empty) => break,
                        Err(broadcast::error::TryRecvError::Lagged(_)) => {
                            // Subscriber is too slow, some messages were dropped
                            break;
                        }
                        Err(broadcast::error::TryRecvError::Closed) => break,
                    }
                }

                // Return data from buffer
                let rel_offset = offset.saturating_sub(*buffer_offset) as usize;
                if rel_offset < buffer.len() {
                    let end = (rel_offset + size).min(buffer.len());
                    let data = Bytes::copy_from_slice(&buffer[rel_offset..end]);

                    // Trim buffer if it gets too large
                    if buffer.len() > 1024 * 1024 && rel_offset > 64 * 1024 {
                        let trim = rel_offset - 64 * 1024;
                        buffer.drain(..trim);
                        *buffer_offset += trim as u64;
                    }

                    return Ok(data);
                }

                Ok(Bytes::new())
            }
            _ => Err(FsError::permission_denied("cannot read from this handle")),
        }
    }

    fn write(&self, handle: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let h = handles.get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

        match &h.handle_type {
            HandleType::TopicPublish(topic) => {
                topic.publish(Bytes::copy_from_slice(data))
            }
            _ => Err(FsError::permission_denied("cannot write to this handle")),
        }
    }

    fn close(&self, handle: u64) -> FsResult<()> {
        let mut handles = self.handles.lock().unwrap();

        if let Some(h) = handles.remove(&handle) {
            if let HandleType::TopicSubscribe { topic, subscriber_id, .. } = h.handle_type {
                topic.unsubscribe(subscriber_id);
            }
        }

        Ok(())
    }

    fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = Self::normalize_path(path);

        if path == "/" {
            let mut entries = vec![
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
                },
            ];

            // Add all topics and their .info files
            let topics = self.topics.read().unwrap();
            for topic in topics.values() {
                let mtime = *topic.mtime.read().unwrap();

                // Topic file
                entries.push(FileInfo {
                    path: format!("/{}", topic.name),
                    size: 0,
                    file_type: FileType::Regular,
                    mode: 0o600,
                    uid: 0,
                    gid: 0,
                    atime: mtime,
                    mtime,
                    ctime: topic.created_at,
                    etag: format!("topic-{}", topic.name),
                    symlink_target: None,
                });

                // .info file
                entries.push(FileInfo {
                    path: format!("/{}.info", topic.name),
                    size: topic.get_info().len() as u64,
                    file_type: FileType::Regular,
                    mode: 0o444,
                    uid: 0,
                    gid: 0,
                    atime: mtime,
                    mtime,
                    ctime: topic.created_at,
                    etag: format!("info-{}", topic.name),
                    symlink_target: None,
                });
            }

            return Ok(entries);
        }

        Err(FsError::not_directory(&path))
    }

    fn remove(&self, path: &str) -> FsResult<()> {
        let path = Self::normalize_path(path);

        // Cannot delete special files
        if path == "/" || path == "/README" {
            return Err(FsError::permission_denied("cannot remove special files"));
        }

        // Check if it's a .info file
        if path.ends_with(".info") {
            return Err(FsError::permission_denied(".info files cannot be deleted directly; delete the topic instead"));
        }

        // Try to remove topic
        if let Some(topic_name) = path.strip_prefix('/') {
            if !topic_name.is_empty() && !topic_name.contains('/') {
                let mut topics = self.topics.write().unwrap();
                if topics.remove(topic_name).is_some() {
                    return Ok(());
                } else {
                    return Err(FsError::not_found(&path));
                }
            }
        }

        Err(FsError::not_found(&path))
    }
}

// FFI implementation
fn systemtime_to_timestamp(time: SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn make_cresult_ok() -> CResult {
    CResult {
        code: FS9_OK,
        error_msg: ptr::null(),
        error_msg_len: 0,
    }
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
        FsError::AlreadyExists(_) => fs9_sdk_ffi::FS9_ERR_ALREADY_EXISTS,
        FsError::IsDirectory(_) => FS9_ERR_IS_DIRECTORY,
        FsError::NotDirectory(_) => fs9_sdk_ffi::FS9_ERR_NOT_DIRECTORY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        FsError::PermissionDenied(_) => fs9_sdk_ffi::FS9_ERR_PERMISSION_DENIED,
        FsError::InvalidArgument(_) => FS9_ERR_INVALID_ARGUMENT,
        _ => fs9_sdk_ffi::FS9_ERR_INTERNAL,
    }
}

unsafe extern "C" fn create_provider(config: *const c_char, config_len: size_t) -> *mut c_void {
    let config: PubSubFsConfig = if config.is_null() || config_len == 0 {
        PubSubFsConfig::default()
    } else {
        let slice = std::slice::from_raw_parts(config as *const u8, config_len);
        serde_json::from_slice(slice).unwrap_or_default()
    };

    let provider = Box::new(PubSubFsProvider::new(config));
    Box::into_raw(provider) as *mut c_void
}

unsafe extern "C" fn destroy_provider(provider: *mut c_void) {
    if !provider.is_null() {
        drop(Box::from_raw(provider as *mut PubSubFsProvider));
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
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);
    let path = std::str::from_utf8_unchecked(
        std::slice::from_raw_parts(path as *const u8, path_len)
    );

    match provider.stat(path) {
        Ok(info) => {
            (*out_info).size = info.size;
            (*out_info).file_type = if info.file_type == FileType::Directory {
                FILE_TYPE_DIRECTORY
            } else {
                FILE_TYPE_REGULAR
            };
            (*out_info).mode = info.mode;
            (*out_info).uid = info.uid;
            (*out_info).gid = info.gid;
            (*out_info).atime = systemtime_to_timestamp(info.atime);
            (*out_info).mtime = systemtime_to_timestamp(info.mtime);
            (*out_info).ctime = systemtime_to_timestamp(info.ctime);
            make_cresult_ok()
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
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    (*out_stats).total_bytes = u64::MAX;
    (*out_stats).free_bytes = u64::MAX;
    (*out_stats).total_inodes = u64::MAX;
    (*out_stats).free_inodes = u64::MAX;
    (*out_stats).block_size = 4096;
    (*out_stats).max_name_len = 255;

    make_cresult_ok()
}

unsafe extern "C" fn open_fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    flags: *const COpenFlags,
    out_handle: *mut u64,
) -> CResult {
    if provider.is_null() || flags.is_null() || out_handle.is_null() {
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);
    let path = std::str::from_utf8_unchecked(
        std::slice::from_raw_parts(path as *const u8, path_len)
    );
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
        Ok(handle) => {
            *out_handle = handle.id();
            make_cresult_ok()
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
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);

    match provider.read(handle, offset, size) {
        Ok(data) => {
            *out_data = fs9_sdk_ffi::vec_to_cbytes(data.to_vec());
            make_cresult_ok()
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
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);
    let data = if data.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(data, data_len)
    };

    match provider.write(handle, data) {
        Ok(written) => {
            *out_written = written;
            make_cresult_ok()
        }
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn close_fn(provider: *mut c_void, handle: u64, _sync: u8) -> CResult {
    if provider.is_null() {
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);

    match provider.close(handle) {
        Ok(()) => make_cresult_ok(),
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
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);
    let path = std::str::from_utf8_unchecked(
        std::slice::from_raw_parts(path as *const u8, path_len)
    );

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
                    uid: entry.uid,
                    gid: entry.gid,
                    atime: systemtime_to_timestamp(entry.atime),
                    mtime: systemtime_to_timestamp(entry.mtime),
                    ctime: systemtime_to_timestamp(entry.ctime),
                };
                if callback(&info, user_data) != 0 {
                    break;
                }
            }
            make_cresult_ok()
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
        return make_cresult_err(FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PubSubFsProvider);
    let path = std::str::from_utf8_unchecked(
        std::slice::from_raw_parts(path as *const u8, path_len)
    );

    match provider.remove(path) {
        Ok(()) => make_cresult_ok(),
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

static PLUGIN_NAME: &[u8] = b"pubsubfs";
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
    fn create_topic_auto() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Topic created automatically on first write
        let handle = provider.open("/test_topic", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();

        let topics = provider.topics.read().unwrap();
        assert!(topics.contains_key("test_topic"));

        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn delete_topic() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Create topic
        let h = provider.open("/test", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();
        provider.close(h.id()).unwrap();

        // Delete topic
        provider.remove("/test").unwrap();

        let topics = provider.topics.read().unwrap();
        assert!(!topics.contains_key("test"));
    }

    #[test]
    fn publish_and_subscribe() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Open pub handle (auto-creates topic)
        let pub_h = provider.open("/chat", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();

        // Open sub handle
        let sub_h = provider.open("/chat", OpenFlags {
            read: true,
            ..Default::default()
        }).unwrap();

        // Publish a message
        provider.write(pub_h.id(), b"hello world").unwrap();

        // Subscribe should receive it
        let data = provider.read(sub_h.id(), 0, 4096).unwrap();
        assert!(String::from_utf8_lossy(&data).contains("hello world"));

        provider.close(pub_h.id()).unwrap();
        provider.close(sub_h.id()).unwrap();
    }

    #[test]
    fn multiple_subscribers() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        let pub_h = provider.open("/broadcast", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();

        let sub1 = provider.open("/broadcast", OpenFlags {
            read: true,
            ..Default::default()
        }).unwrap();

        let sub2 = provider.open("/broadcast", OpenFlags {
            read: true,
            ..Default::default()
        }).unwrap();

        provider.write(pub_h.id(), b"broadcast message").unwrap();

        let data1 = provider.read(sub1.id(), 0, 4096).unwrap();
        let data2 = provider.read(sub2.id(), 0, 4096).unwrap();

        assert!(String::from_utf8_lossy(&data1).contains("broadcast message"));
        assert!(String::from_utf8_lossy(&data2).contains("broadcast message"));

        provider.close(pub_h.id()).unwrap();
        provider.close(sub1.id()).unwrap();
        provider.close(sub2.id()).unwrap();
    }

    #[test]
    fn topic_info() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Create topic
        let h = provider.open("/test", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();
        provider.close(h.id()).unwrap();

        // Read info
        let info_h = provider.open("/test.info", OpenFlags {
            read: true,
            ..Default::default()
        }).unwrap();

        let data = provider.read(info_h.id(), 0, 4096).unwrap();
        let info = String::from_utf8_lossy(&data);

        assert!(info.contains("name: test"));
        assert!(info.contains("subscribers:"));
        assert!(info.contains("messages:"));

        provider.close(info_h.id()).unwrap();
    }

    #[test]
    fn list_topics() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Create topics
        let h1 = provider.open("/topic1", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();
        let h2 = provider.open("/topic2", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();
        provider.close(h1.id()).unwrap();
        provider.close(h2.id()).unwrap();

        // List via readdir
        let entries = provider.readdir("/").unwrap();

        assert!(entries.iter().any(|e| e.path == "/topic1"));
        assert!(entries.iter().any(|e| e.path == "/topic2"));
        assert!(entries.iter().any(|e| e.path == "/topic1.info"));
        assert!(entries.iter().any(|e| e.path == "/topic2.info"));
    }

    #[test]
    fn readdir_root() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Create some topics
        let h1 = provider.open("/chat", OpenFlags { write: true, ..Default::default() }).unwrap();
        let h2 = provider.open("/logs", OpenFlags { write: true, ..Default::default() }).unwrap();
        provider.close(h1.id()).unwrap();
        provider.close(h2.id()).unwrap();

        let entries = provider.readdir("/").unwrap();

        assert!(entries.iter().any(|e| e.path == "/README"));
        assert!(entries.iter().any(|e| e.path == "/chat"));
        assert!(entries.iter().any(|e| e.path == "/chat.info"));
        assert!(entries.iter().any(|e| e.path == "/logs"));
        assert!(entries.iter().any(|e| e.path == "/logs.info"));
    }

    #[test]
    fn ring_buffer_historical_messages() {
        let config = PubSubFsConfig {
            default_ring_size: 3,
            default_channel_size: 10,
        };
        let provider = PubSubFsProvider::new(config);

        let pub_h = provider.open("/test", OpenFlags {
            write: true,
            ..Default::default()
        }).unwrap();

        // Publish several messages
        provider.write(pub_h.id(), b"msg1").unwrap();
        provider.write(pub_h.id(), b"msg2").unwrap();
        provider.write(pub_h.id(), b"msg3").unwrap();
        provider.write(pub_h.id(), b"msg4").unwrap(); // This will evict msg1

        // Late subscriber should get last 3 messages
        let sub_h = provider.open("/test", OpenFlags {
            read: true,
            ..Default::default()
        }).unwrap();

        let data = provider.read(sub_h.id(), 0, 4096).unwrap();
        let content = String::from_utf8_lossy(&data);

        assert!(!content.contains("msg1")); // Evicted
        assert!(content.contains("msg2"));
        assert!(content.contains("msg3"));
        assert!(content.contains("msg4"));

        provider.close(pub_h.id()).unwrap();
        provider.close(sub_h.id()).unwrap();
    }

    #[test]
    fn cannot_open_read_write_simultaneously() {
        let provider = PubSubFsProvider::new(PubSubFsConfig::default());

        // Try to open for both read and write
        let result = provider.open("/test", OpenFlags {
            read: true,
            write: true,
            ..Default::default()
        });

        assert!(result.is_err());
    }
}
