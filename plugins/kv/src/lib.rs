#![allow(clippy::missing_safety_doc)]

use std::collections::BTreeMap;
use std::ptr;
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fs9_sdk::{Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FS9_ERR_ALREADY_EXISTS, FS9_ERR_INVALID_HANDLE,
    FS9_ERR_IS_DIRECTORY, FS9_ERR_NOT_DIRECTORY, FS9_ERR_NOT_FOUND, FS9_OK, FS9_SDK_VERSION,
};
use libc::{c_char, c_void, size_t};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct KvConfig {
    #[serde(default = "default_namespace")]
    namespace: String,
}

fn default_namespace() -> String {
    "default".to_string()
}

impl Default for KvConfig {
    fn default() -> Self {
        Self {
            namespace: default_namespace(),
        }
    }
}

#[derive(Debug, Clone)]
enum KvEntry {
    Directory {
        mode: u32,
        mtime: SystemTime,
    },
    File {
        data: Bytes,
        mode: u32,
        mtime: SystemTime,
    },
}

impl KvEntry {
    fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }

    fn mode(&self) -> u32 {
        match self {
            Self::Directory { mode, .. } | Self::File { mode, .. } => *mode,
        }
    }

    fn mtime(&self) -> SystemTime {
        match self {
            Self::Directory { mtime, .. } | Self::File { mtime, .. } => *mtime,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Self::Directory { .. } => 0,
            Self::File { data, .. } => data.len() as u64,
        }
    }
}

struct KvProvider {
    #[allow(dead_code)]
    config: KvConfig,
    store: RwLock<BTreeMap<String, KvEntry>>,
    handles: Mutex<BTreeMap<u64, (String, OpenFlags)>>,
    next_handle: Mutex<u64>,
}

impl KvProvider {
    fn new(config: KvConfig) -> Self {
        let mut store = BTreeMap::new();
        store.insert(
            "/".to_string(),
            KvEntry::Directory {
                mode: 0o755,
                mtime: SystemTime::now(),
            },
        );

        Self {
            config,
            store: RwLock::new(store),
            handles: Mutex::new(BTreeMap::new()),
            next_handle: Mutex::new(1),
        }
    }

    fn normalize_path(&self, path: &str) -> String {
        let path = if path.is_empty() { "/" } else { path };
        if path == "/" {
            "/".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        }
    }

    fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path = self.normalize_path(path);
        let store = self.store.read().unwrap();

