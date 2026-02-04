# plugins KNOWLEDGE BASE

Dynamic filesystem plugins loaded as .so/.dylib via C FFI. Each plugin implements `FsProvider` through a `PluginVTable` of `unsafe extern "C" fn` pointers.

## STRUCTURE

```
plugins/
├── HOW_TO_BUILD_A_FILESYSTEM.md  # Plugin development guide
├── hellofs/src/lib.rs            # Template plugin — start here for new plugins (730 lines)
├── pagefs/src/lib.rs             # KV-backed FS, 16KB pages, Git-compatible (2012 lines)
├── pubsubfs/src/lib.rs           # Topic pub/sub as files (1227 lines)
├── streamfs/src/lib.rs           # Broadcast channels + ring buffers (955 lines)
└── kv/src/lib.rs                 # Simple key-value store (848 lines)
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Create new plugin | Copy `hellofs/` | Update Cargo.toml name, add to workspace members |
| Understand FFI pattern | `hellofs/src/lib.rs` bottom | `fs9_plugin_version()` + `fs9_plugin_vtable()` exports |
| Git-compatible FS ops | `pagefs/src/lib.rs` | Atomic rename, POSIX perms, uid/gid config |
| Pub/sub pattern | `pubsubfs/src/lib.rs` | Ring buffer history, broadcast to subscribers |
| Streaming pattern | `streamfs/src/lib.rs` | Broadcast channels, subscriber management |

## CONVENTIONS

- **Every plugin is a single `lib.rs`** — cdylib crate, no multi-file modules
- **Two mandatory exports**: `#[no_mangle] pub extern "C" fn fs9_plugin_version() -> u32` and `fs9_plugin_vtable() -> *const PluginVTable`
- **Provider struct behind `*mut c_void`**: `create_provider` → `Box::into_raw()`, `destroy_provider` → `Box::from_raw()`
- **FFI boundary pattern**: Each method casts `*mut c_void` → `&PluginStruct`, calls safe Rust logic, returns `CResult`
- **Error codes**: Return `FS9_OK` (0) or `FS9_ERR_*` negative codes from `sdk-ffi`
- **Plugin naming**: Crate = `fs9-plugin-{name}`, library output = `libfs9_plugin_{name}.so`
- **Config via JSON**: `create_provider(config: *const c_char, config_len: size_t)` — parse with `serde_json`
- **`#[allow(dead_code)]`** on some struct fields is acceptable (WIP fields in kv, streamfs)

## ANTI-PATTERNS

- **Never call async from FFI functions** — plugins use synchronous Rust internally (BTreeMap, HashMap, Mutex)
- **Never forget `destroy_provider`** — leaks the Box'd provider struct
- **Never return Rust types across FFI** — use `CFileInfo`, `CBytes`, `CResult` from `sdk-ffi`
- **Don't add plugin to Makefile** without also adding to workspace `Cargo.toml` members

## NEW PLUGIN CHECKLIST

1. Copy `hellofs/` → `plugins/myfs/`
2. Update `Cargo.toml`: name = `fs9-plugin-myfs`, lib type = `["cdylib"]`
3. Add `"plugins/myfs"` to root `Cargo.toml` workspace members
4. Implement provider struct + all 14 FFI callback functions
5. Add `cargo build --release -p fs9-plugin-myfs` to Makefile `plugins` target
6. Add `cp` command for the .so/.dylib in Makefile
7. Test: `make plugins && make server` → mount via API or sh9
