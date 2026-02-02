# How to Build a FS9 Filesystem Plugin

This guide explains how to create a new filesystem plugin for FS9.

## Quick Start

```bash
# 1. Create plugin directory
mkdir -p plugins/myfs/src

# 2. Copy template from hellofs
cp plugins/hellofs/Cargo.toml plugins/myfs/
cp plugins/hellofs/src/lib.rs plugins/myfs/src/

# 3. Edit Cargo.toml - change package name
sed -i 's/hellofs/myfs/g' plugins/myfs/Cargo.toml

# 4. Add to workspace in root Cargo.toml
# members = [..., "plugins/myfs"]

# 5. Build
cargo build -p fs9-plugin-myfs --release

# 6. Load via API
curl -X POST http://localhost:9999/api/v1/plugin/load \
  -d '{"name":"myfs","path":"./target/release/libfs9_plugin_myfs.so"}'
```

## Plugin Structure

```
plugins/myfs/
├── Cargo.toml
└── src/
    └── lib.rs
```

### Cargo.toml Template

```toml
[package]
name = "fs9-plugin-myfs"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["cdylib", "rlib"]  # cdylib for .so, rlib for tests

[dependencies]
fs9-sdk = { path = "../../sdk" }
fs9-sdk-ffi = { path = "../../sdk-ffi" }
bytes.workspace = true
serde.workspace = true
serde_json.workspace = true
libc = "0.2"
```

## Required Exports

Your plugin MUST export these two functions:

```rust
#[no_mangle]
pub extern "C" fn fs9_plugin_version() -> u32 {
    FS9_SDK_VERSION  // from fs9_sdk_ffi
}

#[no_mangle]
pub extern "C" fn fs9_plugin_vtable() -> *const PluginVTable {
    &VTABLE  // static vtable with all callbacks
}
```

## VTable Callbacks

```rust
static PLUGIN_NAME: &[u8] = b"myplugin";
static PLUGIN_VERSION: &[u8] = b"0.1.0";

static VTABLE: PluginVTable = PluginVTable {
    sdk_version: FS9_SDK_VERSION,
    name: PLUGIN_NAME.as_ptr() as *const libc::c_char,
    name_len: PLUGIN_NAME.len(),
    version: PLUGIN_VERSION.as_ptr() as *const libc::c_char,
    version_len: PLUGIN_VERSION.len(),
    create: create_provider,
    destroy: destroy_provider,
    get_capabilities: get_caps,
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
```

## Minimal Implementation

```rust
use std::ptr;
use fs9_sdk::{Capabilities, FileInfo, FileType, FsError, FsResult, Handle, OpenFlags};
use fs9_sdk_ffi::*;
use libc::{c_char, c_void, size_t};

struct MyProvider {
    // Your state here
}

impl MyProvider {
    fn new() -> Self { Self {} }
    
    fn stat(&self, path: &str) -> FsResult<FileInfo> {
        if path == "/" {
            Ok(FileInfo {
                path: "/".to_string(),
                size: 0,
                file_type: FileType::Directory,
                mode: 0o755,
                ..Default::default()
            })
        } else {
            Err(FsError::not_found(path))
        }
    }
    
    // Implement other methods...
}

// FFI wrapper
unsafe extern "C" fn create_provider(_: *const c_char, _: size_t) -> *mut c_void {
    Box::into_raw(Box::new(MyProvider::new())) as *mut c_void
}

unsafe extern "C" fn destroy_provider(p: *mut c_void) {
    if !p.is_null() {
        drop(Box::from_raw(p as *mut MyProvider));
    }
}

unsafe extern "C" fn stat_fn(
    provider: *mut c_void,
    path: *const c_char,
    path_len: size_t,
    out: *mut CFileInfo,
) -> CResult {
    let provider = &*(provider as *const MyProvider);
    let path = std::str::from_utf8_unchecked(
        std::slice::from_raw_parts(path as *const u8, path_len)
    );
    
    match provider.stat(path) {
        Ok(info) => {
            (*out).size = info.size;
            (*out).file_type = FILE_TYPE_DIRECTORY;
            (*out).mode = info.mode;
            CResult { code: FS9_OK, error_msg: ptr::null(), error_msg_len: 0 }
        }
        Err(_) => CResult { 
            code: FS9_ERR_NOT_FOUND, 
            error_msg: ptr::null(), 
            error_msg_len: 0 
        }
    }
}
```

