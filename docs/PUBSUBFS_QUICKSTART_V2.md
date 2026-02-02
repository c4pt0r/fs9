# PubSubFS V2 Quick Start Guide

PubSubFS 已重构为更简洁的管道式设计！

## ✨ 新特性

### 简化的路径

| 操作 | 旧版本 (V1) | 新版本 (V2) | 改进 |
|------|------------|------------|------|
| 发布 | `echo "hi" > /pubsub/topics/chat/pub` | `echo "hi" > /pubsub/chat` | **-54%** 字符 |
| 订阅 | `cat /pubsub/topics/chat/sub` | `cat /pubsub/chat` | **-54%** 字符 |
| 信息 | `cat /pubsub/topics/chat/.info` | `cat /pubsub/chat.info` | **-33%** 字符 |
| 列表 | `cat /pubsub/.topics` | `ls /pubsub` | 标准命令 |
| 创建 | `echo "create chat" > /pubsub/.ctl` | `echo "hi" > /pubsub/chat` | 自动创建 |
| 删除 | `echo "delete chat" > /pubsub/.ctl` | `rm /pubsub/chat` | 标准命令 |

### 核心理念：像管道一样

```
/pubsub/chat 是一个双向管道：
- 写入 (>) = 发布消息
- 读取 (<) = 订阅消息
```

## 启动服务器

```bash
# 构建插件
make plugins

# 启动服务器
RUST_LOG=info cargo run -p fs9-server
```

## 基本使用（sh9）

### 1. 挂载 PubSubFS

```sh9
sh9:/> mount pubsubfs /pubsub
mounted pubsubfs at /pubsub
```

### 2. 创建 Topic（自动）

不需要显式创建！第一次写入时自动创建：

```sh9
sh9:/> echo "hello world" > /pubsub/chat
# topic "chat" 自动创建
```

### 3. 发布消息

```sh9
sh9:/> echo "alice: hi everyone!" > /pubsub/chat
sh9:/> echo "bob: hello alice!" > /pubsub/chat
sh9:/> echo '{"event":"user.login","id":123}' > /pubsub/events
```

### 4. 订阅消息

**方式 A: 使用 cat（所有历史 + 流式）**

```sh9
sh9:/> cat /pubsub/chat
[2024-01-28 21:00:00] alice: hi everyone!
[2024-01-28 21:00:05] bob: hello alice!
# 等待新消息...
```

**方式 B: 使用 tail -f（推荐：最后 N 条 + 流式）**

```sh9
sh9:/> tail -f /pubsub/chat
[2024-01-28 21:00:00] alice: hi everyone!
[2024-01-28 21:00:05] bob: hello alice!
# 持续显示新消息...

# 只显示最后 5 条，然后持续
sh9:/> tail -n 5 -f /pubsub/logs
```

### 5. 查看 Topic 信息

```sh9
sh9:/> cat /pubsub/chat.info
name: chat
subscribers: 2
messages: 42
ring_size: 100
created: 2024-01-28 20:00:00
modified: 2024-01-28 21:05:30
```

### 6. 列出所有 Topics

```sh9
sh9:/> ls /pubsub
README      chat        chat.info   logs        logs.info   events      events.info
```

### 7. 删除 Topic

```sh9
sh9:/> rm /pubsub/chat
sh9:/> ls /pubsub
README      logs        logs.info   events      events.info
```

## 实用场景

### 场景 1: 聊天室

**终端 1 - Alice**:
```sh9
sh9:/> echo "alice: Hello everyone!" > /pubsub/chatroom
sh9:/> echo "alice: How are you?" > /pubsub/chatroom
```

**终端 2 - Bob**:
```sh9
sh9:/> echo "bob: Hi Alice!" > /pubsub/chatroom
sh9:/> echo "bob: I'm good, thanks!" > /pubsub/chatroom
```

