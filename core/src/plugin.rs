//! Plugin loading and management for dynamic filesystem backends.
//!
//! This module provides the ability to load filesystem providers from dynamic
//! libraries (.so on Linux, .dylib on macOS, .dll on Windows).

use std::collections::HashMap;
use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::slice;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use fs9_sdk::{
    Capabilities, FileInfo, FileType, FsError, FsResult, FsStats, Handle, OpenFlags, StatChanges,
};
use fs9_sdk_ffi::{
    CBytes, CFileInfo, CFsStats, COpenFlags, CResult, CStatChanges, PluginVTable,
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FILE_TYPE_SYMLINK, FS9_ERR_ALREADY_EXISTS,
    FS9_ERR_DIRECTORY_NOT_EMPTY, FS9_ERR_INTERNAL, FS9_ERR_INVALID_ARGUMENT,
    FS9_ERR_INVALID_HANDLE, FS9_ERR_IS_DIRECTORY, FS9_ERR_NOT_DIRECTORY, FS9_ERR_NOT_FOUND,
    FS9_ERR_NOT_IMPLEMENTED, FS9_ERR_PERMISSION_DENIED, FS9_OK, FS9_SDK_VERSION,
};
use libc::c_void;
use libloading::{Library, Symbol};
use tracing::debug;

use fs9_sdk::FsProvider;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("failed to load library: {0}")]
    LoadError(String),

    #[error("symbol not found: {0}")]
    SymbolNotFound(String),

    #[error("version mismatch: plugin version {plugin} != SDK version {sdk}")]
    VersionMismatch { plugin: u32, sdk: u32 },

    #[error("plugin creation failed: {0}")]
    CreationFailed(String),

    #[error("plugin not found: {0}")]
    NotFound(String),

    #[error("plugin already loaded: {0}")]
    AlreadyLoaded(String),
}

struct LoadedPlugin {
    #[allow(dead_code)]
    library: Library,
    vtable: PluginVTable,
    name: String,
}

pub struct PluginManager {
    plugins: Mutex<HashMap<String, Arc<LoadedPlugin>>>,
}

