# PubSubFS - Publish/Subscribe File System Plugin

A topic-based pub/sub system for FS9 inspired by Unix pipes: everything is a file.

## Features

- **Pipe-like interface**: Read=subscribe, Write=publish
- **Simple paths**: `/pubsub/chat` instead of `/pubsub/topics/chat/pub`
- **Auto-create topics**: First write creates the topic automatically
- **Multiple publishers/subscribers**: Broadcast to all subscribers
- **Ring buffer**: Late joiners receive recent historical messages
- **Real-time streaming**: New messages delivered immediately
- **Standard Unix commands**: `echo >`, `cat`, `tail -f`, `ls`, `rm`

## File Structure

```
/pubsub/
  README            # Documentation
  chat              # Topic file: read=subscribe, write=publish
  chat.info         # Topic metadata (subscribers, messages, etc)
  logs              # Another topic
  logs.info         # Its metadata
```

## Quick Start

### Mount

```bash
mount pubsubfs /pubsub
```

### Create Topics (Auto)

Topics are created automatically on first write:

```bash
echo "hello world" > /pubsub/chat
```

### Publish Messages

```bash
echo "alice: hi!" > /pubsub/chat
echo '{"event":"user.login"}' > /pubsub/events
```

### Subscribe to Messages

**Option A: cat (all history + streaming)**

```bash
cat /pubsub/chat
```

**Option B: tail -f (last N + streaming, recommended)**

```bash
tail -f /pubsub/chat
tail -n 5 -f /pubsub/logs
```

### View Topic Info

```bash
cat /pubsub/chat.info
```

Output:
```
name: chat
subscribers: 3
messages: 142
ring_size: 100
created: 2024-01-28 10:30:00
modified: 2024-01-28 10:35:42
```

### List Topics

```bash
ls /pubsub
```

Output:
```
README  chat  chat.info  logs  logs.info  events  events.info
```

### Delete Topics

```bash
rm /pubsub/chat
```

## Configuration

### Mount-time Configuration

```bash
mount pubsubfs /pubsub '{"default_ring_size":1000,"default_channel_size":500}'
```

Options:
- `default_ring_size`: Historical messages to keep (default: 100)
- `default_channel_size`: Broadcast channel buffer size (default: 100)

## Use Cases

### 1. Real-time Chat

```bash
# Publisher (Alice)
echo "alice: hello!" > /pubsub/chat

# Publisher (Bob)
echo "bob: hi alice!" > /pubsub/chat

# Subscriber
tail -f /pubsub/chat
```

### 2. Log Aggregation

```bash
# Application logs
echo "[ERROR] Database timeout" > /pubsub/logs &

# Monitor errors only
tail -f /pubsub/logs | grep ERROR > /errors.log &

# Count all logs
tail -f /pubsub/logs | wc -l &
```

### 3. Event Bus

```bash
# Service A publishes events
echo '{"event":"user.created","id":123}' > /pubsub/events

# Service B subscribes
tail -f /pubsub/events | process_events.sh &

# Service C also subscribes
tail -f /pubsub/events | grep "error" &
```

### 4. Real-time Metrics

```bash
# Publisher
while true; do
  echo "cpu:45% mem:8GB" > /pubsub/metrics
  sleep 5
done &

# Subscriber
tail -f /pubsub/metrics
```

## Advanced Usage

### Multiple Subscribers

```bash
# All receive the same messages
tail -f /pubsub/events | service1.sh &
tail -f /pubsub/events | service2.sh &
tail -f /pubsub/events | tee /archive.log &
```

### Filtering

```bash
tail -f /pubsub/logs | grep ERROR
tail -f /pubsub/events | grep "user"
```

### Historical Messages Only

```bash
# View last 50 messages, don't subscribe
tail -50 /pubsub/logs
```

### Combining Topics

```bash
# Merge multiple topics
(tail -f /pubsub/logs & tail -f /pubsub/errors) | tee /combined.log
```

## Message Format

Messages are automatically timestamped:

```
[2024-01-28 10:30:15] hello world
[2024-01-28 10:30:16] how are you
```

## FUSE Usage

Mount via FUSE to use real Unix tools:

```bash
# Terminal 1: Start server
RUST_LOG=info cargo run -p fs9-server

# Terminal 2: Mount FUSE
mkdir /tmp/fs9
cargo run -p fs9-fuse -- /tmp/fs9 --server http://localhost:9999

# Terminal 3: Use standard tools
cd /tmp/fs9/pubsub
echo "hello" > chat
tail -f chat | awk '{print $3}'
```