**终端 3 - 监听所有人**:
```sh9
sh9:/> tail -f /pubsub/chatroom
[2024-01-28 21:10:00] alice: Hello everyone!
[2024-01-28 21:10:05] bob: Hi Alice!
[2024-01-28 21:10:10] alice: How are you?
[2024-01-28 21:10:15] bob: I'm good, thanks!
```

### 场景 2: 日志聚合

**应用服务器持续发布日志**:
```sh9
sh9:/> while true; do
  echo "[INFO] Processing request $((i++))" > /pubsub/app-logs
  sleep 1
done &
```

**监控错误**:
```sh9
sh9:/> tail -f /pubsub/app-logs | grep ERROR > /errors.log &
```

**统计日志数量**:
```sh9
sh9:/> tail -f /pubsub/app-logs | wc -l &
```

**查看最近 20 条日志**:
```sh9
sh9:/> tail -20 /pubsub/app-logs
```

### 场景 3: 事件总线

**服务 A 发布事件**:
```sh9
sh9:/> echo '{"event":"user.created","id":123}' > /pubsub/events
sh9:/> echo '{"event":"order.placed","id":456}' > /pubsub/events
```

**服务 B 订阅处理**:
```sh9
sh9:/> tail -f /pubsub/events | while read event; do
  echo "Processing: $event"
done &
```

**服务 C 也订阅**:
```sh9
sh9:/> tail -f /pubsub/events | grep "user" > /user-events.log &
```

### 场景 4: 实时指标

**指标发布者**:
```sh9
sh9:/> while true; do
  cpu=$(echo "cpu:$((RANDOM % 100))%")
  mem=$(echo "mem:$((RANDOM % 16))GB")
  echo "$cpu $mem" > /pubsub/metrics
  sleep 5
done &
```

**仪表盘订阅**:
```sh9
sh9:/> tail -f /pubsub/metrics
[2024-01-28 21:20:00] cpu:45% mem:8GB
[2024-01-28 21:20:05] cpu:52% mem:9GB
[2024-01-28 21:20:10] cpu:38% mem:7GB
```

**查看当前指标**:
```sh9
sh9:/> tail -1 /pubsub/metrics
[2024-01-28 21:20:10] cpu:38% mem:7GB
```

## FUSE 模式

### 挂载

**终端 1 - 服务器**:
```bash
RUST_LOG=info cargo run -p fs9-server
```

**终端 2 - FUSE**:
```bash
mkdir -p /tmp/fs9
cargo run -p fs9-fuse -- /tmp/fs9 --server http://localhost:9999 --foreground
```

**终端 3 - 使用标准工具**:
```bash
cd /tmp/fs9/pubsub

# 发布
echo "hello" > chat

# 订阅（使用真正的 tail -f）
tail -f chat

# 高级用法
tail -f logs | grep ERROR | awk '{print $3}' > /critical.log &

# 列出
ls -lh

# 删除
rm chat
```

## 高级技巧

### 1. 多路复用

```sh9
# 合并多个 topic
sh9:/> (tail -f /pubsub/logs & tail -f /pubsub/errors) | tee /combined.log
```

### 2. 过滤和转换

```sh9
# 只订阅特定模式
sh9:/> tail -f /pubsub/events | grep "error" > /errors-only.log

# 提取字段
sh9:/> tail -f /pubsub/metrics | cut -d ' ' -f 1
```

### 3. 查看历史但不订阅

```sh9
# 只看最后 50 条，不等待新消息
sh9:/> tail -50 /pubsub/logs
```

### 4. 检查 Topic 状态

```sh9
# 快速查看订阅者数量
sh9:/> cat /pubsub/chat.info | grep subscribers
subscribers: 3

# 查看消息总数
sh9:/> cat /pubsub/logs.info | grep messages
messages: 1542
```

### 5. 清理旧 Topics

```sh9
# 删除所有 topics（慎用！）
sh9:/> for topic in $(ls /pubsub | grep -v README | grep -v .info); do
  rm /pubsub/$topic
done
```

