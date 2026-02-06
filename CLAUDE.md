# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

FS9 is a Plan 9-inspired distributed filesystem in Rust with a 10-method core API. The system uses a unified `FsProvider` trait that all storage backends implement, allowing dynamic mounting and hot-plugging of filesystem providers at runtime.

## Build & Development Commands

```bash
# Build everything (debug mode)
make build

# Build plugins and copy to ./plugins directory for auto-loading
make plugins

# Build in release mode
make release

# Run the server (development mode with logging)
make server
RUST_LOG=info cargo run -p fs9-server

# Run on custom port
FS9_PORT=8080 make server

# Build specific components
cargo build -p fs9-server        # Server only
cargo build -p sh9                # Shell only
cargo build -p fs9-fuse           # FUSE adapter
cargo build -p fs9-plugin-pagefs  # Single plugin
```

## Testing

```bash
# Run all tests (Rust + Python)
make test

# Run only Rust tests
make test-rust
cargo test --workspace

# Run unit tests only (no E2E)
make test-unit
cargo test --workspace --exclude fs9-tests

# Run E2E integration tests
make test-e2e
cargo test -p fs9-tests

# Run sh9 integration tests
cargo test -p sh9

# Run specific plugin tests
cargo test -p fs9-plugin-pagefs
cargo test -p fs9-plugin-streamfs

# FUSE integration tests (requires running server in another terminal)
# Terminal 1: RUST_LOG=info cargo run -p fs9-server
# Terminal 2:
cargo test -p fs9-fuse --test integration -- --ignored
cargo test -p fs9-fuse --test integration test_fuse_git -- --ignored --nocapture
cargo test -p fs9-fuse --test integration test_fuse_bash -- --ignored --nocapture

# Run single test
cargo test -p sh9 test_name
cargo test -p fs9-plugin-pagefs test_name
```

## Code Quality

```bash
# Format all code
make fmt
cargo fmt --all

# Run linter (fails on warnings)
make lint
cargo clippy --workspace --all-targets -- -D warnings

# Run all checks (fmt + lint + test)
make check

# Check formatting without modifying
make fmt-check
```

## Architecture

### Core Components