## Performance

- **Latency**: < 1ms (local), ~100ms (polling interval for tail -f)
- **Throughput**: Thousands of messages/second
- **Memory**: ring_size × avg_message_size × num_topics
- **Subscribers**: Hundreds to thousands concurrent subscribers

## Limitations

- **In-memory only**: Messages not persisted to disk
- **No ordering guarantees**: Multiple publishers may interleave
- **Lagged subscribers**: Very slow subscribers may miss messages
- **No acknowledgments**: Fire-and-forget delivery
- **Max message size**: 1MB per message

## Comparison

### PubSubFS vs StreamFS

| Feature | PubSubFS | StreamFS |
|---------|----------|----------|
| Path style | `/pubsub/chat` | `/streamfs/chat` |
| Topic concept | Explicit (files) | Implicit (files) |
| Metadata | `chat.info` | None |
| Message format | Timestamped | Raw |
| Use case | Pub/Sub messaging | Streaming data |

### cat vs tail

| Command | History | New Messages | Use Case |
|---------|---------|--------------|----------|
| `cat /pubsub/chat` | All | Continuous | Need full history |
| `tail -f /pubsub/chat` | Last 10 | Continuous | **Real-time (recommended)** |
| `tail -n 5 -f /pubsub/chat` | Last 5 | Continuous | Custom history |
| `tail -20 /pubsub/chat` | Last 20 | None | Quick history check |

## Design Philosophy

### Like Unix Pipes

```
/pubsub/chat is a bidirectional pipe:
- Write (>) = Publish
- Read (<) = Subscribe
```

### Minimal Concepts

- **One file per topic**: `/pubsub/chat` (not a directory)
- **Info file**: `/pubsub/chat.info` (separate metadata)
- **Standard commands**: `echo >`, `cat`, `tail -f`, `ls`, `rm`
- **Auto-create**: No need for explicit create command

### Path Length Comparison

| Operation | Old Path | New Path | Improvement |
|-----------|----------|----------|-------------|
| Publish | `/pubsub/topics/chat/pub` (28 chars) | `/pubsub/chat` (13 chars) | **-54%** |
| Subscribe | `/pubsub/topics/chat/sub` (28 chars) | `/pubsub/chat` (13 chars) | **-54%** |
| Info | `/pubsub/topics/chat/.info` (30 chars) | `/pubsub/chat.info` (20 chars) | **-33%** |

## Troubleshooting

### Topic not found

```bash
# Check if it exists
ls /pubsub

# Create it by writing
echo "first message" > /pubsub/mytopic
```

### Cannot open for both read and write

```bash
# ERROR: This doesn't work
# (attempts to open with both flags)

# CORRECT: Separate operations
echo "msg" > /pubsub/chat  # Write
tail -f /pubsub/chat       # Read
```

### Late subscriber missing messages

Normal if ring buffer is full. Check buffer size:

```bash
cat /pubsub/chat.info | grep ring_size
```

Increase by remounting:

```bash
umount /pubsub
mount pubsubfs /pubsub '{"default_ring_size":1000}'
```

## Development

### Building

```bash
cargo build -p fs9-plugin-pubsubfs --release
```

### Testing

```bash
cargo test -p fs9-plugin-pubsubfs
```

### Loading

```bash
# Auto-load
make plugins

# Manual load
cargo build --release -p fs9-plugin-pubsubfs
cp target/release/libfs9_plugin_pubsubfs.so ./plugins/
```

## API Examples

### Python

```python
import fs9

client = fs9.Client("http://localhost:9999")

# Publish
with client.open("/pubsub/chat", write=True) as f:
    f.write(b"hello world")

# Subscribe
with client.open("/pubsub/chat", read=True) as f:
    while True:
        data = f.read(4096)
        if data:
            print(data.decode())
```

### Rust

```rust
use fs9_client::{Client, OpenFlags};

let client = Client::new("http://localhost:9999")?;

// Publish
let handle = client.open("/pubsub/chat", OpenFlags::write()).await?;
client.write(handle, b"hello world").await?;
client.close(handle).await?;

// Subscribe
let handle = client.open("/pubsub/chat", OpenFlags::read()).await?;
loop {
    let data = client.read(handle, 0, 4096).await?;
    if !data.is_empty() {
        println!("{}", String::from_utf8_lossy(&data));
    }
}
```

## License

MIT OR Apache-2.0