impl PluginManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: Mutex::new(HashMap::new()),
        }
    }

    /// Load a plugin from a dynamic library path.
    ///
    /// # Safety
    ///
    /// The plugin library must export the required symbols with correct signatures.
    /// The caller must ensure the library is compatible with the current SDK version.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The library cannot be loaded
    /// - Required symbols are missing
    /// - Version check fails
    /// - A plugin with the same name is already loaded
    pub fn load(&self, name: &str, library_path: &Path) -> Result<(), PluginError> {
        self.load_internal(Some(name), library_path).map(|_| ())
    }

    pub fn load_from_path(&self, library_path: &Path) -> Result<String, PluginError> {
        self.load_internal(None, library_path)
    }

    fn load_internal(&self, name_override: Option<&str>, library_path: &Path) -> Result<String, PluginError> {
        debug!(path = ?library_path, "Loading plugin");

        let library = unsafe { Library::new(library_path) }
            .map_err(|e| PluginError::LoadError(e.to_string()))?;

        let get_version: Symbol<fs9_sdk_ffi::GetVersionFn> =
            unsafe { library.get(b"fs9_plugin_version\0") }
                .map_err(|_| PluginError::SymbolNotFound("fs9_plugin_version".to_string()))?;

        let plugin_version = unsafe { get_version() };
        if plugin_version != FS9_SDK_VERSION {
            return Err(PluginError::VersionMismatch {
                plugin: plugin_version,
                sdk: FS9_SDK_VERSION,
            });
        }

        let get_vtable: Symbol<unsafe extern "C" fn() -> *const PluginVTable> =
            unsafe { library.get(b"fs9_plugin_vtable\0") }
                .map_err(|_| PluginError::SymbolNotFound("fs9_plugin_vtable".to_string()))?;

        let vtable_ptr = unsafe { get_vtable() };
        if vtable_ptr.is_null() {
            return Err(PluginError::CreationFailed(
                "vtable pointer is null".to_string(),
            ));
        }

        let vtable = unsafe { ptr::read(vtable_ptr) };

        if vtable.sdk_version != FS9_SDK_VERSION {
            return Err(PluginError::VersionMismatch {
                plugin: vtable.sdk_version,
                sdk: FS9_SDK_VERSION,
            });
        }

        let name = if let Some(n) = name_override {
            n.to_string()
        } else {
            let name_ptr = vtable.name;
            let name_len = vtable.name_len;
            if name_ptr.is_null() || name_len == 0 {
                return Err(PluginError::CreationFailed("plugin name is null".to_string()));
            }
            unsafe {
                let bytes = slice::from_raw_parts(name_ptr as *const u8, name_len);
                std::str::from_utf8(bytes)
                    .map_err(|_| PluginError::CreationFailed("invalid plugin name".to_string()))?
                    .to_string()
            }
        };

        let version_str = {
            let ver_ptr = vtable.version;
            let ver_len = vtable.version_len;
            if !ver_ptr.is_null() && ver_len > 0 {
                unsafe {
                    let bytes = slice::from_raw_parts(ver_ptr as *const u8, ver_len);
                    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
                }
            } else {
                None
            }
        };

        let mut plugins = self.plugins.lock().unwrap();

        if plugins.contains_key(&name) {
            return Err(PluginError::AlreadyLoaded(name));
        }

        let loaded = Arc::new(LoadedPlugin {
            library,
            vtable,
            name: name.clone(),
        });

        plugins.insert(name.clone(), loaded);
        
        if let Some(ver) = version_str {
            debug!(name = %name, version = %ver, "Plugin loaded successfully");
        } else {
            debug!(name = %name, "Plugin loaded successfully");
        }

        Ok(name)
    }

    pub fn unload(&self, name: &str) -> Result<(), PluginError> {
        let mut plugins = self.plugins.lock().unwrap();

        if plugins.remove(name).is_some() {
            debug!(name = %name, "Plugin unloaded");
            Ok(())
        } else {
            Err(PluginError::NotFound(name.to_string()))
        }
    }

    pub fn create_provider(
        &self,
        plugin_name: &str,
        config: &str,
    ) -> Result<PluginProvider, PluginError> {
        let plugins = self.plugins.lock().unwrap();

        let plugin = plugins
            .get(plugin_name)
            .ok_or_else(|| PluginError::NotFound(plugin_name.to_string()))?
            .clone();

        drop(plugins);

        let config_cstr =
            CString::new(config).map_err(|e| PluginError::CreationFailed(e.to_string()))?;

        // Safety: We're calling FFI with valid arguments
        let provider_ptr = unsafe {
            (plugin.vtable.create)(config_cstr.as_ptr(), config.len())
        };

        if provider_ptr.is_null() {
            return Err(PluginError::CreationFailed(
                "provider creation returned null".to_string(),
            ));
        }

        Ok(PluginProvider {
            plugin,
            provider: provider_ptr,
        })
    }

    #[must_use]
    pub fn is_loaded(&self, name: &str) -> bool {
        self.plugins.lock().unwrap().contains_key(name)
    }

    #[must_use]
    pub fn loaded_plugins(&self) -> Vec<String> {
        self.plugins.lock().unwrap().keys().cloned().collect()
    }

    /// Load all plugins from a directory.
    ///
    /// Scans the directory for `.so` (Linux), `.dylib` (macOS), or `.dll` (Windows) files
    /// and attempts to load each one. The plugin name is read from the plugin's vtable.
    ///
    /// Returns the number of plugins successfully loaded.
    pub fn load_from_directory(&self, dir: &Path) -> usize {
        let mut loaded = 0;

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                debug!(path = ?dir, error = %e, "Failed to read plugin directory");
                return 0;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let extension = path.extension().and_then(|e| e.to_str());
            let is_plugin = matches!(extension, Some("so") | Some("dylib") | Some("dll"));
            if !is_plugin {
                continue;
            }

            match self.load_from_path(&path) {
                Ok(name) => {
                    debug!(name = %name, path = ?path, "Auto-loaded plugin");
                    loaded += 1;
                }
                Err(PluginError::AlreadyLoaded(_)) => {
                    // Skip already loaded plugins
                }
                Err(e) => {
                    debug!(path = ?path, error = %e, "Failed to load plugin");
                }
            }
        }

        loaded
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PluginProvider {
    plugin: Arc<LoadedPlugin>,
    provider: *mut c_void,
}

// Safety: The provider pointer is only accessed through synchronized FFI calls
unsafe impl Send for PluginProvider {}
unsafe impl Sync for PluginProvider {}

impl Drop for PluginProvider {
    fn drop(&mut self) {
        if !self.provider.is_null() {
            // Safety: We're calling the destroy function with the provider pointer
            unsafe {
                (self.plugin.vtable.destroy)(self.provider);
            }
        }
    }
}

impl PluginProvider {
    #[must_use]
    pub fn plugin_name(&self) -> &str {
        &self.plugin.name
    }
}