**sdk/** - Defines the `FsProvider` trait (10 methods: stat, wstat, statfs, open, read, write, close, readdir, remove, capabilities). This is the contract that all filesystem providers must implement.

**sdk-ffi/** - C-compatible FFI layer for plugin system. Plugins are loaded as dynamic libraries (.so/.dylib) via the `PluginVTable` structure.

**core/** - VFS implementation with four key modules:
- `vfs.rs` - VfsRouter routes operations to appropriate providers based on mount table
- `mount.rs` - MountTable manages path-to-provider mappings
- `handle.rs` - HandleRegistry tracks open file handles, associates them with providers
- `plugin.rs` - PluginManager loads/unloads dynamic libraries, bridges FFI to FsProvider trait
- `providers/` - Built-in providers organized by folder: memfs/, localfs/, proxyfs/, each with registry pattern

**server/** - HTTP REST API server (Axum). Exposes 10-method API as REST endpoints under `/api/v1/`. Auto-loads plugins from `./plugins/` or `FS9_PLUGIN_DIR`.

**fuse/** - FUSE filesystem adapter that exposes FS9 as a POSIX filesystem. Enables using standard tools (git, vim, grep) on FS9. Key modules:
- `fs.rs` - FUSE implementation translating FUSE ops to FsProvider calls
- `inode.rs` - Inode table mapping paths to inode numbers
- `handle.rs` - File handle management for FUSE

**sh9/** - Bash-like interactive shell for FS9 with variables, functions, pipelines, control flow, background jobs (&). Built-in commands include file operations (ls, cat, mkdir, rm, mv, cp), text processing (grep, wc, head, tail), job control (jobs, fg, bg, kill), and FS9-specific commands (mount, lsfs, plugin).

### Plugin System

Plugins implement `FsProvider` via C FFI. Each plugin exports:
- `fs9_plugin_version()` - Returns SDK version for compatibility check
- `fs9_plugin_vtable()` - Returns static `PluginVTable` with all callbacks

Plugin workflow:
1. PluginManager loads .so/.dylib using libloading
2. Validates SDK version matches
3. Reads plugin name from vtable
4. PluginProvider wraps FFI calls to implement FsProvider trait
5. VfsRouter routes operations through mount table to plugin instances

See `plugins/HOW_TO_BUILD_A_FILESYSTEM.md` for plugin development guide.

### Request Flow

Client request → Server HTTP handler → VfsRouter → MountTable.resolve(path) → Provider implementation → HandleRegistry (for file handles) → Response

For plugins: VfsRouter → PluginProvider → FFI vtable call → Plugin implementation

### Built-in Providers

- **LocalFs** - Direct filesystem access
- **MemoryFs** - In-memory BTreeMap storage
- **ProxyFs** - Remote FS9 server proxy (enables distributed namespaces)

### Plugin Providers

- **pagefs** - KV-backed filesystem with 16KB pages, Git-compatible with atomic rename, POSIX permissions
- **pubsubfs** - Pub/Sub filesystem with topic-based messaging, broadcast to multiple subscribers, ring buffer for history
- **streamfs** - Streaming filesystem with broadcast channels and ring buffers
- **kv** - Simple key-value store filesystem
- **hellofs** - Minimal demo filesystem for testing

## Common Workflows

### Running FS9 Server with Plugins

```bash
# Build plugins first
make plugins

# Start server (requires fs9-meta service)
FS9_META_ENDPOINTS=http://localhost:9998 RUST_LOG=info cargo run -p fs9-server

# Start server without meta service (development/testing only)
FS9_SKIP_META_CHECK=1 RUST_LOG=info cargo run -p fs9-server
# Output shows: "Loaded plugins from ./plugins count=4"
# Output shows: "Available plugins plugins=[\"pagefs\", \"streamfs\", \"kv\", \"hellofs\"]"

# Or specify custom plugin directory
FS9_PLUGIN_DIR=/path/to/plugins FS9_SKIP_META_CHECK=1 RUST_LOG=info cargo run -p fs9-server
```

### Using FUSE

```bash
# Terminal 1: Start server
RUST_LOG=info cargo run -p fs9-server

# Terminal 2: Mount via FUSE
mkdir -p /tmp/fs9-mount
cargo run -p fs9-fuse -- /tmp/fs9-mount --server http://localhost:9999 --foreground

# Terminal 3: Use standard tools
cd /tmp/fs9-mount
echo "test" > file.txt
git init && git add . && git commit -m "init"

# Unmount: fusermount -u /tmp/fs9-mount (Linux) or umount /tmp/fs9-mount (macOS)
```

### Using sh9 Shell

```bash
# Start interactive REPL
cargo run -p sh9

# In sh9:
sh9:/> lsfs                          # List available filesystems and mounts
sh9:/> mount pagefs /data            # Mount filesystem at path
sh9:/> echo "hello" > /data/test.txt
sh9:/> cat /data/test.txt
sh9:/> ls -l /data

# Run script file
cargo run -p sh9 -- script.sh9

# Execute single command
cargo run -p sh9 -- -c "ls /; echo done"
```

### Creating a New Plugin

```bash
# 1. Copy template
mkdir -p plugins/myfs/src
cp plugins/hellofs/Cargo.toml plugins/myfs/
cp plugins/hellofs/src/lib.rs plugins/myfs/src/

# 2. Update Cargo.toml name
sed -i 's/hellofs/myfs/g' plugins/myfs/Cargo.toml

# 3. Add to workspace members in root Cargo.toml
# members = [..., "plugins/myfs"]

# 4. Implement FsProvider in lib.rs (see plugins/HOW_TO_BUILD_A_FILESYSTEM.md)

# 5. Build and test
cargo build -p fs9-plugin-myfs --release
cargo test -p fs9-plugin-myfs

# 6. Load via API or copy to ./plugins/
cp target/release/libfs9_plugin_myfs.so ./plugins/
```

## Key Implementation Details

### Handle Management

File handles work in two layers:
1. **VFS layer**: VfsRouter's HandleRegistry assigns global handle IDs, tracks provider association
2. **Provider layer**: Each provider manages its own internal handles

When opening a file: VFS calls provider.open() → gets provider handle → registers in HandleRegistry → returns global handle to client. Read/write operations: VFS looks up provider from HandleRegistry → calls provider with provider's handle.

### Path Resolution

MountTable maintains a BTreeMap of mount points. For path "/data/file.txt":
1. Find longest prefix match (e.g., "/data" mounted to pagefs provider)
2. Extract relative path ("file.txt")
3. Return (provider, relative_path)

VfsRouter always rewrites returned FileInfo.path to absolute VFS path.

### Plugin Configuration

Plugins receive JSON config on creation. Example for pagefs:
```json
{"uid": 1000, "gid": 1000}
```

Configuration is parsed in the plugin's `create_provider()` FFI function using serde_json.

### Capabilities System

Each provider declares capabilities (READ, WRITE, CREATE, DELETE, DIRECTORY, TRUNCATE, RENAME, CHMOD, CHOWN, UTIME, SYMLINK). VfsRouter checks capabilities before routing operations, returns NOT_IMPLEMENTED if unsupported.

### Error Handling

All filesystem operations return `FsResult<T>` = `Result<T, FsError>`. Standard error types: NotFound, AlreadyExists, PermissionDenied, IsDirectory, NotDirectory, DirectoryNotEmpty, InvalidHandle, NotImplemented, Internal.

FFI plugins use error codes: FS9_OK, FS9_ERR_NOT_FOUND, FS9_ERR_ALREADY_EXISTS, etc. PluginProvider translates between Rust FsError and C error codes.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FS9_HOST` | `0.0.0.0` | Server bind address |
| `FS9_PORT` | `9999` | Server port |
| `FS9_JWT_SECRET` | *(empty)* | JWT secret for auth (disabled if not set) |
| `FS9_META_ENDPOINTS` | *(none)* | **Required.** URL of fs9-meta service for token validation |
| `FS9_META_KEY` | *(none)* | Admin key for fs9-meta service |
| `FS9_SKIP_META_CHECK` | *(none)* | Set to skip meta_url requirement (testing only) |
| `FS9_PLUGIN_DIR` | *(none)* | Additional plugin directory to auto-load |
| `FS9_SERVER_ENDPOINTS` | `http://localhost:9999` | Server URL for sh9/fuse/fs9-admin client |
| `RUST_LOG` | *(none)* | Logging level: error, warn, info, debug, trace |

## Workspace Structure

```
sdk/              FsProvider trait definition
sdk-ffi/          C FFI types and vtable for plugins
core/             VFS router, mount table, handle registry, plugin manager
config/           Configuration types
server/           HTTP REST API server (Axum)
fuse/             FUSE adapter
sh9/              Interactive shell
plugins/
  ├── pagefs/     Git-compatible filesystem with 16KB pages
  ├── streamfs/   Streaming filesystem
  ├── kv/         Key-value store
  └── hellofs/    Demo filesystem
clients/
  ├── rust/       Rust client SDK
  └── python/     Python client SDK
tests/            E2E integration tests
```

## Linting Configuration

Workspace uses strict linting:
- `unsafe_code = "warn"` - Warn on unsafe code (necessary for FFI in plugin.rs)
- Clippy pedantic + nursery modes enabled
- CI fails on clippy warnings (`-D warnings`)
