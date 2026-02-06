#!/bin/bash
# FS9 Quickstart - Complete quick start example
#
# This script demonstrates a complete FS9 workflow:
# 1. Build the project (fs9-server, fs9-meta, sh9, fs9-admin, plugins)
# 2. Start fs9-meta (metadata service) and fs9-server
# 3. Create a namespace with pagefs mount
# 4. Generate a user token
# 5. Start the sh9 interactive shell
#
# Usage:
#   ./quickstart.sh              # Run with default settings
#   ./quickstart.sh --no-build   # Skip build step
#
# Configuration file:
#   You can also use a config file instead of environment variables:
#   ./target/release/fs9-server -c /path/to/fs9.yaml
#   ./target/release/fs9-meta -c /path/to/meta.yaml
#
# Environment variables:
#   FS9_JWT_SECRET  - JWT secret (default: my-secret-key-change-me)
#   FS9_PORT        - Server port (default: 9999)
#   FS9_META_PORT   - Meta service port (default: 9998)
#   NAMESPACE       - Namespace name (default: demo)
#   FS9_USER        - Username for token (default: demo)

set -e

cd "$(dirname "$0")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Configuration
export FS9_JWT_SECRET="${FS9_JWT_SECRET:-my-secret-key-change-me}"
FS9_PORT="${FS9_PORT:-9999}"
FS9_META_PORT="${FS9_META_PORT:-9998}"
FS9_SERVER="http://localhost:${FS9_PORT}"
FS9_META_URL="http://localhost:${FS9_META_PORT}"
NAMESPACE="${NAMESPACE:-demo}"
FS9_USER="${FS9_USER:-demo}"

echo -e "${CYAN}${BOLD}=== FS9 Quickstart ===${NC}"
echo ""

# Parse arguments
SKIP_BUILD=false
for arg in "$@"; do
    case $arg in
        --no-build)
            SKIP_BUILD=true
            ;;
    esac
done

# Step 1: Build
if [ "$SKIP_BUILD" = false ]; then
    echo -e "${YELLOW}[1/5] Building...${NC}"
    cargo build --release 2>&1 | tail -1
    make plugins 2>/dev/null || true
    echo -e "${GREEN}✓ Build complete${NC}"
else
    echo -e "${YELLOW}[1/5] Skipping build${NC}"
fi
echo ""

# Step 2: Stop old processes and start services
echo -e "${YELLOW}[2/5] Starting services...${NC}"
pkill -f "fs9-server" 2>/dev/null || true
pkill -f "fs9-meta" 2>/dev/null || true
sleep 1

./target/release/fs9-meta --port "$FS9_META_PORT" --jwt-secret "$FS9_JWT_SECRET" &
META_PID=$!
sleep 2

if ! kill -0 $META_PID 2>/dev/null; then
    echo -e "${RED}✗ Meta service failed to start${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Meta service running (PID: $META_PID)${NC}"

FS9_META_URL="$FS9_META_URL" FS9_PORT="$FS9_PORT" ./target/release/fs9-server &
SERVER_PID=$!
sleep 2

if ! kill -0 $SERVER_PID 2>/dev/null; then
    echo -e "${RED}✗ Server failed to start${NC}"
    kill $META_PID 2>/dev/null || true
    exit 1
fi
echo -e "${GREEN}✓ Server running (PID: $SERVER_PID)${NC}"
echo ""

# Step 3: Create namespace with mount
echo -e "${YELLOW}[3/5] Creating namespace '${NAMESPACE}' with pagefs...${NC}"
./target/release/fs9-admin \
    -s "$FS9_SERVER" \
    --secret "$FS9_JWT_SECRET" \
    ns create "$NAMESPACE" --mount pagefs --set uid=1000 --set gid=1000 2>/dev/null || {
    # Namespace might already exist, try to mount
    echo "  Namespace exists, mounting pagefs..."
    ./target/release/fs9-admin \
        -s "$FS9_SERVER" \
        --secret "$FS9_JWT_SECRET" \
        mount add pagefs -n "$NAMESPACE" --set uid=1000 --set gid=1000 2>/dev/null || true
}
echo ""

# Step 4: Generate token
echo -e "${YELLOW}[4/5] Generating token...${NC}"
TOKEN=$(./target/release/fs9-admin \
    -s "$FS9_SERVER" \
    --secret "$FS9_JWT_SECRET" \
    token generate -u "$FS9_USER" -n "$NAMESPACE" -q)

if [ -z "$TOKEN" ]; then
    echo -e "${RED}✗ Failed to generate token${NC}"
    kill $SERVER_PID $META_PID 2>/dev/null || true
    exit 1
fi
echo -e "${GREEN}✓ Token generated${NC}"
echo ""

# Step 5: Show info and start shell
echo -e "${CYAN}${BOLD}=== Ready ===${NC}"
echo ""
echo -e "  Server:    ${CYAN}${FS9_SERVER}${NC}"
echo -e "  Namespace: ${CYAN}${NAMESPACE}${NC}"
echo -e "  User:      ${CYAN}${FS9_USER}${NC}"
echo -e "  Server PID: ${SERVER_PID}"
echo -e "  Meta PID:   ${META_PID}"
echo ""
echo -e "${YELLOW}Example commands in sh9:${NC}"
echo "  ls                       # List files"
echo "  echo 'hello' > test.txt  # Create file"
echo "  cat test.txt             # Read file"
echo "  mkdir mydir              # Create directory"
echo "  exit                     # Exit shell"
echo ""
echo -e "${YELLOW}To stop services later: ${NC}kill $SERVER_PID $META_PID"
echo ""

echo -e "${YELLOW}[5/5] Starting sh9 shell...${NC}"
echo ""

./target/release/sh9 -s "$FS9_SERVER" -t "$TOKEN"

# Cleanup
echo ""
echo -e "${YELLOW}Stopping services...${NC}"
kill $SERVER_PID 2>/dev/null || true
kill $META_PID 2>/dev/null || true
echo -e "${GREEN}✓ Done${NC}"
