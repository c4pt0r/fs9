#!/bin/bash
# FS9 Multi-tenant Demo: Start Server
# Starts FS9 server with JWT authentication configured

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Configuration
export JWT_SECRET="demo-secret-key-for-testing-only-12345"
export FS9_PORT=9999

# Create config file
CONFIG_FILE="$SCRIPT_DIR/fs9-demo.yaml"
cat > "$CONFIG_FILE" << EOF
server:
  host: "127.0.0.1"
  port: $FS9_PORT
  auth:
    enabled: true
    jwt_secret: "$JWT_SECRET"

logging:
  level: "info"
  filter: "fs9_server=info"

# Don't pre-create mounts, let each namespace manage its own
mounts: []
EOF

echo "=========================================="
echo "  FS9 Multi-tenant Demo Server"
echo "=========================================="
echo ""
echo "Configuration:"
echo "  Port:       $FS9_PORT"
echo "  JWT Secret: $JWT_SECRET"
echo "  Config:     $CONFIG_FILE"
echo ""

# Build if needed
echo "[1/2] Building server..."
cd "$PROJECT_ROOT"
cargo build -p fs9-server --release 2>&1 | tail -3

# Start server
echo ""
echo "[2/2] Starting server..."
echo ""
FS9_CONFIG="$CONFIG_FILE" RUST_LOG=info cargo run -p fs9-server --release 2>&1 &
SERVER_PID=$!

echo "Server PID: $SERVER_PID"
echo ""

# Wait for server to start
echo "Waiting for server to be ready..."
for i in {1..30}; do
    if curl -s http://127.0.0.1:$FS9_PORT/health > /dev/null 2>&1; then
        echo "✅ Server is ready!"
        echo ""
        echo "To stop: kill $SERVER_PID"
        echo ""
        echo "Now run: ./02-setup-tenants.sh"
        break
    fi
    sleep 0.5
done

# Save PID for subsequent scripts
echo "$SERVER_PID" > "$SCRIPT_DIR/.server.pid"
echo "$JWT_SECRET" > "$SCRIPT_DIR/.jwt-secret"

wait $SERVER_PID
