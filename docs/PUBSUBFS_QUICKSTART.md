# PubSubFS Quick Start Guide

快速上手 FS9 的 Pub/Sub 文件系统。

## 启动服务器

```bash
# 启动 FS9 服务器（自动加载 pubsubfs 插件）
make server

# 或手动启动
RUST_LOG=info cargo run -p fs9-server
```

服务器输出应该显示：
```
Loaded plugins from ./plugins count=5
Available plugins plugins=["pagefs", "pubsubfs", "streamfs", "kv", "hellofs"]
```

## 方式 1: 使用 sh9 Shell（推荐）

### 启动 sh9

```bash
# 在另一个终端启动 sh9
cargo run -p sh9
```

### 挂载 PubSubFS

```sh9
sh9:/> mount pubsubfs /pubsub
mounted pubsubfs at /pubsub

sh9:/> ls /
pubsub
```

### 创建 Topic

```sh9
sh9:/> echo "create chat" > /pubsub/.ctl

sh9:/> echo "create logs buffer_size=500" > /pubsub/.ctl

sh9:/> cat /pubsub/.topics
chat
logs
```

### 发布和订阅

**终端 1 - 订阅者**：
```sh9
sh9:/> cat /pubsub/topics/chat/sub
# 等待消息...
```

**终端 2 - 发布者**：
```sh9
sh9:/> echo "hello everyone!" > /pubsub/topics/chat/pub

sh9:/> echo "how are you?" > /pubsub/topics/chat/pub
```

**终端 1 会实时显示**：
```
[2024-01-28 20:10:15] hello everyone!
[2024-01-28 20:10:20] how are you?
```

### 查看 Topic 信息

```sh9
sh9:/> cat /pubsub/topics/chat/.info
name: chat
subscribers: 2
messages: 15
ring_size: 100
created: 2024-01-28 20:05:00
modified: 2024-01-28 20:10:20
```

### 多个订阅者

```sh9
# 终端 1: 订阅并过滤
sh9:/> cat /pubsub/topics/logs/sub | grep ERROR &

# 终端 2: 订阅并计数
sh9:/> cat /pubsub/topics/logs/sub | wc -l &

# 终端 3: 发布日志
sh9:/> echo "[INFO] Server started" > /pubsub/topics/logs/pub
sh9:/> echo "[ERROR] Connection failed" > /pubsub/topics/logs/pub
sh9:/> echo "[DEBUG] Processing request" > /pubsub/topics/logs/pub
```

## 方式 2: 使用 HTTP API

### 挂载 PubSubFS

```bash
curl -X POST http://localhost:9999/api/v1/mount \
  -H "Content-Type: application/json" \
  -d '{
    "path": "/pubsub",
    "provider": "pubsubfs"
  }'
```

### 创建 Topic

```bash
# 写入控制文件
echo "create mytopic" | curl -X POST http://localhost:9999/api/v1/write \
  -d @- \
  --data-urlencode 'path=/pubsub/.ctl'
```

### 发布消息

```bash
echo "hello world" | curl -X POST http://localhost:9999/api/v1/write \
  -d @- \
  --data-urlencode 'path=/pubsub/topics/mytopic/pub'
```

### 订阅消息

```bash
# 读取会持续返回新消息
curl "http://localhost:9999/api/v1/read?path=/pubsub/topics/mytopic/sub&size=4096"
```

## 方式 3: 使用 FUSE（高级）

### 挂载 FUSE

**终端 1 - FS9 服务器**：
```bash
RUST_LOG=info cargo run -p fs9-server
```

**终端 2 - FUSE 挂载**：
```bash
mkdir -p /tmp/fs9-mount
cargo run -p fs9-fuse -- /tmp/fs9-mount --server http://localhost:9999 --foreground
```

**终端 3 - 使用标准工具**：
```bash
cd /tmp/fs9-mount

# 挂载 pubsubfs
# (需要通过 API 或 sh9 先挂载)

# 然后可以用标准 shell 命令
echo "create demo" > pubsub/.ctl
echo "hello" > pubsub/topics/demo/pub
cat pubsub/topics/demo/sub
```

## 实用场景示例

### 1. 实时日志监控

```sh9
# 创建日志 topic
sh9:/> echo "create app-logs" > /pubsub/.ctl

# 应用写入日志
sh9:/> echo "[INFO] Request received" > /pubsub/topics/app-logs/pub &
sh9:/> echo "[ERROR] Database timeout" > /pubsub/topics/app-logs/pub &

# 监控错误
sh9:/> cat /pubsub/topics/app-logs/sub | grep ERROR > /errors.log &

# 统计日志数量
sh9:/> cat /pubsub/topics/app-logs/sub | wc -l &
```

