# FS9 PROJECT KNOWLEDGE BASE

**Generated:** 2026-02-05
**Branch:** master

## OVERVIEW

Plan 9-inspired distributed filesystem in Rust. 10-method `FsProvider` trait unifies all storage backends. Dynamic plugin system via C FFI. Includes HTTP server, FUSE adapter, and bash-like shell (sh9).

## STRUCTURE

```
fs9/
├── sdk/              # FsProvider trait + core types (Capabilities, FsError, FileInfo, Handle)
├── sdk-ffi/          # C FFI layer: PluginVTable, CFileInfo, CBytes, CResult, error codes
├── core/             # VFS: router, mount table, handle registry, plugin manager, built-in providers
├── config/           # YAML config loader with layered priority (files → env vars)
├── server/           # Axum HTTP server: REST API handlers, JWT auth, fs9-meta integration
├── meta/             # Token metadata service: user/namespace management, SQLite/Postgres backend
├── fuse/             # FUSE adapter: translates FUSE ops → FsProvider calls via HTTP client
├── sh9/              # Bash-like shell: lexer → parser → AST → evaluator, 68 test scripts
├── plugins/
│   ├── pagefs/       # KV-backed FS with 16KB pages, Git-compatible (atomic rename, POSIX perms)
│   ├── pubsubfs/     # Topic-based pub/sub as files (write=publish, tail -f=subscribe)
│   ├── streamfs/     # Broadcast channels + ring buffers for real-time streaming
│   ├── kv/           # Simple key-value store
│   └── hellofs/      # Minimal demo/template plugin
├── clients/
│   ├── rust/         # Fs9Client with builder pattern, async reqwest
│   └── python/       # Async Python client (aiohttp)
├── tests/            # E2E integration tests (require running server)
└── docs/             # Architecture docs, demo scripts
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add FsProvider method | `sdk/src/provider.rs` → `sdk-ffi/src/lib.rs` → every plugin | Must update trait, FFI vtable, AND all implementations |
| New built-in provider | `core/src/providers/` | Copy `memfs/`, register in `registry.rs` `default_registry()` |
| New plugin | `plugins/hellofs/` as template | See `plugins/HOW_TO_BUILD_A_FILESYSTEM.md` |
| REST endpoint | `server/src/api/handlers.rs` | Routes in `server/src/api/mod.rs` |
| Add sh9 built-in cmd | `sh9/src/eval.rs` | Search `"cmd_name"` pattern in eval dispatch |
| Config option | `config/src/types.rs` | YAML struct definitions with serde defaults |
| Auth changes | `server/src/auth.rs` | JWT middleware, `AuthState` |
| Meta integration | `server/src/meta_client.rs` | Token validation via fs9-meta service |
| Mount/path resolution | `core/src/mount.rs` | BTreeMap longest-prefix match |
| Handle lifecycle | `core/src/handle.rs` | Two-layer: VFS global → provider-local handles |
| FUSE behavior | `fuse/src/fs.rs` | `Fs9Fuse` implements `fuser::Filesystem` |

## CODE MAP

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| `FsProvider` | trait | `sdk/src/provider.rs` | Core contract: 10 async methods all backends implement |
| `FsError` | enum | `sdk/src/error.rs` | 16 error variants with HTTP status mapping |
| `Capabilities` | bitflags | `sdk/src/capabilities.rs` | READ, WRITE, CREATE, DELETE, TRUNCATE, RENAME, CHMOD... |
| `VfsRouter` | struct | `core/src/vfs.rs` | Routes ops through mount table to providers, checks caps |
| `MountTable` | struct | `core/src/mount.rs` | Path→provider mapping, longest prefix resolution |
| `HandleRegistry` | struct | `core/src/handle.rs` | Global handle IDs → provider association |
| `PluginManager` | struct | `core/src/plugin.rs` | Loads .so/.dylib, validates SDK version, manages lifecycle |
| `PluginProvider` | struct | `core/src/plugin.rs` | Wraps FFI vtable calls to implement FsProvider |
| `ProviderRegistry` | struct | `core/src/providers/registry.rs` | Factory pattern for built-in providers |
| `PluginVTable` | struct | `sdk-ffi/src/lib.rs` | C ABI function pointer table (14 callbacks) |
| `Shell` | struct | `sh9/src/shell.rs` | REPL loop, connects to FS9 server |
| `Fs9Fuse` | struct | `fuse/src/fs.rs` | FUSE filesystem impl, bridges to Fs9Client |

## CONVENTIONS

- **Workspace lints**: `unsafe_code = "warn"`, clippy `all + pedantic + nursery = "warn"`. CI fails on clippy warnings (`-D warnings`)
- **All operations are async** via `#[async_trait]`. Even built-in providers use async
- **Plugin FFI**: Every plugin exports `fs9_plugin_version()` and `fs9_plugin_vtable()`. Unsafe code confined to FFI boundary
- **Error handling**: `FsResult<T>` everywhere. Each `FsError` variant maps to an HTTP status code
- **Provider pattern**: Built-in = `impl FsProvider` directly. Plugin = `unsafe extern "C" fn` → `PluginProvider` wrapper
- **Handle two-layer**: VFS assigns global handle IDs, provider manages its own. Never leak provider handles to clients
- **Path rewriting**: VfsRouter always converts provider-relative paths back to absolute VFS paths in responses
- **Config loading**: Defaults → `/etc/fs9/` → `~/.config/fs9/` → `./fs9.yaml` → `FS9_CONFIG=` → env vars (highest priority)
- **Meta service required**: `FS9_META_ENDPOINTS` must be set for production. Use `FS9_SKIP_META_CHECK=1` only for testing
- **`#[allow(dead_code)]`** used on struct fields in 8 files — acceptable for WIP fields

