# FS9 Pub/Sub 设计方案

## 概述

为 FS9 设计内置的发布订阅（Pub/Sub）系统，需要在保持 Unix 哲学的同时提供优秀的 sh9 用户体验。

## 方案对比

### 方案 1: 扩展 StreamFS（最简单）

**实现**: 直接使用现有的 streamfs，无需额外开发

**使用方式**:
```bash
# 挂载 streamfs
mount streamfs /streams

# 发布消息
echo "hello world" > /streams/topic1

# 订阅（在后台运行）
cat /streams/topic1 &

# 多个发布者
echo "message 1" > /streams/topic1 &
echo "message 2" > /streams/topic1 &

# 多个订阅者
cat /streams/topic1 | grep "important" &
cat /streams/topic1 | wc -l &
```

**优点**:
- ✅ 无需开发，立即可用
- ✅ 完全符合 Unix 哲学：一切皆文件
- ✅ 可以用管道、重定向等组合
- ✅ 支持多发布者、多订阅者
- ✅ 有 ring buffer 支持迟到的订阅者

**缺点**:
- ❌ 没有显式的 topic 管理
- ❌ 无法查询订阅者数量
- ❌ 缺少过滤、路由等高级功能
- ❌ 命令较长，不够简洁

**评分**: 简单性 ★★★★★ | 易用性 ★★★☆☆ | 功能性 ★★☆☆☆

---

### 方案 2: PubSubFS 专用文件系统（推荐）

**实现**: 创建新的 `pubsubfs` 插件，提供专门的 pub/sub 语义

**文件系统结构**:
```
/pubsub/
  ├── .ctl              # 控制文件（创建/删除 topic，配置）
  ├── .topics           # 列出所有 topics（只读）
  └── topics/
      ├── chat/
      │   ├── .info     # topic 元信息（订阅者数、消息数等）
      │   ├── pub       # 写入 = 发布消息
      │   └── sub       # 读取 = 订阅消息（阻塞流式读取）
      ├── logs/
      │   ├── .info
      │   ├── pub
      │   └── sub
      └── events/
          ├── .info
          ├── pub
          └── sub
```

**使用方式**:
```bash
# 挂载 pubsubfs
mount pubsubfs /pubsub

# 创建 topic
echo "create chat" > /pubsub/.ctl
echo "create logs buffer_size=1000" > /pubsub/.ctl

# 列出所有 topics
cat /pubsub/.topics
# 输出: chat logs events

# 发布消息
echo "hello everyone" > /pubsub/topics/chat/pub
echo '{"level":"info","msg":"started"}' > /pubsub/topics/logs/pub

# 订阅消息（持续读取）
cat /pubsub/topics/chat/sub
# 阻塞等待，每有新消息就输出一行

# 后台订阅并处理
cat /pubsub/topics/logs/sub | grep ERROR > /errors.log &

# 查看 topic 信息
cat /pubsub/topics/chat/.info
# 输出:
# name: chat
# subscribers: 2
# messages: 142
# created: 2024-01-28 10:30:00

# 删除 topic
echo "delete chat" > /pubsub/.ctl
```

**高级功能**:
```bash
# 带过滤的订阅（通过配置）
echo "create errors filter='severity>=error'" > /pubsub/.ctl

# 持久化 topic（可选）
echo "create persistent retention=7d" > /pubsub/.ctl

# 消息格式：每行一条消息，带时间戳
# [2024-01-28 10:30:15] hello everyone
# [2024-01-28 10:30:16] how are you
```

**实现要点**:
```rust
struct Topic {
    name: String,
    buffer: RingBuffer<Message>,     // 消息环形缓冲区
    subscribers: Arc<RwLock<Vec<Subscriber>>>,
    broadcast_tx: broadcast::Sender<Message>,
    stats: TopicStats,
}

struct Message {
    timestamp: SystemTime,
    data: Bytes,
    metadata: HashMap<String, String>,  // 扩展性
}

struct Subscriber {
    id: u64,
    receiver: broadcast::Receiver<Message>,
    filter: Option<Filter>,  // 未来扩展
}
```

**配置选项**:
- `buffer_size`: ring buffer 大小（默认 100）
- `channel_size`: broadcast channel 大小（默认 100）
- `retention`: 消息保留时间（可选）
- `max_message_size`: 最大消息大小（默认 1MB）
- `persistence`: 是否持久化（默认 false）

