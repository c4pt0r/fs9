#!/bin/bash
# FS9 Multi-tenant Demo: Verify Isolation
# Verifies data isolation between tenants

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

gen_token() {
    "$FS9_ADMIN" -s "$SERVER" --secret "$JWT_SECRET" token generate -u "$1" -n "$2" -r "$3" -T 3600 -q
}

echo "=========================================="
echo "  FS9 Multi-tenant Isolation Verification"
echo "=========================================="
echo ""

ACME_TOKEN=$(gen_token "alice" "acme-corp" "admin")
BETA_TOKEN=$(gen_token "dave" "beta-startup" "admin")
GAMMA_TOKEN=$(gen_token "frank" "gamma-labs" "admin")
GHOST_TOKEN=$(gen_token "hacker" "ghost-ns" "admin")

api() {
    local token="$1"
    local method="$2"
    local endpoint="$3"
    shift 3

    curl -s -X "$method" "$SERVER$endpoint" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        "$@"
}

api_code() {
    local token="$1"
    local method="$2"
    local endpoint="$3"
    shift 3

    curl -s -o /dev/null -w "%{http_code}" -X "$method" "$SERVER$endpoint" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        "$@"
}

PASS=0
FAIL=0

check() {
    local desc="$1"
    local expected="$2"
    local actual="$3"

    if [ "$expected" = "$actual" ]; then
        echo "  ✅ $desc"
        ((PASS++))
    else
        echo "  ❌ $desc (expected $expected, got $actual)"
        ((FAIL++))
    fi
}

# Test 1: Unknown namespace rejected
echo "[Test 1] Unknown namespace → 403"
CODE=$(api_code "$GHOST_TOKEN" GET "/api/v1/stat?path=/")
check "ghost-ns namespace rejected" "403" "$CODE"

# Test 2: Each tenant can only see their own data
echo ""
echo "[Test 2] Data isolation between tenants"

for tenant in "acme-corp" "beta-startup" "gamma-labs"; do
    token=$(gen_token "testuser" "$tenant" "admin")

    api "$token" POST "/api/v1/mount" -d '{"path": "/", "provider": "memfs", "config": {}}' > /dev/null 2>&1 || true

    RESP=$(api "$token" POST "/api/v1/open" -d "{\"path\": \"/marker-$tenant.txt\", \"flags\": 578}")
    HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)
    if [ -n "$HANDLE" ]; then
        curl -s -X POST "$SERVER/api/v1/write?handle_id=$HANDLE&offset=0" \
            -H "Authorization: Bearer $token" \
            --data-binary "This is $tenant" > /dev/null
        api "$token" POST "/api/v1/close" -d "{\"handle_id\": \"$HANDLE\"}" > /dev/null
    fi
done

for tenant in "acme-corp" "beta-startup" "gamma-labs"; do
    token=$(gen_token "testuser" "$tenant" "admin")

    CODE=$(api_code "$token" GET "/api/v1/stat?path=/marker-$tenant.txt")
    check "$tenant can see own marker" "200" "$CODE"

    for other in "acme-corp" "beta-startup" "gamma-labs"; do
        if [ "$other" != "$tenant" ]; then
            CODE=$(api_code "$token" GET "/api/v1/stat?path=/marker-$other.txt")
            check "$tenant cannot see $other marker" "404" "$CODE"
        fi
    done
done

# Test 3: Handle isolation
echo ""
echo "[Test 3] Handle isolation between tenants"

RESP=$(api "$ACME_TOKEN" POST "/api/v1/open" -d '{"path": "/acme-handle-test.txt", "flags": 578}')
ACME_HANDLE=$(echo "$RESP" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("handle_id",""))' 2>/dev/null)

if [ -n "$ACME_HANDLE" ]; then
    CODE=$(api_code "$ACME_TOKEN" POST "/api/v1/read" -d "{\"handle_id\": \"$ACME_HANDLE\", \"offset\": 0, \"size\": 100}")
    check "ACME can use own handle" "200" "$CODE"

    CODE=$(api_code "$BETA_TOKEN" POST "/api/v1/read" -d "{\"handle_id\": \"$ACME_HANDLE\", \"offset\": 0, \"size\": 100}")
    check "Beta cannot use ACME handle" "400" "$CODE"

    api "$ACME_TOKEN" POST "/api/v1/close" -d "{\"handle_id\": \"$ACME_HANDLE\"}" > /dev/null
fi

# Test 4: Role-based access control
echo ""
echo "[Test 4] Role-based access control"

ADMIN_TOKEN=$(gen_token "superadmin" "admin" "admin")
CODE=$(api_code "$ADMIN_TOKEN" POST "/api/v1/namespaces" -d '{"name": "test-rbac-ns"}')
if [ "$CODE" = "201" ] || [ "$CODE" = "409" ]; then
    check "Admin can create namespace" "OK" "OK"
else
    check "Admin can create namespace" "201/409" "$CODE"
fi

OPERATOR_TOKEN=$(gen_token "ops" "acme-corp" "operator")
CODE=$(api_code "$OPERATOR_TOKEN" POST "/api/v1/namespaces" -d '{"name": "should-fail-ns"}')
check "Operator cannot create namespace" "403" "$CODE"

READER_TOKEN=$(gen_token "reader" "acme-corp" "read-only")
CODE=$(api_code "$READER_TOKEN" GET "/api/v1/namespaces")
check "Reader cannot list namespaces" "403" "$CODE"

# Test 5: No auth = rejected
echo ""
echo "[Test 5] Authentication required"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X GET "$SERVER/api/v1/stat?path=/")
check "No token → 401" "401" "$CODE"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X GET "$SERVER/api/v1/namespaces")
check "No token on namespace API → 401" "401" "$CODE"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X GET "$SERVER/health")
check "Health endpoint no auth needed" "200" "$CODE"

echo ""
echo "=========================================="
echo "  Summary"
echo "=========================================="
echo ""
echo "  ✅ Passed: $PASS"
echo "  ❌ Failed: $FAIL"
echo ""

if [ $FAIL -eq 0 ]; then
    echo "  🎉 All isolation tests passed!"
    exit 0
else
    echo "  ⚠️  Some tests failed"
    exit 1
fi
