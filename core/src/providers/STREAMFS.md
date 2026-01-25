# StreamFS - Streaming File System Plugin

A streaming filesystem for FS9 that supports multiple concurrent readers/writers with real-time data fanout.

## Features

- **Multiple Writers**: Concurrent append operations to the same stream
- **Multiple Readers**: Independent consumption with broadcast fanout
- **Ring Buffer**: Historical data for late-joining readers
- **Non-blocking**: Slow readers don't block writers or other readers

## Architecture

```
StreamFS
├── streams: HashMap<String, Arc<StreamFile>>
└── handles: TokioRwLock<HashMap<u64, StreamHandle>>

StreamFile
├── ring_buffer: Vec<Bytes>          # Circular buffer (100 chunks default)
├── sender: broadcast::Sender<Bytes>  # Real-time fanout
└── readers: HashMap<u64, ReaderState>
```

## Usage

### Writing to Streams

```bash
# Create stream and write data
echo "event1" > /streamfs/events
echo "event2" > /streamfs/events
```

Writes are append-only. The offset parameter is ignored.

### Reading from Streams

```bash
# Read existing data (historical from ring buffer)
cat /streamfs/events

# Stream mode - wait for new data indefinitely
cat --stream /streamfs/events
```

### Listing Streams

```bash
ls /streamfs
```

### Removing Streams

```bash
rm /streamfs/events
```

## Configuration

Default settings:
- Ring buffer size: 100 chunks
- Broadcast channel size: 100 messages

## API

StreamFS implements `FsProvider` with these behaviors:

| Operation | Behavior |
|-----------|----------|
| `stat` | Returns stream metadata (size = total bytes written) |
| `open` | Creates stream if not exists; registers reader if read mode |
| `read` | Returns buffered data or waits for new data (30s timeout) |
| `write` | Appends to stream, broadcasts to all readers |
| `close` | Unregisters reader |
| `readdir` | Lists all active streams |
| `remove` | Closes and removes stream |

## Limitations

- In-memory only (not persistent across restarts)
- No seek support (streaming only)
- No truncation support
