#![allow(missing_docs)]
#![allow(clippy::missing_safety_doc)]

use libc::{c_char, c_void, size_t};
use std::ffi::CStr;
use std::ptr;
use std::slice;

pub const FS9_SDK_VERSION: u32 = 1;

pub const FS9_OK: i32 = 0;
pub const FS9_ERR_NOT_FOUND: i32 = -1;
pub const FS9_ERR_PERMISSION_DENIED: i32 = -2;
pub const FS9_ERR_ALREADY_EXISTS: i32 = -3;
pub const FS9_ERR_INVALID_ARGUMENT: i32 = -4;
pub const FS9_ERR_NOT_DIRECTORY: i32 = -5;
pub const FS9_ERR_IS_DIRECTORY: i32 = -6;
pub const FS9_ERR_DIRECTORY_NOT_EMPTY: i32 = -7;
pub const FS9_ERR_INVALID_HANDLE: i32 = -8;
pub const FS9_ERR_INTERNAL: i32 = -9;
pub const FS9_ERR_NOT_IMPLEMENTED: i32 = -10;
pub const FS9_ERR_BACKEND_UNAVAILABLE: i32 = -11;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CFileInfo {
    pub path: *const c_char,
    pub path_len: size_t,
    pub size: u64,
    pub file_type: u8,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
}

impl Default for CFileInfo {
    fn default() -> Self {
        Self {
            path: ptr::null(),
            path_len: 0,
            size: 0,
            file_type: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
        }
    }
}

pub const FILE_TYPE_REGULAR: u8 = 0;
pub const FILE_TYPE_DIRECTORY: u8 = 1;
pub const FILE_TYPE_SYMLINK: u8 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CBytes {
    pub data: *const u8,
    pub len: size_t,
    pub cap: size_t,
}