## Capabilities

Return a bitmask of supported operations:

```rust
unsafe extern "C" fn get_capabilities(_: *mut c_void) -> u64 {
    (Capabilities::READ 
     | Capabilities::WRITE 
     | Capabilities::CREATE 
     | Capabilities::DELETE 
     | Capabilities::DIRECTORY
    ).bits()
}
```

Available capabilities:
- `READ` - Can read files
- `WRITE` - Can write files
- `CREATE` - Can create files/dirs
- `DELETE` - Can delete files/dirs
- `DIRECTORY` - Supports directories
- `TRUNCATE` - Can truncate files
- `RENAME` - Can rename files
- `CHMOD/CHOWN` - Can change permissions

## Configuration

Plugins receive JSON config on creation:

```rust
#[derive(Deserialize, Default)]
struct MyConfig {
    #[serde(default)]
    option1: String,
    #[serde(default = "default_size")]
    buffer_size: usize,
}

fn default_size() -> usize { 1024 }

unsafe extern "C" fn create_provider(cfg: *const c_char, len: size_t) -> *mut c_void {
    let config: MyConfig = if cfg.is_null() || len == 0 {
        MyConfig::default()
    } else {
        let slice = std::slice::from_raw_parts(cfg as *const u8, len);
        serde_json::from_slice(slice).unwrap_or_default()
    };
    
    Box::into_raw(Box::new(MyProvider::new(config))) as *mut c_void
}
```

Mount with config:
```bash
curl -X POST http://localhost:9999/api/v1/mount \
  -d '{"path":"/myfs","provider":"myfs","config":{"option1":"value"}}'
```

## Error Handling

Use predefined error codes:

```rust
fn fserror_to_code(err: &FsError) -> i32 {
    match err {
        FsError::NotFound(_) => FS9_ERR_NOT_FOUND,
        FsError::AlreadyExists(_) => FS9_ERR_ALREADY_EXISTS,
        FsError::IsDirectory(_) => FS9_ERR_IS_DIRECTORY,
        FsError::NotDirectory(_) => FS9_ERR_NOT_DIRECTORY,
        FsError::InvalidHandle(_) => FS9_ERR_INVALID_HANDLE,
        FsError::PermissionDenied(_) => FS9_ERR_PERMISSION_DENIED,
        _ => FS9_ERR_INTERNAL,
    }
}
```

## Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_lifecycle() {
        unsafe {
            let p = create_provider(ptr::null(), 0);
            assert!(!p.is_null());
            destroy_provider(p);
        }
    }

    #[test]
    fn test_stat_root() {
        let provider = MyProvider::new(MyConfig::default());
        let info = provider.stat("/").unwrap();
        assert_eq!(info.file_type, FileType::Directory);
    }
}
```

Run tests:
```bash
cargo test -p fs9-plugin-myfs
```

## Examples

| Plugin | Description | Key Features |
|--------|-------------|--------------|
| `hellofs` | Demo filesystem | Virtual files, simple config |
| `streamfs` | Streaming FS | Broadcast channels, ring buffer |
| `kv` | Key-value store | BTreeMap storage, nested dirs |

## Checklist

- [ ] Cargo.toml with `crate-type = ["cdylib", "rlib"]`
- [ ] Export `fs9_plugin_version()` returning `FS9_SDK_VERSION`
- [ ] Export `fs9_plugin_vtable()` returning static vtable
- [ ] Implement all vtable callbacks (can return NOT_IMPLEMENTED)
- [ ] Handle null pointers in all FFI functions
- [ ] Add to workspace members in root Cargo.toml
- [ ] Write tests for core functionality
- [ ] Build with `--release` for .so file
