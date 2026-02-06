#!/bin/bash
# FS9 Quickstart - Complete quick start example
#
# This script demonstrates a complete FS9 workflow:
# 1. Build the project (fs9-server, fs9-meta, sh9, fs9-admin, plugins, optionally fs9-fuse)
# 2. Start fs9-meta (metadata service) and fs9-server
# 3. Create a namespace with pagefs mount
# 4. Generate a user token
# 5. Optionally mount FS9 via FUSE
# 6. Start the sh9 interactive shell
#
# Usage:
#   ./quickstart.sh              # Run with default settings
#   ./quickstart.sh --no-build   # Skip build step
#   ./quickstart.sh --fuse       # Also mount FS9 via FUSE at /tmp/fs9-mount
#   ./quickstart.sh --fuse /mnt  # Mount via FUSE at custom path
#
# Configuration file:
#   You can also use a config file instead of environment variables:
#   ./target/release/fs9-server -c /path/to/fs9.yaml
#   ./target/release/fs9-meta -c /path/to/meta.yaml
#
# Environment variables:
#   FS9_JWT_SECRET       - JWT secret (default: my-secret-key-change-me)
#   FS9_SERVER_ENDPOINTS - Server URL (default: http://localhost:9999)
#   FS9_META_ENDPOINTS   - Meta service URL (default: http://localhost:9998)
#   NAMESPACE            - Namespace name (default: demo)
#   FS9_USER             - Username for token (default: demo)
#   FUSE_MOUNT           - FUSE mount point (default: /tmp/fs9-mount, use with --fuse)

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
export FS9_SERVER_ENDPOINTS="${FS9_SERVER_ENDPOINTS:-http://localhost:9999}"
export FS9_META_ENDPOINTS="${FS9_META_ENDPOINTS:-http://localhost:9998}"
NAMESPACE="${NAMESPACE:-demo}"
FS9_USER="${FS9_USER:-demo}"

