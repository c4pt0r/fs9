# PubSubFS 使用文档

PubSubFS 是一个基于文件系统接口的发布-订阅（Pub/Sub）系统，通过简单的文件操作实现消息的发布和订阅。

> **重要说明**：本文档中的所有示例均在 sh9 Shell 中运行。sh9 是 FS9 的专用 Shell，支持文件操作、管道、后台任务等功能。

## 目录

- [核心概念](#核心概念)
- [快速开始](#快速开始)
- [基本操作](#基本操作)
- [高级用法](#高级用法)
- [使用场景](#使用场景)
- [最佳实践](#最佳实践)

## 核心概念

### 设计哲学

PubSubFS 采用"管道式"设计理念：
- **写入 = 发布**：向主题文件写入数据即发布消息
- **读取 = 订阅**：读取主题文件即订阅消息
- **自动创建**：首次写入自动创建主题
- **扁平结构**：简洁的路径设计

### 文件系统结构

```
/pubsub/                    # 挂载点
├── README                  # 使用说明
├── topic1                  # 主题文件（读=订阅，写=发布）
├── topic1.info            # 主题信息（订阅者数量、消息数等）
├── logs                    # 另一个主题
└── logs.info              # 日志主题的信息
```

### 消息特性

- **持久化历史**：环形缓冲区保存最近 1000 条消息
- **时间戳**：每条消息自动添加时间戳
- **广播**：一个发布者可以广播给多个订阅者
- **实时订阅**：使用 `tail -f` 实现实时消息流

## 快速开始

### 1. 启动 FS9 服务器

```bash
# 确保已编译 pubsubfs 插件
make plugins

# 启动服务器（会自动加载插件）
RUST_LOG=info cargo run -p fs9-server
```

### 2. 使用 sh9 Shell

```bash
# 启动 sh9
cargo run -p sh9

# 挂载 pubsubfs
sh9:/> mount pubsubfs /pubsub
mounted pubsubfs at /pubsub

# 查看初始状态
sh9:/> ls /pubsub
README
```

### 3. 第一条消息

```bash
# 发布消息（自动创建主题）
sh9:/> echo "Hello, PubSubFS!" > /pubsub/chat

# 订阅（读取历史消息）
sh9:/> cat /pubsub/chat
Hello, PubSubFS!

# 查看主题信息
sh9:/> cat /pubsub/chat.info
Topic: chat
Subscribers: 0
Total messages: 1
```

## 基本操作

### 发布消息

向主题文件写入即发布消息：

```bash
# 发布单条消息
echo "消息内容" > /pubsub/topic

# 发布多条消息
echo "消息 1" > /pubsub/topic
echo "消息 2" > /pubsub/topic
echo "消息 3" > /pubsub/topic
```

### 订阅历史消息

读取主题文件获取所有历史消息：

```bash
# 读取所有历史消息
cat /pubsub/topic

# 读取最近 N 条消息
tail -n 10 /pubsub/topic
```

### 实时订阅

使用 `tail -f` 实现实时消息流：

```bash
# 阻塞式实时订阅（Ctrl+C 停止）
tail -f /pubsub/topic

# 后台实时订阅（配合输出重定向）
tail -f /pubsub/topic > /pubsub/output &
```

**重要提示**：使用 `tail -f` 订阅时，主题必须已经存在。如果主题不存在，需要先发布一条消息创建主题：

```bash
# 先创建主题
echo "初始消息" > /pubsub/topic

# 再启动订阅
tail -f /pubsub/topic &
```

### 管理主题

```bash
# 列出所有主题
ls /pubsub

# 查看主题信息
cat /pubsub/topic.info

# 删除主题
rm /pubsub/topic
```

## 高级用法

### 1. 后台订阅与日志记录

将消息流持续写入文件：

```bash
# 启动后台订阅
tail -f /pubsub/logs > /pubsub/logs_backup &

# 发布消息（会同时写入备份）
echo "系统启动" > /pubsub/logs
echo "用户登录" > /pubsub/logs

# 查看备份
cat /pubsub/logs_backup

# 停止后台订阅
kill %1
```

### 2. 多订阅者模式

多个客户端同时订阅同一主题：

**终端 1（发布者）：**
```bash
sh9:/> mount pubsubfs /pubsub
sh9:/> echo "广播消息" > /pubsub/broadcast
```

**终端 2（订阅者 1）：**
```bash
sh9:/> mount pubsubfs /pubsub
sh9:/> tail -f /pubsub/broadcast
广播消息
```

**终端 3（订阅者 2）：**
```bash
sh9:/> mount pubsubfs /pubsub
sh9:/> tail -f /pubsub/broadcast
广播消息
```

### 3. 消息过滤

使用管道过滤消息：

```bash
# 过滤包含 "ERROR" 的消息
tail -f /pubsub/logs | grep "ERROR"

# 统计消息数量
cat /pubsub/logs | wc -l

# 提取最近 10 条消息
tail -n 10 /pubsub/logs
```

### 4. 管道与组合

```bash
# 将一个主题的消息转发到另一个主题
cat /pubsub/input > /pubsub/output

# 实时转发
tail -f /pubsub/input > /pubsub/output &

# 多主题聚合（需要在 shell 中循环）
cat /pubsub/topic1 > /pubsub/all
cat /pubsub/topic2 > /pubsub/all
```

### 5. 监控与调试

```bash
# 查看主题信息
cat /pubsub/logs.info
cat /pubsub/events.info

# 列出所有主题（不包括 .info 和 README）
ls /pubsub
```

**注意**：sh9 目前不支持复杂的循环结构，如需监控请使用外部脚本或多次手动执行。

## 使用场景

### 1. 日志聚合

**场景**：多个服务将日志发布到同一主题，统一收集和监控。

```bash
# 服务 A
echo "[Service-A] 启动成功" > /pubsub/logs

# 服务 B
echo "[Service-B] 处理请求" > /pubsub/logs

# 日志收集器（后台）
tail -f /pubsub/logs > /var/log/aggregated.log &

# 实时监控
tail -f /pubsub/logs | grep "ERROR"
```

### 2. 事件通知

**场景**：系统事件通知多个订阅者。

```bash
# 事件发布者
echo "USER_LOGIN: alice" > /pubsub/events
echo "FILE_UPLOADED: report.pdf" > /pubsub/events

# 审计订阅者
tail -f /pubsub/events > /pubsub/audit_log &

# 告警订阅者
tail -f /pubsub/events | grep "ERROR" > /pubsub/alerts &
```

### 3. 进程间通信

**场景**：不同进程通过主题交换消息。

```bash
# 进程 A（生产者）
while true; do
    echo "$(date): 心跳" > /pubsub/heartbeat
    sleep 5
done &

# 进程 B（消费者）
tail -f /pubsub/heartbeat
```

### 4. 实时数据流

**场景**：传感器数据流处理。

```bash
# 传感器数据发布
echo "温度: 25.3°C" > /pubsub/sensors
echo "湿度: 60%" > /pubsub/sensors

# 数据处理（提取温度值）
tail -f /pubsub/sensors | grep "温度" > /pubsub/temperature &

# 数据可视化（读取处理后的数据）
tail -f /pubsub/temperature
```

### 5. 任务队列

**场景**：简单的任务分发系统。

```bash
# 任务发布者
echo "TASK: process file1.txt" > /pubsub/tasks
echo "TASK: process file2.txt" > /pubsub/tasks

# 工作进程（订阅任务）
tail -f /pubsub/tasks > /tmp/worker_queue.txt &

# 处理任务（读取队列）
cat /tmp/worker_queue.txt
```

## 最佳实践

### 1. 主题命名

```bash
# 好的命名
/pubsub/logs          # 简短清晰
/pubsub/events        # 语义明确
/pubsub/metrics       # 功能明确

# 避免的命名
/pubsub/data          # 过于宽泛
/pubsub/temp123       # 缺乏语义
```

### 2. 消息格式

```bash
# 推荐：结构化消息
echo "LEVEL=INFO|SERVICE=api|MESSAGE=请求成功" > /pubsub/logs

# 推荐：JSON 格式
echo '{"level":"INFO","service":"api","message":"请求成功"}' > /pubsub/logs

# 避免：无结构消息
echo "发生了一些事情" > /pubsub/logs
```

### 3. 资源管理

```bash
# 定期清理不需要的主题
rm /pubsub/old_topic

# 监控订阅者数量
cat /pubsub/topic.info

# 及时关闭后台订阅
jobs              # 查看后台任务
kill %1           # 终止后台订阅
```

### 4. 错误处理

```bash
# 检查主题是否存在
cat /pubsub/topic

# 如果不存在会报错，可以创建
echo "初始消息" > /pubsub/topic
```

**注意**：sh9 的错误处理较简单，建议在应用层处理错误逻辑。

### 5. 性能优化

```bash
# 批量发布多条消息
echo "msg1" > /pubsub/topic
echo "msg2" > /pubsub/topic
echo "msg3" > /pubsub/topic

# 使用后台订阅获取实时消息（推荐）
tail -f /pubsub/topic > /pubsub/output &

# 而非反复读取整个主题（不推荐）
cat /pubsub/topic
```

### 6. 调试技巧

```bash
# 查看消息详情
cat /pubsub/topic

# 查看最新消息
tail -n 1 /pubsub/topic

# 统计消息数量
cat /pubsub/topic | wc -l

# 查看时间戳范围
cat /pubsub/topic | head -n 1    # 最早
cat /pubsub/topic | tail -n 1    # 最新

# 查看主题状态
cat /pubsub/topic.info
```

## 完整示例

### 微服务日志系统

将以下代码保存为 `log_system.sh9`，然后运行 `cargo run -p sh9 -- log_system.sh9`：

```bash
# 挂载 PubSubFS
mount pubsubfs /pubsub

# 启动日志收集器（后台）
tail -f /pubsub/logs > /pubsub/aggregated_logs &
echo "日志收集器已启动"

# 启动错误监控（后台）
tail -f /pubsub/logs | grep "ERROR" > /pubsub/errors &
echo "错误监控已启动"

# 模拟微服务产生日志
echo "[2026-01-29 10:00:00] INFO: 服务启动" > /pubsub/logs
echo "[2026-01-29 10:00:05] INFO: 连接数据库成功" > /pubsub/logs
echo "[2026-01-29 10:00:10] ERROR: 无法连接到缓存" > /pubsub/logs
echo "[2026-01-29 10:00:15] INFO: 处理请求 #1001" > /pubsub/logs

# 等待一秒让消息处理
sleep 1

# 查看聚合日志
echo "=== 所有日志 ==="
cat /pubsub/aggregated_logs

# 查看错误日志
echo ""
echo "=== 错误日志 ==="
cat /pubsub/errors

# 查看主题状态
echo ""
echo "=== 主题状态 ==="
cat /pubsub/logs.info

# 清理后台任务
kill %1
kill %2
echo ""
echo "日志系统已停止"
```

### 实时聊天系统

将以下代码保存为 `chat.sh9`，然后运行 `cargo run -p sh9 -- chat.sh9`：

```bash
# 挂载 PubSubFS
mount pubsubfs /chat

# 创建聊天室
echo "=== 欢迎来到聊天室 ===" > /chat/room1

# 启动消息订阅（后台）
tail -f /chat/room1 &

# 发送消息
echo "[Alice] 大家好！" > /chat/room1
sleep 1
echo "[Bob] 嗨，Alice！" > /chat/room1
sleep 1
echo "[Charlie] 你们好！" > /chat/room1

# 等待查看消息
sleep 2

# 查看聊天历史
echo ""
echo "=== 聊天历史 ==="
cat /chat/room1

# 停止订阅
kill %1
```

## 技术限制

1. **历史消息限制**：环形缓冲区最多保存 1000 条消息
2. **并发订阅**：支持多个订阅者，但每个订阅者独立接收所有消息
3. **消息顺序**：保证同一发布者的消息顺序，多发布者之间按到达时间排序
4. **消息大小**：单条消息建议不超过 1MB

## 常见问题

**Q: 消息会丢失吗？**
A: 历史消息保存在内存中，服务器重启会丢失。如需持久化，请将消息备份到其他存储。

**Q: 如何实现消息确认？**
A: PubSubFS 是广播模式，不支持消息确认。如需确认，可在应用层实现。

**Q: 支持消息过滤吗？**
A: 使用标准 Unix 工具（grep, awk 等）在订阅端进行过滤。

**Q: 如何查看订阅者列表？**
A: 查看 `topic.info` 文件中的订阅者数量。

**Q: 后台订阅会自动重连吗？**
A: 不会。如果服务器重启，需要重新启动后台订阅任务。

## 参考资料

- [PubSubFS 实现原理](./src/lib.rs)
- [FS9 核心文档](../../README.md)
- [sh9 Shell 文档](../../sh9/README.md)
