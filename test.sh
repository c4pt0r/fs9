#!/bin/bash
# FS9 完整使用示例

set -e

# 0. 编译（如果还没编译）
cd /Users/dongxu/fs9
cargo build --release

# 1. 杀掉之前的 server（如果有）
pkill -f fs9-server || true
sleep 1

# 2. 设置 JWT Secret 并启动 Server
export FS9_JWT_SECRET="my-super-secret-key-12345"
./target/release/fs9-server &
SERVER_PID=$!
sleep 2

# 3. 创建 Namespace（忽略已存在的错误）
./target/release/fs9-admin \
  -s http://localhost:9999 \
  --secret "$FS9_JWT_SECRET" \
  ns create myns 2>/dev/null || true

# 4. 生成 admin Token 用于 mount 操作
ADMIN_TOKEN=$(./target/release/fs9-admin \
  -s http://localhost:9999 \
  --secret "$FS9_JWT_SECRET" \
  token generate -u admin -n myns -r admin 2>/dev/null | sed -n '3p')

# 5. Mount pagefs 到 / （需要 admin/operator 权限）
curl -s -X POST http://localhost:9999/api/v1/mount \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"path": "/", "provider": "pagefs", "config": {}}' || true

# 6. 生成用户 Token (read-write 权限)
TOKEN=$(./target/release/fs9-admin \
  -s http://localhost:9999 \
  --secret "$FS9_JWT_SECRET" \
  token generate -u alice -n myns -r read-write 2>/dev/null | sed -n '3p')

echo "Token: $TOKEN"

# 7. 使用 sh9 交互
./target/release/sh9 -s http://localhost:9999 -t "$TOKEN"

# sh9 里面可以执行:
#   pwd
#   mkdir mydir
#   echo "hello world" > test.txt
#   cat test.txt
#   ls -la
#   cd mydir
#   exit
