# core KNOWLEDGE BASE

VFS layer: routes filesystem operations through mount table to providers. Manages handles, plugins, and built-in provider registry.

## STRUCTURE

```
core/src/
├── vfs.rs                    # VfsRouter: impl FsProvider, routes via MountTable, checks Capabilities
├── mount.rs                  # MountTable: BTreeMap path→provider, longest-prefix resolution
├── handle.rs                 # HandleRegistry: two-layer handle mapping (VFS global ↔ provider-local)
├── plugin.rs                 # PluginManager + PluginProvider: FFI bridge, .so/.dylib loading (778 lines)
├── providers/
│   ├── mod.rs                # Re-exports all providers
│   ├── registry.rs           # ProviderRegistry + default_registry() factory
│   ├── memfs/mod.rs          # In-memory BTreeMap filesystem (737 lines)
│   ├── localfs/mod.rs        # Passthrough to host OS filesystem (488 lines)
│   └── proxyfs/mod.rs        # HTTP proxy to remote FS9 servers (554 lines)
└── lib.rs                    # Re-exports: VfsRouter, MountTable, HandleRegistry, PluginManager, providers
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Path resolution logic | `mount.rs` | `MountTable::resolve()` — BTreeMap longest prefix match |
| Capability checking | `vfs.rs` | VfsRouter checks caps before routing `wstat`, `open`, `remove` |
| Handle lifecycle | `handle.rs` | `register()` → `get()` → `close()`. Handles have TTL expiration |
| Plugin loading | `plugin.rs` | `PluginManager::load()` — libloading, version check, vtable extraction |
| FFI boundary | `plugin.rs` | `PluginProvider` wraps unsafe vtable calls as `impl FsProvider` |
| Add built-in provider | `registry.rs` | Add to `default_registry()`, create new subdir in `providers/` |
| ProxyFs hop limits | `proxyfs/mod.rs` | `X-Fs9-Hop-Count` header, `TooManyHops` error at max |

## CONVENTIONS

- **VfsRouter impl FsProvider** — the VFS itself is a provider, enabling recursive composition
- **Path rewriting**: VfsRouter converts provider-relative paths back to absolute VFS paths in every response
- **Handle two-layer**: VFS assigns monotonic `u64` handle IDs; provider manages its own opaque handles internally
- **Plugin unsafe code is contained** in `plugin.rs` only — all FFI calls go through `PluginProvider` methods
- **Built-in providers** follow the pattern: `pub struct XxxFs` + `#[async_trait] impl FsProvider for XxxFs`
- **Registry uses factory fns**: `fn(ProviderConfig) -> FsResult<Arc<dyn FsProvider>>`

## ANTI-PATTERNS

- **Never leak provider handles** — clients only see VFS-level handle IDs from `HandleRegistry`
- **Never bypass VfsRouter** for capability checks — always go through `VfsRouter::open/wstat/remove`
- **Don't add unsafe outside plugin.rs** — all FFI is centralized there
- **Don't create providers without registering** in `default_registry()` or plugin system