### 2. 聊天室

```sh9
# 创建聊天室
sh9:/> echo "create chatroom" > /pubsub/.ctl

# 用户 Alice 发言
sh9:/> echo "Alice: Hi everyone!" > /pubsub/topics/chatroom/pub

# 用户 Bob 发言
sh9:/> echo "Bob: Hello Alice!" > /pubsub/topics/chatroom/pub

# 所有人看到消息
sh9:/> cat /pubsub/topics/chatroom/sub
[2024-01-28 20:15:00] Alice: Hi everyone!
[2024-01-28 20:15:05] Bob: Hello Alice!
```

### 3. 事件总线

```sh9
# 创建事件 topic
sh9:/> echo "create events" > /pubsub/.ctl

# 服务 A 发布事件
sh9:/> echo '{"event":"user.created","id":123}' > /pubsub/topics/events/pub

# 服务 B 订阅处理
sh9:/> cat /pubsub/topics/events/sub | while read event; do
  echo "Processing: $event"
done &

# 服务 C 也订阅
sh9:/> cat /pubsub/topics/events/sub | while read event; do
  echo "Logging: $event"
done &
```

### 4. 指标收集

```sh9
# 创建指标 topic
sh9:/> echo "create metrics" > /pubsub/.ctl

# 定期发布指标
sh9:/> while true; do
  echo "cpu:45% mem:2.1GB" > /pubsub/topics/metrics/pub
  sleep 5
done &

# 实时显示
sh9:/> cat /pubsub/topics/metrics/sub
```

## 常见问题

### Q: 如何看到所有 topics？

```sh9
sh9:/> cat /pubsub/.topics
chat
logs
events
metrics
```

### Q: 如何删除 topic？

```sh9
sh9:/> echo "delete chat" > /pubsub/.ctl
```

### Q: 订阅者能看到历史消息吗？

能！每个 topic 有一个 ring buffer（默认 100 条消息）。新订阅者会先收到历史消息，然后接收新消息。

```sh9
# 发布一些消息
sh9:/> echo "msg1" > /pubsub/topics/test/pub
sh9:/> echo "msg2" > /pubsub/topics/test/pub

# 迟到的订阅者仍能看到
sh9:/> cat /pubsub/topics/test/sub
[2024-01-28 20:20:00] msg1
[2024-01-28 20:20:01] msg2
```

### Q: 如何增加历史消息数量？

```sh9
sh9:/> echo "create big-topic buffer_size=1000" > /pubsub/.ctl
```

### Q: 消息会持久化吗？

不会。PubSubFS 是纯内存实现，服务器重启后消息会丢失。如需持久化，考虑：
- 订阅者将消息写入文件（如 PageFS）
- 使用专门的消息队列系统

### Q: 能保证消息顺序吗？

单个发布者的消息顺序能保证。多个发布者的消息可能交错。

### Q: 性能如何？

- 延迟：< 1ms（本地）
- 吞吐：每秒数千条消息
- 订阅者：支持数百到数千并发订阅者

## 下一步

- 查看完整文档：`plugins/pubsubfs/README.md`
- 查看设计文档：`docs/PUBSUB_DESIGN.md`
- 尝试实现你自己的用例！

## 开发和调试

### 查看插件加载状态

```sh9
sh9:/> plugin list
pagefs
pubsubfs
streamfs
kv
hellofs
```

### 重启服务器重新加载插件

```bash
# Ctrl+C 停止服务器
# 然后重新启动
RUST_LOG=debug cargo run -p fs9-server
```

### 运行测试

```bash
cargo test -p fs9-plugin-pubsubfs
```

## 故障排查

### 插件未加载

检查 `./plugins/` 目录：
```bash
ls -la plugins/libfs9_plugin_pubsubfs.so
```

如果不存在，重新构建：
```bash
make plugins
```

### 权限错误

```sh9
# pub 文件只能写
sh9:/> cat /pubsub/topics/chat/pub  # ERROR: Permission denied
sh9:/> echo "msg" > /pubsub/topics/chat/pub  # OK

# sub 文件只能读
sh9:/> echo "msg" > /pubsub/topics/chat/sub  # ERROR: Permission denied
sh9:/> cat /pubsub/topics/chat/sub  # OK
```

### Topic 不存在

先检查是否创建：
```sh9
sh9:/> cat /pubsub/.topics

# 如果没有，创建它
sh9:/> echo "create mytopic" > /pubsub/.ctl
```

Happy messaging! 🚀
