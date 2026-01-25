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
├── fuse/             # FUSE adapter (stub)
├── sh9/              # Interactive shell for FS9
├── plugins/
│   ├── s3/           # S3-backed filesystem
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
| `FS9_JWT_SECRET` | *(empty)* | JWT secret for authentication. If not set, auth is disabled |
| `RUST_LOG` | *(none)* | Logging level: `error`, `warn`, `info`, `debug`, `trace` |

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
| If/Else | `if [...]; then ... fi` | `if [ $x -eq 1 ]; then echo yes; fi` |
| For Loop | `for ... in ...; do ... done` | `for i in 1 2 3; do echo $i; done` |
| While Loop | `while ...; do ... done` | `while [ $x -lt 5 ]; do x=$((x+1)); done` |
| Functions | `name() { ... }` | `greet() { echo "Hi $1"; }; greet World` |
| Background | `cmd &` | `sleep 10 &; jobs` |
| HTTP | `http get/post URL` | `http get http://api.example.com` |

### Built-in Commands

**File Operations:** `ls` (`-l`), `cat`, `mkdir`, `rm`, `mv`, `cp`, `stat`, `touch`, `truncate`, `pwd`, `cd`  
**Text Processing:** `echo`, `grep` (with `-E`), `wc` (`-l`/`-w`/`-c`), `head` (`-n`), `tail` (`-n`)  
**Control:** `true`, `false`, `exit`, `return`, `break`, `continue`, `local`, `export`, `test`/`[`  
**System:** `mount` (list mounts)  
**Advanced:** `http` (GET/POST), `sleep`, `jobs`, `wait`

---

## API Overview

FS9 exposes a 10-method REST API:

| Endpoint | Method | Description |
|----------|--------|-------------|
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
```

## License

MIT OR Apache-2.0
