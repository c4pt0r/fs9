# FS9 - Plan 9 Inspired Distributed File System

A modern distributed filesystem in Rust with a 10-method core API inspired by Plan 9.

## Features

- **Unified Interface**: All storage backends implement the same `FsProvider` trait
- **Dynamic Mounting**: Hot-plug storage backends at runtime
- **Plugin System**: Native C ABI for high-performance plugins
- **Cross-Server Mounting**: ProxyFS enables distributed namespace hierarchies
- **Client SDKs**: Rust and Python clients included
- **sh9 Shell**: Bash-like interactive shell for FS9

## Quick Start

```bash
# Build everything
make build

# Run the server
make server

# Run all tests
make test
```

## Project Structure

```
fs9/
├── sdk/              # Core types and FsProvider trait
├── sdk-ffi/          # C-compatible ABI for plugins
├── core/             # VFS router, mount table, handle registry
├── server/           # HTTP REST API server (Axum)
├── fuse/             # FUSE adapter for mounting FS9 as local filesystem
├── sh9/              # Interactive shell for FS9
├── plugins/
│   ├── pagefs/       # KV-backed filesystem (16KB pages, Git compatible)
│   ├── pubsubfs/     # Pub/Sub filesystem (topic-based messaging)
│   ├── streamfs/     # Streaming filesystem (real-time data fanout)
│   └── kv/           # Key-value store filesystem
├── clients/
│   ├── rust/         # Rust client SDK
│   └── python/       # Python client SDK
└── tests/            # E2E integration tests
```

---

## Deployment

### Prerequisites

- Rust 1.75+ (`rustup install stable`)
- Make (optional, for convenience commands)

### Building

```bash
# Build all components (debug)
cargo build --workspace

# Build in release mode (recommended for production)
cargo build --workspace --release

# Build specific components
cargo build -p fs9-server --release    # Server only
cargo build -p sh9 --release           # Shell only
```

Release binaries will be in `target/release/`.

---

## Server Deployment

### Running the Server

```bash
# Development mode (with logging)
RUST_LOG=info cargo run -p fs9-server

# Or use the built binary
./target/release/fs9-server
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FS9_HOST` | `0.0.0.0` | Host address to bind |
| `FS9_PORT` | `9999` | Port to listen on |
| `FS9_JWT_SECRET` | *(empty)* | JWT secret for authentication. **Required for multi-tenancy.** All API requests must include a valid JWT when set |
| `FS9_DANGER_SKIP_AUTH` | *(unset)* | Set to `1` to bypass all JWT checks. **Development/testing only — do NOT use in production.** All requests become anonymous admin in the default namespace |
| `FS9_PLUGIN_DIR` | *(none)* | Additional directory to load plugins from |
| `RUST_LOG` | *(none)* | Logging level: `error`, `warn`, `info`, `debug`, `trace` |

### Auto-Loading Plugins

The server automatically loads plugins from:
1. `FS9_PLUGIN_DIR` environment variable (if set)
2. `./plugins` directory (if exists)

Plugin files must be named `libfs9_plugin_<name>.so` (Linux) or `.dylib` (macOS).

```bash
# Copy plugin to ./plugins
cp target/debug/libfs9_plugin_pagefs.so ./plugins/

# Start server - plugins are auto-loaded
RUST_LOG=info ./target/release/fs9-server
# Output: Loaded plugins from ./plugins count=1
# Output: Available plugins plugins=["pagefs"]
```

### Examples

```bash
# Run on custom port
FS9_PORT=8080 ./target/release/fs9-server

# Run with authentication enabled
FS9_JWT_SECRET="your-secret-key" FS9_PORT=9999 ./target/release/fs9-server

# Run with debug logging
RUST_LOG=debug ./target/release/fs9-server
```

### Systemd Service (Production)

Create `/etc/systemd/system/fs9-server.service`:

```ini
[Unit]
Description=FS9 Distributed Filesystem Server
After=network.target

[Service]
Type=simple
User=fs9
Group=fs9
Environment="FS9_HOST=0.0.0.0"
Environment="FS9_PORT=9999"
Environment="FS9_JWT_SECRET=your-production-secret"
Environment="RUST_LOG=info"
ExecStart=/usr/local/bin/fs9-server
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
# Install binary
sudo cp target/release/fs9-server /usr/local/bin/

# Enable and start service
sudo systemctl daemon-reload
sudo systemctl enable fs9-server
sudo systemctl start fs9-server

# Check status
sudo systemctl status fs9-server
```

