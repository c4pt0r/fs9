#!/bin/bash
# FS9 Multi-tenant Demo: Setup Tenants
# Creates three tenants, each with different users

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib/jwt.sh"

SERVER="http://127.0.0.1:9999"
JWT_SECRET=$(cat "$SCRIPT_DIR/.jwt-secret" 2>/dev/null || echo "demo-secret-key-for-testing-only-12345")

echo "=========================================="
echo "  FS9 Multi-tenant Demo: Setup Tenants"
echo "=========================================="
echo ""

# First create a bootstrap namespace for management operations
# (JWT must have ns field, we use "admin" namespace)
echo "[1/4] Creating admin namespace for management..."

# Generate admin token (ns=admin, role=admin)
ADMIN_TOKEN=$(generate_jwt "$JWT_SECRET" "superadmin" "admin" "admin" 3600)

# Try to create admin namespace (may already exist)
RESP=$(curl -s -w "\n%{http_code}" -X POST "$SERVER/api/v1/namespaces" \
    -H "Authorization: Bearer $ADMIN_TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"name": "admin"}')

HTTP_CODE=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')

if [ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "409" ]; then
    echo "  ✅ admin namespace ready"
else
    echo "  ⚠️  Response ($HTTP_CODE): $BODY"
fi

# Create three tenants
echo ""
echo "[2/4] Creating tenant namespaces..."

TENANTS=("acme-corp" "beta-startup" "gamma-labs")

for tenant in "${TENANTS[@]}"; do
    RESP=$(curl -s -w "\n%{http_code}" -X POST "$SERVER/api/v1/namespaces" \
        -H "Authorization: Bearer $ADMIN_TOKEN" \
        -H "Content-Type: application/json" \
        -d "{\"name\": \"$tenant\"}")
    
    HTTP_CODE=$(echo "$RESP" | tail -1)
    BODY=$(echo "$RESP" | sed '$d')
    
    if [ "$HTTP_CODE" = "201" ]; then
        echo "  ✅ Created: $tenant"
    elif [ "$HTTP_CODE" = "409" ]; then
        echo "  ⏭️  Already exists: $tenant"
    else
        echo "  ❌ Failed ($HTTP_CODE): $tenant - $BODY"
    fi
done

# List all namespaces
echo ""
echo "[3/4] Listing all namespaces..."
curl -s "$SERVER/api/v1/namespaces" \
    -H "Authorization: Bearer $ADMIN_TOKEN" | python3 -m json.tool

# Generate sample user tokens for each tenant
echo ""
echo "[4/4] Generating user tokens..."
echo ""

mkdir -p "$SCRIPT_DIR/tokens"

# ACME Corp
echo "=== ACME Corp ===" | tee "$SCRIPT_DIR/tokens/acme-corp.env"
ACME_ADMIN=$(generate_jwt "$JWT_SECRET" "alice" "acme-corp" "admin" 86400)
ACME_OPERATOR=$(generate_jwt "$JWT_SECRET" "bob" "acme-corp" "operator" 86400)
ACME_USER=$(generate_jwt "$JWT_SECRET" "charlie" "acme-corp" "reader" 86400)
echo "ACME_ADMIN_TOKEN=$ACME_ADMIN" >> "$SCRIPT_DIR/tokens/acme-corp.env"
echo "ACME_OPERATOR_TOKEN=$ACME_OPERATOR" >> "$SCRIPT_DIR/tokens/acme-corp.env"
echo "ACME_USER_TOKEN=$ACME_USER" >> "$SCRIPT_DIR/tokens/acme-corp.env"
echo "  alice (admin), bob (operator), charlie (reader)"

# Beta Startup
echo "=== Beta Startup ===" | tee "$SCRIPT_DIR/tokens/beta-startup.env"
BETA_ADMIN=$(generate_jwt "$JWT_SECRET" "dave" "beta-startup" "admin" 86400)
BETA_USER=$(generate_jwt "$JWT_SECRET" "eve" "beta-startup" "reader" 86400)
echo "BETA_ADMIN_TOKEN=$BETA_ADMIN" >> "$SCRIPT_DIR/tokens/beta-startup.env"
echo "BETA_USER_TOKEN=$BETA_USER" >> "$SCRIPT_DIR/tokens/beta-startup.env"
echo "  dave (admin), eve (reader)"

# Gamma Labs
echo "=== Gamma Labs ===" | tee "$SCRIPT_DIR/tokens/gamma-labs.env"
GAMMA_ADMIN=$(generate_jwt "$JWT_SECRET" "frank" "gamma-labs" "admin" 86400)
GAMMA_OPERATOR=$(generate_jwt "$JWT_SECRET" "grace" "gamma-labs" "operator" 86400)
echo "GAMMA_ADMIN_TOKEN=$GAMMA_ADMIN" >> "$SCRIPT_DIR/tokens/gamma-labs.env"
echo "GAMMA_OPERATOR_TOKEN=$GAMMA_OPERATOR" >> "$SCRIPT_DIR/tokens/gamma-labs.env"
echo "  frank (admin), grace (operator)"

echo ""
echo "=========================================="
echo "  Setup Complete!"
echo "=========================================="
echo ""
echo "Tokens saved to: $SCRIPT_DIR/tokens/"
echo ""
echo "Next steps:"
echo "  ./03-tenant-demo.sh acme-corp alice    # ACME admin operations"
echo "  ./03-tenant-demo.sh beta-startup dave  # Beta admin operations"
echo "  ./03-tenant-demo.sh gamma-labs frank   # Gamma admin operations"
echo ""
