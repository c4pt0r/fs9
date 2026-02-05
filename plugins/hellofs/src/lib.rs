#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ptr;
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

use bytes::Bytes;
use fs9_sdk::{Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FS9_ERR_INVALID_HANDLE, FS9_ERR_IS_DIRECTORY,
    FS9_ERR_NOT_DIRECTORY, FS9_ERR_NOT_FOUND, FS9_OK, FS9_SDK_VERSION,
};
use libc::{c_char, c_void, size_t};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct HelloConfig {
    #[serde(default = "default_greeting")]
    greeting: String,
}

fn default_greeting() -> String {
    "Hello, World!".to_string()
}

impl Default for HelloConfig {
    fn default() -> Self {
        Self {
            greeting: default_greeting(),
        }
    }
}

#[derive(Debug, Clone)]
struct HelloFile {
    data: Bytes,
    mtime: SystemTime,
}

struct HelloProvider {
    greeting: String,
    files: RwLock<HashMap<String, HelloFile>>,
    handles: Mutex<HashMap<u64, (String, OpenFlags)>>,
    next_handle: Mutex<u64>,
}

impl HelloProvider {
    fn new(config: HelloConfig) -> Self {
        Self {
            greeting: config.greeting,
            files: RwLock::new(HashMap::new()),
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
                etag: String::new(),
                symlink_target: None,
            });
        }

        if path == "/hello" {
            let msg = format!("{}\n", self.greeting);
            return Ok(FileInfo {
                path: "/hello".to_string(),
                size: msg.len() as u64,
                file_type: FileType::Regular,
                mode: 0o444,
                uid: 0,
                gid: 0,
                atime: SystemTime::now(),
                mtime: SystemTime::now(),
                ctime: SystemTime::now(),
                etag: "virtual".to_string(),
                symlink_target: None,
            });
        }

        let files = self.files.read().unwrap();
        files
            .get(&path)
            .map(|f| FileInfo {
                path: path.clone(),
                size: f.data.len() as u64,
                file_type: FileType::Regular,
                mode: 0o644,
                uid: 0,
                gid: 0,
                atime: f.mtime,
                mtime: f.mtime,
                ctime: f.mtime,
                etag: String::new(),
                symlink_target: None,
            })
            .ok_or_else(|| FsError::not_found(&path))
    }

    fn open(&self, path: &str, flags: OpenFlags) -> FsResult<(Handle, FileInfo)> {
        let path = self.normalize_path(path);

        if path == "/" {
            if !flags.directory {
                return Err(FsError::is_directory(&path));
            }
        } else if path == "/hello" {
            if flags.write {
                return Err(FsError::permission_denied("cannot write to /hello"));
            }
        } else if flags.create {
            let mut files = self.files.write().unwrap();
            if !files.contains_key(&path) {
                files.insert(
                    path.clone(),
                    HelloFile {
                        data: Bytes::new(),
                        mtime: SystemTime::now(),
                    },
                );
            }
        } else {
            let files = self.files.read().unwrap();
            if !files.contains_key(&path) {
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

        if path == "/" {
            return Err(FsError::is_directory(path));
        }

        if path == "/hello" {
            let msg = format!("{}\n", self.greeting);
            let data = Bytes::from(msg);
            let start = (offset as usize).min(data.len());
            let end = (start + size).min(data.len());
            return Ok(data.slice(start..end));
        }

        let files = self.files.read().unwrap();
        let file = files.get(path).ok_or_else(|| FsError::not_found(path))?;

        let start = (offset as usize).min(file.data.len());
        let end = (start + size).min(file.data.len());
        Ok(file.data.slice(start..end))
    }

    fn write(&self, handle: u64, offset: u64, data: &[u8]) -> FsResult<usize> {
        let handles = self.handles.lock().unwrap();
        let (path, flags) = handles
            .get(&handle)
            .ok_or_else(|| FsError::invalid_handle(handle))?
            .clone();
        drop(handles);

        if path == "/" {
            return Err(FsError::is_directory(&path));
        }

        if path == "/hello" {
            return Err(FsError::permission_denied("cannot write to /hello"));
        }

        let mut files = self.files.write().unwrap();
        let file = files
            .get_mut(&path)
            .ok_or_else(|| FsError::not_found(&path))?;

        let offset = if flags.append {
            file.data.len()
        } else {
            offset as usize
        };

        let mut buf = file.data.to_vec();
        if offset + data.len() > buf.len() {
            buf.resize(offset + data.len(), 0);
        }
        buf[offset..offset + data.len()].copy_from_slice(data);
        file.data = Bytes::from(buf);
        file.mtime = SystemTime::now();

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

        if path != "/" {
            return Err(FsError::not_directory(&path));
        }

        let files = self.files.read().unwrap();
        let mut entries = vec![FileInfo {
            path: "/hello".to_string(),
            size: self.greeting.len() as u64 + 1,
            file_type: FileType::Regular,
            mode: 0o444,
            uid: 0,
            gid: 0,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            etag: "virtual".to_string(),
            symlink_target: None,
        }];

        for (path, file) in files.iter() {
            entries.push(FileInfo {
                path: path.clone(),
                size: file.data.len() as u64,
                file_type: FileType::Regular,
                mode: 0o644,
                uid: 0,
                gid: 0,
                atime: file.mtime,
                mtime: file.mtime,
                ctime: file.mtime,
                etag: String::new(),
                symlink_target: None,
            });
        }

        Ok(entries)
    }

    fn remove(&self, path: &str) -> FsResult<()> {
        let path = self.normalize_path(path);

        if path == "/" {
            return Err(FsError::permission_denied("cannot remove root"));
        }

        if path == "/hello" {
            return Err(FsError::permission_denied("cannot remove /hello"));
        }

        self.files
            .write()
            .unwrap()
            .remove(&path)
            .map(|_| ())
            .ok_or_else(|| FsError::not_found(&path))
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
        FsError::NotDirectory(_) => FS9_ERR_NOT_DIRECTORY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        FsError::PermissionDenied(_) => fs9_sdk_ffi::FS9_ERR_PERMISSION_DENIED,
        _ => fs9_sdk_ffi::FS9_ERR_INTERNAL,
    }
}

unsafe extern "C" fn create_provider(config: *const c_char, config_len: size_t) -> *mut c_void {
    let config: HelloConfig = if config.is_null() || config_len == 0 {
        HelloConfig::default()
    } else {
        let slice = std::slice::from_raw_parts(config as *const u8, config_len);
        serde_json::from_slice(slice).unwrap_or_default()
    };

    let provider = Box::new(HelloProvider::new(config));
    Box::into_raw(provider) as *mut c_void
}

unsafe extern "C" fn destroy_provider(provider: *mut c_void) {
    if !provider.is_null() {
        drop(Box::from_raw(provider as *mut HelloProvider));
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

    let provider = &*(provider as *const HelloProvider);
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

    const MB: u64 = 1024 * 1024;
    (*out_stats).total_bytes = 64 * MB;
    (*out_stats).free_bytes = 60 * MB;
    (*out_stats).total_inodes = 10_000;
    (*out_stats).free_inodes = 9_000;
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

    let provider = &*(provider as *const HelloProvider);
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

    let provider = &*(provider as *const HelloProvider);

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

    let provider = &*(provider as *const HelloProvider);
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

    let provider = &*(provider as *const HelloProvider);

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

    let provider = &*(provider as *const HelloProvider);
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

    let provider = &*(provider as *const HelloProvider);
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

static PLUGIN_NAME: &[u8] = b"hellofs";
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
    fn read_virtual_hello_file() {
        let provider = HelloProvider::new(HelloConfig::default());

        let flags = OpenFlags::read();
        let handle = provider.open("/hello", flags).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"Hello, World!\n");
        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn custom_greeting() {
        let config = HelloConfig {
            greeting: "Hi there!".to_string(),
        };
        let provider = HelloProvider::new(config);

        let flags = OpenFlags::read();
        let handle = provider.open("/hello", flags).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"Hi there!\n");
        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn create_and_read_file() {
        let provider = HelloProvider::new(HelloConfig::default());

        let flags = OpenFlags::create_file();
        let handle = provider.open("/test.txt", flags).unwrap();
        provider.write(handle.id(), 0, b"test data").unwrap();
        provider.close(handle.id()).unwrap();

        let flags = OpenFlags::read();
        let handle = provider.open("/test.txt", flags).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"test data");
        provider.close(handle.id()).unwrap();
    }

    #[test]
    fn readdir_includes_hello() {
        let provider = HelloProvider::new(HelloConfig::default());

        let entries = provider.readdir("/").unwrap();
        assert!(entries.iter().any(|e| e.path == "/hello"));
    }

    #[test]
    fn cannot_write_to_hello() {
        let provider = HelloProvider::new(HelloConfig::default());

        let flags = OpenFlags {
            read: false,
            write: true,
            create: false,
            truncate: false,
            append: false,
            directory: false,
        };
        let result = provider.open("/hello", flags);
        assert!(result.is_err());
    }

    #[test]
    fn cannot_remove_hello() {
        let provider = HelloProvider::new(HelloConfig::default());
        let result = provider.remove("/hello");
        assert!(result.is_err());
    }
}
