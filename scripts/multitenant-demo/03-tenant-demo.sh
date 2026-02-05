#!/bin/bash
# FS9 Multi-tenant Demo: Tenant Operations
# Simulates user operations under a specific tenant

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib/jwt.sh"

SERVER="http://127.0.0.1:9999"
JWT_SECRET=$(cat "$SCRIPT_DIR/.jwt-secret" 2>/dev/null || echo "demo-secret-key-for-testing-only-12345")

# Arguments
TENANT="${1:-acme-corp}"
USER="${2:-alice}"
ROLE="${3:-admin}"

# Generate token
TOKEN=$(generate_jwt "$JWT_SECRET" "$USER" "$TENANT" "$ROLE" 3600)

echo "=========================================="
echo "  FS9 Multi-tenant Demo: $TENANT"
echo "=========================================="
echo ""
echo "User:      $USER"
echo "Namespace: $TENANT"
echo "Role:      $ROLE"
echo ""

# Helper function for API calls
api() {
    local method="$1"
    local endpoint="$2"
    shift 2
    
    curl -s -X "$method" "$SERVER$endpoint" \
        -H "Authorization: Bearer $TOKEN" \
        -H "Content-Type: application/json" \
        "$@"
}

api_raw() {
    local method="$1"
    local endpoint="$2"
    shift 2
    
    curl -s -X "$method" "$SERVER$endpoint" \
        -H "Authorization: Bearer $TOKEN" \
        "$@"
}

# Check connection
echo "[1/7] Checking connection..."
if api GET "/health" > /dev/null; then
    echo "  ✅ Server is reachable"
else
    echo "  ❌ Cannot connect to server"
    exit 1
fi

# Mount memfs to root (if not already mounted)
echo ""
echo "[2/7] Setting up filesystem (mount memfs at /)..."
MOUNT_RESP=$(api POST "/api/v1/mount" -d '{"path": "/", "provider": "memfs", "config": {}}' 2>&1)
if echo "$MOUNT_RESP" | grep -q "error"; then
    echo "  ⏭️  Already mounted or error: $(echo $MOUNT_RESP | python3 -c 'import sys,json; print(json.load(sys.stdin).get("error","unknown"))' 2>/dev/null || echo "$MOUNT_RESP")"
else
    echo "  ✅ Mounted memfs at /"
fi

# List current mounts
echo ""
echo "[3/7] Current mounts:"
api GET "/api/v1/mounts" | python3 -m json.tool 2>/dev/null || echo "  (no mounts)"

# Create directory structure
echo ""
echo "[4/7] Creating directory structure..."

# Create some files to establish directories
for dir in "projects" "shared" "tmp"; do
    # Create a hidden file to "create" the directory (memfs auto-creates parent dirs)
    RESP=$(api POST "/api/v1/open" -d "{\"path\": \"/$dir/.keep\", \"flags\": 578}")
    HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)
    if [ -n "$HANDLE" ]; then
        api POST "/api/v1/close" -d "{\"handle_id\": \"$HANDLE\"}" > /dev/null
        echo "  ✅ Created /$dir/"
    fi
done

# Write some files
echo ""
echo "[5/7] Writing files..."

write_file() {
    local path="$1"
    local content="$2"
    
    # Open for write+create+truncate (O_WRONLY|O_CREAT|O_TRUNC = 0x242 = 578)
    RESP=$(api POST "/api/v1/open" -d "{\"path\": \"$path\", \"flags\": 578}")
    HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)
    
    if [ -z "$HANDLE" ]; then
        echo "  ❌ Failed to open $path"
        return 1
    fi
    
    # Write content
    api_raw POST "/api/v1/write?handle_id=$HANDLE&offset=0" --data-binary "$content" > /dev/null
    
    # Close
    api POST "/api/v1/close" -d "{\"handle_id\": \"$HANDLE\"}" > /dev/null
    
    echo "  ✅ Wrote $path (${#content} bytes)"
}

write_file "/projects/readme.md" "# $TENANT Projects

Welcome to the $TENANT workspace!

Created by: $USER
Namespace: $TENANT
"

write_file "/projects/config.json" "{
  \"tenant\": \"$TENANT\",
  \"created_by\": \"$USER\",
  \"created_at\": \"$(date -Iseconds)\",
  \"features\": [\"multi-tenant\", \"isolated\", \"secure\"]
}"

write_file "/shared/notes.txt" "Shared notes for $TENANT team.

This file is only visible to users in the $TENANT namespace.
Other tenants cannot see this file.
"

write_file "/tmp/session-$(date +%s).log" "Session started at $(date)
User: $USER
Tenant: $TENANT
Role: $ROLE
"

# List files
echo ""
echo "[6/7] Listing files..."

list_dir() {
    local path="$1"
    echo ""
    echo "  📁 $path:"
    RESP=$(api GET "/api/v1/readdir?path=$path" 2>&1)
    if echo "$RESP" | python3 -c 'import sys,json; files=json.load(sys.stdin); [print(f"     {\"📁\" if f.get(\"is_dir\") else \"📄\"} {f[\"path\"]} ({f[\"size\"]} bytes)") for f in files]' 2>/dev/null; then
        :
    else
        echo "     (empty or error)"
    fi
}

list_dir "/"
list_dir "/projects"
list_dir "/shared"
list_dir "/tmp"

# Read a file content
echo ""
echo "[7/7] Reading file content..."
echo ""
echo "  📄 /projects/readme.md:"
echo "  ─────────────────────────"

# Open for read
RESP=$(api POST "/api/v1/open" -d '{"path": "/projects/readme.md", "flags": 0}')
HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)

if [ -n "$HANDLE" ]; then
    # Read content
    CONTENT=$(api POST "/api/v1/read" -d "{\"handle_id\": \"$HANDLE\", \"offset\": 0, \"size\": 4096}")
    echo "$CONTENT" | sed 's/^/  /'
    
    # Close
    api POST "/api/v1/close" -d "{\"handle_id\": \"$HANDLE\"}" > /dev/null
fi

echo ""
echo "=========================================="
echo "  Demo Complete!"
echo "=========================================="
echo ""
echo "This was namespace: $TENANT"
echo "Files created here are ONLY visible to $TENANT users."
echo ""
echo "Try another tenant to see isolation:"
echo "  ./03-tenant-demo.sh beta-startup dave"
echo ""
