# FS9 Docker Compose Setup

Deploy FS9 with Docker Compose for multi-tenant distributed filesystem.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Docker Network                    │
│                                                     │
│  ┌─────────────┐         ┌─────────────────────┐   │
│  │  fs9-meta   │◄────────│    fs9-server       │   │
│  │  (Auth)     │         │  (Filesystem API)   │   │
│  │  :9998      │         │  :9999              │   │
│  └─────────────┘         └─────────────────────┘   │
│         │                         │                 │
└─────────┼─────────────────────────┼─────────────────┘
          │                         │
          ▼                         ▼
    Admin Script              sh9 / FUSE / API
```

## Quick Start

```bash
# 1. Copy and edit environment file
cp .env.example .env
# Edit .env to set FS9_JWT_SECRET and FS9_META_KEY

# 2. Build and start services
docker compose up -d

# 3. Check services are healthy
docker compose ps
docker compose logs -f  # Watch logs

# 4. Create your first tenant
./scripts/fs9-tenant.sh create-tenant myproject

# 5. Connect with sh9
export FS9_SERVER_URL=http://localhost:9999
export FS9_TOKEN=<token from step 4>
cargo run -p sh9
```

## Tenant Management

```bash
# Create a new tenant (namespace + admin user + token)
./scripts/fs9-tenant.sh create-tenant myproject

# List namespaces
./scripts/fs9-tenant.sh list-namespaces

# Create additional user
./scripts/fs9-tenant.sh create-user myproject alice read-write

# Generate new token for user
./scripts/fs9-tenant.sh generate-token <user_id>

# Delete namespace (and all users)
./scripts/fs9-tenant.sh delete-namespace myproject
```

## Services

### fs9-meta (Port 9998)

Central authentication and namespace management:
- Token validation for fs9-server
- Namespace CRUD
- User management
- Token generation

### fs9-server (Port 9999)

The main filesystem server:
- REST API for filesystem operations
- Automatic plugin loading from /app/plugins
- Multi-tenant with namespace isolation
- JWT-based authentication

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FS9_JWT_SECRET` | `change-me...` | JWT signing secret (share between services) |
| `FS9_META_KEY` | `admin-key...` | Admin API key for meta service |
| `RUST_LOG` | `info` | Logging level |

### Volumes

- `meta-data` - SQLite database for users/namespaces
- `server-data` - Filesystem data storage

## Connecting Clients

### sh9 (Interactive Shell)

```bash
export FS9_SERVER_URL=http://localhost:9999
export FS9_TOKEN=<your-token>
cargo run -p sh9

# In sh9:
sh9:/> lsfs
sh9:/> mount pagefs /data
sh9:/> echo "hello" > /data/test.txt
sh9:/> cat /data/test.txt
```

### FUSE Mount

```bash
export FS9_TOKEN=<your-token>
mkdir -p /tmp/fs9
cargo run -p fs9-fuse -- /tmp/fs9 --server http://localhost:9999 --foreground

# In another terminal:
cd /tmp/fs9
echo "hello" > test.txt
git init && git add . && git commit -m "init"
```

### REST API

```bash
TOKEN=<your-token>

# List mounts
curl -H "Authorization: Bearer $TOKEN" http://localhost:9999/api/v1/mounts

# Mount filesystem
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"plugin": "pagefs", "path": "/data"}' \
  http://localhost:9999/api/v1/mounts

# Read file
curl -H "Authorization: Bearer $TOKEN" http://localhost:9999/api/v1/read?path=/data/test.txt
```

## Production Considerations

1. **Secrets**: Generate strong random values for `FS9_JWT_SECRET` and `FS9_META_KEY`
2. **TLS**: Put a reverse proxy (nginx, traefik) in front with TLS
3. **Volumes**: Mount to persistent storage, not Docker volumes
4. **Backup**: Back up the meta-data SQLite database regularly
5. **Monitoring**: Add Prometheus/Grafana for metrics

## Troubleshooting

```bash
# Check service health
docker compose ps
curl http://localhost:9998/health  # Meta
curl http://localhost:9999/health  # Server

# View logs
docker compose logs meta
docker compose logs server

# Restart services
docker compose restart

# Rebuild after code changes
docker compose build --no-cache
docker compose up -d
```
