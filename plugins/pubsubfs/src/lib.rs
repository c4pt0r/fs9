#![allow(clippy::missing_safety_doc)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use bytes::Bytes;
use fs9_sdk::{FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use serde::Deserialize;
use tokio::sync::broadcast;

pub mod ffi;

#[cfg(test)]
mod tests;

const DEFAULT_RING_SIZE: usize = 100;
const DEFAULT_CHANNEL_SIZE: usize = 100;
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

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
pub(crate) struct PubSubFsConfig {
    #[serde(default = "default_ring_size")]
    pub(crate) default_ring_size: usize,
    #[serde(default = "default_channel_size")]
    pub(crate) default_channel_size: usize,
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
        if data.ends_with(b"\n") {
            data = data.slice(..data.len() - 1);
        }

        Self {
            timestamp: SystemTime::now(),
            data,
        }
    }

    fn format(&self) -> Bytes {
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
            return Err(FsError::invalid_argument(format!(
                "message too large: {} > {}",
                data.len(),
                MAX_MESSAGE_SIZE
            )));
        }

        let msg = Message::new(data);
        let len = msg.data.len();

        {
            let mut ring = self.ring_buffer.write().unwrap();
            if ring.len() >= self.ring_size {
                ring.pop_front();
            }
            ring.push_back(msg.clone());
        }

        self.total_messages.fetch_add(1, Ordering::SeqCst);
        *self.mtime.write().unwrap() = SystemTime::now();

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

        let historical = self.ring_buffer.read().unwrap().iter().cloned().collect();

        (id, receiver, historical)
    }

    fn unsubscribe(&self, subscriber_id: u64) {
        self.subscribers.write().unwrap().remove(&subscriber_id);
    }

    fn get_info(&self) -> String {
        let subscriber_count = self.subscribers.read().unwrap().len();
        let message_count = self.total_messages.load(Ordering::SeqCst);
        let created = self
            .created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let modified = self
            .mtime
            .read()
            .unwrap()
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
            chrono::DateTime::<chrono::Utc>::from(*self.mtime.read().unwrap())
                .format("%Y-%m-%d %H:%M:%S"),
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

pub(crate) struct PubSubFsProvider {
    pub(crate) topics: RwLock<HashMap<String, Arc<Topic>>>,
    default_ring_size: usize,
    default_channel_size: usize,
    handles: Mutex<HashMap<u64, PubSubHandle>>,
    next_handle_id: AtomicU64,
}

impl PubSubFsProvider {
    pub(crate) fn new(config: PubSubFsConfig) -> Self {
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

    pub(crate) fn stat(&self, path: &str) -> FsResult<FileInfo> {
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

        if let Some(topic_name) = path.strip_prefix('/').and_then(|p| p.strip_suffix(".info")) {
            let topics = self.topics.read().unwrap();
            if let Some(topic) = topics.get(topic_name) {
                let mtime = *topic.mtime.read().unwrap();
                return Ok(FileInfo {
                    path: path.clone(),
                    size: topic.get_info().len() as u64,
                    file_type: FileType::Regular,
                    mode: 0o444,
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

        if let Some(topic_name) = path.strip_prefix('/') {
            if !topic_name.is_empty() && !topic_name.contains('/') {
                let topics = self.topics.read().unwrap();
                if let Some(topic) = topics.get(topic_name) {
                    let mtime = *topic.mtime.read().unwrap();
                    return Ok(FileInfo {
                        path: path.clone(),
                        size: 0,
                        file_type: FileType::Regular,
                        mode: 0o600,
                        uid: 0,
                        gid: 0,
                        atime: mtime,
                        mtime,
                        ctime: topic.created_at,
                        etag: format!("topic-{}", topic_name),
                        symlink_target: None,
                    });
                } else {
                    return Err(FsError::not_found(&path));
                }
            }
        }

        Err(FsError::not_found(&path))
    }

    pub(crate) fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let path = Self::normalize_path(path);
        let handle_id = self.next_handle_id.fetch_add(1, Ordering::SeqCst);

        let handle_type = if path == "/README" {
            if flags.write {
                return Err(FsError::permission_denied("README is read-only"));
            }
            HandleType::ReadmeFile(README_CONTENT.as_bytes().to_vec())
        } else if let Some(topic_name) =
            path.strip_prefix('/').and_then(|p| p.strip_suffix(".info"))
        {
            if flags.write {
                return Err(FsError::permission_denied(".info files are read-only"));
            }
            let topics = self.topics.read().unwrap();
            let topic = topics
                .get(topic_name)
                .ok_or_else(|| FsError::not_found(topic_name))?
                .clone();
            HandleType::TopicInfo(topic)
        } else if let Some(topic_name) = path.strip_prefix('/') {
            if topic_name.is_empty() || topic_name.contains('/') || topic_name == "README" {
                return Err(FsError::not_found(&path));
            }

            if flags.read && flags.write {
                return Err(FsError::invalid_argument(
                    "cannot open a topic for both read (subscribe) and write (publish)",
                ));
            }
            if flags.write {
                let topic = self.create_topic_if_needed(topic_name);
                HandleType::TopicPublish(topic)
            } else if flags.read {
                let topics = self.topics.read().unwrap();
                let topic = topics
                    .get(topic_name)
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

        let info = self.stat(&path)?;

        let handle = PubSubHandle {
            id: handle_id,
            path: path.clone(),
            handle_type,
        };

        self.handles.lock().unwrap().insert(handle_id, handle);
        Ok((Handle::new(handle_id), info))
    }

    pub(crate) fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let mut handles = self.handles.lock().unwrap();
        let h = handles
            .get_mut(&handle)
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
                if !*historical_sent {
                    for msg in &historical[*historical_index..] {
                        let formatted = msg.format();
                        buffer.extend_from_slice(&formatted);
                    }
                    *historical_sent = true;
                }

                loop {
                    match receiver.try_recv() {
                        Ok(msg) => {
                            let formatted = msg.format();
                            buffer.extend_from_slice(&formatted);
                        }
                        Err(broadcast::error::TryRecvError::Empty) => break,
                        Err(broadcast::error::TryRecvError::Lagged(_)) => {
                            break;
                        }
                        Err(broadcast::error::TryRecvError::Closed) => break,
                    }
                }

                let rel_offset = offset.saturating_sub(*buffer_offset) as usize;
                if rel_offset < buffer.len() {
                    let end = (rel_offset + size).min(buffer.len());
                    let data = Bytes::copy_from_slice(&buffer[rel_offset..end]);

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

    pub(crate) fn write(&self, handle: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let h = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

        match &h.handle_type {
            HandleType::TopicPublish(topic) => topic.publish(Bytes::copy_from_slice(data)),
            _ => Err(FsError::permission_denied("cannot write to this handle")),
        }
    }

    pub(crate) fn close(&self, handle: u64) -> FsResult<()> {
        let mut handles = self.handles.lock().unwrap();

        if let Some(h) = handles.remove(&handle) {
            if let HandleType::TopicSubscribe {
                topic,
                subscriber_id,
                ..
            } = h.handle_type
            {
                topic.unsubscribe(subscriber_id);
            }
        }

        Ok(())
    }

    pub(crate) fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = Self::normalize_path(path);

        if path == "/" {
            let mut entries = vec![FileInfo {
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
            }];

            let topics = self.topics.read().unwrap();
            for topic in topics.values() {
                let mtime = *topic.mtime.read().unwrap();

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

    pub(crate) fn remove(&self, path: &str) -> FsResult<()> {
        let path = Self::normalize_path(path);

        if path == "/" || path == "/README" {
            return Err(FsError::permission_denied("cannot remove special files"));
        }

        if path.ends_with(".info") {
            return Err(FsError::permission_denied(
                ".info files cannot be deleted directly; delete the topic instead",
            ));
        }

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