**优点**:
- ✅ 清晰的 topic 概念
- ✅ 元信息查询（订阅者数、消息数）
- ✅ 显式的控制接口
- ✅ 仍然遵循文件系统语义
- ✅ 可扩展（未来加过滤、持久化等）
- ✅ 命令简洁，语义清晰

**缺点**:
- ⚠️ 需要开发新插件
- ⚠️ 稍微复杂一点

**评分**: 简单性 ★★★★☆ | 易用性 ★★★★★ | 功能性 ★★★★★

---

### 方案 3: 混合方案（文件系统 + Builtin 命令）

**实现**: PubSubFS + sh9 内置命令包装

**使用方式**:
```bash
# 方式 1: 直接使用文件系统（高级用户）
echo "hello" > /pubsub/topics/chat/pub
cat /pubsub/topics/chat/sub

# 方式 2: 使用 builtin 命令（简单易用）
pub chat "hello world"              # 发布到 chat topic
sub chat                             # 订阅 chat topic
sub chat | grep important > /out &   # 可以用管道

# Topic 管理
topic create chat                    # 创建 topic
topic create logs --buffer 1000      # 带参数创建
topic list                           # 列出所有 topics
topic info chat                      # 查看 topic 信息
topic delete chat                    # 删除 topic

# 后台订阅
sub chat &                           # 后台订阅
jobs                                 # 查看后台任务
```

**sh9 内置命令**:
```bash
# 在 sh9/src/eval.rs 中添加
"pub" => {
    // pub <topic> <message>
    let topic = args.get(0).ok_or_else(|| Sh9Error::Usage("pub <topic> <message>"))?;
    let message = args[1..].join(" ");
    let path = format!("/pubsub/topics/{}/pub", topic);
    self.client()?.write_file(&path, message.as_bytes()).await?;
    Ok(0)
}

"sub" => {
    // sub <topic>
    let topic = args.get(0).ok_or_else(|| Sh9Error::Usage("sub <topic>"))?;
    let path = format!("/pubsub/topics/{}/sub", topic);
    // 流式读取并输出
    let handle = self.client()?.open(&path, OpenFlags::read()).await?;
    loop {
        let data = self.client()?.read(&handle, 0, 4096).await?;
        if data.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        ctx.stdout.write(&data)?;
    }
}

"topic" => {
    // topic create|delete|list|info <name> [options]
    let subcmd = args.get(0).ok_or_else(|| Sh9Error::Usage("topic create|delete|list|info"))?;
    match subcmd.as_str() {
        "create" => { /* echo "create $name" > /pubsub/.ctl */ }
        "delete" => { /* echo "delete $name" > /pubsub/.ctl */ }
        "list" => { /* cat /pubsub/.topics */ }
        "info" => { /* cat /pubsub/topics/$name/.info */ }
        _ => Err(Sh9Error::Usage("unknown subcommand"))
    }
}
```

**优点**:
- ✅ 两种使用方式，灵活度高
- ✅ 简单场景下命令极简
- ✅ 高级场景下可以直接操作文件系统
- ✅ 学习曲线平滑（从简单到高级）
- ✅ 可组合性强

**缺点**:
- ⚠️ 需要开发文件系统插件和 sh9 命令
- ⚠️ 概念稍多（文件 vs 命令）

**评分**: 简单性 ★★★☆☆ | 易用性 ★★★★★ | 功能性 ★★★★★

---

### 方案 4: 纯 Builtin 命令（最简洁）

**实现**: 在 sh9 中内置 pub/sub 功能，不依赖文件系统

**使用方式**:
```bash
# 极简设计
pub chat "hello world"               # 发布
sub chat                             # 订阅
unsub chat                           # 取消订阅

# Topic 管理
topics                               # 列出所有 topics
topic-info chat                      # 查看信息
```

**优点**:
- ✅ 命令最简洁
- ✅ 学习成本最低

**缺点**:
- ❌ 不符合 FS9 的文件系统哲学
- ❌ 无法用管道、重定向等 Unix 工具组合
- ❌ 只能在 sh9 中使用，其他客户端无法访问
- ❌ 难以扩展

**评分**: 简单性 ★★★★★ | 易用性 ★★★★☆ | 功能性 ★★☆☆☆

---

## 推荐方案: 方案 2 (PubSubFS) 或 方案 3 (混合)

### 如果追求纯粹的 Unix 哲学和架构一致性:
**选择方案 2 (PubSubFS)**