### Health Check

```bash
curl http://localhost:9999/health
```

---

## Client Deployment (sh9 Shell)

sh9 is an interactive shell for FS9 with bash-like syntax supporting variables, functions, pipelines, control flow, and more.

### Installation

```bash
# Build
cargo build -p sh9 --release

# Install to ~/.cargo/bin (or copy to your PATH)
cargo install --path sh9

# Or copy manually
sudo cp target/release/sh9 /usr/local/bin/
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FS9_SERVER_URL` | `http://localhost:9999` | FS9 server URL to connect to |

### Usage

```bash
# Start interactive REPL
sh9

# Execute a single command
sh9 -c "ls /; echo hello"

# Execute a script file
sh9 script.sh9

# Connect to specific server
FS9_SERVER_URL=http://myserver:9999 sh9
```

### Interactive Mode

```
$ sh9
sh9 - FS9 Shell v0.1.0
Type 'exit' to quit, 'help' for help.

sh9:/> ls
sh9:/> mkdir /mydir
sh9:/> echo "hello world" > /mydir/file.txt
sh9:/> cat /mydir/file.txt
hello world
sh9:/> exit
```

### sh9 Features

| Feature | Syntax | Example |
|---------|--------|---------|
| Variables | `$VAR`, `${VAR}` | `x=5; echo $x` |
| Arithmetic | `$((expr))` | `echo $((2 + 3))` |
| Pipelines | `cmd1 \| cmd2` | `ls \| grep txt` |
| Redirection | `>`, `>>`, `<` | `echo hi > file` |
| Background Jobs | `cmd &` | `tail -f /logs > /output &` |
| Job Control | `jobs`, `fg`, `bg`, `kill %N` | `jobs; kill %1` |
| If/Else | `if [...]; then ... fi` | `if [ $x -eq 1 ]; then echo yes; fi` |
| For Loop | `for ... in ...; do ... done` | `for i in 1 2 3; do echo $i; done` |
| While Loop | `while ...; do ... done` | `while [ $x -lt 5 ]; do x=$((x+1)); done` |
| Functions | `name() { ... }` | `greet() { echo "Hi $1"; }; greet World` |
| HTTP | `http get/post URL` | `http get http://api.example.com` |

### Background Jobs & Output Redirection

sh9 supports background jobs with full output redirection, perfect for real-time data streaming:

```bash
# Start a background subscriber
sh9:/> tail -f /pub/logs > /pub/logs_backup &
[1] Started

# Publish messages
sh9:/> echo "log 1" > /pub/logs
sh9:/> echo "log 2" > /pub/logs

# Check job status
sh9:/> jobs
[1] Running tail -f /pub/logs > /pub/logs_backup

# View captured output
sh9:/> cat /pub/logs_backup
log 1
log 2

# Terminate background job
sh9:/> kill %1
[1] Terminated tail -f /pub/logs > /pub/logs_backup
```

### Built-in Commands

**File Operations:** `ls` (`-l`), `cat`, `mkdir`, `rm`, `mv`, `cp`, `stat`, `touch`, `truncate`, `pwd`, `cd`
**Text Processing:** `echo`, `grep` (with `-E`), `wc` (`-l`/`-w`/`-c`), `head` (`-n`), `tail` (`-n`, `-f`)
**Control:** `true`, `false`, `exit`, `return`, `break`, `continue`, `local`, `export`, `test`/`[`
**Filesystem:** `mount` (list/create mounts), `lsfs` (list available filesystems), `plugin` (load/unload/list plugins)
**Job Control:** `jobs` (list jobs), `fg` (foreground), `bg` (background), `kill` (terminate jobs), `wait` (wait for completion)
**Advanced:** `http` (GET/POST), `sleep`

### Filesystem Management

```bash
# List available filesystems and current mounts
sh9:/> lsfs
Available filesystems:
  kv
  pagefs
  pubsubfs
  streamfs
  hellofs

Current mounts:
  /                    memfs

# List loaded plugins
sh9:/> plugin list
pagefs
pubsubfs

# Mount a filesystem
sh9:/> mount pagefs /page
mounted pagefs at /page

# Use it
sh9:/> echo "hello" > /page/test.txt
sh9:/> cat /page/test.txt
hello

# List all mounts
sh9:/> mount
/                    memfs
/page                pagefs

# Load a plugin manually (if not auto-loaded)
sh9:/> plugin load myplugin /path/to/libfs9_plugin_myplugin.so
loaded plugin 'myplugin': loaded

# Unload a plugin
sh9:/> plugin unload myplugin
unloaded plugin 'myplugin'
```