impl Default for CBytes {
    fn default() -> Self {
        Self {
            data: ptr::null(),
            len: 0,
            cap: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CResult {
    pub code: i32,
    pub error_msg: *const c_char,
    pub error_msg_len: size_t,
}

impl CResult {
    pub fn ok() -> Self {
        Self {
            code: FS9_OK,
            error_msg: ptr::null(),
            error_msg_len: 0,
        }
    }

    pub fn err(code: i32, msg: *const c_char, len: size_t) -> Self {
        Self {
            code,
            error_msg: msg,
            error_msg_len: len,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct COpenFlags {
    pub read: u8,
    pub write: u8,
    pub create: u8,
    pub truncate: u8,
    pub append: u8,
    pub directory: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CStatChanges {
    pub has_mode: u8,
    pub mode: u32,
    pub has_uid: u8,
    pub uid: u32,
    pub has_gid: u8,
    pub gid: u32,
    pub has_size: u8,
    pub size: u64,
    pub has_atime: u8,
    pub atime: i64,
    pub has_mtime: u8,
    pub mtime: i64,
    pub has_name: u8,
    pub name: *const c_char,
    pub name_len: size_t,
    pub has_symlink_target: u8,
    pub symlink_target: *const c_char,
    pub symlink_target_len: size_t,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CFsStats {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub block_size: u32,
    pub max_name_len: u32,
}

pub type CreateProviderFn =
    unsafe extern "C" fn(config: *const c_char, config_len: size_t) -> *mut c_void;
pub type DestroyProviderFn = unsafe extern "C" fn(provider: *mut c_void);
pub type GetVersionFn = unsafe extern "C" fn() -> u32;
pub type GetCapabilitiesFn = unsafe extern "C" fn(provider: *mut c_void) -> u64;

pub type StatFn = unsafe extern "C" fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    out_info: *mut CFileInfo,
) -> CResult;

pub type WstatFn = unsafe extern "C" fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    changes: *const CStatChanges,
) -> CResult;

pub type StatfsFn = unsafe extern "C" fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    out_stats: *mut CFsStats,
) -> CResult;

pub type OpenFn = unsafe extern "C" fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    flags: *const COpenFlags,
    out_handle: *mut u64,
) -> CResult;

pub type ReadFn = unsafe extern "C" fn(
    provider: *mut c_void,
    handle: u64,
    offset: u64,
    size: size_t,
    out_data: *mut CBytes,
) -> CResult;

pub type WriteFn = unsafe extern "C" fn(
    provider: *mut c_void,
    handle: u64,
    offset: u64,
    data: *const u8,
    data_len: size_t,
    out_written: *mut size_t,
) -> CResult;

pub type CloseFn = unsafe extern "C" fn(provider: *mut c_void, handle: u64, sync: u8) -> CResult;

pub type ReaddirFn = unsafe extern "C" fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    callback: ReaddirCallback,
    user_data: *mut c_void,
) -> CResult;

pub type ReaddirCallback =
    unsafe extern "C" fn(info: *const CFileInfo, user_data: *mut c_void) -> i32;

pub type RemoveFn =
    unsafe extern "C" fn(provider: *mut c_void, path: *const c_char, path_len: size_t) -> CResult;

#[repr(C)]
pub struct PluginVTable {
    pub version: u32,
    pub create: CreateProviderFn,
    pub destroy: DestroyProviderFn,
    pub get_capabilities: GetCapabilitiesFn,
    pub stat: StatFn,
    pub wstat: WstatFn,
    pub statfs: StatfsFn,
    pub open: OpenFn,
    pub read: ReadFn,
    pub write: WriteFn,
    pub close: CloseFn,
    pub readdir: ReaddirFn,
    pub remove: RemoveFn,
}

pub unsafe fn str_from_c(ptr: *const c_char, len: size_t) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    let bytes = slice::from_raw_parts(ptr as *const u8, len);
    std::str::from_utf8(bytes).ok()
}

pub unsafe fn cstr_from_c(ptr: *const c_char) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

#[no_mangle]
pub extern "C" fn fs9_sdk_version() -> u32 {
    FS9_SDK_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn fs9_bytes_free(bytes: *mut CBytes) {
    if bytes.is_null() {
        return;
    }
    let bytes = &mut *bytes;
    if !bytes.data.is_null() && bytes.cap > 0 {
        let _ = Vec::from_raw_parts(bytes.data as *mut u8, bytes.len, bytes.cap);
    }
    bytes.data = ptr::null();
    bytes.len = 0;
    bytes.cap = 0;
}

pub fn vec_to_cbytes(v: Vec<u8>) -> CBytes {
    let len = v.len();
    let cap = v.capacity();
    let ptr = v.leak().as_ptr();
    CBytes {
        data: ptr,
        len,
        cap,
    }
}

pub fn fs_error_to_code(err: &fs9_sdk::FsError) -> i32 {
    use fs9_sdk::FsError;
    match err {
        FsError::NotFound(_) => FS9_ERR_NOT_FOUND,
        FsError::PermissionDenied(_) => FS9_ERR_PERMISSION_DENIED,
        FsError::AlreadyExists(_) => FS9_ERR_ALREADY_EXISTS,
        FsError::InvalidArgument(_) => FS9_ERR_INVALID_ARGUMENT,
        FsError::NotDirectory(_) => FS9_ERR_NOT_DIRECTORY,
        FsError::IsDirectory(_) => FS9_ERR_IS_DIRECTORY,
        FsError::DirectoryNotEmpty(_) => FS9_ERR_DIRECTORY_NOT_EMPTY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        FsError::NotImplemented(_) => FS9_ERR_NOT_IMPLEMENTED,
        FsError::BackendUnavailable(_) => FS9_ERR_BACKEND_UNAVAILABLE,
        _ => FS9_ERR_INTERNAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constant() {
        assert_eq!(fs9_sdk_version(), 1);
    }

    #[test]
    fn cresult_ok() {
        let result = CResult::ok();
        assert_eq!(result.code, FS9_OK);
        assert!(result.error_msg.is_null());
    }

    #[test]
    fn vec_to_cbytes_conversion() {
        let v = vec![1u8, 2, 3, 4, 5];
        let cb = vec_to_cbytes(v);
        assert_eq!(cb.len, 5);
        assert!(!cb.data.is_null());

        unsafe {
            let mut cb = cb;
            fs9_bytes_free(&mut cb);
            assert!(cb.data.is_null());
        }
    }

    #[test]
    fn error_code_mapping() {
        use fs9_sdk::FsError;

        assert_eq!(
            fs_error_to_code(&FsError::not_found("test")),
            FS9_ERR_NOT_FOUND
        );
        assert_eq!(
            fs_error_to_code(&FsError::permission_denied("test")),
            FS9_ERR_PERMISSION_DENIED
        );
        assert_eq!(
            fs_error_to_code(&FsError::already_exists("test")),
            FS9_ERR_ALREADY_EXISTS
        );
    }
}
