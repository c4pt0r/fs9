# FS9 - Plan 9 Inspired Distributed File System

A modern distributed filesystem in Rust with a 10-method core API inspired by Plan 9.

## Features

- **Unified Interface**: All storage backends implement the same `FsProvider` trait
- **Dynamic Mounting**: Hot-plug storage backends at runtime
- **Plugin System**: Native C ABI for high-performance plugins
- **Cross-Server Mounting**: ProxyFS enables distributed namespace hierarchies
- **Client SDKs**: Rust and Python clients included
- **sh9 Shell**: Bash-like interactive shell for FS9

## Performance

FS9 is optimized for high-throughput scenarios:

- **Sharded Handle Registry**: 64-shard lock-free reads for ~10x throughput under contention
- **O(log n) Path Resolution**: BTreeMap range queries for mount point lookup
- **Bounded Token Cache**: moka-based LRU with 100K capacity and automatic TTL
- **Request Backpressure**: Configurable timeouts (30s default) and concurrency limits (1000 default)
- **Async-Safe FFI**: Plugin calls offloaded to blocking thread pool
- **Optimistic Locking**: Namespace creation outside write lock

### Production Features

- **Graceful Shutdown**: SIGTERM/Ctrl+C signal handling with handle draining before exit
- **Per-Tenant Rate Limiting**: Governor-based token bucket with per-namespace (1000 QPS) and per-user (100 QPS) limits
- **Prometheus Metrics**: `GET /metrics` endpoint with request counters, latency histograms, and cache hit/miss stats
- **Token Revocation**: `POST /api/v1/auth/revoke` to immediately invalidate compromised tokens
- **Circuit Breaker**: Meta service calls protected with automatic CLOSED→OPEN→HALF_OPEN state machine and exponential backoff retry
- **Streaming File Transfer**: Full streaming I/O — writes consume body as stream (no OOM), reads use chunked transfer encoding
- **Stateless Download/Upload**: `GET /api/v1/download` with HTTP Range support (206 Partial Content), `PUT /api/v1/upload` for streaming uploads
- **Request Body Limits**: 2MB default for API requests, 256MB for file writes (configurable)
- **PostgreSQL Backend**: fs9-meta supports PostgreSQL for high-availability metadata storage (`cargo build -p fs9-meta --features postgres`)
- **OpenTelemetry Tracing**: Optional distributed tracing via OTLP exporter (`cargo build -p fs9-server --features otel`, set `OTEL_EXPORTER_OTLP_ENDPOINT`)
- **DashMap Namespace Manager**: Lock-free concurrent reads for namespace lookups

### Server Configuration

```yaml
server:
  request_timeout_secs: 30        # Request timeout (optional)
  max_concurrent_requests: 1000   # Max concurrent requests (optional)
  shutdown_timeout_secs: 30       # Graceful shutdown timeout (optional)
  max_body_size_bytes: 2097152    # Default body limit: 2MB (optional)
  max_write_size_bytes: 268435456 # Write endpoint limit: 256MB (optional)

  rate_limit:
    enabled: true
    namespace_qps: 1000           # Per-namespace requests/sec
    user_qps: 100                 # Per-user requests/sec

  metrics:
    enabled: true
    path: "/metrics"              # Prometheus scrape endpoint

  meta_resilience:
    failure_threshold: 5          # Failures before circuit opens
    recovery_timeout_secs: 30     # Time before half-open retry
    max_retry_attempts: 3         # Retry count with exponential backoff
    base_delay_ms: 100            # Base delay between retries
```

## Quick Start

```bash
# One-command quickstart (builds, starts server, creates namespace, launches shell)
./quickstart.sh

# Or manually:
make build              # Build everything
make plugins            # Build plugins
make server             # Run the server
```

### Using fs9-admin CLI

```bash
# Create a namespace with pagefs mounted at /
fs9-admin -s http://localhost:9999 --secret "$FS9_JWT_SECRET" \
  ns create myns --mount pagefs --set uid=1000 --set gid=1000

# Generate a token
TOKEN=$(fs9-admin -s http://localhost:9999 --secret "$FS9_JWT_SECRET" \
  token generate -u alice -n myns -q)

# Start sh9 shell
sh9 -s http://localhost:9999 -t "$TOKEN"

# Mount additional filesystems
fs9-admin mount add pagefs -n myns -p /data --set uid=1000
fs9-admin mount add memfs -n myns -p /tmp
fs9-admin mount list -n myns
```