---

## PubSubFS Plugin

PubSubFS is a topic-based publish-subscribe filesystem. Everything is a file:

- **Write = Publish**: Writing to a topic file publishes a message
- **Read = Subscribe**: Reading from a topic file subscribes to messages
- **Auto-create**: Topics are automatically created on first write
- **Real-time**: Use `tail -f` for continuous message streaming

### Quick Example

```bash
# Mount PubSubFS
sh9:/> mount pubsubfs /pub

# Publish messages
sh9:/> echo "Hello World" > /pub/chat
sh9:/> echo "Second message" > /pub/chat

# Subscribe (read historical messages)
sh9:/> cat /pub/chat
Hello World
Second message

# Real-time subscription (background)
sh9:/> tail -f /pub/logs > /pub/logs_backup &
sh9:/> echo "log entry 1" > /pub/logs
sh9:/> echo "log entry 2" > /pub/logs
sh9:/> cat /pub/logs_backup
log entry 1
log entry 2
sh9:/> kill %1

# View topic info
sh9:/> cat /pub/chat.info
name: chat
subscribers: 0
messages: 2
ring_size: 100
created: 2026-01-29 10:30:00
modified: 2026-01-29 10:30:15

# Delete topic
sh9:/> rm /pub/chat
```

### Features

- **Multiple subscribers**: Broadcast messages to all active subscribers
- **Message history**: Ring buffer stores recent messages (configurable, default 100)
- **Pure text output**: Messages are output exactly as written (no added timestamps)
- **Background subscriptions**: Use `tail -f` with `&` for async processing

### Use Cases

- Log aggregation from multiple services
- Event notification system
- Inter-process communication
- Real-time data streaming
- Simple message queues

For detailed usage, see `plugins/pubsubfs/USAGE.md`.

---

## PageFS Plugin

PageFS is a KV-backed filesystem plugin optimized for Git operations. It provides:

- **16KB page-based storage** - Efficient for both small and large files
- **POSIX-compatible rename** - Atomic rename with proper conflict handling (required for Git lockfiles)
- **Configurable uid/gid** - Prevents Git safe.directory warnings
- **Correct timestamps** - Handles pre-1970 dates correctly

### Configuration

PageFS can be configured via JSON when mounted:

```json
{
  "uid": 1000,
  "gid": 1000
}
```

### Capabilities

| Capability | Supported |
|------------|-----------|
| Basic R/W | Yes |
| Truncate | Yes |
| Rename | Yes |
| Directories | Yes |
| Permissions | Yes |
| Timestamps | Yes |

---

## FUSE Mount

Mount FS9 as a local filesystem using FUSE. This enables using standard tools like `git`, `vim`, `grep`, etc. on FS9.

### Prerequisites

```bash
# Ubuntu/Debian
sudo apt install fuse3 libfuse3-dev

# macOS
brew install macfuse
```

### Building

```bash
cargo build -p fs9-fuse --release
```

### Usage

```bash
# Terminal 1: Start the server
RUST_LOG=info cargo run -p fs9-server

# Terminal 2: Mount FS9
mkdir -p /tmp/fs9-mount
./target/release/fs9-fuse /tmp/fs9-mount --server http://localhost:9999 --foreground

# Terminal 3: Use the filesystem
cd /tmp/fs9-mount
echo "Hello FUSE" > hello.txt
cat hello.txt
git init && git add . && git commit -m "init"
```

### Command Line Options

```
fs9-fuse <MOUNTPOINT> [OPTIONS]

Arguments:
  <MOUNTPOINT>  Directory to mount the filesystem

Options:
  -s, --server <URL>   FS9 server URL [default: http://localhost:9999]
  -f, --foreground     Run in foreground (don't daemonize)
  -o, --options <OPT>  FUSE mount options
  -h, --help           Print help
```

### Unmounting

```bash
# Linux
fusermount -u /tmp/fs9-mount

# macOS
umount /tmp/fs9-mount
```

### Git Workflow Example

```bash
# Mount FS9
mkdir -p /tmp/fs9-git
./target/release/fs9-fuse /tmp/fs9-git --server http://localhost:9999 -f &

# Create a git repository
cd /tmp/fs9-git
mkdir myproject && cd myproject
git init
echo "# My Project" > README.md
git add README.md
git commit -m "Initial commit"

# All git operations work
git branch feature
git checkout feature
echo "new feature" >> README.md
git add . && git commit -m "Add feature"
git checkout main
git merge feature

# Unmount when done
fusermount -u /tmp/fs9-git
```

