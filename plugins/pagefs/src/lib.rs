#![allow(clippy::missing_safety_doc)]

use std::collections::BTreeMap;
use std::ptr;
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags, StatChanges,
};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FS9_ERR_ALREADY_EXISTS, FS9_ERR_INVALID_HANDLE,
    FS9_ERR_IS_DIRECTORY, FS9_ERR_NOT_DIRECTORY, FS9_ERR_NOT_FOUND, FS9_OK, FS9_SDK_VERSION,
};
use libc::{c_char, c_void, size_t};
use serde::{Deserialize, Serialize};

const PAGE_SIZE: usize = 16 * 1024;
const ROOT_INODE: u64 = 1;

/// Convert a signed Unix timestamp (seconds since epoch) to SystemTime.
/// Handles negative timestamps (pre-1970) correctly.
fn timestamp_to_system_time(ts: i64) -> SystemTime {
    if ts >= 0 {
        UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)
    } else {
        UNIX_EPOCH - std::time::Duration::from_secs((-ts) as u64)
    }
}

pub trait KvBackend: Send + Sync {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;
    fn set(&self, key: &[u8], value: &[u8]);
    fn scan(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)>;
    fn delete(&self, key: &[u8]);
}

pub struct InMemoryKv {
    data: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl InMemoryKv {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(BTreeMap::new()),
        }
    }
}

impl Default for InMemoryKv {
    fn default() -> Self {
        Self::new()
    }
}

impl KvBackend for InMemoryKv {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.data.read().unwrap().get(key).cloned()
    }

    fn set(&self, key: &[u8], value: &[u8]) {
        self.data
            .write()
            .unwrap()
            .insert(key.to_vec(), value.to_vec());
    }

    fn scan(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let data = self.data.read().unwrap();
        data.range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn delete(&self, key: &[u8]) {
        self.data.write().unwrap().remove(key);
    }
}

#[cfg(feature = "s3")]
pub struct S3KvBackend {
    client: aws_sdk_s3::Client,
    bucket: String,
    prefix: String,
    runtime: tokio::runtime::Runtime,
}

#[cfg(feature = "s3")]
impl S3KvBackend {
    pub fn new(bucket: String, prefix: String) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let config = runtime.block_on(aws_config::load_defaults(aws_config::BehaviorVersion::latest()));
        let client = aws_sdk_s3::Client::new(&config);
        Self {
            client,
            bucket,
            prefix,
            runtime,
        }
    }

    fn make_key(&self, key: &[u8]) -> String {
        let hex_key = key.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        if self.prefix.is_empty() {
            hex_key
        } else {
            format!("{}/{}", self.prefix, hex_key)
        }
    }

    fn parse_key(&self, s3_key: &str) -> Option<Vec<u8>> {
        let hex = if self.prefix.is_empty() {
            s3_key
        } else {
            s3_key.strip_prefix(&format!("{}/", self.prefix))?
        };
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect()
    }
}

