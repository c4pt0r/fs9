# sh9 tail 命令使用指南

## 概述

sh9 的 `tail` 命令已增强，支持：
- 从文件读取（不仅仅是 stdin）
- `-f/--follow` 模式（持续跟踪新内容）
- 与 PubSubFS 完美配合，用于实时消息订阅

## 基本用法

### 显示文件最后 10 行（默认）

```sh9
sh9:/> tail /pubsub/chat.info
```

### 显示最后 N 行

```sh9
# 使用 -n 参数
sh9:/> tail -n 20 /logs/app.log

# 或简写形式
sh9:/> tail -20 /logs/app.log
```

### 从 stdin 读取

```sh9
sh9:/> cat /data/file.txt | tail -5
```

## Follow 模式（实时订阅）

### 基本的 follow 模式

```sh9
sh9:/> tail -f /pubsub/topics/chat/sub
# 持续显示新消息，直到 Ctrl+C
```

### 显示最后 N 行，然后 follow

```sh9
sh9:/> tail -n 5 -f /pubsub/topics/logs/sub
# 先显示最后 5 条历史消息
# 然后持续显示新消息
```

## PubSubFS 使用场景

### 场景 1：订阅聊天消息

```sh9
# 启动服务器并挂载 pubsubfs
sh9:/> mount pubsubfs /pubsub

# 创建聊天 topic
sh9:/> echo "create chat" > /pubsub/.ctl

# 终端 1：订阅并实时查看
sh9:/> tail -f /pubsub/topics/chat/sub
[2024-01-28 20:30:00] alice: hello!
[2024-01-28 20:30:05] bob: hi there!
# ... 持续显示新消息 ...

# 终端 2：发布消息
sh9:/> echo "alice: hello!" > /pubsub/topics/chat/pub
sh9:/> echo "bob: hi there!" > /pubsub/topics/chat/pub
```

### 场景 2：监控日志流

```sh9
# 创建日志 topic
sh9:/> echo "create app-logs" > /pubsub/.ctl

# 后台应用持续发布日志
sh9:/> while true; do
  echo "[INFO] Processing request" > /pubsub/topics/app-logs/pub
  sleep 1
done &

# 实时监控，只显示最近 10 条
sh9:/> tail -n 10 -f /pubsub/topics/app-logs/sub
[2024-01-28 20:35:01] [INFO] Processing request
[2024-01-28 20:35:02] [INFO] Processing request
# ... 持续显示 ...
```

### 场景 3：错误日志过滤

```sh9
# 监控并只显示错误
sh9:/> tail -f /pubsub/topics/logs/sub | grep ERROR
[2024-01-28 20:40:15] [ERROR] Database timeout
[2024-01-28 20:40:23] [ERROR] Connection refused
# ... 只显示包含 ERROR 的行 ...
```

### 场景 4：实时指标监控

```sh9
# 创建指标 topic
sh9:/> echo "create metrics" > /pubsub/.ctl

# 发布者持续发送指标
sh9:/> while true; do
  echo "cpu:45% mem:2.1GB disk:50GB" > /pubsub/topics/metrics/pub
  sleep 5
done &

# 订阅者实时查看
sh9:/> tail -f /pubsub/topics/metrics/sub
[2024-01-28 20:45:00] cpu:45% mem:2.1GB disk:50GB
[2024-01-28 20:45:05] cpu:47% mem:2.2GB disk:50GB
# ... 每 5 秒更新 ...
```

### 场景 5：事件总线

```sh9
# 创建事件 topic
sh9:/> echo "create events" > /pubsub/.ctl

# 服务 A 发布事件
sh9:/> echo '{"event":"user.created","id":123}' > /pubsub/topics/events/pub
sh9:/> echo '{"event":"order.placed","id":456}' > /pubsub/topics/events/pub

# 服务 B 订阅并处理
sh9:/> tail -f /pubsub/topics/events/sub | while read event; do
  echo "Processing: $event"
done
Processing: [2024-01-28 20:50:00] {"event":"user.created","id":123}
Processing: [2024-01-28 20:50:05] {"event":"order.placed","id":456}
# ... 持续处理新事件 ...
```

## 与传统文件的区别

### 普通文件

```sh9
# 读取静态文件的最后几行
sh9:/> tail -10 /data/static.log

# follow 模式等待文件追加（不常用于 FS9）
sh9:/> tail -f /data/growing.log
```