- 完全融入 FS9 生态
- 所有客户端（Rust、Python、FUSE）都能使用
- 可以用标准 Unix 工具处理
- 实现相对简单

### 如果追求最佳用户体验:
**选择方案 3 (混合)**

- 新手用 `pub`/`sub` 命令快速上手
- 高手用文件系统获得完全控制
- 适合各种使用场景

## 实现优先级

### Phase 1: 核心功能（MVP）
```rust
// pubsubfs plugin
- Topic 创建/删除
- pub 文件写入 = 发布
- sub 文件读取 = 订阅（流式）
- Ring buffer 支持历史消息
- Broadcast channel 支持多订阅者
```

### Phase 2: 元信息和管理
```rust
- .info 文件显示统计
- .topics 文件列出所有 topic
- .ctl 控制接口
```

### Phase 3: sh9 便捷命令（如果选方案 3）
```rust
- pub <topic> <message>
- sub <topic>
- topic create/delete/list/info
```

### Phase 4: 高级功能（未来）
```rust
- 消息过滤
- 持久化存储
- 消息确认机制
- 优先级队列
- 死信队列
```

## 典型使用场景

### 场景 1: 实时日志聚合
```bash
# 启动日志收集器
mount pubsubfs /pubsub
echo "create logs buffer_size=1000" > /pubsub/.ctl

# 应用写入日志
echo '[INFO] Server started' > /pubsub/topics/logs/pub
echo '[ERROR] Connection failed' > /pubsub/topics/logs/pub

# 订阅并过滤错误
cat /pubsub/topics/logs/sub | grep ERROR &

# 持久化所有日志
cat /pubsub/topics/logs/sub > /persistent/all.log &
```

### 场景 2: 微服务通信
```bash
# 服务 A 发布事件
echo '{"event":"user.created","id":123}' > /pubsub/topics/events/pub

# 服务 B 订阅处理
cat /pubsub/topics/events/sub | while read event; do
  process_event "$event"
done
```

### 场景 3: 实时监控
```bash
# 监控脚本持续发布指标
while true; do
  cpu=$(get_cpu_usage)
  echo "cpu:$cpu" > /pubsub/topics/metrics/pub
  sleep 1
done &

# 仪表盘订阅展示
cat /pubsub/topics/metrics/sub | while read metric; do
  update_dashboard "$metric"
done
```

### 场景 4: 聊天系统
```bash
# 用户 1 发言
echo "alice: hello everyone" > /pubsub/topics/chat/pub

# 用户 2 发言
echo "bob: hi alice!" > /pubsub/topics/chat/pub

# 所有用户订阅
cat /pubsub/topics/chat/sub
```

## 性能考虑

### 内存管理
- Ring buffer 大小可配置（默认 100 条）
- 单条消息大小限制（默认 1MB）
- 自动清理断开的订阅者

### 并发性能
- 使用 tokio broadcast channel（无锁）
- 读写分离（写入不阻塞读取）
- 支持数千并发订阅者

### 消息延迟
- 本地内存通信，延迟 < 1ms
- 网络延迟取决于 FS9 HTTP API

## 配置示例

```yaml
# fs9.yaml
plugins:
  pubsubfs:
    default_buffer_size: 100      # ring buffer 默认大小
    default_channel_size: 100     # broadcast channel 默认大小
    max_topics: 1000              # 最大 topic 数
    max_message_size: 1048576     # 1MB
    message_ttl: 3600             # 消息 TTL（秒）
    enable_persistence: false     # 是否启用持久化
```

## 总结

| 方案 | 开发成本 | 易用性 | 功能性 | 一致性 | 推荐度 |
|------|---------|--------|--------|--------|--------|
| 方案1: StreamFS | ★☆☆☆☆ | ★★★☆☆ | ★★☆☆☆ | ★★★★★ | ⭐⭐⭐ |
| 方案2: PubSubFS | ★★★☆☆ | ★★★★★ | ★★★★★ | ★★★★★ | ⭐⭐⭐⭐⭐ |
| 方案3: 混合 | ★★★★☆ | ★★★★★ | ★★★★★ | ★★★★☆ | ⭐⭐⭐⭐⭐ |
| 方案4: Builtin | ★★☆☆☆ | ★★★★☆ | ★★☆☆☆ | ★☆☆☆☆ | ⭐⭐ |

**最终推荐**:
- **短期**: 方案 2 (PubSubFS) - 快速实现，功能完整
- **长期**: 方案 3 (混合) - 最佳用户体验，适合生产环境
