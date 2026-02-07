#![allow(clippy::missing_safety_doc)]

use std::collections::BTreeMap;
use std::ptr;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use fs9_sdk::FsError;
use fs9_sdk_ffi::{
    CResult, FS9_ERR_ALREADY_EXISTS, FS9_ERR_INVALID_HANDLE, FS9_ERR_IS_DIRECTORY,
    FS9_ERR_NOT_DIRECTORY, FS9_ERR_NOT_FOUND,
};
use serde::{Deserialize, Serialize};

pub mod ffi;
pub mod provider;

#[cfg(test)]
mod tests;

pub(crate) const PAGE_SIZE: usize = 16 * 1024;
pub(crate) const ROOT_INODE: u64 = 1;

/// Convert a signed Unix timestamp (seconds since epoch) to SystemTime.
/// Handles negative timestamps (pre-1970) correctly.
pub(crate) fn timestamp_to_system_time(ts: i64) -> SystemTime {
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
        let config =
            runtime.block_on(aws_config::load_defaults(aws_config::BehaviorVersion::latest()));
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
            match self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(&s3_key)
                .send()
                .await
            {
                Ok(output) => output
                    .body
                    .collect()
                    .await
                    .ok()
                    .map(|data| data.into_bytes().to_vec()),
                Err(_) => None,
            }
        })
    }

    fn set(&self, key: &[u8], value: &[u8]) {
        let s3_key = self.make_key(key);
        let body = aws_sdk_s3::primitives::ByteStream::from(value.to_vec());
        self.runtime.block_on(async {
            let _ = self
                .client
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
                let mut req = self
                    .client
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
            let _ = self
                .client
                .delete_object()
                .bucket(&self.bucket)
                .key(&s3_key)
                .send()
                .await;
        });
    }
}

pub(crate) mod keys {
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
pub(crate) struct Superblock {
    pub(crate) next_inode: u64,
    pub(crate) page_size: usize,
    pub(crate) total_pages: u64,
    pub(crate) used_pages: u64,
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
pub(crate) enum InodeType {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Inode {
    pub(crate) id: u64,
    pub(crate) inode_type: InodeType,
    pub(crate) mode: u32,
    pub(crate) size: u64,
    pub(crate) page_count: u64,
    pub(crate) atime: i64,
    pub(crate) mtime: i64,
    pub(crate) ctime: i64,
    pub(crate) nlink: u32,
}

impl Inode {
    pub(crate) fn new_file(id: u64, mode: u32) -> Self {
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

    pub(crate) fn new_directory(id: u64, mode: u32) -> Self {
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

    pub(crate) fn is_directory(&self) -> bool {
        self.inode_type == InodeType::Directory
    }

    pub(crate) fn touch_mtime(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.mtime = now;
        self.ctime = now;
    }

    pub(crate) fn touch_atime(&mut self) {
        self.atime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct PageFsConfig {
    #[serde(default)]
    pub(crate) uid: u32,
    #[serde(default)]
    pub(crate) gid: u32,
    #[serde(default)]
    pub(crate) backend: BackendConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum BackendConfig {
    #[default]
    Memory,
    #[cfg(feature = "s3")]
    S3 {
        bucket: String,
        #[serde(default)]
        prefix: String,
    },
}

pub(crate) fn systemtime_to_timestamp(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn make_cresult_err(code: i32) -> CResult {
    CResult {
        code,
        error_msg: ptr::null(),
        error_msg_len: 0,
    }
}

pub(crate) fn fserror_to_code(err: &FsError) -> i32 {
    match err {
        FsError::NotFound(_) => FS9_ERR_NOT_FOUND,
        FsError::AlreadyExists(_) => FS9_ERR_ALREADY_EXISTS,
        FsError::IsDirectory(_) => FS9_ERR_IS_DIRECTORY,
        FsError::NotDirectory(_) => FS9_ERR_NOT_DIRECTORY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        _ => fs9_sdk_ffi::FS9_ERR_INTERNAL,
    }
}