SKIP_BUILD=false
ENABLE_FUSE=false
FUSE_MOUNTPOINT=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --no-build)
            SKIP_BUILD=true
            shift
            ;;
        --fuse)
            ENABLE_FUSE=true
            shift
            if [[ $# -gt 0 && ! "$1" == --* ]]; then
                FUSE_MOUNTPOINT="$1"
                shift
            fi
            ;;
        *)
            shift
            ;;
    esac
done
FUSE_MOUNTPOINT="${FUSE_MOUNTPOINT:-${FUSE_MOUNT:-/tmp/fs9-mount}}"

if [ "$ENABLE_FUSE" = true ]; then
    TOTAL_STEPS=6
else
    TOTAL_STEPS=5
fi

echo -e "${CYAN}${BOLD}=== FS9 Quickstart ===${NC}"
echo ""

STEP=1

# Step 1: Build
if [ "$SKIP_BUILD" = false ]; then
    echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Building...${NC}"
    cargo build --release 2>&1 | tail -1
    make plugins 2>/dev/null || true
    if [ "$ENABLE_FUSE" = true ]; then
        cargo build -p fs9-fuse --release 2>&1 | tail -1
    fi
    echo -e "${GREEN}✓ Build complete${NC}"
else
    echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Skipping build${NC}"
fi
echo ""
STEP=$((STEP + 1))

# Step 2: Stop old processes and start services
echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Starting services...${NC}"
pkill -f "fs9-fuse" 2>/dev/null || true
pkill -f "fs9-server" 2>/dev/null || true
pkill -f "fs9-meta" 2>/dev/null || true
sleep 1

FS9_META_PORT=$(echo "$FS9_META_ENDPOINTS" | sed 's|.*:||')
./target/release/fs9-meta --port "$FS9_META_PORT" --jwt-secret "$FS9_JWT_SECRET" &
META_PID=$!
sleep 2

if ! kill -0 $META_PID 2>/dev/null; then
    echo -e "${RED}✗ Meta service failed to start${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Meta service running (PID: $META_PID)${NC}"

FS9_PORT=$(echo "$FS9_SERVER_ENDPOINTS" | sed 's|.*:||')
FS9_META_ENDPOINTS="$FS9_META_ENDPOINTS" FS9_PORT="$FS9_PORT" ./target/release/fs9-server &
SERVER_PID=$!
sleep 2

if ! kill -0 $SERVER_PID 2>/dev/null; then
    echo -e "${RED}✗ Server failed to start${NC}"
    kill $META_PID 2>/dev/null || true
    exit 1
fi
echo -e "${GREEN}✓ Server running (PID: $SERVER_PID)${NC}"
echo ""
STEP=$((STEP + 1))

# Step 3: Create namespace with mount
echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Creating namespace '${NAMESPACE}' with pagefs...${NC}"
./target/release/fs9-admin \
    --secret "$FS9_JWT_SECRET" \
    ns create "$NAMESPACE" --mount pagefs --set uid=1000 --set gid=1000 2>/dev/null || {
    echo "  Namespace exists, mounting pagefs..."
    ./target/release/fs9-admin \
        --secret "$FS9_JWT_SECRET" \
        mount add pagefs -n "$NAMESPACE" --set uid=1000 --set gid=1000 2>/dev/null || true
}
echo ""
STEP=$((STEP + 1))

# Step 4: Generate token
echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Generating token...${NC}"
TOKEN=$(./target/release/fs9-admin \
    --secret "$FS9_JWT_SECRET" \
    token generate -u "$FS9_USER" -n "$NAMESPACE" -q)

if [ -z "$TOKEN" ]; then
    echo -e "${RED}✗ Failed to generate token${NC}"
    kill $SERVER_PID $META_PID 2>/dev/null || true
    exit 1
fi
echo -e "${GREEN}✓ Token generated${NC}"
echo ""
STEP=$((STEP + 1))

# Step 5 (optional): Mount FUSE
FUSE_PID=""
if [ "$ENABLE_FUSE" = true ]; then
    echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Mounting FUSE at ${FUSE_MOUNTPOINT}...${NC}"

    mkdir -p "$FUSE_MOUNTPOINT"

    # Unmount if already mounted
    if mount | grep -q "$FUSE_MOUNTPOINT"; then
        if [[ "$OSTYPE" == darwin* ]]; then
            umount "$FUSE_MOUNTPOINT" 2>/dev/null || true
        else
            fusermount -u "$FUSE_MOUNTPOINT" 2>/dev/null || true
        fi
        sleep 1
    fi

    ./target/release/fs9-fuse "$FUSE_MOUNTPOINT" \
        --server "$FS9_SERVER_ENDPOINTS" \
        --token "$TOKEN" \
        --foreground --auto-unmount &
    FUSE_PID=$!
    sleep 2

    if ! kill -0 $FUSE_PID 2>/dev/null; then
        echo -e "${RED}✗ FUSE mount failed${NC}"
        echo -e "${YELLOW}  Continuing without FUSE...${NC}"
        FUSE_PID=""
    else
        echo -e "${GREEN}✓ FUSE mounted at ${FUSE_MOUNTPOINT} (PID: $FUSE_PID)${NC}"
    fi
    echo ""
    STEP=$((STEP + 1))
fi

# Final step: Show info and start shell
echo -e "${CYAN}${BOLD}=== Ready ===${NC}"
echo ""
echo -e "  Server:    ${CYAN}${FS9_SERVER_ENDPOINTS}${NC}"
echo -e "  Namespace: ${CYAN}${NAMESPACE}${NC}"
echo -e "  User:      ${CYAN}${FS9_USER}${NC}"
echo -e "  Server PID: ${SERVER_PID}"
echo -e "  Meta PID:   ${META_PID}"
if [ -n "$FUSE_PID" ]; then
echo -e "  FUSE PID:   ${FUSE_PID}"
echo -e "  FUSE mount: ${CYAN}${FUSE_MOUNTPOINT}${NC}"
fi
echo ""
echo -e "${YELLOW}Example commands in sh9:${NC}"
echo "  ls                       # List files"
echo "  echo 'hello' > test.txt  # Create file"
echo "  cat test.txt             # Read file"
echo "  mkdir mydir              # Create directory"
echo "  exit                     # Exit shell"
if [ -n "$FUSE_PID" ]; then
echo ""
echo -e "${YELLOW}FUSE mount at ${FUSE_MOUNTPOINT} — use standard tools:${NC}"
echo "  ls ${FUSE_MOUNTPOINT}"
echo "  cat ${FUSE_MOUNTPOINT}/test.txt"
echo "  cd ${FUSE_MOUNTPOINT} && git init"
fi
echo ""
CLEANUP_PIDS="$SERVER_PID $META_PID"
if [ -n "$FUSE_PID" ]; then
    CLEANUP_PIDS="$FUSE_PID $CLEANUP_PIDS"
fi
echo -e "${YELLOW}To stop services later: ${NC}kill $CLEANUP_PIDS"
echo ""

echo -e "${YELLOW}[${STEP}/${TOTAL_STEPS}] Starting sh9 shell...${NC}"
echo ""

./target/release/sh9 -t "$TOKEN"

# Cleanup
echo ""
echo -e "${YELLOW}Stopping services...${NC}"
if [ -n "$FUSE_PID" ]; then
    kill $FUSE_PID 2>/dev/null || true
    sleep 1
    if [[ "$OSTYPE" == darwin* ]]; then
        umount "$FUSE_MOUNTPOINT" 2>/dev/null || true
    else
        fusermount -u "$FUSE_MOUNTPOINT" 2>/dev/null || true
    fi
fi
kill $SERVER_PID 2>/dev/null || true
kill $META_PID 2>/dev/null || true
echo -e "${GREEN}✓ Done${NC}"