---

## Multi-Tenancy & Authentication

FS9 supports multi-tenant isolation via JWT-based authentication and per-namespace state separation.

### Architecture

```
┌─────────────────────────────────────────────────┐
│                   FS9 Server                     │
│                                                  │
│  ┌──────────────────────────────────────────┐   │
│  │           Auth Middleware                  │   │
│  │   JWT → RequestContext { ns, user, roles } │   │
│  └──────────────┬───────────────────────────┘   │
│                 │                                 │
│  ┌──────────────▼───────────────────────────┐   │
│  │         NamespaceManager                   │   │
│  │                                            │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  │   │
│  │  │ ns:acme │  │ ns:beta │  │ ns:prod │  │   │
│  │  │ VfsRouter│  │ VfsRouter│  │ VfsRouter│  │   │
│  │  │ MountTbl │  │ MountTbl │  │ MountTbl │  │   │
│  │  │ Handles  │  │ Handles  │  │ Handles  │  │   │
│  │  └─────────┘  └─────────┘  └─────────┘  │   │
│  └──────────────────────────────────────────┘   │
│                                                  │
│  ┌──────────────────────────────────────────┐   │
│  │     PluginManager (shared, global)        │   │
│  │     .so loaded once, providers per-ns     │   │
│  └──────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
```

### Key Concepts

- **Namespace**: An isolated unit of state. Each namespace has its own mount table, file handles, and VFS router. Data in one namespace is invisible to another.
- **JWT Binding**: Each JWT token is bound to exactly one namespace via the `ns` claim. A token cannot access other namespaces.
- **RequestContext**: Extracted from the JWT by the auth middleware and carried through every request. Contains `ns`, `user_id`, and `roles`.
- **Shared Plugins**: Plugin libraries (`.so`) are loaded once globally. Provider instances are created per-namespace for isolation.

### JWT Claims

```json
{
  "sub": "user123",
  "ns": "acme",
  "roles": ["operator"],
  "iat": 1706900000,
  "exp": 1706903600
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `sub` | Yes | User/subject identifier |
| `ns` | Yes | Namespace this token is bound to |
| `roles` | No | Roles for authorization (`operator`, `admin`) |
| `exp` | Yes | Expiration timestamp |
| `iat` | Yes | Issued-at timestamp |

### Enabling Authentication

```bash
# Set JWT secret to enable auth (required for multi-tenancy)
FS9_JWT_SECRET="your-secret-key" ./target/release/fs9-server
```

When `FS9_JWT_SECRET` is set:
- All API requests (except `/health`) require a valid `Authorization: Bearer <token>` header
- Missing/invalid/expired tokens → **401 Unauthorized**
- Unknown namespace → **403 Forbidden**

### Generating Tokens (Example)

```bash
# Using a JWT library or CLI tool:
# Header: {"alg": "HS256", "typ": "JWT"}
# Payload: {"sub": "user1", "ns": "acme", "roles": ["operator"], "iat": ..., "exp": ...}
# Secret: your-secret-key

# Example with curl (assuming you have a token):
TOKEN="eyJhbGciOiJIUzI1NiJ9..."

curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:9999/api/v1/stat?path=/
```

### Namespace Isolation Guarantees

| Isolation Type | Guarantee |
|---------------|-----------|
| **Data** | Tenant A writes `/file.txt` → Tenant B stat `/file.txt` returns not_found |
| **Handles** | Tenant A's file handle cannot be used by Tenant B |
| **Mounts** | Tenant A's mount table is invisible to Tenant B |
| **Readdir** | Each tenant only sees their own files |
| **Storage** | Keys are prefix-encoded with namespace for backend isolation |

### Roles & Permissions

| Role | Capabilities |
|------|-------------|
| *(none)* | Read/write files within namespace |
| `operator` | Mount/unmount filesystems, load plugins |
| `admin` | Namespace management, all operations |

---

## API Overview

FS9 exposes a 10-method REST API. All endpoints (except `/health`) require `Authorization: Bearer <JWT>` when auth is enabled.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check (no auth required) |
| `/api/v1/stat` | GET | Get file/directory metadata |
| `/api/v1/wstat` | POST | Modify metadata (chmod, truncate, rename) |
| `/api/v1/statfs` | GET | Get filesystem statistics |
| `/api/v1/open` | POST | Open file or create file/directory |
| `/api/v1/read` | POST | Read from file handle |
| `/api/v1/write` | POST | Write to file handle |
| `/api/v1/close` | POST | Close file handle |
| `/api/v1/readdir` | GET | List directory contents |
| `/api/v1/remove` | DELETE | Delete file or empty directory |
| `/api/v1/capabilities` | GET | Query provider capabilities |
| `/api/v1/mounts` | GET | List mounts in current namespace |
| `/api/v1/mount` | POST | Mount a filesystem (operator/admin) |
| `/api/v1/plugin/list` | GET | List loaded plugins |
| `/api/v1/plugin/load` | POST | Load a plugin (admin) |
| `/api/v1/plugin/unload` | POST | Unload a plugin (admin) |

---

## Usage Examples

### Rust Client

```rust
use fs9_client::{Fs2Client, OpenFlags};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Fs2Client::new("http://localhost:9999")?;
    
    // Write a file
    client.write_file("/hello.txt", b"Hello, FS9!").await?;
    
    // Read it back
    let data = client.read_file("/hello.txt").await?;
    println!("{}", String::from_utf8_lossy(&data));
    
    Ok(())
}
```

### Python Client

```python
import asyncio
from fs9_client import Fs2Client

