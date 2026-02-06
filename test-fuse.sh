#!/bin/bash
set -e

SERVER="${FS9_SERVER_URL:-http://fs9-server:9999}"
TOKEN="$FS9_TOKEN"
MOUNT="/mnt/fs9"

echo "========================================"
echo "  FS9 FUSE Test on Linux"
echo "========================================"
echo "Server: $SERVER"
echo ""

# Wait for server
echo "--- Waiting for server ---"
for i in $(seq 1 10); do
    if curl -sf "$SERVER/health" > /dev/null 2>&1; then
        echo "Server ready!"
        break
    fi
    echo "  Waiting... ($i/10)"
    sleep 2
done

# Mount pagefs via REST API (may already be mounted)
echo ""
echo "--- Mounting pagefs at /data ---"
curl -sf -X POST "$SERVER/api/v1/mount" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path":"/data","provider":"pagefs","config":{}}' 2>/dev/null || echo "(already mounted or mount response)"

# Create mount point and mount FUSE
echo ""
echo "--- Mounting FUSE ---"
mkdir -p "$MOUNT"
fs9-fuse "$MOUNT" --server "$SERVER" --token "$TOKEN" --foreground &
FUSE_PID=$!
sleep 2

# Check FUSE is running
if ! kill -0 $FUSE_PID 2>/dev/null; then
    echo "FUSE process died!"
    exit 1
fi
echo "FUSE mounted at $MOUNT (PID=$FUSE_PID)"

echo ""
echo "=== Test 1: ls root ==="
ls -la "$MOUNT/"

echo ""
echo "=== Test 2: ls /data ==="
ls -la "$MOUNT/data/" 2>&1 || echo "(empty dir expected)"

echo ""
echo "=== Test 3: Write a file ==="
echo "Hello from FUSE on Linux!" > "$MOUNT/data/hello.txt"
echo "Written: hello.txt"

echo ""
echo "=== Test 4: Read file back ==="
cat "$MOUNT/data/hello.txt"

echo ""
echo "=== Test 5: Create directory ==="
mkdir -p "$MOUNT/data/testdir"
echo "Created: testdir/"

echo ""
echo "=== Test 6: Write file in directory ==="
echo "nested file content" > "$MOUNT/data/testdir/nested.txt"
cat "$MOUNT/data/testdir/nested.txt"

echo ""
echo "=== Test 7: List directory ==="
ls -la "$MOUNT/data/"
echo "---"
ls -la "$MOUNT/data/testdir/"

echo ""
echo "=== Test 8: File stat ==="
stat "$MOUNT/data/hello.txt"

echo ""
echo "=== Test 9: Overwrite file ==="
echo "Updated content!" > "$MOUNT/data/hello.txt"
cat "$MOUNT/data/hello.txt"

echo ""
echo "=== Test 10: Remove file ==="
rm "$MOUNT/data/testdir/nested.txt"
ls -la "$MOUNT/data/testdir/" 2>&1 || echo "(empty after delete)"

echo ""
echo "=== Test 11: Remove directory ==="
rmdir "$MOUNT/data/testdir"
ls -la "$MOUNT/data/"

echo ""
echo "=== Test 12: Git operations ==="
cd "$MOUNT/data"
git init
git config user.email "test@fs9.dev"
git config user.name "FS9 Test"
git add hello.txt
git commit -m "Initial commit from FUSE"
git log --oneline
cd /

echo ""
echo "========================================"
echo "  All FUSE tests passed!"
echo "========================================"

# Cleanup
kill $FUSE_PID 2>/dev/null
wait $FUSE_PID 2>/dev/null || true
fusermount3 -u "$MOUNT" 2>/dev/null || true
