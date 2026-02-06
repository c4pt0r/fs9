#!/bin/bash
# FS9 Multi-tenant Demo: Tenant Operations
# Simulates user operations under a specific tenant

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

SERVER="http://127.0.0.1:9999"
JWT_SECRET=$(cat "$SCRIPT_DIR/.jwt-secret" 2>/dev/null || echo "demo-secret-key-for-testing-only-12345")

FS9_ADMIN="$PROJECT_ROOT/target/debug/fs9-admin"
if [ ! -f "$FS9_ADMIN" ]; then
    FS9_ADMIN="$PROJECT_ROOT/target/release/fs9-admin"
fi
if [ ! -f "$FS9_ADMIN" ]; then
    echo "❌ fs9-admin not found. Run: cargo build -p fs9-cli"
    exit 1
fi

admin() {
    "$FS9_ADMIN" -s "$SERVER" --secret "$JWT_SECRET" "$@"
}

TENANT="${1:-acme-corp}"
USER="${2:-alice}"
ROLE="${3:-admin}"

TOKEN=$(admin token generate -u "$USER" -n "$TENANT" -r "$ROLE" -T 3600 -q)

echo "=========================================="
echo "  FS9 Multi-tenant Demo: $TENANT"
echo "=========================================="
echo ""
echo "User:      $USER"
echo "Namespace: $TENANT"
echo "Role:      $ROLE"
echo ""

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

echo "[1/7] Checking connection..."
if api GET "/health" > /dev/null; then
    echo "  ✅ Server is reachable"
else
    echo "  ❌ Cannot connect to server"
    exit 1
fi

echo ""
echo "[2/7] Setting up filesystem (mount memfs at /)..."
if admin mount add memfs -n "$TENANT" 2>/dev/null; then
    echo "  ✅ Mounted memfs at /"
else
    echo "  ⏭️  Already mounted or error"
fi

echo ""
echo "[3/7] Current mounts:"
admin mount list -n "$TENANT"

echo ""
echo "[4/7] Creating directory structure..."

for dir in "projects" "shared" "tmp"; do
    RESP=$(api POST "/api/v1/open" -d "{\"path\": \"/$dir/.keep\", \"flags\": 578}")
    HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)
    if [ -n "$HANDLE" ]; then
        api POST "/api/v1/close" -d "{\"handle_id\": \"$HANDLE\"}" > /dev/null
        echo "  ✅ Created /$dir/"
    fi
done

echo ""
echo "[5/7] Writing files..."

write_file() {
    local path="$1"
    local content="$2"

    RESP=$(api POST "/api/v1/open" -d "{\"path\": \"$path\", \"flags\": 578}")
    HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)

    if [ -z "$HANDLE" ]; then
        echo "  ❌ Failed to open $path"
        return 1
    fi

    api_raw POST "/api/v1/write?handle_id=$HANDLE&offset=0" --data-binary "$content" > /dev/null
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

echo ""
echo "[7/7] Reading file content..."
echo ""
echo "  📄 /projects/readme.md:"
echo "  ─────────────────────────"

RESP=$(api POST "/api/v1/open" -d '{"path": "/projects/readme.md", "flags": 0}')
HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)

if [ -n "$HANDLE" ]; then
    CONTENT=$(api POST "/api/v1/read" -d "{\"handle_id\": \"$HANDLE\", \"offset\": 0, \"size\": 4096}")
    echo "$CONTENT" | sed 's/^/  /'
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