async def main():
    async with Fs2Client("http://localhost:9999") as client:
        # Write a file
        await client.write_file("/hello.txt", b"Hello from Python!")
        
        # Read it back
        data = await client.read_file("/hello.txt")
        print(data.decode())

asyncio.run(main())
```

---

## Development

```bash
# Setup Python environment
make setup-python

# Format code
make fmt

# Run linter
make lint

# Run all checks
make check

# Generate documentation
make doc-open

# Watch and rebuild on changes
make dev
```

## Testing

```bash
# Run all tests (Rust + Python)
make test

# Run only Rust tests
make test-rust

# Run only E2E tests
make test-e2e

# Run only Python tests
make test-python

# Run sh9 tests
cargo test -p sh9

# Run PageFS unit tests
cargo test -p fs9-plugin-pagefs
```

### FUSE Integration Tests

FUSE integration tests require a running FS9 server. They test real filesystem operations including Git workflows and bash pipes.

```bash
# Terminal 1: Start the server
RUST_LOG=info cargo run -p fs9-server

# Terminal 2: Run FUSE integration tests
cargo test -p fs9-fuse --test integration -- --ignored
```

#### Available Test Suites

| Test Suite | Command | Description |
|------------|---------|-------------|
| All FUSE tests | `cargo test -p fs9-fuse --test integration -- --ignored` | Run all 32 integration tests |
| Git tests | `cargo test -p fs9-fuse --test integration test_fuse_git -- --ignored` | Git init, commit, branch, stash, clone |
| Bash pipe tests | `cargo test -p fs9-fuse --test integration test_fuse_bash -- --ignored` | Pipes, redirects, grep, sed, awk, tee |
| Basic file tests | `cargo test -p fs9-fuse --test integration test_fuse_write -- --ignored` | Read, write, truncate, append |

#### Git E2E Tests

```bash
# Run with verbose output
cargo test -p fs9-fuse --test integration test_fuse_git -- --ignored --nocapture
```

Tests:
- `test_fuse_git_init_add_commit` - Basic git workflow
- `test_fuse_git_executable_preserved` - File permissions (755)
- `test_fuse_git_branch_and_checkout` - Branch operations
- `test_fuse_git_stash` - Stash and pop
- `test_fuse_git_clone_local` - Local clone

#### Bash Pipe E2E Tests

```bash
# Run with verbose output
cargo test -p fs9-fuse --test integration test_fuse_bash -- --ignored --nocapture
```

Tests:
- `test_fuse_bash_pipe_redirect` - `echo > file`, `echo >> file`
- `test_fuse_bash_pipe_grep` - `cat | grep`, `grep -c`
- `test_fuse_bash_pipe_sort_uniq` - `sort | uniq`
- `test_fuse_bash_pipe_wc` - `wc -l`, `wc -w`
- `test_fuse_bash_pipe_head_tail` - `head`, `tail`, `head | tail`
- `test_fuse_bash_pipe_sed_awk` - `sed`, `awk`
- `test_fuse_bash_pipe_tee` - `tee` to multiple files
- `test_fuse_bash_pipe_xargs` - `ls | xargs cat`
- `test_fuse_bash_subshell_and_redirect` - `(cmd; cmd) > file`
- `test_fuse_bash_here_document` - `cat << EOF`

## License

MIT OR Apache-2.0