### PubSubFS 文件

```sh9
# PubSubFS 的 sub 文件是流式的
sh9:/> tail -f /pubsub/topics/chat/sub
# 持续接收新消息，这是主要用途

# 注意：不带 -f 时只显示历史消息（ring buffer）
sh9:/> tail -20 /pubsub/topics/chat/sub
# 只显示最近 20 条历史消息，然后退出
```

## 实用技巧

### 1. 查看历史消息

```sh9
# 只看最后 50 条历史消息，不订阅新消息
sh9:/> tail -n 50 /pubsub/topics/logs/sub
```

### 2. 持续订阅并保存

```sh9
# 订阅并同时保存到文件
sh9:/> tail -f /pubsub/topics/logs/sub | tee /data/archive.log
```

### 3. 多个订阅者

```sh9
# 终端 1：只看错误
sh9:/> tail -f /pubsub/topics/logs/sub | grep ERROR

# 终端 2：统计行数
sh9:/> tail -f /pubsub/topics/logs/sub | wc -l

# 终端 3：保存所有日志
sh9:/> tail -f /pubsub/topics/logs/sub > /data/all.log
```

### 4. 格式化输出

```sh9
# 提取 JSON 字段
sh9:/> tail -f /pubsub/topics/events/sub | while read line; do
  echo "$line" | grep -o '"id":[0-9]*'
done
```

### 5. 条件过滤

```sh9
# 只显示特定用户的消息
sh9:/> tail -f /pubsub/topics/chat/sub | grep "alice:"
```

## 性能考虑

### Ring Buffer 大小

PubSubFS 的历史消息数量由 ring buffer 大小决定：

```sh9
# 创建大缓冲区的 topic
sh9:/> echo "create big-logs buffer_size=1000" > /pubsub/.ctl

# 可以看到更多历史
sh9:/> tail -100 /pubsub/topics/big-logs/sub
```

### Follow 模式延迟

Follow 模式每 100ms 轮询一次新数据：
- 延迟：~100ms
- CPU 占用：极低
- 适合实时性要求不高的场景

## 退出 Follow 模式

在 follow 模式下，使用 `Ctrl+C` 退出：

```sh9
sh9:/> tail -f /pubsub/topics/chat/sub
[2024-01-28 21:00:00] message 1
[2024-01-28 21:00:01] message 2
^C
sh9:/>
```

## 与其他命令组合

### 与 grep 组合

```sh9
sh9:/> tail -f /pubsub/topics/logs/sub | grep -i error
```

### 与 wc 组合

```sh9
sh9:/> tail -100 /pubsub/topics/logs/sub | wc -l
```

### 与 head 组合

```sh9
# 查看第 10-20 行
sh9:/> tail -20 /pubsub/topics/logs/sub | head -10
```

### 与 cut 组合

```sh9
# 提取时间戳
sh9:/> tail -f /pubsub/topics/metrics/sub | cut -d ' ' -f 1-2
```

## 对比：cat vs tail

### cat

```sh9
# cat 显示所有内容
sh9:/> cat /pubsub/topics/chat/sub
# 显示所有历史消息

# cat --stream 持续读取
sh9:/> cat --stream /pubsub/topics/chat/sub
# 类似 tail -f，但从头开始显示所有历史
```

### tail

```sh9
# tail 只显示最后几行
sh9:/> tail /pubsub/topics/chat/sub
# 只显示最后 10 条历史消息

# tail -f 只显示最后几行，然后持续读取
sh9:/> tail -n 5 -f /pubsub/topics/chat/sub
# 先显示最后 5 条，然后持续显示新消息
```

## 总结

| 命令 | 用途 | 适用场景 |
|------|------|----------|
| `tail file` | 查看文件末尾 | 查看历史消息 |
| `tail -n N file` | 查看最后 N 行 | 指定历史消息数量 |
| `tail -f file` | 持续跟踪新内容 | **实时订阅 PubSubFS** |
| `tail -n N -f file` | 先看 N 行，再跟踪 | 查看最近历史 + 实时订阅 |
| `cat --stream file` | 全部历史 + 跟踪 | 需要完整历史记录时 |

**推荐**：在 PubSubFS 中，使用 `tail -f` 进行实时消息订阅！