fn cresult_to_fserror(result: CResult) -> FsError {
    let msg = if !result.error_msg.is_null() && result.error_msg_len > 0 {
        unsafe {
            let bytes = slice::from_raw_parts(result.error_msg as *const u8, result.error_msg_len);
            String::from_utf8_lossy(bytes).into_owned()
        }
    } else {
        String::new()
    };

    match result.code {
        FS9_ERR_NOT_FOUND => FsError::not_found(msg),
        FS9_ERR_PERMISSION_DENIED => FsError::permission_denied(msg),
        FS9_ERR_ALREADY_EXISTS => FsError::already_exists(msg),
        FS9_ERR_INVALID_ARGUMENT => FsError::invalid_argument(msg),
        FS9_ERR_NOT_DIRECTORY => FsError::not_directory(msg),
        FS9_ERR_IS_DIRECTORY => FsError::is_directory(msg),
        FS9_ERR_DIRECTORY_NOT_EMPTY => FsError::directory_not_empty(msg),
        FS9_ERR_INVALID_HANDLE => FsError::invalid_handle(0),
        FS9_ERR_NOT_IMPLEMENTED => FsError::not_implemented(msg),
        FS9_ERR_INTERNAL | _ => FsError::internal(if msg.is_empty() {
            format!("plugin error code: {}", result.code)
        } else {
            msg
        }),
    }
}

fn cfileinfo_to_fileinfo(info: &CFileInfo) -> FileInfo {
    let path = if !info.path.is_null() && info.path_len > 0 {
        unsafe {
            let bytes = slice::from_raw_parts(info.path as *const u8, info.path_len);
            String::from_utf8_lossy(bytes).into_owned()
        }
    } else {
        String::new()
    };

    let file_type = match info.file_type {
        FILE_TYPE_DIRECTORY => FileType::Directory,
        FILE_TYPE_SYMLINK => FileType::Symlink,
        FILE_TYPE_REGULAR | _ => FileType::Regular,
    };

    let atime = timestamp_to_systemtime(info.atime);
    let mtime = timestamp_to_systemtime(info.mtime);
    let ctime = timestamp_to_systemtime(info.ctime);

    FileInfo {
        path,
        size: info.size,
        file_type,
        mode: info.mode,
        uid: info.uid,
        gid: info.gid,
        atime,
        mtime,
        ctime,
        etag: String::new(),
        symlink_target: None,
    }
}