#[cfg(feature = "s3")]
impl KvBackend for S3KvBackend {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let s3_key = self.make_key(key);
        self.runtime.block_on(async {
            match self.client
                .get_object()
                .bucket(&self.bucket)
                .key(&s3_key)
                .send()
                .await
            {
                Ok(output) => {
                    output.body.collect().await.ok().map(|data| data.into_bytes().to_vec())
                }
                Err(_) => None,
            }
        })
    }

    fn set(&self, key: &[u8], value: &[u8]) {
        let s3_key = self.make_key(key);
        let body = aws_sdk_s3::primitives::ByteStream::from(value.to_vec());
        self.runtime.block_on(async {
            let _ = self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&s3_key)
                .body(body)
                .send()
                .await;
        });
    }

    fn scan(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let s3_prefix = self.make_key(prefix);
        self.runtime.block_on(async {
            let mut results = Vec::new();
            let mut continuation_token: Option<String> = None;

            loop {
                let mut req = self.client
                    .list_objects_v2()
                    .bucket(&self.bucket)
                    .prefix(&s3_prefix);

                if let Some(token) = continuation_token.take() {
                    req = req.continuation_token(token);
                }

                match req.send().await {
                    Ok(output) => {
                        if let Some(contents) = output.contents {
                            for obj in contents {
                                if let Some(key) = obj.key {
                                    if let Some(parsed_key) = self.parse_key(&key) {
                                        if let Some(value) = self.get(&parsed_key) {
                                            results.push((parsed_key, value));
                                        }
                                    }
                                }
                            }
                        }
                        if output.is_truncated.unwrap_or(false) {
                            continuation_token = output.next_continuation_token;
                        } else {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            results
        })
    }

    fn delete(&self, key: &[u8]) {
        let s3_key = self.make_key(key);
        self.runtime.block_on(async {
            let _ = self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(&s3_key)
                .send()
                .await;
        });
    }
}

mod keys {
    pub fn superblock() -> Vec<u8> {
        b"S".to_vec()
    }

    pub fn inode(inode_id: u64) -> Vec<u8> {
        let mut key = vec![b'I'];
        key.extend_from_slice(&inode_id.to_be_bytes());
        key
    }

    pub fn dir_entry(parent_inode: u64, name: &str) -> Vec<u8> {
        let mut key = vec![b'D'];
        key.extend_from_slice(&parent_inode.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(name.as_bytes());
        key
    }

    pub fn dir_prefix(parent_inode: u64) -> Vec<u8> {
        let mut key = vec![b'D'];
        key.extend_from_slice(&parent_inode.to_be_bytes());
        key.push(b':');
        key
    }

    pub fn page(inode_id: u64, page_num: u64) -> Vec<u8> {
        let mut key = vec![b'P'];
        key.extend_from_slice(&inode_id.to_be_bytes());
        key.push(b':');
        key.extend_from_slice(&page_num.to_be_bytes());
        key
    }

    pub fn page_prefix(inode_id: u64) -> Vec<u8> {
        let mut key = vec![b'P'];
        key.extend_from_slice(&inode_id.to_be_bytes());
        key.push(b':');
        key
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Superblock {
    next_inode: u64,
    page_size: usize,
    total_pages: u64,
    used_pages: u64,
}

impl Default for Superblock {
    fn default() -> Self {
        Self {
            next_inode: ROOT_INODE + 1,
            page_size: PAGE_SIZE,
            total_pages: 1_000_000,
            used_pages: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum InodeType {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Inode {
    id: u64,
    inode_type: InodeType,
    mode: u32,
    size: u64,
    page_count: u64,
    atime: i64,
    mtime: i64,
    ctime: i64,
    nlink: u32,
}

impl Inode {
    fn new_file(id: u64, mode: u32) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            id,
            inode_type: InodeType::File,
            mode,
            size: 0,
            page_count: 0,
            atime: now,
            mtime: now,
            ctime: now,
            nlink: 1,
        }
    }

    fn new_directory(id: u64, mode: u32) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            id,
            inode_type: InodeType::Directory,
            mode,
            size: 0,
            page_count: 0,
            atime: now,
            mtime: now,
            ctime: now,
            nlink: 2,
        }
    }

    fn is_directory(&self) -> bool {
        self.inode_type == InodeType::Directory
    }

    fn touch_mtime(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.mtime = now;
        self.ctime = now;
    }

    fn touch_atime(&mut self) {
        self.atime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PageFsConfig {
    #[serde(default)]
    uid: u32,
    #[serde(default)]
    gid: u32,
    #[serde(default)]
    backend: BackendConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
enum BackendConfig {
    #[default]
    Memory,
    #[cfg(feature = "s3")]
    S3 {
        bucket: String,
        #[serde(default)]
        prefix: String,
    },
}

pub struct PageFsProvider {
    kv: Box<dyn KvBackend>,
    handles: Mutex<BTreeMap<u64, (u64, String, OpenFlags)>>,
    next_handle: Mutex<u64>,
    uid: u32,
    gid: u32,
}

impl PageFsProvider {
    pub fn new(kv: Box<dyn KvBackend>) -> Self {
        Self::with_config(kv, 0, 0)
    }

    pub fn with_config(kv: Box<dyn KvBackend>, uid: u32, gid: u32) -> Self {
        let provider = Self {
            kv,
            handles: Mutex::new(BTreeMap::new()),
            next_handle: Mutex::new(1),
            uid,
            gid,
        };
        provider.init_filesystem();
        provider
    }

    pub fn with_memory_backend() -> Self {
        Self::new(Box::new(InMemoryKv::new()))
    }

    fn init_filesystem(&self) {
        if self.kv.get(&keys::superblock()).is_none() {
            let sb = Superblock::default();
            self.save_superblock(&sb);

            let root = Inode::new_directory(ROOT_INODE, 0o755);
            self.save_inode(&root);
        }
    }

    fn load_superblock(&self) -> Superblock {
        self.kv
            .get(&keys::superblock())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    fn save_superblock(&self, sb: &Superblock) {
        let data = serde_json::to_vec(sb).unwrap();
        self.kv.set(&keys::superblock(), &data);
    }

    fn alloc_inode(&self) -> u64 {
        let mut sb = self.load_superblock();
        let id = sb.next_inode;
        sb.next_inode += 1;
        self.save_superblock(&sb);
        id
    }

    fn load_inode(&self, inode_id: u64) -> Option<Inode> {
        self.kv
            .get(&keys::inode(inode_id))
            .and_then(|data| serde_json::from_slice(&data).ok())
    }

    fn save_inode(&self, inode: &Inode) {
        let data = serde_json::to_vec(inode).unwrap();
        self.kv.set(&keys::inode(inode.id), &data);
    }

    fn delete_inode(&self, inode_id: u64) {
        self.kv.delete(&keys::inode(inode_id));
    }

    fn lookup(&self, parent_inode: u64, name: &str) -> Option<u64> {
        self.kv
            .get(&keys::dir_entry(parent_inode, name))
            .and_then(|data| {
                if data.len() == 8 {
                    Some(u64::from_be_bytes(data.try_into().unwrap()))
                } else {
                    None
                }
            })
    }

    fn link(&self, parent_inode: u64, name: &str, child_inode: u64) {
        self.kv.set(
            &keys::dir_entry(parent_inode, name),
            &child_inode.to_be_bytes(),
        );
    }

    fn unlink(&self, parent_inode: u64, name: &str) {
        self.kv.delete(&keys::dir_entry(parent_inode, name));
    }

    fn list_dir(&self, parent_inode: u64) -> Vec<(String, u64)> {
        let prefix = keys::dir_prefix(parent_inode);
        self.kv
            .scan(&prefix)
            .into_iter()
            .filter_map(|(key, value)| {
                let name_bytes = &key[prefix.len()..];
                let name = String::from_utf8(name_bytes.to_vec()).ok()?;
                let child_inode = u64::from_be_bytes(value.try_into().ok()?);
                Some((name, child_inode))
            })
            .collect()
    }

    fn read_page(&self, inode_id: u64, page_num: u64) -> Option<Vec<u8>> {
        self.kv.get(&keys::page(inode_id, page_num))
    }

    fn write_page(&self, inode_id: u64, page_num: u64, data: &[u8]) {
        let mut page_data = data.to_vec();
        if page_data.len() < PAGE_SIZE {
            page_data.resize(PAGE_SIZE, 0);
        }
        self.kv.set(&keys::page(inode_id, page_num), &page_data);
    }

    fn delete_pages(&self, inode_id: u64) {
        let prefix = keys::page_prefix(inode_id);
        let pages: Vec<_> = self.kv.scan(&prefix).into_iter().map(|(k, _)| k).collect();
        for key in pages {
            self.kv.delete(&key);
        }
    }

    fn resolve_path(&self, path: &str) -> FsResult<(u64, Inode)> {
        let path = self.normalize_path(path);
        if path == "/" {
            let inode = self
                .load_inode(ROOT_INODE)
                .ok_or_else(|| FsError::internal("root inode missing"))?;
            return Ok((ROOT_INODE, inode));
        }

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current_inode = ROOT_INODE;

        for (i, part) in parts.iter().enumerate() {
            let child_inode = self
                .lookup(current_inode, part)
                .ok_or_else(|| FsError::not_found(&path))?;

            if i < parts.len() - 1 {
                let inode = self
                    .load_inode(child_inode)
                    .ok_or_else(|| FsError::not_found(&path))?;
                if !inode.is_directory() {
                    return Err(FsError::not_directory(*part));
                }
            }
            current_inode = child_inode;
        }

        let inode = self
            .load_inode(current_inode)
            .ok_or_else(|| FsError::not_found(&path))?;
        Ok((current_inode, inode))
    }

    fn resolve_parent(&self, path: &str) -> FsResult<(u64, String)> {
        let path = self.normalize_path(path);
        if path == "/" {
            return Err(FsError::invalid_argument("cannot get parent of root"));
        }

        let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
        let parent_path = if parent.is_empty() { "/" } else { parent };

        let (parent_inode, parent_node) = self.resolve_path(parent_path)?;
        if !parent_node.is_directory() {
            return Err(FsError::not_directory(parent_path));
        }

        Ok((parent_inode, name.to_string()))
    }

    fn normalize_path(&self, path: &str) -> String {
        let path = if path.is_empty() { "/" } else { path };
        if path == "/" {
            "/".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        }
    }

    fn pages_needed(size: u64) -> u64 {
        if size == 0 {
            0
        } else {
            (size + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64
        }
    }

    pub fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path = self.normalize_path(path);
        let (_, inode) = self.resolve_path(&path)?;

        Ok(FileInfo {
            path: path.clone(),
            size: inode.size,
            file_type: if inode.is_directory() {
                FileType::Directory
            } else {
                FileType::Regular
            },
            mode: inode.mode,
            uid: self.uid,
            gid: self.gid,
            atime: timestamp_to_system_time(inode.atime),
            mtime: timestamp_to_system_time(inode.mtime),
            ctime: timestamp_to_system_time(inode.ctime),
            etag: String::new(),
            symlink_target: None,
        })
    }

    pub fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        let path = self.normalize_path(path);

        let inode_id = if flags.create {
            match self.resolve_path(&path) {
                Ok((id, _)) => id,
                Err(FsError::NotFound(_)) => {
                    let (parent_inode, name) = self.resolve_parent(&path)?;
                    let new_id = self.alloc_inode();

                    let inode = if flags.directory {
                        Inode::new_directory(new_id, 0o755)
                    } else {
                        let mut f = Inode::new_file(new_id, 0o644);
                        f.page_count = 1;
                        self.write_page(new_id, 0, &vec![0u8; PAGE_SIZE]);
                        f
                    };

                    self.save_inode(&inode);
                    self.link(parent_inode, &name, new_id);

                    new_id
                }
                Err(e) => return Err(e),
            }
        } else {
            let (id, _) = self.resolve_path(&path)?;
            id
        };

        if flags.truncate {
            if let Some(mut inode) = self.load_inode(inode_id) {
                if !inode.is_directory() {
                    self.delete_pages(inode_id);
                    inode.size = 0;
                    inode.page_count = 1;
                    self.write_page(inode_id, 0, &vec![0u8; PAGE_SIZE]);
                    inode.touch_mtime();
                    self.save_inode(&inode);
                }
            }
        }

        let mut next = self.next_handle.lock().unwrap();
        let handle_id = *next;
        *next += 1;

        self.handles
            .lock()
            .unwrap()
            .insert(handle_id, (inode_id, path, flags));

        Ok(Handle::new(handle_id))
    }

    pub fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let handles = self.handles.lock().unwrap();
        let (inode_id, path, _) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        let mut inode = self
            .load_inode(inode_id)
            .ok_or_else(|| FsError::not_found(&path))?;

        if inode.is_directory() {
            return Err(FsError::is_directory(&path));
        }

        let file_size = inode.size;
        if offset >= file_size {
            return Ok(Bytes::new());
        }

        let read_end = ((offset + size as u64).min(file_size)) as usize;
        let read_start = offset as usize;
        let total_to_read = read_end - read_start;

        let mut result = vec![0u8; total_to_read];
        let mut bytes_read = 0usize;
        let mut current_offset = offset as usize;

        while bytes_read < total_to_read {
            let page_num = (current_offset / PAGE_SIZE) as u64;
            let page_offset = current_offset % PAGE_SIZE;
            let bytes_in_page = (PAGE_SIZE - page_offset).min(total_to_read - bytes_read);

            if let Some(page_data) = self.read_page(inode_id, page_num) {
                let available = page_data.len().saturating_sub(page_offset);
                let to_copy = bytes_in_page.min(available);
                if to_copy > 0 {
                    result[bytes_read..bytes_read + to_copy]
                        .copy_from_slice(&page_data[page_offset..page_offset + to_copy]);
                }
            }

            bytes_read += bytes_in_page;
            current_offset += bytes_in_page;
        }

        inode.touch_atime();
        self.save_inode(&inode);

        Ok(Bytes::from(result))
    }

    pub fn write(&self, handle: u64, offset: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let (inode_id, path, flags) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        let mut inode = self
            .load_inode(inode_id)
            .ok_or_else(|| FsError::not_found(&path))?;

        if inode.is_directory() {
            return Err(FsError::is_directory(&path));
        }

        let write_offset = if flags.append {
            inode.size as usize
        } else {
            offset as usize
        };

        let mut bytes_written = 0usize;
        let mut current_offset = write_offset;

        while bytes_written < data.len() {
            let page_num = (current_offset / PAGE_SIZE) as u64;
            let page_offset = current_offset % PAGE_SIZE;
            let bytes_to_write = (PAGE_SIZE - page_offset).min(data.len() - bytes_written);

            let mut page_data = self
                .read_page(inode_id, page_num)
                .unwrap_or_else(|| vec![0u8; PAGE_SIZE]);

            if page_data.len() < PAGE_SIZE {
                page_data.resize(PAGE_SIZE, 0);
            }

            page_data[page_offset..page_offset + bytes_to_write]
                .copy_from_slice(&data[bytes_written..bytes_written + bytes_to_write]);

            self.write_page(inode_id, page_num, &page_data);

            bytes_written += bytes_to_write;
            current_offset += bytes_to_write;
        }

        let new_size = (write_offset + data.len()) as u64;
        if new_size > inode.size {
            inode.size = new_size;
            inode.page_count = Self::pages_needed(new_size).max(1);
        }
        inode.touch_mtime();
        self.save_inode(&inode);

        Ok(data.len())
    }

    pub fn close(&self, handle: u64) -> FsResult<()> {
        self.handles
            .lock()
            .unwrap()
            .remove(&handle)
            .map(|_| ())
            .ok_or_else(|| FsError::invalid_handle(handle))
    }

    pub fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = self.normalize_path(path);
        let (inode_id, inode) = self.resolve_path(&path)?;

        if !inode.is_directory() {
            return Err(FsError::not_directory(&path));
        }

        let entries = self.list_dir(inode_id);
        let mut result = Vec::with_capacity(entries.len());

        for (name, child_inode_id) in entries {
            if let Some(child_inode) = self.load_inode(child_inode_id) {
                let child_path = if path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path, name)
                };

                result.push(FileInfo {
                    path: child_path,
                    size: child_inode.size,
                    file_type: if child_inode.is_directory() {
                        FileType::Directory
                    } else {
                        FileType::Regular
                    },
                    mode: child_inode.mode,
                    uid: self.uid,
                    gid: self.gid,
                    atime: timestamp_to_system_time(child_inode.atime),
                    mtime: timestamp_to_system_time(child_inode.mtime),
                    ctime: timestamp_to_system_time(child_inode.ctime),
                    etag: String::new(),
                    symlink_target: None,
                });
            }
        }

        result.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    pub fn remove(&self, path: &str) -> FsResult<()> {
        let path = self.normalize_path(path);
        if path == "/" {
            return Err(FsError::permission_denied("cannot remove root"));
        }

        let (inode_id, inode) = self.resolve_path(&path)?;

        if inode.is_directory() {
            let children = self.list_dir(inode_id);
            if !children.is_empty() {
                return Err(FsError::directory_not_empty(&path));
            }
        } else {
            self.delete_pages(inode_id);
        }

        let (parent_inode, name) = self.resolve_parent(&path)?;
        self.unlink(parent_inode, &name);
        self.delete_inode(inode_id);

        Ok(())
    }

    pub fn wstat(&self, path: &str, changes: &StatChanges) -> FsResult<()> {
        let path = self.normalize_path(path);

        if let Some(new_name) = &changes.name {
            return self.rename(&path, new_name);
        }

        let (inode_id, mut inode) = self.resolve_path(&path)?;

        if let Some(m) = changes.mode {
            inode.mode = m;
        }

        if let Some(new_size) = changes.size {
            if inode.is_directory() {
                return Err(FsError::is_directory(&path));
            }

            let old_page_count = inode.page_count;
            let new_page_count = Self::pages_needed(new_size).max(1);

            if new_page_count < old_page_count {
                for page_num in new_page_count..old_page_count {
                    self.kv.delete(&keys::page(inode_id, page_num));
                }
            } else if new_page_count > old_page_count {
                for page_num in old_page_count..new_page_count {
                    self.write_page(inode_id, page_num, &vec![0u8; PAGE_SIZE]);
                }
            }

            if new_size < inode.size {
                let last_page = new_page_count - 1;
                let page_offset = (new_size % PAGE_SIZE as u64) as usize;
                if page_offset > 0 {
                    if let Some(mut page_data) = self.read_page(inode_id, last_page) {
                        for i in page_offset..PAGE_SIZE {
                            page_data[i] = 0;
                        }
                        self.write_page(inode_id, last_page, &page_data);
                    }
                }
            }

            inode.size = new_size;
            inode.page_count = new_page_count;
        }

        if let Some(atime) = changes.atime {
            inode.atime = systemtime_to_timestamp(atime);
        }

        if let Some(mtime) = changes.mtime {
            inode.mtime = systemtime_to_timestamp(mtime);
        } else {
            inode.touch_mtime();
        }

        self.save_inode(&inode);

        Ok(())
    }

    fn rename(&self, old_path: &str, new_name: &str) -> FsResult<()> {
        let new_path = if new_name.starts_with('/') {
            self.normalize_path(new_name)
        } else {
            let parent = self
                .parent_path(old_path)
                .unwrap_or_else(|| "/".to_string());
            if parent == "/" {
                self.normalize_path(&format!("/{new_name}"))
            } else {
                self.normalize_path(&format!("{parent}/{new_name}"))
            }
        };

        if old_path == new_path {
            return Ok(());
        }

        let (src_inode_id, src_inode) = self.resolve_path(old_path)?;

        if let Ok((dst_inode_id, dst_inode)) = self.resolve_path(&new_path) {
            if dst_inode.is_directory() {
                if !src_inode.is_directory() {
                    return Err(FsError::is_directory(&new_path));
                }
                let entries = self.list_dir(dst_inode_id);
                if !entries.is_empty() {
                    return Err(FsError::directory_not_empty(&new_path));
                }
            } else if src_inode.is_directory() {
                return Err(FsError::not_directory(&new_path));
            }
            if !dst_inode.is_directory() {
                self.delete_pages(dst_inode_id);
            }
            self.delete_inode(dst_inode_id);
        }

        let (old_parent_id, old_name) = self.resolve_parent(old_path)?;
        self.unlink(old_parent_id, &old_name);

        let (new_parent_id, new_entry_name) = self.resolve_parent(&new_path)?;
        self.link(new_parent_id, &new_entry_name, src_inode_id);

        Ok(())
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        if path == "/" {
            return None;
        }
        let path = path.trim_end_matches('/');
        match path.rfind('/') {
            Some(0) => Some("/".to_string()),
            Some(idx) => Some(path[..idx].to_string()),
            None => None,
        }
    }
}

fn systemtime_to_timestamp(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
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
        FsError::AlreadyExists(_) => FS9_ERR_ALREADY_EXISTS,
        FsError::IsDirectory(_) => FS9_ERR_IS_DIRECTORY,
        FsError::NotDirectory(_) => FS9_ERR_NOT_DIRECTORY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        _ => fs9_sdk_ffi::FS9_ERR_INTERNAL,
    }
}

unsafe extern "C" fn create_provider(config: *const c_char, config_len: size_t) -> *mut c_void {
    let cfg: PageFsConfig = if config.is_null() || config_len == 0 {
        PageFsConfig::default()
    } else {
        let config_slice = std::slice::from_raw_parts(config as *const u8, config_len);
        serde_json::from_slice(config_slice).unwrap_or_default()
    };

    let backend: Box<dyn KvBackend> = match cfg.backend {
        BackendConfig::Memory => Box::new(InMemoryKv::new()),
        #[cfg(feature = "s3")]
        BackendConfig::S3 { bucket, prefix } => Box::new(S3KvBackend::new(bucket, prefix)),
    };

    let provider = Box::new(PageFsProvider::with_config(backend, cfg.uid, cfg.gid));
    Box::into_raw(provider) as *mut c_void
}

unsafe extern "C" fn destroy_provider(provider: *mut c_void) {
    if !provider.is_null() {
        drop(Box::from_raw(provider as *mut PageFsProvider));
    }
}

unsafe extern "C" fn get_capabilities(_provider: *mut c_void) -> u64 {
    (Capabilities::BASIC_RW | Capabilities::TRUNCATE | Capabilities::RENAME | Capabilities::CHMOD | Capabilities::UTIME).bits()
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

    let provider = &*(provider as *const PageFsProvider);
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
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    changes: *const CStatChanges,
) -> CResult {
    if provider.is_null() || changes.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PageFsProvider);
    let path =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(path as *const u8, path_len));
    let c = &*changes;

    let stat_changes = StatChanges {
        mode: if c.has_mode != 0 { Some(c.mode) } else { None },
        uid: if c.has_uid != 0 { Some(c.uid) } else { None },
        gid: if c.has_gid != 0 { Some(c.gid) } else { None },
        size: if c.has_size != 0 { Some(c.size) } else { None },
        atime: if c.has_atime != 0 {
            Some(timestamp_to_system_time(c.atime))
        } else {
            None
        },
        mtime: if c.has_mtime != 0 {
            Some(timestamp_to_system_time(c.mtime))
        } else {
            None
        },
        name: if c.has_name != 0 && !c.name.is_null() {
            std::str::from_utf8(std::slice::from_raw_parts(c.name as *const u8, c.name_len))
                .ok()
                .map(String::from)
        } else {
            None
        },
        symlink_target: None,
    };

    match provider.wstat(path, &stat_changes) {
        Ok(()) => CResult {
            code: FS9_OK,
            error_msg: ptr::null(),
            error_msg_len: 0,
        },
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
}

unsafe extern "C" fn statfs_fn(
    provider: *mut c_void,
    _path: *const c_char,
    _path_len: size_t,
    out_stats: *mut CFsStats,
) -> CResult {
    if provider.is_null() || out_stats.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PageFsProvider);
    let sb = provider.load_superblock();

    (*out_stats).total_bytes = sb.total_pages * sb.page_size as u64;
    (*out_stats).free_bytes = (sb.total_pages - sb.used_pages) * sb.page_size as u64;
    (*out_stats).total_inodes = 1_000_000;
    (*out_stats).free_inodes = 1_000_000 - sb.next_inode;
    (*out_stats).block_size = sb.page_size as u32;
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
) -> CResult {
    if provider.is_null() || flags.is_null() || out_handle.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PageFsProvider);
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
        Ok(handle) => {
            *out_handle = handle.id();
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

    let provider = &*(provider as *const PageFsProvider);

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
    offset: u64,
    data: *const u8,
    data_len: size_t,
    out_written: *mut size_t,
) -> CResult {
    if provider.is_null() || out_written.is_null() {
        return make_cresult_err(fs9_sdk_ffi::FS9_ERR_INVALID_ARGUMENT);
    }

    let provider = &*(provider as *const PageFsProvider);
    let data = if data.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(data, data_len)
    };

    match provider.write(handle, offset, data) {
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

    let provider = &*(provider as *const PageFsProvider);

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

    let provider = &*(provider as *const PageFsProvider);
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

    let provider = &*(provider as *const PageFsProvider);
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

static PLUGIN_NAME: &[u8] = b"pagefs";
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

    fn create_provider() -> PageFsProvider {
        PageFsProvider::with_memory_backend()
    }

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
    fn root_exists() {
        let provider = create_provider();
        let info = provider.stat("/").unwrap();
        assert_eq!(info.file_type, FileType::Directory);
    }

    #[test]
    fn create_file_allocates_one_page() {
        let provider = create_provider();

        let handle = provider
            .open("/test.txt", OpenFlags::create_file())
            .unwrap();
        provider.close(handle.id()).unwrap();

        let inode = provider.load_inode(2).unwrap();
        assert_eq!(inode.page_count, 1);

        let page = provider.read_page(2, 0).unwrap();
        assert_eq!(page.len(), PAGE_SIZE);
    }

    #[test]
    fn create_and_read_file() {
        let provider = create_provider();

        let handle = provider
            .open("/test.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"hello pagefs").unwrap();
        provider.close(handle.id()).unwrap();

        let handle = provider.open("/test.txt", OpenFlags::read()).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"hello pagefs");
        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn write_across_page_boundary() {
        let provider = create_provider();

        let handle = provider
            .open("/cross.txt", OpenFlags::create_file())
            .unwrap();

        let data: Vec<u8> = (0..PAGE_SIZE + 1000).map(|i| (i % 256) as u8).collect();
        provider.write(handle.id(), 0, &data).unwrap();
        provider.close(handle.id()).unwrap();

        let inode = provider.resolve_path("/cross.txt").unwrap().1;
        assert_eq!(inode.page_count, 2);

        let handle = provider.open("/cross.txt", OpenFlags::read()).unwrap();
        let read_data = provider.read(handle.id(), 0, data.len()).unwrap();
        assert_eq!(&read_data[..], &data[..]);
        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn read_partial_page() {
        let provider = create_provider();

        let handle = provider
            .open("/partial.txt", OpenFlags::create_file())
            .unwrap();
        let data = b"0123456789ABCDEF0123456789";
        provider.write(handle.id(), 0, data).unwrap();
        provider.close(handle.id()).unwrap();

        let handle = provider.open("/partial.txt", OpenFlags::read()).unwrap();
        let result = provider.read(handle.id(), 10, 10).unwrap();
        assert_eq!(&result[..], b"ABCDEF0123");
        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn create_directory() {
        let provider = create_provider();

        let handle = provider.open("/mydir", OpenFlags::create_dir()).unwrap();
        provider.close(handle.id()).unwrap();

        let info = provider.stat("/mydir").unwrap();
        assert_eq!(info.file_type, FileType::Directory);
    }

    #[test]
    fn nested_directories() {
        let provider = create_provider();

        provider
            .open("/a", OpenFlags::create_dir())
            .unwrap()
            .id()
            .pipe(|h| provider.close(h).unwrap());
        provider
            .open("/a/b", OpenFlags::create_dir())
            .unwrap()
            .id()
            .pipe(|h| provider.close(h).unwrap());
        provider
            .open("/a/b/c", OpenFlags::create_dir())
            .unwrap()
            .id()
            .pipe(|h| provider.close(h).unwrap());

        let handle = provider
            .open("/a/b/c/file.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"deep file").unwrap();
        provider.close(handle.id()).unwrap();

        let handle = provider.open("/a/b/c/file.txt", OpenFlags::read()).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"deep file");
    }

    #[test]
    fn readdir_lists_children() {
        let provider = create_provider();

        for name in ["c.txt", "a.txt", "b.txt"] {
            let path = format!("/{}", name);
            let handle = provider.open(&path, OpenFlags::create_file()).unwrap();
            provider.close(handle.id()).unwrap();
        }

        let entries = provider.readdir("/").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/a.txt");
        assert_eq!(entries[1].path, "/b.txt");
        assert_eq!(entries[2].path, "/c.txt");
    }

    #[test]
    fn remove_file_deletes_pages() {
        let provider = create_provider();

        let handle = provider
            .open("/todelete.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"will be deleted").unwrap();
        provider.close(handle.id()).unwrap();

        let inode_id = provider.resolve_path("/todelete.txt").unwrap().0;
        assert!(provider.read_page(inode_id, 0).is_some());

        provider.remove("/todelete.txt").unwrap();

        assert!(provider.read_page(inode_id, 0).is_none());
        assert!(matches!(
            provider.stat("/todelete.txt"),
            Err(FsError::NotFound(_))
        ));
    }

    #[test]
    fn cannot_remove_non_empty_dir() {
        let provider = create_provider();

        provider
            .open("/parent", OpenFlags::create_dir())
            .unwrap()
            .id()
            .pipe(|h| provider.close(h).unwrap());

        let handle = provider
            .open("/parent/child.txt", OpenFlags::create_file())
            .unwrap();
        provider.close(handle.id()).unwrap();

        assert!(matches!(
            provider.remove("/parent"),
            Err(FsError::DirectoryNotEmpty(_))
        ));

        provider.remove("/parent/child.txt").unwrap();
        provider.remove("/parent").unwrap();
    }

    #[test]
    fn truncate_file() {
        let provider = create_provider();

        let handle = provider
            .open("/trunc.txt", OpenFlags::create_file())
            .unwrap();
        provider
            .write(handle.id(), 0, b"long content here that will be truncated")
            .unwrap();
        provider.close(handle.id()).unwrap();

        provider
            .wstat("/trunc.txt", &StatChanges::truncate(10))
            .unwrap();

        let info = provider.stat("/trunc.txt").unwrap();
        assert_eq!(info.size, 10);

        let handle = provider.open("/trunc.txt", OpenFlags::read()).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"long conte");
    }

    #[test]
    fn extend_file_via_wstat() {
        let provider = create_provider();

        let handle = provider
            .open("/extend.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"short").unwrap();
        provider.close(handle.id()).unwrap();

        provider
            .wstat("/extend.txt", &StatChanges::truncate(20))
            .unwrap();

        let info = provider.stat("/extend.txt").unwrap();
        assert_eq!(info.size, 20);
    }

    #[test]
    fn append_mode() {
        let provider = create_provider();

        let handle = provider
            .open("/append.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"first").unwrap();
        provider.close(handle.id()).unwrap();

        let flags = OpenFlags {
            write: true,
            append: true,
            ..Default::default()
        };
        let handle = provider.open("/append.txt", flags).unwrap();
        provider.write(handle.id(), 0, b"second").unwrap();
        provider.close(handle.id()).unwrap();

        let handle = provider.open("/append.txt", OpenFlags::read()).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"firstsecond");
    }

    #[test]
    fn large_file_spanning_many_pages() {
        let provider = create_provider();

        let handle = provider
            .open("/large.bin", OpenFlags::create_file())
            .unwrap();

        let data: Vec<u8> = (0..(PAGE_SIZE * 3 + 5000))
            .map(|i| (i % 256) as u8)
            .collect();
        provider.write(handle.id(), 0, &data).unwrap();
        provider.close(handle.id()).unwrap();

        let info = provider.stat("/large.bin").unwrap();
        assert_eq!(info.size, data.len() as u64);

        let inode = provider.resolve_path("/large.bin").unwrap().1;
        assert_eq!(inode.page_count, 4);

        let handle = provider.open("/large.bin", OpenFlags::read()).unwrap();
        let read_data = provider.read(handle.id(), 0, data.len()).unwrap();
        assert_eq!(read_data.len(), data.len());
        assert_eq!(&read_data[..], &data[..]);
    }

    #[test]
    fn sparse_write() {
        let provider = create_provider();

        let handle = provider
            .open("/sparse.txt", OpenFlags::create_file())
            .unwrap();
        provider
            .write(handle.id(), PAGE_SIZE as u64, b"sparse data")
            .unwrap();
        provider.close(handle.id()).unwrap();

        let info = provider.stat("/sparse.txt").unwrap();
        assert_eq!(info.size, PAGE_SIZE as u64 + 11);

        let inode = provider.resolve_path("/sparse.txt").unwrap().1;
        assert_eq!(inode.page_count, 2);

        let handle = provider.open("/sparse.txt", OpenFlags::read()).unwrap();
        let first_page = provider.read(handle.id(), 0, PAGE_SIZE).unwrap();
        assert!(first_page.iter().all(|&b| b == 0));

        let second_page = provider.read(handle.id(), PAGE_SIZE as u64, 11).unwrap();
        assert_eq!(&second_page[..], b"sparse data");
    }

    #[test]
    fn kv_operations() {
        let kv = InMemoryKv::new();

        kv.set(b"key1", b"value1");
        kv.set(b"key2", b"value2");
        kv.set(b"other", b"other_value");

        assert_eq!(kv.get(b"key1"), Some(b"value1".to_vec()));
        assert_eq!(kv.get(b"missing"), None);

        let scanned = kv.scan(b"key");
        assert_eq!(scanned.len(), 2);

        kv.delete(b"key1");
        assert_eq!(kv.get(b"key1"), None);
    }

    #[test]
    fn page_size_is_16kb() {
        assert_eq!(PAGE_SIZE, 16 * 1024);
    }

    #[test]
    fn negative_timestamps_handled_correctly() {
        use std::time::{Duration, UNIX_EPOCH};

        let positive = timestamp_to_system_time(100);
        assert_eq!(positive, UNIX_EPOCH + Duration::from_secs(100));

        let zero = timestamp_to_system_time(0);
        assert_eq!(zero, UNIX_EPOCH);

        let negative = timestamp_to_system_time(-100);
        assert_eq!(negative, UNIX_EPOCH - Duration::from_secs(100));
    }

    #[test]
    fn configurable_uid_gid() {
        let provider = PageFsProvider::with_config(Box::new(InMemoryKv::new()), 1000, 1001);

        let info = provider.stat("/").unwrap();
        assert_eq!(info.uid, 1000);
        assert_eq!(info.gid, 1001);

        let handle = provider
            .open("/file.txt", OpenFlags::create_file())
            .unwrap();
        provider.close(handle.id()).unwrap();

        let entries = provider.readdir("/").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uid, 1000);
        assert_eq!(entries[0].gid, 1001);
    }

    #[test]
    fn rename_file_same_dir() {
        let provider = PageFsProvider::with_memory_backend();

        let handle = provider.open("/old.txt", OpenFlags::create_file()).unwrap();
        provider.write(handle.id(), 0, b"content").unwrap();
        provider.close(handle.id()).unwrap();

        provider
            .wstat("/old.txt", &StatChanges::rename("new.txt"))
            .unwrap();

        assert!(provider.stat("/old.txt").is_err());
        let info = provider.stat("/new.txt").unwrap();
        assert_eq!(info.size, 7);
    }

    #[test]
    fn rename_file_cross_dir() {
        let provider = PageFsProvider::with_memory_backend();

        provider.open("/subdir", OpenFlags::create_dir()).unwrap();
        let handle = provider
            .open("/file.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"data").unwrap();
        provider.close(handle.id()).unwrap();

        provider
            .wstat("/file.txt", &StatChanges::rename("/subdir/moved.txt"))
            .unwrap();

        assert!(provider.stat("/file.txt").is_err());
        let info = provider.stat("/subdir/moved.txt").unwrap();
        assert_eq!(info.size, 4);
    }

    #[test]
    fn rename_replaces_existing_file() {
        let provider = PageFsProvider::with_memory_backend();

        let h1 = provider.open("/src.txt", OpenFlags::create_file()).unwrap();
        provider.write(h1.id(), 0, b"source").unwrap();
        provider.close(h1.id()).unwrap();

        let h2 = provider.open("/dst.txt", OpenFlags::create_file()).unwrap();
        provider.write(h2.id(), 0, b"old content").unwrap();
        provider.close(h2.id()).unwrap();

        provider
            .wstat("/src.txt", &StatChanges::rename("dst.txt"))
            .unwrap();

        assert!(provider.stat("/src.txt").is_err());
        let info = provider.stat("/dst.txt").unwrap();
        assert_eq!(info.size, 6);

        let handle = provider.open("/dst.txt", OpenFlags::read()).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"source");
    }

    #[test]
    fn rename_file_to_dir_fails() {
        let provider = PageFsProvider::with_memory_backend();

        let handle = provider
            .open("/file.txt", OpenFlags::create_file())
            .unwrap();
        provider.close(handle.id()).unwrap();

        provider.open("/dir", OpenFlags::create_dir()).unwrap();

        let result = provider.wstat("/file.txt", &StatChanges::rename("dir"));
        assert!(matches!(result, Err(FsError::IsDirectory(_))));
    }

    #[test]
    fn rename_dir_to_file_fails() {
        let provider = PageFsProvider::with_memory_backend();

        provider.open("/dir", OpenFlags::create_dir()).unwrap();

        let handle = provider.open("/file", OpenFlags::create_file()).unwrap();
        provider.close(handle.id()).unwrap();

        let result = provider.wstat("/dir", &StatChanges::rename("file"));
        assert!(matches!(result, Err(FsError::NotDirectory(_))));
    }

    #[test]
    fn rename_dir_to_nonempty_dir_fails() {
        let provider = PageFsProvider::with_memory_backend();

        provider.open("/src", OpenFlags::create_dir()).unwrap();
        provider.open("/dst", OpenFlags::create_dir()).unwrap();

        let handle = provider
            .open("/dst/child.txt", OpenFlags::create_file())
            .unwrap();
        provider.close(handle.id()).unwrap();

        let result = provider.wstat("/src", &StatChanges::rename("dst"));
        assert!(matches!(result, Err(FsError::DirectoryNotEmpty(_))));
    }

    #[cfg(feature = "s3")]
    mod s3_tests {
        use super::*;

        #[test]
        fn s3_make_key_without_prefix() {
            let backend = S3KvBackend {
                client: create_mock_client(),
                bucket: "test-bucket".to_string(),
                prefix: String::new(),
                runtime: tokio::runtime::Runtime::new().unwrap(),
            };

            assert_eq!(backend.make_key(b"S"), "53");
            assert_eq!(backend.make_key(b"I\x00\x00\x00\x00\x00\x00\x00\x01"), "490000000000000001");
            assert_eq!(backend.make_key(b"hello"), "68656c6c6f");
        }

        #[test]
        fn s3_make_key_with_prefix() {
            let backend = S3KvBackend {
                client: create_mock_client(),
                bucket: "test-bucket".to_string(),
                prefix: "pagefs".to_string(),
                runtime: tokio::runtime::Runtime::new().unwrap(),
            };

            assert_eq!(backend.make_key(b"S"), "pagefs/53");
            assert_eq!(backend.make_key(b"hello"), "pagefs/68656c6c6f");
        }

        #[test]
        fn s3_parse_key_without_prefix() {
            let backend = S3KvBackend {
                client: create_mock_client(),
                bucket: "test-bucket".to_string(),
                prefix: String::new(),
                runtime: tokio::runtime::Runtime::new().unwrap(),
            };

            assert_eq!(backend.parse_key("53"), Some(vec![0x53]));
            assert_eq!(backend.parse_key("68656c6c6f"), Some(b"hello".to_vec()));
        }

        #[test]
        fn s3_parse_key_with_prefix() {
            let backend = S3KvBackend {
                client: create_mock_client(),
                bucket: "test-bucket".to_string(),
                prefix: "pagefs".to_string(),
                runtime: tokio::runtime::Runtime::new().unwrap(),
            };

            assert_eq!(backend.parse_key("pagefs/53"), Some(vec![0x53]));
            assert_eq!(backend.parse_key("pagefs/68656c6c6f"), Some(b"hello".to_vec()));
            assert_eq!(backend.parse_key("other/53"), None);
        }

        #[test]
        fn s3_key_roundtrip() {
            let backend = S3KvBackend {
                client: create_mock_client(),
                bucket: "test-bucket".to_string(),
                prefix: "test".to_string(),
                runtime: tokio::runtime::Runtime::new().unwrap(),
            };

            let test_keys: Vec<&[u8]> = vec![
                b"S",
                b"I\x00\x00\x00\x00\x00\x00\x00\x01",
                b"D\x00\x00\x00\x00\x00\x00\x00\x01:file.txt",
                b"P\x00\x00\x00\x00\x00\x00\x00\x02:\x00\x00\x00\x00\x00\x00\x00\x00",
            ];

            for key in test_keys {
                let encoded = backend.make_key(key);
                let decoded = backend.parse_key(&encoded);
                assert_eq!(decoded, Some(key.to_vec()), "Roundtrip failed for {:?}", key);
            }
        }

        fn create_mock_client() -> aws_sdk_s3::Client {
            let config = aws_sdk_s3::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("us-east-1"))
                .build();
            aws_sdk_s3::Client::from_conf(config)
        }

        #[test]
        #[ignore]
        fn s3_integration_basic_operations() {
            let bucket = std::env::var("FS9_TEST_S3_BUCKET")
                .expect("FS9_TEST_S3_BUCKET env var required for S3 integration tests");
            let prefix = std::env::var("FS9_TEST_S3_PREFIX")
                .unwrap_or_else(|_| format!("pagefs-test-{}", std::process::id()));

            let backend = S3KvBackend::new(bucket, prefix);

            let test_key = b"test-key";
            let test_value = b"test-value-12345";

            backend.set(test_key, test_value);

            let retrieved = backend.get(test_key);
            assert_eq!(retrieved, Some(test_value.to_vec()));

            backend.delete(test_key);

            let after_delete = backend.get(test_key);
            assert_eq!(after_delete, None);
        }

        #[test]
        #[ignore]
        fn s3_integration_scan() {
            let bucket = std::env::var("FS9_TEST_S3_BUCKET")
                .expect("FS9_TEST_S3_BUCKET env var required for S3 integration tests");
            let prefix = std::env::var("FS9_TEST_S3_PREFIX")
                .unwrap_or_else(|_| format!("pagefs-test-{}", std::process::id()));

            let backend = S3KvBackend::new(bucket, prefix);

            backend.set(b"prefix:a", b"value-a");
            backend.set(b"prefix:b", b"value-b");
            backend.set(b"prefix:c", b"value-c");
            backend.set(b"other:x", b"value-x");

            let results = backend.scan(b"prefix:");
            assert_eq!(results.len(), 3);

            for (k, _) in &results {
                assert!(k.starts_with(b"prefix:"));
            }

            backend.delete(b"prefix:a");
            backend.delete(b"prefix:b");
            backend.delete(b"prefix:c");
            backend.delete(b"other:x");
        }

        #[test]
        #[ignore]
        fn s3_integration_full_pagefs() {
            let bucket = std::env::var("FS9_TEST_S3_BUCKET")
                .expect("FS9_TEST_S3_BUCKET env var required for S3 integration tests");
            let prefix = std::env::var("FS9_TEST_S3_PREFIX")
                .unwrap_or_else(|_| format!("pagefs-test-{}", std::process::id()));

            let backend = Box::new(S3KvBackend::new(bucket, prefix));
            let provider = PageFsProvider::new(backend);

            let info = provider.stat("/").unwrap();
            assert_eq!(info.file_type, FileType::Directory);

            let handle = provider.open("/test.txt", OpenFlags::create_file()).unwrap();
            provider.write(handle.id(), 0, b"Hello S3 PageFS!").unwrap();
            provider.close(handle.id()).unwrap();

            let handle = provider.open("/test.txt", OpenFlags::read()).unwrap();
            let data = provider.read(handle.id(), 0, 100).unwrap();
            assert_eq!(&data[..], b"Hello S3 PageFS!");
            provider.close(handle.id()).unwrap();

            provider.remove("/test.txt").unwrap();
            assert!(provider.stat("/test.txt").is_err());
        }
    }
}

trait PipeExt: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}

impl<T> PipeExt for T {}