## ANTI-PATTERNS

- **Never suppress unsafe warnings** without FFI justification. Unsafe code must stay within `plugin.rs`, `sdk-ffi/`, and plugin FFI boundary functions
- **Never use `as any`, `@ts-ignore` equivalents** — no `#[allow(clippy::*)]` to hide real issues
- **No `#[deny]` or `#[forbid]` attributes** in codebase — rely on workspace-level lints
- **Don't add FsProvider methods** without updating: (1) trait in sdk, (2) Box/Arc impls in provider.rs, (3) FFI vtable in sdk-ffi, (4) all 5 plugins, (5) VfsRouter, (6) server handlers
- **Don't test plugins in isolation without FFI** — plugins are .so libraries, not native Rust; test through server or E2E
- **FUSE tests require running server** in separate terminal — they're `#[ignored]` by default

## COMMANDS

```bash
make build              # Debug build all crates
make plugins            # Build plugins (release) → copy to ./plugins/
make test               # All tests (Rust + Python)
make test-unit          # Unit tests only (no E2E)
make test-e2e           # E2E (builds server first)
make lint               # clippy --workspace -D warnings
make fmt                # cargo fmt + ruff format
make check              # fmt + lint + test
make server             # RUST_LOG=info cargo run -p fs9-server (requires FS9_META_ENDPOINTS)
FS9_SKIP_META_CHECK=1 make server  # Dev mode without meta service
cargo test -p sh9       # sh9 tests (includes 68 integration scripts)
cargo test -p fs9-fuse --test integration -- --ignored  # FUSE tests (needs server)
```

## DEPENDENCY GRAPH

```
sdk ← sdk-ffi ← plugins/*
sdk ← core ← server
sdk ← clients/rust ← fuse
config ← server, fuse
```

All crates depend on `sdk`. Plugins depend on `sdk-ffi` (C ABI). Server and fuse depend on `core` and `config`. FUSE uses `clients/rust` (HTTP client), not core directly.

## NOTES

- **FUSE runs in its own tokio runtime** — `main()` is sync, creates runtime manually (not `#[tokio::main]`)
- **sh9 evaluator is 3152 lines** — largest file, handles all built-in commands, pipelines, job control
- **No CI config found** (.github/workflows absent) — run `make check` locally
- **pubsubfs is not in CLAUDE.md's plugin list** but IS in Cargo.toml and Makefile — it's a valid plugin
- **ProxyFs enables cross-server mounting** — has hop limit protection (`TooManyHops` error)
- **Plugin .so naming**: `libfs9_plugin_{name}.so` (Linux) / `.dylib` (macOS)
- **fs9-meta is required** — server exits on startup if `FS9_META_ENDPOINTS` is not set (unless `FS9_SKIP_META_CHECK=1`)
- **Key env vars**: `FS9_META_ENDPOINTS` (meta service URL), `FS9_META_KEY` (admin key), `FS9_SKIP_META_CHECK` (testing only)

## PERFORMANCE OPTIMIZATIONS (2026-02-05)

### Implemented

| Optimization | Location | Improvement |
|--------------|----------|-------------|
| HandleRegistry sharding | `core/src/handle.rs` | 64 shards reduce lock contention, ~5-10x read throughput |
| FFI spawn_blocking | `core/src/plugin.rs` | All FFI calls offloaded to blocking threads, prevents async starvation |
| MountTable O(log n) | `core/src/mount.rs` | BTreeMap range query replaces O(n) iteration |
| TokenCache (moka) | `server/src/token_cache.rs` | Bounded LRU cache with automatic TTL, 100K default capacity |
| Handle cleanup task | `server/src/namespace.rs` | Background task every 60s cleans stale handles per namespace |
| Request backpressure | `server/src/main.rs` | TimeoutLayer (30s default), ConcurrencyLimitLayer (1000 default) |
| Namespace lock opt | `server/src/namespace.rs` | Optimistic creation outside write lock |
| HandleMap compact | `server/src/state.rs` | HashSet<u64> replaces bidirectional HashMap<UUID,u64> |

### Completed (Breaking Change - SDK v2)

| Optimization | Location | Improvement |
|--------------|----------|-------------|
| FsProvider::open returns (Handle, FileInfo) | `sdk/src/provider.rs`, all providers/plugins | Eliminates redundant stat() call after open() |

**Breaking change notes:**
- SDK version bumped from 1 to 2 (`FS9_SDK_VERSION` in sdk-ffi)
- FFI `OpenFn` signature changed: added `out_info: *mut CFileInfo` parameter
- All plugins must be recompiled against SDK v2

### Config Options Added

```yaml
server:
  request_timeout_secs: 30    # Request timeout (optional)
  max_concurrent_requests: 1000  # Concurrent request limit (optional)
```