## Project Structure

```
fs9/
├── sdk/              # Core types and FsProvider trait
├── sdk-ffi/          # C-compatible ABI for plugins
├── core/             # VFS router, mount table, handle registry
├── server/           # HTTP REST API server (Axum)
├── meta/             # Metadata service (namespaces, users, tokens, API keys)
├── fuse/             # FUSE adapter for mounting FS9 as local filesystem
├── sh9/              # Interactive shell for FS9
├── cli/              # Admin CLI (fs9-admin)
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

## Production Deployment Guide

This section provides a complete, step-by-step guide for deploying FS9 in a production environment.

### Deployment Overview

FS9 production deployment consists of three main components that must be deployed in order:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Deployment Order                              │
│                                                                  │
│  Step 1: Database        Step 2: fs9-meta       Step 3: fs9-server
│  ┌──────────────┐       ┌──────────────┐       ┌──────────────┐ │
│  │  PostgreSQL  │  ───► │   fs9-meta   │  ───► │  fs9-server  │ │
│  │  or SQLite   │       │  (metadata)  │       │  (main API)  │ │
│  └──────────────┘       └──────────────┘       └──────────────┘ │
│                                                                  │
│  Step 4: Configure       Step 5: Deploy         Step 6: Verify  │
│  ┌──────────────┐       ┌──────────────┐       ┌──────────────┐ │
│  │  Namespaces  │  ───► │   Clients    │  ───► │ Health Check │ │
│  │  & Plugins   │       │  (sh9/FUSE)  │       │ & Monitoring │ │
│  └──────────────┘       └──────────────┘       └──────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Step 1: Prepare the Environment

#### 1.1 Create System User

```bash
# Create dedicated user for FS9 services
sudo useradd -r -s /bin/false -m -d /var/lib/fs9 fs9

# Create required directories
sudo mkdir -p /etc/fs9
sudo mkdir -p /var/lib/fs9/{data,plugins,meta}
sudo mkdir -p /var/log/fs9

# Set ownership
sudo chown -R fs9:fs9 /var/lib/fs9 /var/log/fs9
sudo chown root:fs9 /etc/fs9
sudo chmod 750 /etc/fs9
```

#### 1.2 Install Binaries

```bash
# Build release binaries
cargo build --workspace --release

# Install binaries
sudo cp target/release/fs9-server /usr/local/bin/
sudo cp target/release/fs9-meta /usr/local/bin/
sudo cp target/release/fs9-admin /usr/local/bin/
sudo cp target/release/sh9 /usr/local/bin/

# Set permissions
sudo chmod 755 /usr/local/bin/fs9-*
sudo chmod 755 /usr/local/bin/sh9
```

#### 1.3 Install Plugins

```bash
# Build plugins in release mode
make plugins