fn timestamp_to_systemtime(timestamp: i64) -> SystemTime {
    if timestamp >= 0 {
        UNIX_EPOCH + Duration::from_secs(timestamp as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs((-timestamp) as u64)
    }
}

fn systemtime_to_timestamp(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

fn openflags_to_copenflags(flags: &OpenFlags) -> COpenFlags {
    COpenFlags {
        read: u8::from(flags.read),
        write: u8::from(flags.write),
        create: u8::from(flags.create),
        truncate: u8::from(flags.truncate),
        append: u8::from(flags.append),
        directory: u8::from(flags.directory),
    }
}

fn statchanges_to_cstatchanges(changes: &StatChanges) -> (CStatChanges, Option<CString>, Option<CString>) {
    let name_cstr = changes.name.as_ref().and_then(|s| CString::new(s.as_str()).ok());
    let symlink_cstr = changes.symlink_target.as_ref().and_then(|s| CString::new(s.as_str()).ok());

    let c_changes = CStatChanges {
        has_mode: u8::from(changes.mode.is_some()),
        mode: changes.mode.unwrap_or(0),
        has_uid: u8::from(changes.uid.is_some()),
        uid: changes.uid.unwrap_or(0),
        has_gid: u8::from(changes.gid.is_some()),
        gid: changes.gid.unwrap_or(0),
        has_size: u8::from(changes.size.is_some()),
        size: changes.size.unwrap_or(0),
        has_atime: u8::from(changes.atime.is_some()),
        atime: changes.atime.map_or(0, systemtime_to_timestamp),
        has_mtime: u8::from(changes.mtime.is_some()),
        mtime: changes.mtime.map_or(0, systemtime_to_timestamp),
        has_name: u8::from(name_cstr.is_some()),
        name: name_cstr.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        name_len: name_cstr.as_ref().map_or(0, |s| s.as_bytes().len()),
        has_symlink_target: u8::from(symlink_cstr.is_some()),
        symlink_target: symlink_cstr.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
        symlink_target_len: symlink_cstr.as_ref().map_or(0, |s| s.as_bytes().len()),
    };

    (c_changes, name_cstr, symlink_cstr)
}

fn cfsstats_to_fsstats(stats: &CFsStats) -> FsStats {
    FsStats {
        total_bytes: stats.total_bytes,
        free_bytes: stats.free_bytes,
        total_inodes: stats.total_inodes,
        free_inodes: stats.free_inodes,
        block_size: stats.block_size,
        max_name_len: stats.max_name_len,
    }
}

#[async_trait]
impl FsProvider for PluginProvider {
    async fn stat(&self, path: &str) -> FsResult<FileInfo> {
        let path_cstr = CString::new(path).map_err(|e| FsError::invalid_argument(e.to_string()))?;
        let mut out_info = CFileInfo::default();

        let result = unsafe {
            (self.plugin.vtable.stat)(
                self.provider,
                path_cstr.as_ptr(),
                path.len(),
                &mut out_info,
            )
        };

        if result.code == FS9_OK {
            Ok(cfileinfo_to_fileinfo(&out_info))
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn wstat(&self, path: &str, changes: StatChanges) -> FsResult<()> {
        let path_cstr = CString::new(path).map_err(|e| FsError::invalid_argument(e.to_string()))?;
        let (c_changes, _name_cstr, _symlink_cstr) = statchanges_to_cstatchanges(&changes);

        let result = unsafe {
            (self.plugin.vtable.wstat)(
                self.provider,
                path_cstr.as_ptr(),
                path.len(),
                &c_changes,
            )
        };

        if result.code == FS9_OK {
            Ok(())
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn statfs(&self, path: &str) -> FsResult<FsStats> {
        let path_cstr = CString::new(path).map_err(|e| FsError::invalid_argument(e.to_string()))?;
        let mut out_stats = CFsStats::default();

        let result = unsafe {
            (self.plugin.vtable.statfs)(
                self.provider,
                path_cstr.as_ptr(),
                path.len(),
                &mut out_stats,
            )
        };

        if result.code == FS9_OK {
            Ok(cfsstats_to_fsstats(&out_stats))
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
        let path_cstr = CString::new(path).map_err(|e| FsError::invalid_argument(e.to_string()))?;
        let c_flags = openflags_to_copenflags(&flags);
        let mut out_handle: u64 = 0;

        let result = unsafe {
            (self.plugin.vtable.open)(
                self.provider,
                path_cstr.as_ptr(),
                path.len(),
                &c_flags,
                &mut out_handle,
            )
        };

        if result.code == FS9_OK {
            Ok(Handle::new(out_handle))
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn read(&self, handle: &Handle, offset: u64, size: usize) -> FsResult<Bytes> {
        let mut out_data = CBytes::default();

        let result = unsafe {
            (self.plugin.vtable.read)(
                self.provider,
                handle.id(),
                offset,
                size,
                &mut out_data,
            )
        };

        if result.code == FS9_OK {
            if out_data.data.is_null() || out_data.len == 0 {
                return Ok(Bytes::new());
            }

            let bytes = unsafe {
                let data = slice::from_raw_parts(out_data.data, out_data.len);
                Bytes::copy_from_slice(data)
            };

            unsafe {
                fs9_sdk_ffi::fs9_bytes_free(&mut out_data);
            }

            Ok(bytes)
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn write(&self, handle: &Handle, offset: u64, data: Bytes) -> FsResult<usize> {
        let mut out_written: usize = 0;

        let result = unsafe {
            (self.plugin.vtable.write)(
                self.provider,
                handle.id(),
                offset,
                data.as_ptr(),
                data.len(),
                &mut out_written,
            )
        };

        if result.code == FS9_OK {
            Ok(out_written)
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn close(&self, handle: Handle, sync: bool) -> FsResult<()> {
        let result = unsafe {
            (self.plugin.vtable.close)(self.provider, handle.id(), u8::from(sync))
        };

        if result.code == FS9_OK {
            Ok(())
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn readdir(&self, path: &str) -> FsResult<Vec<FileInfo>> {
        let path_cstr = CString::new(path).map_err(|e| FsError::invalid_argument(e.to_string()))?;

        let entries: Arc<Mutex<Vec<FileInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let entries_ptr = Arc::into_raw(entries.clone()) as *mut c_void;

        unsafe extern "C" fn collect_entry(info: *const CFileInfo, user_data: *mut c_void) -> i32 {
            if info.is_null() || user_data.is_null() {
                return -1;
            }

            let entries = &*(user_data as *const Mutex<Vec<FileInfo>>);
            let file_info = cfileinfo_to_fileinfo(&*info);

            if let Ok(mut guard) = entries.lock() {
                guard.push(file_info);
                0
            } else {
                -1
            }
        }

        let result = unsafe {
            (self.plugin.vtable.readdir)(
                self.provider,
                path_cstr.as_ptr(),
                path.len(),
                collect_entry,
                entries_ptr,
            )
        };

        // Safety: Reclaims Arc that was leaked via into_raw above
        let entries = unsafe { Arc::from_raw(entries_ptr as *const Mutex<Vec<FileInfo>>) };

        if result.code == FS9_OK {
            let guard = entries.lock().unwrap();
            Ok(guard.clone())
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    async fn remove(&self, path: &str) -> FsResult<()> {
        let path_cstr = CString::new(path).map_err(|e| FsError::invalid_argument(e.to_string()))?;

        let result = unsafe {
            (self.plugin.vtable.remove)(self.provider, path_cstr.as_ptr(), path.len())
        };

        if result.code == FS9_OK {
            Ok(())
        } else {
            Err(cresult_to_fserror(result))
        }
    }

    fn capabilities(&self) -> Capabilities {
        let caps_bits = unsafe { (self.plugin.vtable.get_capabilities)(self.provider) };
        Capabilities::from_bits_truncate(caps_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_manager_new() {
        let manager = PluginManager::new();
        assert!(manager.loaded_plugins().is_empty());
    }

    #[test]
    fn plugin_manager_not_found() {
        let manager = PluginManager::new();
        let result = manager.unload("nonexistent");
        assert!(matches!(result, Err(PluginError::NotFound(_))));
    }

    #[test]
    fn plugin_manager_create_provider_not_found() {
        let manager = PluginManager::new();
        let result = manager.create_provider("nonexistent", "{}");
        assert!(matches!(result, Err(PluginError::NotFound(_))));
    }

    #[test]
    fn timestamp_conversion_positive() {
        let ts = 1_700_000_000i64;
        let st = timestamp_to_systemtime(ts);
        let back = systemtime_to_timestamp(st);
        assert_eq!(ts, back);
    }

    #[test]
    fn timestamp_conversion_zero() {
        let ts = 0i64;
        let st = timestamp_to_systemtime(ts);
        assert_eq!(st, UNIX_EPOCH);
    }

    #[test]
    fn openflags_conversion() {
        let flags = OpenFlags::create_file();
        let c_flags = openflags_to_copenflags(&flags);
        assert_eq!(c_flags.read, 1);
        assert_eq!(c_flags.write, 1);
        assert_eq!(c_flags.create, 1);
        assert_eq!(c_flags.truncate, 0);
        assert_eq!(c_flags.append, 0);
        assert_eq!(c_flags.directory, 0);
    }

    #[test]
    fn statchanges_conversion() {
        let changes = StatChanges::chmod(0o644);
        let (c_changes, _name, _symlink) = statchanges_to_cstatchanges(&changes);
        assert_eq!(c_changes.has_mode, 1);
        assert_eq!(c_changes.mode, 0o644);
        assert_eq!(c_changes.has_uid, 0);
        assert_eq!(c_changes.has_name, 0);
    }

    #[test]
    fn cresult_to_error_mapping() {
        let result = CResult {
            code: FS9_ERR_NOT_FOUND,
            error_msg: ptr::null(),
            error_msg_len: 0,
        };
        let err = cresult_to_fserror(result);
        assert!(err.is_not_found());

        let result = CResult {
            code: FS9_ERR_PERMISSION_DENIED,
            error_msg: ptr::null(),
            error_msg_len: 0,
        };
        let err = cresult_to_fserror(result);
        assert!(err.is_permission_denied());
    }

    #[test]
    fn cfsstats_conversion() {
        let c_stats = CFsStats {
            total_bytes: 1_000_000,
            free_bytes: 500_000,
            total_inodes: 10000,
            free_inodes: 5000,
            block_size: 4096,
            max_name_len: 255,
        };
        let stats = cfsstats_to_fsstats(&c_stats);
        assert_eq!(stats.total_bytes, 1_000_000);
        assert_eq!(stats.free_bytes, 500_000);
        assert_eq!(stats.block_size, 4096);
    }

    #[test]
    fn cfileinfo_conversion() {
        let path = b"/test/file.txt\0";
        let c_info = CFileInfo {
            path: path.as_ptr() as *const i8,
            path_len: path.len() - 1,
            size: 1024,
            file_type: FILE_TYPE_REGULAR,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            atime: 1_700_000_000,
            mtime: 1_700_000_100,
            ctime: 1_700_000_050,
        };
        let info = cfileinfo_to_fileinfo(&c_info);
        assert_eq!(info.path, "/test/file.txt");
        assert_eq!(info.size, 1024);
        assert_eq!(info.file_type, FileType::Regular);
        assert_eq!(info.mode, 0o644);
    }
}