## 性能考虑

### Ring Buffer 大小

默认保留 100 条历史消息。创建时可配置：

```sh9
# 通过配置挂载
mount pubsubfs /pubsub '{"default_ring_size":1000,"default_channel_size":500}'
```

### 订阅者延迟

- **tail -f**: 100ms 轮询间隔
- **cat --stream**: 100ms 轮询间隔
- 适合实时性要求不高的场景（< 1 秒）

## 对比表

### V1 vs V2

| 特性 | V1（旧版） | V2（新版） |
|------|-----------|-----------|
| 路径长度 | `/pubsub/topics/chat/pub` (28字符) | `/pubsub/chat` (13字符) |
| 创建方式 | `echo "create chat" > .ctl` | 自动创建 |
| 列出 topics | `cat .topics` | `ls /pubsub` |
| 删除 topics | `echo "delete chat" > .ctl` | `rm /pubsub/chat` |
| 学习曲线 | 需要记住 `.ctl` 语法 | 标准 Unix 命令 |
| 心智模型 | 目录树结构 | 管道/文件 |

### cat vs tail

| 命令 | 历史消息 | 新消息 | 适用场景 |
|------|---------|--------|----------|
| `cat /pubsub/chat` | ✅ 全部 | ✅ 持续 | 需要完整历史 |
| `cat --stream /pubsub/chat` | ✅ 全部 | ✅ 持续 | 同上（显式流式） |
| `tail -f /pubsub/chat` | ⚠️ 最后 10 条 | ✅ 持续 | **实时订阅（推荐）** |
| `tail -n 5 -f /pubsub/chat` | ⚠️ 最后 5 条 | ✅ 持续 | 自定义历史数量 |
| `tail -20 /pubsub/chat` | ⚠️ 最后 20 条 | ❌ 不持续 | 快速查看历史 |

## 常见问题

### Q: 如何创建 topic？

A: 不需要！第一次写入时自动创建：
```sh9
echo "hello" > /pubsub/newtopic
```

### Q: 能同时读写吗？

A: 不能。需要分开两个句柄：
```sh9
# 错误：不能同时读写
# （这在底层会尝试 open(read=true, write=true)）

# 正确：分开操作
echo "msg" > /pubsub/chat  # 写
tail -f /pubsub/chat       # 读
```

### Q: 消息会持久化吗？

A: 不会。重启服务器后消息丢失。如需持久化：
```sh9
tail -f /pubsub/logs > /persistent/logs.txt &
```

### Q: .info 文件可以删除吗？

A: 不能单独删除。删除 topic 时自动删除：
```sh9
rm /pubsub/chat  # 同时删除 chat 和 chat.info
```

### Q: 如何增加历史消息数量？

A: 重新挂载时配置：
```sh9
umount /pubsub
mount pubsubfs /pubsub '{"default_ring_size":1000}'
```

## 迁移指南（V1 → V2）

| V1 命令 | V2 命令 | 说明 |
|---------|---------|------|
| `echo "create chat" > /pubsub/.ctl` | `echo "hi" > /pubsub/chat` | 自动创建 |
| `echo "msg" > /pubsub/topics/chat/pub` | `echo "msg" > /pubsub/chat` | 扁平化 |
| `cat /pubsub/topics/chat/sub` | `tail -f /pubsub/chat` | 推荐用 tail |
| `cat /pubsub/.topics` | `ls /pubsub` | 标准命令 |
| `cat /pubsub/topics/chat/.info` | `cat /pubsub/chat.info` | 去掉前缀点 |
| `echo "delete chat" > /pubsub/.ctl` | `rm /pubsub/chat` | 标准命令 |

## 下一步

- 查看完整文档：`plugins/pubsubfs/README.md`
- 查看设计文档：`docs/PUBSUB_DESIGN.md`
- 尝试 FUSE 模式获得完整 Unix 工具支持！

Happy messaging! 🚀
