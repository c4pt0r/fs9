#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ptr;
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fs9_sdk::{Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FS9_ERR_ALREADY_EXISTS, FS9_ERR_INVALID_HANDLE,
    FS9_ERR_IS_DIRECTORY, FS9_ERR_NOT_FOUND, FS9_OK, FS9_SDK_VERSION,
};
use libc::{c_char, c_void, size_t};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct S3Config {
    #[serde(default = "default_bucket")]
    bucket: String,
    #[serde(default)]
    prefix: String,
}

fn default_bucket() -> String {
    "fs2-bucket".to_string()
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            bucket: default_bucket(),
            prefix: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct S3Object {
    data: Bytes,
    is_directory: bool,
    mode: u32,
    mtime: SystemTime,
}

struct S3Provider {
    #[allow(dead_code)]
    config: S3Config,
    objects: RwLock<HashMap<String, S3Object>>,
    handles: Mutex<HashMap<u64, (String, OpenFlags)>>,
    next_handle: Mutex<u64>,
}

impl S3Provider {
    fn new(config: S3Config) -> Self {
        let mut objects = HashMap::new();
        objects.insert(
            "/".to_string(),
            S3Object {
                data: Bytes::new(),
                is_directory: true,
                mode: 0o755,
                mtime: SystemTime::now(),
            },
        );

        Self {
            config,
            objects: RwLock::new(objects),
            handles: Mutex::new(HashMap::new()),
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
        let objects = self.objects.read().unwrap();

        objects
            .get(&path)
            .map(|obj| FileInfo {
                path: path.clone(),
                size: obj.data.len() as u64,
                file_type: if obj.is_directory {
                    FileType::Directory
                } else {
                    FileType::Regular
                },
                mode: obj.mode,
                uid: 0,
                gid: 0,
                atime: obj.mtime,
                mtime: obj.mtime,
                ctime: obj.mtime,
                etag: String::new(),
                symlink_target: None,
            })
            .ok_or_else(|| FsError::not_found(&path))
    }

    fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        let path = self.normalize_path(path);

        if flags.create {
            let mut objects = self.objects.write().unwrap();
            if !objects.contains_key(&path) {
                if flags.directory {
                    objects.insert(
                        path.clone(),
                        S3Object {
                            data: Bytes::new(),
                            is_directory: true,
                            mode: 0o755,
                            mtime: SystemTime::now(),
                        },
                    );
                } else {
                    let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/");
                    let parent_path = if parent.is_empty() { "/" } else { parent };
                    if !objects.contains_key(parent_path) {
                        return Err(FsError::not_found(parent_path));
                    }
                    objects.insert(
                        path.clone(),
                        S3Object {
                            data: Bytes::new(),
                            is_directory: false,
                            mode: 0o644,
                            mtime: SystemTime::now(),
                        },
                    );
                }
            }
        } else {
            let objects = self.objects.read().unwrap();
            if !objects.contains_key(&path) {
                return Err(FsError::not_found(&path));
            }
        }

        let mut next = self.next_handle.lock().unwrap();
        let handle_id = *next;
        *next += 1;

        self.handles
            .lock()
            .unwrap()
            .insert(handle_id, (path, flags));
        Ok(Handle::new(handle_id))
    }

    fn read(&self, handle: u64, offset: u64, size: usize) -> FsResult<Bytes> {
        let handles = self.handles.lock().unwrap();
        let (path, _) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?;

        let objects = self.objects.read().unwrap();
        let obj = objects.get(path).ok_or_else(|| FsError::not_found(path))?;

        if obj.is_directory {
            return Err(FsError::is_directory(path));
        }

        let start = (offset as usize).min(obj.data.len());
        let end = (start + size).min(obj.data.len());
        Ok(obj.data.slice(start..end))
    }

    fn write(&self, handle: u64, offset: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let (path, flags) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        let mut objects = self.objects.write().unwrap();
        let obj = objects
            .get_mut(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        if obj.is_directory {
            return Err(FsError::is_directory(&path));
        }

        let offset = if flags.append {
            obj.data.len()
        } else {
            offset as usize
        };

        let mut buf = obj.data.to_vec();
        if offset + data.len() > buf.len() {
            buf.resize(offset + data.len(), 0);
        }
        buf[offset..offset + data.len()].copy_from_slice(data);
        obj.data = Bytes::from(buf);
        obj.mtime = SystemTime::now();

        Ok(data.len())
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
        let objects = self.objects.read().unwrap();

        let obj = objects
            .get(&path)
            .ok_or_else(|| FsError::not_found(&path))?;
        if !obj.is_directory {
            return Err(FsError::not_directory(&path));
        }

        let prefix = if path == "/" {
            "/".to_string()
        } else {
            format!("{}/", path)
        };

        let entries: Vec<FileInfo> = objects
            .iter()
            .filter(|(k, _)| {
                if *k == &path || *k == "/" {
                    return false;
                }
                if !k.starts_with(&prefix) && !(path == "/" && k.starts_with('/')) {
                    return false;
                }
                let relative = if path == "/" {
                    &k[1..]
                } else {
                    &k[prefix.len()..]
                };
                !relative.contains('/')
            })
            .map(|(k, v)| FileInfo {
                path: k.clone(),
                size: v.data.len() as u64,
                file_type: if v.is_directory {
                    FileType::Directory
                } else {
                    FileType::Regular
                },
                mode: v.mode,
                uid: 0,
                gid: 0,
                atime: v.mtime,
                mtime: v.mtime,
                ctime: v.mtime,
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

        let mut objects = self.objects.write().unwrap();
        objects
            .remove(&path)
            .map(|_| ())
            .ok_or_else(|| FsError::not_found(&path))
    }

    fn wstat(&self, path: &str, mode: Option<u32>, size: Option<u64>) -> FsResult<()> {
        let path = self.normalize_path(path);
        let mut objects = self.objects.write().unwrap();

        let obj = objects
            .get_mut(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        if let Some(m) = mode {
            obj.mode = m;
        }
        if let Some(s) = size {
            let mut buf = obj.data.to_vec();
            buf.resize(s as usize, 0);
            obj.data = Bytes::from(buf);
        }
        obj.mtime = SystemTime::now();

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
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        _ => fs9_sdk_ffi::FS9_ERR_INTERNAL,
    }
}

unsafe extern "C" fn create_provider(config: *const c_char, config_len: size_t) -> *mut c_void {
    let config: S3Config = if config.is_null() || config_len == 0 {
        S3Config::default()
    } else {
        let slice = std::slice::from_raw_parts(config as *const u8, config_len);
        match serde_json::from_slice(slice) {
            Ok(c) => c,
            Err(_) => S3Config::default(),
        }
    };

    let provider = Box::new(S3Provider::new(config));
    Box::into_raw(provider) as *mut c_void
}

unsafe extern "C" fn destroy_provider(provider: *mut c_void) {
    if !provider.is_null() {
        drop(Box::from_raw(provider as *mut S3Provider));
    }
}

unsafe extern "C" fn get_capabilities(_provider: *mut c_void) -> u64 {
    Capabilities::BASIC_RW.bits()
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

    let provider = &*(provider as *const S3Provider);
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

    let provider = &*(provider as *const S3Provider);
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

    const GB: u64 = 1024 * 1024 * 1024;
    (*out_stats).total_bytes = 100 * GB;
    (*out_stats).free_bytes = 50 * GB;
    (*out_stats).total_inodes = 1_000_000;
    (*out_stats).free_inodes = 900_000;
    (*out_stats).block_size = 4096;
    (*out_stats).max_name_len = 1024;

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

    let provider = &*(provider as *const S3Provider);
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

    let provider = &*(provider as *const S3Provider);

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

    let provider = &*(provider as *const S3Provider);
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

    let provider = &*(provider as *const S3Provider);

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

    let provider = &*(provider as *const S3Provider);
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

    let provider = &*(provider as *const S3Provider);
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

static VTABLE: PluginVTable = PluginVTable {
    version: FS9_SDK_VERSION,
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
            assert_eq!((*vtable).version, FS9_SDK_VERSION);
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
        let provider = S3Provider::new(S3Config::default());

        let flags = OpenFlags::create_file();
        let handle = provider.open("/test.txt", flags).unwrap();

        provider.write(handle.id(), 0, b"hello world").unwrap();

        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"hello world");

        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn create_directory_and_list() {
        let provider = S3Provider::new(S3Config::default());

        let flags = OpenFlags::create_dir();
        provider.open("/mydir", flags).unwrap();

        let flags = OpenFlags::create_file();
        let handle = provider.open("/mydir/file.txt", flags).unwrap();
        provider.close(handle.id()).unwrap();

        let entries = provider.readdir("/mydir").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/mydir/file.txt");
    }

    #[test]
    fn stat_file() {
        let provider = S3Provider::new(S3Config::default());

        let flags = OpenFlags::create_file();
        let handle = provider.open("/stat_test.txt", flags).unwrap();
        provider.write(handle.id(), 0, b"test data").unwrap();
        provider.close(handle.id()).unwrap();

        let info = provider.stat("/stat_test.txt").unwrap();
        assert_eq!(info.size, 9);
        assert_eq!(info.file_type, FileType::Regular);
    }

    #[test]
    fn remove_file() {
        let provider = S3Provider::new(S3Config::default());

        let flags = OpenFlags::create_file();
        let handle = provider.open("/to_remove.txt", flags).unwrap();
        provider.close(handle.id()).unwrap();

        assert!(provider.stat("/to_remove.txt").is_ok());
        provider.remove("/to_remove.txt").unwrap();
        assert!(provider.stat("/to_remove.txt").is_err());
    }
}
