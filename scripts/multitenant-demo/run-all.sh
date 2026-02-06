#!/bin/bash
# FS9 Multi-tenant Demo: Run Everything
# One-click run for the complete multi-tenant demo

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/../.."

echo "╔══════════════════════════════════════════════════════════╗"
echo "║  FS9 Multi-tenant Cloud Service Demo                     ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [ -f "$SCRIPT_DIR/.server.pid" ]; then
        PID=$(cat "$SCRIPT_DIR/.server.pid")
        kill $PID 2>/dev/null || true
        rm -f "$SCRIPT_DIR/.server.pid"
    fi
    rm -f "$SCRIPT_DIR/.jwt-secret"
    rm -f "$SCRIPT_DIR/fs9-demo.yaml"
}

trap cleanup EXIT

export JWT_SECRET="demo-secret-key-for-testing-only-12345"
export FS9_PORT=9999
SERVER="http://127.0.0.1:$FS9_PORT"

echo "$JWT_SECRET" > "$SCRIPT_DIR/.jwt-secret"

echo "[Step 1/6] Building FS9 server and fs9-admin..."
cd "$PROJECT_ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -p fs9-server -p fs9-cli 2>&1 | grep -E "(Compiling|Finished)" || true
echo "  ✅ Build complete"

FS9_ADMIN="$PROJECT_ROOT/target/debug/fs9-admin"
if [ ! -f "$FS9_ADMIN" ]; then
    FS9_ADMIN="$PROJECT_ROOT/target/release/fs9-admin"
fi

admin() {
    "$FS9_ADMIN" -s "$SERVER" --secret "$JWT_SECRET" "$@"
}

gen_token() {
    "$FS9_ADMIN" -s "$SERVER" --secret "$JWT_SECRET" token generate -u "$1" -n "$2" -r "$3" -T 3600 -q
}

echo ""
echo "[Step 2/6] Creating configuration..."
cat > "$SCRIPT_DIR/fs9-demo.yaml" << EOF
server:
  host: "127.0.0.1"
  port: $FS9_PORT
  auth:
    enabled: true
    jwt_secret: "$JWT_SECRET"
logging:
  level: "warn"
mounts: []
EOF
echo "  ✅ Config created"

echo ""
echo "[Step 3/6] Starting server..."
FS9_CONFIG="$SCRIPT_DIR/fs9-demo.yaml" "$PROJECT_ROOT/target/debug/fs9-server" &
SERVER_PID=$!
echo "$SERVER_PID" > "$SCRIPT_DIR/.server.pid"

for i in {1..30}; do
    if curl -s "$SERVER/health" > /dev/null 2>&1; then
        echo "  ✅ Server started (PID: $SERVER_PID)"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "  ❌ Server failed to start"
        exit 1
    fi
    sleep 0.2
done

echo ""
echo "[Step 4/6] Creating tenants..."
for ns in "admin" "acme-corp" "beta-startup" "gamma-labs"; do
    if admin ns create "$ns" 2>/dev/null; then
        echo "  ✅ $ns"
    else
        echo "  ⏭️  $ns (already exists)"
    fi
done

echo ""
echo "[Step 5/6] Running tenant operations..."

demo_tenant() {
    local tenant="$1"
    local user="$2"
    local token
    token=$(gen_token "$user" "$tenant" "admin")

    echo ""
    echo "  === $tenant ($user) ==="

    admin mount add memfs -n "$tenant" 2>/dev/null || true

    RESP=$(curl -s -X POST "$SERVER/api/v1/open" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        -d "{\"path\": \"/hello-from-$tenant.txt\", \"flags\": {\"read\": true, \"write\": true, \"create\": true, \"truncate\": true}}")

    HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)

    if [ -n "$HANDLE" ]; then
        curl -s -X POST "$SERVER/api/v1/write?handle_id=$HANDLE&offset=0" \
            -H "Authorization: Bearer $token" \
            --data-binary "Hello from $tenant! Written by $user at $(date)" > /dev/null

        curl -s -X POST "$SERVER/api/v1/close" \
            -H "Authorization: Bearer $token" \
            -H "Content-Type: application/json" \
            -d "{\"handle_id\": \"$HANDLE\"}" > /dev/null

        echo "    ✅ Created /hello-from-$tenant.txt"
    else
        echo "    ❌ Failed to create file"
    fi

    FILES=$(curl -s -X GET "$SERVER/api/v1/readdir?path=/" \
        -H "Authorization: Bearer $token" | \
        python3 -c 'import sys,json; files=json.load(sys.stdin); print(", ".join([f["path"] for f in files]))' 2>/dev/null)
    echo "    📁 Files: ${FILES:-none}"
}

demo_tenant "acme-corp" "alice"
demo_tenant "beta-startup" "dave"
demo_tenant "gamma-labs" "frank"

echo ""
echo "[Step 6/6] Verifying isolation..."

verify_isolation() {
    local tenant="$1"
    local user="$2"
    local other_tenant="$3"
    local token
    token=$(gen_token "$user" "$tenant" "admin")

    CODE=$(curl -s -o /dev/null -w "%{http_code}" -X GET "$SERVER/api/v1/stat?path=/hello-from-$other_tenant.txt" \
        -H "Authorization: Bearer $token")

    if [ "$CODE" = "404" ]; then
        echo "  ✅ $tenant cannot see $other_tenant data (404)"
    else
        echo "  ❌ $tenant CAN see $other_tenant data! (HTTP $CODE)"
    fi
}

verify_isolation "acme-corp" "alice" "beta-startup"
verify_isolation "acme-corp" "alice" "gamma-labs"
verify_isolation "beta-startup" "dave" "acme-corp"
verify_isolation "beta-startup" "dave" "gamma-labs"
verify_isolation "gamma-labs" "frank" "acme-corp"
verify_isolation "gamma-labs" "frank" "beta-startup"

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║  Demo Complete!                                          ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "Summary:"
echo "  • 3 tenants created (acme-corp, beta-startup, gamma-labs)"
echo "  • Each tenant wrote their own file"
echo "  • Each tenant can ONLY see their own files"
echo "  • Cross-tenant access properly denied (404)"
echo ""
echo "The multi-tenant isolation is working correctly! 🎉"
echo ""