        store
            .get(&path)
            .map(|entry| FileInfo {
                path: path.clone(),
                size: entry.size(),
                file_type: if entry.is_directory() {
                    FileType::Directory
                } else {
                    FileType::Regular
                },
                mode: entry.mode(),
                uid: 0,
                gid: 0,
                atime: entry.mtime(),
                mtime: entry.mtime(),
                ctime: entry.mtime(),
                etag: String::new(),
                symlink_target: None,
            })
            .ok_or_else(|| FsError::not_found(&path))
    }

    fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let path = self.normalize_path(path);

        if flags.create {
            let mut store = self.store.write().unwrap();
            if !store.contains_key(&path) {
                if flags.directory {
                    store.insert(
                        path.clone(),
                        KvEntry::Directory {
                            mode: 0o755,
                            mtime: SystemTime::now(),
                        },
                    );
                } else {
                    let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/");
                    let parent_path = if parent.is_empty() { "/" } else { parent };
                    if !store.contains_key(parent_path) {
                        return Err(FsError::not_found(parent_path));
                    }
                    store.insert(
                        path.clone(),
                        KvEntry::File {
                            data: Bytes::new(),
                            mode: 0o644,
                            mtime: SystemTime::now(),
                        },
                    );
                }
            }
        } else {
            let store = self.store.read().unwrap();
            if !store.contains_key(&path) {
                return Err(FsError::not_found(&path));
            }
        }

        let info = self.stat(&path)?;

        let mut next = self.next_handle.lock().unwrap();
        let handle_id = *next;
        *next += 1;

        self.handles
            .lock()
            .unwrap()
            .insert(handle_id, (path, flags));
        Ok((Handle::new(handle_id), info))
    }

    fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let handles = self.handles.lock().unwrap();
        let (path, _) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

        let store = self.store.read().unwrap();
        let entry = store.get(path).ok_or_else(|| FsError::not_found(path))?;

        match entry {
            KvEntry::Directory { .. } => Err(FsError::is_directory(path)),
            KvEntry::File { data, .. } => {
                let start = (offset as usize).min(data.len());
                let end = (start + size).min(data.len());
                Ok(data.slice(start..end))
            }
        }
    }

    fn write(&self, handle: u64, offset: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let (path, flags) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        let mut store = self.store.write().unwrap();
        let entry = store
            .get_mut(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        match entry {
            KvEntry::Directory { .. } => Err(FsError::is_directory(&path)),
            KvEntry::File {
                data: file_data,
                mtime,
                ..
            } => {
                let offset = if flags.append {
                    file_data.len()
                } else {
                    offset as usize
                };

                let mut buf = file_data.to_vec();
                if offset + data.len() > buf.len() {
                    buf.resize(offset + data.len(), 0);
                }
                buf[offset..offset + data.len()].copy_from_slice(data);
                *file_data = Bytes::from(buf);
                *mtime = SystemTime::now();

                Ok(data.len())
            }
        }
    }

    fn close(&self, handle: u64) -> FsResult<()> {
        self.handles
            .lock()
            .unwrap()
            .remove(&handle)
            .map(|_| ())
            .ok_or_else(|| FsError::invalid_handle(handle))
    }

    fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path = self.normalize_path(path);
        let store = self.store.read().unwrap();

        let entry = store.get(&path).ok_or_else(|| FsError::not_found(&path))?;
        if !entry.is_directory() {
            return Err(FsError::not_directory(&path));
        }

        let prefix = if path == "/" { "" } else { &path };

        let entries: Vec<FileInfo> = store
            .range(format!("{}/", prefix)..)
            .take_while(|(k, _)| {
                k.starts_with(&format!("{}/", prefix)) || (path == "/" && k.starts_with('/'))
            })
            .filter(|(k, _)| {
                if *k == "/" {
                    return false;
                }
                let relative = if path == "/" {
                    &k[1..]
                } else if k.starts_with(&format!("{}/", path)) {
                    &k[path.len() + 1..]
                } else {
                    return false;
                };
                !relative.is_empty() && !relative.contains('/')
            })
            .map(|(k, v)| FileInfo {
                path: k.clone(),
                size: v.size(),
                file_type: if v.is_directory() {
                    FileType::Directory
                } else {
                    FileType::Regular
                },
                mode: v.mode(),
                uid: 0,
                gid: 0,
                atime: v.mtime(),
                mtime: v.mtime(),
                ctime: v.mtime(),
                etag: String::new(),
                symlink_target: None,
            })
            .collect();

        Ok(entries)
    }

    fn remove(&self, path: &str) -> FsResult<()> {
        let path = self.normalize_path(path);
        if path == "/" {
            return Err(FsError::permission_denied("cannot remove root"));
        }

        let mut store = self.store.write().unwrap();

        let has_children = store
            .range(format!("{}/", path)..)
            .take_while(|(k, _)| k.starts_with(&format!("{}/", path)))
            .next()
            .is_some();

        if has_children {
            return Err(FsError::directory_not_empty(&path));
        }

        store
            .remove(&path)
            .map(|_| ())
            .ok_or_else(|| FsError::not_found(&path))
    }

    fn wstat(&self, path: &str, mode: Option<u32>, size: Option<u64>) -> FsResult<()> {
        let path = self.normalize_path(path);
        let mut store = self.store.write().unwrap();

        let entry = store
            .get_mut(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        match entry {
            KvEntry::Directory {
                mode: entry_mode,
                mtime,
            } => {
                if let Some(m) = mode {
                    *entry_mode = m;
                }
                *mtime = SystemTime::now();
            }
            KvEntry::File {
                data,
                mode: entry_mode,
                mtime,
            } => {
                if let Some(m) = mode {
                    *entry_mode = m;
                }
                if let Some(s) = size {
                    let mut buf = data.to_vec();
                    buf.resize(s as usize, 0);
                    *data = Bytes::from(buf);
                }
                *mtime = SystemTime::now();
            }
        }

        Ok(())
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
    let config: KvConfig = if config.is_null() || config_len == 0 {
        KvConfig::default()
    } else {
        let slice = std::slice::from_raw_parts(config as *const u8, config_len);
        match serde_json::from_slice(slice) {
            Ok(c) => c,
            Err(_) => KvConfig::default(),
        }
    };

    let provider = Box::new(KvProvider::new(config));
    Box::into_raw(provider) as *mut c_void
}

unsafe extern "C" fn destroy_provider(provider: *mut c_void) {
    if !provider.is_null() {
        drop(Box::from_raw(provider as *mut KvProvider));
    }
}

unsafe extern "C" fn get_capabilities(_provider: *mut c_void) -> u64 {
    (Capabilities::BASIC_RW | Capabilities::TRUNCATE).bits()
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

    let provider = &*(provider as *const KvProvider);
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

    let provider = &*(provider as *const KvProvider);
    let path =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(path as *const u8, path_len));
    let changes = &*changes;

    let mode = if changes.has_mode != 0 {
        Some(changes.mode)
    } else {
        None
    };
    let size = if changes.has_size != 0 {
        Some(changes.size)
    } else {
        None
    };

    match provider.wstat(path, mode, size) {
        Ok(()) => CResult {
            code: FS9_OK,
            error_msg: ptr::null(),
            error_msg_len: 0,
        },
        Err(e) => make_cresult_err(fserror_to_code(&e)),
    }
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

    const MB: u64 = 1024 * 1024;
    (*out_stats).total_bytes = 256 * MB;
    (*out_stats).free_bytes = 200 * MB;
    (*out_stats).total_inodes = 100_000;
    (*out_stats).free_inodes = 90_000;
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

    let provider = &*(provider as *const KvProvider);
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

    let provider = &*(provider as *const KvProvider);

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

    let provider = &*(provider as *const KvProvider);
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

    let provider = &*(provider as *const KvProvider);

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

    let provider = &*(provider as *const KvProvider);
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

    let provider = &*(provider as *const KvProvider);
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

static PLUGIN_NAME: &[u8] = b"kv";
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
    fn create_and_read_file() {
        let provider = KvProvider::new(KvConfig::default());

        let flags = OpenFlags::create_file();
        let handle = provider.open("/test.txt", flags).unwrap();

        provider.write(handle.id(), 0, b"kv store data").unwrap();

        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"kv store data");

        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn btree_ordering() {
        let provider = KvProvider::new(KvConfig::default());

        let flags = OpenFlags::create_file();
        for name in ["c.txt", "a.txt", "b.txt"] {
            let path = format!("/{}", name);
            let handle = provider.open(&path, flags).unwrap();
            provider.close(handle.id()).unwrap();
        }

        let entries = provider.readdir("/").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/a.txt");
        assert_eq!(entries[1].path, "/b.txt");
        assert_eq!(entries[2].path, "/c.txt");
    }

    #[test]
    fn nested_directories() {
        let provider = KvProvider::new(KvConfig::default());

        let dir_flags = OpenFlags::create_dir();
        provider.open("/level1", dir_flags).unwrap();
        provider.open("/level1/level2", dir_flags).unwrap();

        let file_flags = OpenFlags::create_file();
        let handle = provider
            .open("/level1/level2/file.txt", file_flags)
            .unwrap();
        provider.close(handle.id()).unwrap();

        let entries = provider.readdir("/level1").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/level1/level2");

        let entries = provider.readdir("/level1/level2").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/level1/level2/file.txt");
    }

    #[test]
    fn cannot_remove_non_empty() {
        let provider = KvProvider::new(KvConfig::default());

        let dir_flags = OpenFlags::create_dir();
        provider.open("/parent", dir_flags).unwrap();

        let file_flags = OpenFlags::create_file();
        let handle = provider.open("/parent/child.txt", file_flags).unwrap();
        provider.close(handle.id()).unwrap();

        let result = provider.remove("/parent");
        assert!(matches!(result, Err(FsError::DirectoryNotEmpty(_))));

        provider.remove("/parent/child.txt").unwrap();
        provider.remove("/parent").unwrap();
    }

    #[test]
    fn truncate_via_wstat() {
        let provider = KvProvider::new(KvConfig::default());

        let flags = OpenFlags::create_file();
        let handle = provider.open("/truncate.txt", flags).unwrap();
        provider
            .write(handle.id(), 0, b"long content here")
            .unwrap();
        provider.close(handle.id()).unwrap();

        provider.wstat("/truncate.txt", None, Some(5)).unwrap();

        let info = provider.stat("/truncate.txt").unwrap();
        assert_eq!(info.size, 5);
    }
}
