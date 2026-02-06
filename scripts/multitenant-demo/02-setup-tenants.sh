#!/bin/bash
# FS9 Multi-tenant Demo: Setup Tenants
# Creates three tenants, each with different users

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

# Helper: run fs9-admin with server and secret
admin() {
    "$FS9_ADMIN" -s "$SERVER" --secret "$JWT_SECRET" "$@"
}

echo "=========================================="
echo "  FS9 Multi-tenant Demo: Setup Tenants"
echo "=========================================="
echo ""

# First create a bootstrap namespace for management operations
echo "[1/4] Creating admin namespace for management..."
if admin ns create admin 2>/dev/null; then
    echo "  ✅ admin namespace ready"
else
    echo "  ⏭️  admin namespace already exists"
fi

# Create three tenants
echo ""
echo "[2/4] Creating tenant namespaces..."

TENANTS=("acme-corp" "beta-startup" "gamma-labs")

for tenant in "${TENANTS[@]}"; do
    if admin ns create "$tenant" 2>/dev/null; then
        echo "  ✅ Created: $tenant"
    else
        echo "  ⏭️  Already exists: $tenant"
    fi
done

# List all namespaces
echo ""
echo "[3/4] Listing all namespaces..."
admin ns list

# Generate sample user tokens for each tenant
echo ""
echo "[4/4] Generating user tokens..."
echo ""

mkdir -p "$SCRIPT_DIR/tokens"

# ACME Corp
echo "=== ACME Corp ===" | tee "$SCRIPT_DIR/tokens/acme-corp.env"
ACME_ADMIN=$(admin token generate -u alice -n acme-corp -r admin -T 86400 -q)
ACME_OPERATOR=$(admin token generate -u bob -n acme-corp -r operator -T 86400 -q)
ACME_USER=$(admin token generate -u charlie -n acme-corp -r read-only -T 86400 -q)
echo "ACME_ADMIN_TOKEN=$ACME_ADMIN" >> "$SCRIPT_DIR/tokens/acme-corp.env"
echo "ACME_OPERATOR_TOKEN=$ACME_OPERATOR" >> "$SCRIPT_DIR/tokens/acme-corp.env"
echo "ACME_USER_TOKEN=$ACME_USER" >> "$SCRIPT_DIR/tokens/acme-corp.env"
echo "  alice (admin), bob (operator), charlie (read-only)"

# Beta Startup
echo "=== Beta Startup ===" | tee "$SCRIPT_DIR/tokens/beta-startup.env"
BETA_ADMIN=$(admin token generate -u dave -n beta-startup -r admin -T 86400 -q)
BETA_USER=$(admin token generate -u eve -n beta-startup -r read-only -T 86400 -q)
echo "BETA_ADMIN_TOKEN=$BETA_ADMIN" >> "$SCRIPT_DIR/tokens/beta-startup.env"
echo "BETA_USER_TOKEN=$BETA_USER" >> "$SCRIPT_DIR/tokens/beta-startup.env"
echo "  dave (admin), eve (read-only)"

# Gamma Labs
echo "=== Gamma Labs ===" | tee "$SCRIPT_DIR/tokens/gamma-labs.env"
GAMMA_ADMIN=$(admin token generate -u frank -n gamma-labs -r admin -T 86400 -q)
GAMMA_OPERATOR=$(admin token generate -u grace -n gamma-labs -r operator -T 86400 -q)
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
