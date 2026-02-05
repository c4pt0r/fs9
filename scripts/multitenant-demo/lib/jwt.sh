#!/bin/bash
# JWT Token Generator for FS9 Demo
# Generates HS256 JWT using bash + base64 + openssl

# Base64 URL encode
base64url_encode() {
    openssl base64 -e -A | tr '+/' '-_' | tr -d '='
}

# Generate JWT token
# Usage: generate_jwt SECRET SUBJECT NAMESPACE ROLE TTL_SECONDS
generate_jwt() {
    local secret="$1"
    local subject="$2"
    local namespace="$3"
    local role="$4"
    local ttl="${5:-3600}"
    
    local now=$(date +%s)
    local exp=$((now + ttl))
    
    # Header
    local header='{"alg":"HS256","typ":"JWT"}'
    local header_b64=$(echo -n "$header" | base64url_encode)
    
    # Payload
    local payload="{\"sub\":\"$subject\",\"ns\":\"$namespace\",\"roles\":[\"$role\"],\"iat\":$now,\"exp\":$exp}"
    local payload_b64=$(echo -n "$payload" | base64url_encode)
    
    # Signature
    local signature=$(echo -n "${header_b64}.${payload_b64}" | openssl dgst -sha256 -hmac "$secret" -binary | base64url_encode)
    
    echo "${header_b64}.${payload_b64}.${signature}"
}

# Decode JWT (for debugging)
decode_jwt() {
    local token="$1"
    local payload=$(echo "$token" | cut -d'.' -f2)
    # Add padding if needed
    local pad=$((4 - ${#payload} % 4))
    [ $pad -ne 4 ] && payload="${payload}$(printf '=%.0s' $(seq 1 $pad))"
    echo "$payload" | tr '_-' '/+' | base64 -d 2>/dev/null
}
