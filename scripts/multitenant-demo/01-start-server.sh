#!/bin/bash
# FS9 Multi-tenant Demo: Start Server
# 启动 FS9 服务器，配置好 JWT 认证

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# 配置
export JWT_SECRET="demo-secret-key-for-testing-only-12345"
export FS9_PORT=9999

# 创建配置文件
CONFIG_FILE="$SCRIPT_DIR/fs9-demo.yaml"
cat > "$CONFIG_FILE" << EOF
server:
  host: "127.0.0.1"
  port: $FS9_PORT
  auth:
    enabled: true
    jwt_secret: "$JWT_SECRET"

logging:
  level: "info"
  filter: "fs9_server=info"

# 不预创建 mount，让每个 namespace 自己管理
mounts: []
EOF

echo "=========================================="
echo "  FS9 Multi-tenant Demo Server"
echo "=========================================="
echo ""
echo "Configuration:"
echo "  Port:       $FS9_PORT"
echo "  JWT Secret: $JWT_SECRET"
echo "  Config:     $CONFIG_FILE"
echo ""

# 构建（如果需要）
echo "[1/2] Building server..."
cd "$PROJECT_ROOT"
cargo build -p fs9-server --release 2>&1 | tail -3

# 启动服务器
echo ""
echo "[2/2] Starting server..."
echo ""
FS9_CONFIG="$CONFIG_FILE" RUST_LOG=info cargo run -p fs9-server --release 2>&1 &
SERVER_PID=$!

echo "Server PID: $SERVER_PID"
echo ""

# 等待服务器启动
echo "Waiting for server to be ready..."
for i in {1..30}; do
    if curl -s http://127.0.0.1:$FS9_PORT/health > /dev/null 2>&1; then
        echo "✅ Server is ready!"
        echo ""
        echo "To stop: kill $SERVER_PID"
        echo ""
        echo "Now run: ./02-setup-tenants.sh"
        break
    fi
    sleep 0.5
done

# 保存 PID 供后续脚本使用
echo "$SERVER_PID" > "$SCRIPT_DIR/.server.pid"
echo "$JWT_SECRET" > "$SCRIPT_DIR/.jwt-secret"

wait $SERVER_PID