# Copy plugins to system directory
sudo mkdir -p /usr/lib/fs9/plugins
sudo cp plugins/*.so /usr/lib/fs9/plugins/ 2>/dev/null || \
sudo cp plugins/*.dylib /usr/lib/fs9/plugins/ 2>/dev/null

# Set permissions
sudo chown -R root:fs9 /usr/lib/fs9
sudo chmod 755 /usr/lib/fs9/plugins/*
```

#### 1.4 Generate Secrets

```bash
# Generate JWT secret (store securely!)
JWT_SECRET=$(openssl rand -base64 32)
echo "JWT_SECRET: $JWT_SECRET"

# Generate meta admin key
META_KEY=$(openssl rand -base64 16)
echo "META_KEY: $META_KEY"

# Store secrets securely (example using file)
sudo tee /etc/fs9/secrets.env > /dev/null << EOF
FS9_JWT_SECRET=${JWT_SECRET}
FS9_META_KEY=${META_KEY}
EOF
sudo chmod 600 /etc/fs9/secrets.env
sudo chown fs9:fs9 /etc/fs9/secrets.env
```

### Step 2: Deploy fs9-meta (Metadata Service)

The metadata service manages namespaces, users, and tokens. It must be running before fs9-server starts.

#### 2.1 Configure fs9-meta

Create `/etc/fs9/meta.yaml`:

```yaml
server:
  host: "127.0.0.1"      # Bind to localhost only (fs9-server connects locally)
  port: 9998

database:
  # SQLite for single-node deployments
  dsn: "sqlite:/var/lib/fs9/meta/fs9-meta.db"
  
  # PostgreSQL for high availability (recommended for production)
  # dsn: "postgres://fs9:password@localhost:5432/fs9_meta"

auth:
  jwt_secret: "${FS9_JWT_SECRET}"  # Will be substituted from environment
```

#### 2.2 Create fs9-meta Systemd Service

Create `/etc/systemd/system/fs9-meta.service`:

```ini
[Unit]
Description=FS9 Metadata Service
Documentation=https://github.com/example/fs9
After=network.target postgresql.service
Wants=network.target

[Service]
Type=simple
User=fs9
Group=fs9
EnvironmentFile=/etc/fs9/secrets.env
Environment="RUST_LOG=info"
Environment="RUST_BACKTRACE=1"

ExecStart=/usr/local/bin/fs9-meta -c /etc/fs9/meta.yaml

WorkingDirectory=/var/lib/fs9/meta
StandardOutput=append:/var/log/fs9/meta.log
StandardError=append:/var/log/fs9/meta.log

Restart=always
RestartSec=5
StartLimitIntervalSec=60
StartLimitBurst=3

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/fs9/meta /var/log/fs9
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

#### 2.3 Start fs9-meta

```bash
# Reload systemd
sudo systemctl daemon-reload

# Enable and start fs9-meta
sudo systemctl enable fs9-meta
sudo systemctl start fs9-meta

# Verify it's running
sudo systemctl status fs9-meta
curl http://127.0.0.1:9998/health
```

### Step 3: Deploy fs9-server (Main API Server)

#### 3.1 Configure fs9-server

Create `/etc/fs9/fs9.yaml`:

```yaml
server:
  host: "0.0.0.0"
  port: 9999
  
  # Connect to local fs9-meta service
  meta_url: "http://127.0.0.1:9998"
  meta_key: "${FS9_META_KEY}"
  
  # Performance tuning
  request_timeout_secs: 30
  max_concurrent_requests: 2000
  
  auth:
    enabled: true
    jwt_secret: "${FS9_JWT_SECRET}"
  
  plugins:
    directories:
      - "/usr/lib/fs9/plugins"
      - "/var/lib/fs9/plugins"

# Default mounts (applied to 'default' namespace on startup)
mounts:
  - path: "/"
    provider: "memfs"

logging:
  level: info
  format: json  # Use JSON for log aggregation
```

#### 3.2 Create fs9-server Systemd Service

Create `/etc/systemd/system/fs9-server.service`:

```ini
[Unit]
Description=FS9 Distributed Filesystem Server
Documentation=https://github.com/example/fs9
After=network.target fs9-meta.service
Requires=fs9-meta.service
Wants=network.target

[Service]
Type=simple
User=fs9
Group=fs9
EnvironmentFile=/etc/fs9/secrets.env
Environment="RUST_LOG=info"
Environment="RUST_BACKTRACE=1"

ExecStart=/usr/local/bin/fs9-server -c /etc/fs9/fs9.yaml

WorkingDirectory=/var/lib/fs9
StandardOutput=append:/var/log/fs9/server.log
StandardError=append:/var/log/fs9/server.log

Restart=always
RestartSec=5
StartLimitIntervalSec=60
StartLimitBurst=3

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/fs9 /var/log/fs9
PrivateTmp=true

# Resource limits
LimitNOFILE=65536
LimitNPROC=4096

[Install]
WantedBy=multi-user.target
```

#### 3.3 Start fs9-server

```bash
# Reload systemd
sudo systemctl daemon-reload

# Enable and start fs9-server
sudo systemctl enable fs9-server
sudo systemctl start fs9-server

# Verify it's running
sudo systemctl status fs9-server
curl http://localhost:9999/health
```

### Step 4: Configure Namespaces and Access

#### 4.1 Initialize fs9-admin

```bash
# Load secrets
source /etc/fs9/secrets.env

# Initialize fs9-admin config
fs9-admin init -s http://localhost:9999 -k "$FS9_JWT_SECRET"
```

#### 4.2 Create Production Namespaces

```bash
# Create a namespace for production workloads
fs9-admin ns create prod

# Mount pagefs with persistent storage
fs9-admin mount add pagefs -n prod -p / \
  --set uid=1000 \
  --set gid=1000 \
  --set backend.type=s3 \
  --set backend.bucket=fs9-prod-data \
  --set backend.prefix=prod

# Create namespace for staging
fs9-admin ns create staging

# Mount with memory backend for staging
fs9-admin mount add pagefs -n staging -p / \
  --set uid=1000 \
  --set gid=1000

# Mount pubsubfs for real-time messaging
fs9-admin mount add pubsubfs -n prod -p /events

# Verify mounts
fs9-admin mount list -n prod
fs9-admin mount list -n staging
```

#### 4.3 Create Service Accounts and Tokens

```bash
# Generate token for application service (1 hour TTL)
APP_TOKEN=$(fs9-admin token generate -u app-service -n prod -r operator -T 3600 -q)
echo "APP_TOKEN: $APP_TOKEN"

# Generate token for admin operations (8 hour TTL)
ADMIN_TOKEN=$(fs9-admin token generate -u admin -n prod -r admin -T 28800 -q)
echo "ADMIN_TOKEN: $ADMIN_TOKEN"

# Generate read-only token for monitoring (24 hour TTL)
MONITOR_TOKEN=$(fs9-admin token generate -u monitor -n prod -T 86400 -q)
echo "MONITOR_TOKEN: $MONITOR_TOKEN"
```

### Step 5: Deploy Clients

#### 5.1 Application Integration

For applications connecting to FS9:

```bash
# Set environment variables for client
export FS9_SERVER="http://fs9-server.internal:9999"
export FS9_TOKEN="$APP_TOKEN"
```

#### 5.2 FUSE Mount (Optional)

For mounting FS9 as a local filesystem:

```bash
# Install FUSE binary
sudo cp target/release/fs9-fuse /usr/local/bin/
sudo chmod 755 /usr/local/bin/fs9-fuse

# Create mount point
sudo mkdir -p /mnt/fs9

# Create FUSE systemd mount
sudo tee /etc/systemd/system/mnt-fs9.mount > /dev/null << EOF
[Unit]
Description=FS9 FUSE Mount
After=fs9-server.service
Requires=fs9-server.service

[Mount]
What=fs9-fuse
Where=/mnt/fs9
Type=fuse
Options=_netdev,server=http://localhost:9999,token=${APP_TOKEN}

[Install]
WantedBy=multi-user.target
EOF

# Enable and start mount
sudo systemctl daemon-reload
sudo systemctl enable mnt-fs9.mount
sudo systemctl start mnt-fs9.mount
```

### Step 6: Verify Deployment

#### 6.1 Health Checks

```bash
# Check all services
echo "=== fs9-meta ===" && curl -s http://127.0.0.1:9998/health
echo "=== fs9-server ===" && curl -s http://localhost:9999/health

# Check service status
sudo systemctl status fs9-meta fs9-server
```

#### 6.2 Functional Verification

```bash
# Test with sh9 shell
source /etc/fs9/secrets.env
TOKEN=$(fs9-admin token generate -u test -n prod -q)

sh9 -s http://localhost:9999 -t "$TOKEN" -c "
  echo 'Hello FS9' > /test.txt
  cat /test.txt
  rm /test.txt
"
```

#### 6.3 Check Logs

```bash
# View recent logs
sudo tail -f /var/log/fs9/server.log
sudo tail -f /var/log/fs9/meta.log

# Or with journalctl
sudo journalctl -u fs9-server -f
sudo journalctl -u fs9-meta -f
```

### Production Checklist

Before going live, verify:

- [ ] **Secrets**: JWT secret and meta key are securely generated and stored
- [ ] **TLS**: Reverse proxy (nginx/Caddy) configured with TLS for external access
- [ ] **Firewall**: Only port 443 (HTTPS) exposed externally; 9998/9999 internal only
- [ ] **Backups**: Database backup strategy for fs9-meta (if using PostgreSQL)
- [ ] **Monitoring**: Health check endpoints integrated with monitoring system
- [ ] **Logging**: Log rotation configured for `/var/log/fs9/`
- [ ] **Tokens**: Token TTLs appropriate for use case; rotation strategy defined
- [ ] **Resources**: `LimitNOFILE` and `LimitNPROC` tuned for expected load

### Reverse Proxy Configuration (Nginx)

For production, place fs9-server behind a reverse proxy with TLS:

```nginx
upstream fs9 {
    server 127.0.0.1:9999;
    keepalive 32;
}

server {
    listen 443 ssl http2;
    server_name fs9.example.com;
    
    ssl_certificate /etc/ssl/certs/fs9.crt;
    ssl_certificate_key /etc/ssl/private/fs9.key;
    
    # Security headers
    add_header Strict-Transport-Security "max-age=31536000" always;
    add_header X-Content-Type-Options "nosniff" always;
    
    location / {
        proxy_pass http://fs9;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        # Timeout settings (match server config)
        proxy_connect_timeout 5s;
        proxy_read_timeout 30s;
        proxy_send_timeout 30s;
        
        # For large file uploads
        client_max_body_size 100M;
    }
    
    location /health {
        proxy_pass http://fs9;
        access_log off;
    }
}
```

### Troubleshooting

| Symptom | Cause | Solution |
|---------|-------|----------|
| `meta_url is required` | fs9-meta not configured | Set `FS9_META_ENDPOINTS` or `server.meta_url` in config |
| `connection refused to :9998` | fs9-meta not running | Start fs9-meta: `sudo systemctl start fs9-meta` |
| `401 Unauthorized` | Invalid/expired token | Generate new token with `fs9-admin token generate` |
| `403 Forbidden` | Namespace not found | Create namespace: `fs9-admin ns create <name>` |
| `plugin not found` | Plugin not loaded | Check plugin path in config; verify .so/.dylib exists |

---

## Server Deployment

### Running the Server

```bash
# Development mode (with logging)
RUST_LOG=info cargo run -p fs9-server

# Or use the built binary
./target/release/fs9-server

# With config file
./target/release/fs9-server -c /path/to/fs9.yaml
```

### Command Line Options

```
fs9-server [OPTIONS]

Options:
  -c, --config <CONFIG>  Path to configuration file [env: FS9_CONFIG]
  -h, --help             Print help
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FS9_CONFIG` | *(none)* | Path to configuration file |
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
| `FS9_SERVER` | `http://localhost:9999` | FS9 server URL to connect to |
| `FS9_TOKEN` | *(empty)* | JWT token for authentication |

### Usage

```bash
# Start interactive REPL (with token)
sh9 -s http://localhost:9999 -t "$TOKEN"

# Or use environment variables
export FS9_SERVER=http://localhost:9999
export FS9_TOKEN="your-jwt-token"
sh9

# Execute a single command
sh9 -t "$TOKEN" -c "ls /; echo hello"

# Execute a script file
sh9 -t "$TOKEN" script.sh9

# Generate token and start shell in one line
sh9 -t "$(fs9-admin token generate -u alice -n myns -q)"
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

PageFS can be configured via `--set` or JSON:

```bash
# Using --set (recommended)
fs9-admin mount add pagefs -n myns --set uid=1000 --set gid=1000

# S3 backend
fs9-admin mount add pagefs -n myns \
  --set uid=1000 \
  --set gid=1000 \
  --set backend.type=s3 \
  --set backend.bucket=my-bucket \
  --set backend.prefix=fs9-data

# Or using JSON
fs9-admin mount add pagefs -n myns -c '{"uid": 1000, "gid": 1000}'
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `uid` | u32 | 0 | User ID for file ownership |
| `gid` | u32 | 0 | Group ID for file ownership |
| `backend.type` | string | "memory" | Backend type: "memory" or "s3" |
| `backend.bucket` | string | - | S3 bucket name (required for S3) |
| `backend.prefix` | string | "" | S3 key prefix |

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

## FS9 Metadata Service (fs9-meta)

fs9-meta is a standalone REST service for managing FS9 metadata including namespaces, users, roles, API keys, and JWT tokens. It supports SQLite (default) and PostgreSQL backends.

### Building

```bash
cargo build -p fs9-meta --release
```

### Running

```bash
# With command line options
./target/release/fs9-meta --jwt-secret "your-secret"

# With config file
./target/release/fs9-meta -c /path/to/meta.yaml

# With environment variables
FS9_JWT_SECRET="your-secret" FS9_META_DSN="sqlite:meta.db" ./target/release/fs9-meta
```

### Command Line Options

```
fs9-meta [OPTIONS]

Options:
  -c, --config <CONFIG>          Path to configuration file
      --dsn <DSN>                Database DSN [env: FS9_META_DSN] [default: sqlite:fs9-meta.db]
      --host <HOST>              Host to bind to [env: FS9_META_HOST] [default: 0.0.0.0]
      --port <PORT>              Port to listen on [env: FS9_META_PORT] [default: 9998]
      --jwt-secret <JWT_SECRET>  JWT secret for token signing [env: FS9_JWT_SECRET]
  -h, --help                     Print help
```

### Configuration File

```yaml
# meta.yaml
server:
  host: "0.0.0.0"
  port: 9998

database:
  dsn: "sqlite:fs9-meta.db"

auth:
  jwt_secret: "your-secret-key"
```

### API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/api/v1/namespaces` | GET, POST | List/create namespaces |
| `/api/v1/namespaces/:name` | GET, DELETE | Get/delete namespace |
| `/api/v1/namespaces/:ns/mounts` | GET, POST | List/create mounts |
| `/api/v1/namespaces/:ns/mounts/*path` | GET, DELETE | Get/delete mount |
| `/api/v1/users` | GET, POST | List/create users |
| `/api/v1/users/by-name/:username` | GET | Get user by username |
| `/api/v1/users/:id` | DELETE | Delete user |
| `/api/v1/users/:id/roles` | GET, POST | List/assign roles |
| `/api/v1/users/:id/roles/:ns/:role` | DELETE | Revoke role |
| `/api/v1/tokens/generate` | POST | Generate JWT token |
| `/api/v1/tokens/validate` | POST | Validate JWT token |
| `/api/v1/tokens/refresh` | POST | Refresh JWT token |
| `/api/v1/apikeys` | GET, POST | List/create API keys |
| `/api/v1/apikeys/validate` | POST | Validate API key |
| `/api/v1/apikeys/:id` | DELETE | Revoke API key |

### Example Usage

```bash
# Create a namespace
curl -X POST http://localhost:9998/api/v1/namespaces \
  -H "Content-Type: application/json" \
  -d '{"name": "myns"}'

# Generate a token
curl -X POST http://localhost:9998/api/v1/tokens/generate \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "namespace": "myns", "roles": ["read-write"]}'

# Validate a token
curl -X POST http://localhost:9998/api/v1/tokens/validate \
  -H "Content-Type: application/json" \
  -d '{"token": "eyJ..."}'
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

### fs9-admin CLI

The `fs9-admin` CLI provides convenient commands for namespace and token management.

```bash
# Initialize config (saves to ~/.fs9/admin.toml)
fs9-admin init -s http://localhost:9999 -k "your-jwt-secret"

# After init, you can omit -s and --secret flags
fs9-admin ns list
```

#### Namespace Management

```bash
# Create namespace
fs9-admin ns create myns

# Create namespace with auto-mount
fs9-admin ns create myns --mount pagefs
fs9-admin ns create myns --mount pagefs:/data --set uid=1000 --set gid=1000

# List namespaces
fs9-admin ns list

# Get namespace details
fs9-admin ns get myns
```

#### Mount Management

```bash
# Mount a provider
fs9-admin mount add pagefs -n myns
fs9-admin mount add pagefs -n myns -p /data

# Mount with configuration using --set (key=value format)
fs9-admin mount add pagefs -n myns --set uid=1000 --set gid=1000

# Nested config with dot notation
fs9-admin mount add pagefs -n myns \
  --set uid=1000 \
  --set backend.type=s3 \
  --set backend.bucket=my-bucket \
  --set backend.prefix=data

# Or use JSON config
fs9-admin mount add pagefs -n myns -c '{"uid": 1000, "gid": 1000}'

# List mounts
fs9-admin mount list -n myns
```

#### Token Management

```bash
# Generate token (verbose output)
fs9-admin token generate -u alice -n myns

# Generate token (quiet mode - only outputs token, for scripting)
TOKEN=$(fs9-admin token generate -u alice -n myns -q)

# Generate with specific role and TTL
fs9-admin token generate -u alice -n myns -r admin -T 3600

# Decode token (without verification)
fs9-admin token decode "$TOKEN"
```

### Generating Tokens (Manual)

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
| `/api/v1/write` | POST | Write to file handle (streaming) |
| `/api/v1/download` | GET | Stateless file download with Range support |
| `/api/v1/upload` | PUT | Stateless streaming file upload |
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
