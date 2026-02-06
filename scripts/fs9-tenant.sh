#!/bin/bash
#
# FS9 Tenant Management Script
#
# Usage:
#   ./scripts/fs9-tenant.sh create-tenant <name>     # Create namespace + admin user + token
#   ./scripts/fs9-tenant.sh create-user <ns> <user>  # Create user in namespace
#   ./scripts/fs9-tenant.sh list-namespaces          # List all namespaces
#   ./scripts/fs9-tenant.sh list-users <ns>          # List users in namespace
#   ./scripts/fs9-tenant.sh generate-token <user_id> # Generate token for user
#   ./scripts/fs9-tenant.sh delete-namespace <name>  # Delete namespace
#
# Environment:
#   FS9_META_URL  - Meta service URL (default: http://localhost:9998)
#   FS9_META_KEY  - Admin key (default: admin-key-change-me)

set -e

META_URL="${FS9_META_URL:-http://localhost:9998}"
SERVER_URL="${FS9_SERVER_URL:-http://localhost:9999}"
META_KEY="${FS9_META_KEY:-admin-key-change-me}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() { echo -e "${BLUE}[INFO]${NC} $1"; }
success() { echo -e "${GREEN}[OK]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Check if jq is available
if ! command -v jq &> /dev/null; then
    error "jq is required. Install with: apt install jq (or brew install jq)"
fi

# API helper
api() {
    local method="$1"
    local endpoint="$2"
    local data="$3"
    
    if [ -n "$data" ]; then
        curl -sf -X "$method" \
            -H "Content-Type: application/json" \
            -H "x-fs9-meta-key: $META_KEY" \
            -d "$data" \
            "${META_URL}${endpoint}" 2>/dev/null || echo '{"error": "connection failed"}'
    else
        curl -sf -X "$method" \
            -H "x-fs9-meta-key: $META_KEY" \
            "${META_URL}${endpoint}" 2>/dev/null || echo '{"error": "connection failed"}'
    fi
}

# Commands

cmd_create_tenant() {
    local name="$1"
    [ -z "$name" ] && error "Usage: create-tenant <name>"
    
    # Check meta service is reachable
    if ! curl -sf "${META_URL}/health" > /dev/null 2>&1; then
        error "Cannot connect to meta service at ${META_URL}. Is it running?"
    fi
    
    info "Creating namespace: $name"
    
    # Create namespace
    local ns_result
    ns_result=$(api POST "/api/v1/admin/namespaces" "{\"name\": \"$name\"}")
    
    if echo "$ns_result" | jq -e '.id' > /dev/null 2>&1; then
        success "Namespace created"
    else
        error "Failed to create namespace: $(echo "$ns_result" | jq -r '.detail // .error // .')"
    fi
    
    # Create admin user
    info "Creating admin user: admin"
    local user_result
    user_result=$(api POST "/api/v1/admin/namespaces/$name/users" '{"username": "admin", "roles": ["admin"]}')
    
    local user_id
    if user_id=$(echo "$user_result" | jq -r '.id // empty' 2>/dev/null) && [ -n "$user_id" ]; then
        success "Admin user created (id: $user_id)"
    else
        error "Failed to create user: $(echo "$user_result" | jq -r '.detail // .error // .')"
    fi
    
    # Generate token
    info "Generating token..."
    local token_result
    token_result=$(api POST "/api/v1/admin/tokens" "{\"user_id\": \"$user_id\", \"ttl_seconds\": 2592000}")
    
    local token
    if token=$(echo "$token_result" | jq -r '.token // empty' 2>/dev/null) && [ -n "$token" ]; then
        success "Token generated"
    else
        error "Failed to generate token: $(echo "$token_result" | jq -r '.detail // .error // .')"
    fi
    
    # Create namespace on fs9-server (if reachable)
    info "Creating namespace on fs9-server ($SERVER_URL)..."
    
    local ns_result
    ns_result=$(curl -sf -X POST -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        -d "{\"name\": \"$name\"}" \
        "${SERVER_URL}/api/v1/namespaces" 2>/dev/null || echo '{"error": "connection failed"}')
    
    if echo "$ns_result" | jq -e '.name' > /dev/null 2>&1; then
        success "Namespace created on server"
    elif echo "$ns_result" | grep -q "already exists"; then
        warn "Namespace already exists on server"
    else
        warn "Could not create namespace on server (server may be offline): $ns_result"
    fi
    
    echo ""
    echo "=========================================="
    echo -e "${GREEN}Tenant created successfully!${NC}"
    echo "=========================================="
    echo ""
    echo "Namespace: $name"
    echo "User:      admin"
    echo "User ID:   $user_id"
    echo ""
    echo "Token (valid 30 days):"
    echo -e "${YELLOW}$token${NC}"
    echo ""
    echo "To connect with sh9:"
    echo "  export FS9_SERVER_URL=$SERVER_URL"
    echo "  export FS9_TOKEN=$token"
    echo "  cargo run -p sh9"
    echo ""
}

cmd_create_user() {
    local namespace="$1"
    local username="$2"
    local roles="${3:-read-write}"
    
    [ -z "$namespace" ] || [ -z "$username" ] && error "Usage: create-user <namespace> <username> [roles]"
    
    info "Creating user '$username' in namespace '$namespace' with roles: $roles"
    
    # Convert roles to JSON array
    local roles_json
    roles_json=$(echo "$roles" | tr ',' '\n' | jq -R . | jq -s .)
    
    local result
    result=$(api POST "/api/v1/admin/namespaces/$namespace/users" "{\"username\": \"$username\", \"roles\": $roles_json}")
    
    local user_id
    if user_id=$(echo "$result" | jq -r '.id // empty' 2>/dev/null) && [ -n "$user_id" ]; then
        success "User created"
        echo ""
        echo "User ID: $user_id"
        echo ""
        echo "To generate a token:"
        echo "  $0 generate-token $user_id"
    else
        error "Failed: $(echo "$result" | jq -r '.detail // .error // .')"
    fi
}

cmd_list_namespaces() {
    info "Listing namespaces..."
    local result
    result=$(api GET "/api/v1/admin/namespaces")
    
    echo ""
    echo "$result" | jq -r '.[] | "  \(.name) (users: \(.user_count), created: \(.created_at))"'
    echo ""
}

cmd_list_users() {
    local namespace="$1"
    [ -z "$namespace" ] && error "Usage: list-users <namespace>"
    
    info "Listing users in namespace '$namespace'..."
    local result
    result=$(api GET "/api/v1/admin/namespaces/$namespace/users")
    
    echo ""
    echo "$result" | jq -r '.[] | "  \(.username) (id: \(.id), roles: \(.roles | join(",")), active: \(.active))"'
    echo ""
}

cmd_generate_token() {
    local user_id="$1"
    local ttl="${2:-2592000}"  # Default 30 days
    
    [ -z "$user_id" ] && error "Usage: generate-token <user_id> [ttl_seconds]"
    
    info "Generating token for user $user_id (TTL: ${ttl}s)..."
    
    local result
    result=$(api POST "/api/v1/admin/tokens" "{\"user_id\": \"$user_id\", \"ttl_seconds\": $ttl}")
    
    local token
    if token=$(echo "$result" | jq -r '.token // empty' 2>/dev/null) && [ -n "$token" ]; then
        echo ""
        echo "Namespace: $(echo "$result" | jq -r '.namespace')"
        echo "Roles:     $(echo "$result" | jq -r '.roles | join(",")')"
        echo "Expires:   $(echo "$result" | jq -r '.expires_at')"
        echo ""
        echo "Token:"
        echo -e "${YELLOW}$token${NC}"
        echo ""
    else
        error "Failed: $(echo "$result" | jq -r '.detail // .error // .')"
    fi
}

cmd_delete_namespace() {
    local name="$1"
    [ -z "$name" ] && error "Usage: delete-namespace <name>"
    
    warn "This will delete namespace '$name' and ALL its users!"
    read -p "Are you sure? (y/N) " -n 1 -r
    echo
    
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        local result
        result=$(api DELETE "/api/v1/admin/namespaces/$name")
        
        if echo "$result" | jq -e '.status == "deleted"' > /dev/null 2>&1; then
            success "Namespace deleted"
        else
            error "Failed: $(echo "$result" | jq -r '.detail // .error // .')"
        fi
    else
        info "Cancelled"
    fi
}

cmd_health() {
    info "Checking meta service health..."
    local result
    result=$(curl -s "${META_URL}/health")
    
    if echo "$result" | jq -e '.status == "ok"' > /dev/null 2>&1; then
        success "Meta service is healthy"
    else
        error "Meta service unhealthy: $result"
    fi
}

# Main

case "${1:-}" in
    create-tenant)
        cmd_create_tenant "$2"
        ;;
    create-user)
        cmd_create_user "$2" "$3" "$4"
        ;;
    list-namespaces|list-ns)
        cmd_list_namespaces
        ;;
    list-users)
        cmd_list_users "$2"
        ;;
    generate-token|token)
        cmd_generate_token "$2" "$3"
        ;;
    delete-namespace|delete-ns)
        cmd_delete_namespace "$2"
        ;;
    health)
        cmd_health
        ;;
    *)
        echo "FS9 Tenant Management"
        echo ""
        echo "Usage: $0 <command> [args]"
        echo ""
        echo "Commands:"
        echo "  create-tenant <name>           Create namespace + admin user + token"
        echo "  create-user <ns> <user> [roles] Create user (roles: read-only,read-write,admin)"
        echo "  list-namespaces                List all namespaces"
        echo "  list-users <namespace>         List users in namespace"
        echo "  generate-token <user_id> [ttl] Generate token for user"
        echo "  delete-namespace <name>        Delete namespace and all users"
        echo "  health                         Check meta service health"
        echo ""
        echo "Environment:"
        echo "  FS9_META_URL  Meta service URL (default: http://localhost:9998)"
        echo "  FS9_META_KEY  Admin key (default: admin-key-change-me)"
        ;;
esac
