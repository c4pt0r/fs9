# FS9 Python Client

Python client SDK for FS9 distributed filesystem.

## Installation

```bash
pip install fs9-client
```

## Quick Start

### Async API

```python
import asyncio
from fs9_client import Fs2Client, OpenFlags

async def main():
    async with Fs2Client("http://localhost:8080") as client:
        # Check connection
        if await client.health():
            print("Connected!")

        # High-level file operations
        await client.write_file("/hello.txt", b"Hello, World!")
        data = await client.read_file("/hello.txt")
        print(data.decode())

        # Directory operations
        await client.mkdir("/mydir")
        entries = await client.readdir("/")
        for entry in entries:
            print(f"{entry.name}: {entry.file_type.value}")

        # Low-level handle-based operations
        handle = await client.open("/data.bin", OpenFlags.create_new())
        await client.write(handle, b"binary data", offset=0)
        await client.close(handle)

        # File metadata
        info = await client.stat("/hello.txt")
        print(f"Size: {info.size}, Mode: {oct(info.mode)}")

        # Cleanup
        await client.remove("/hello.txt")

asyncio.run(main())
```

### Sync API

```python
from fs9_client import SyncFs2Client

with SyncFs2Client("http://localhost:8080") as client:
    client.write_file("/test.txt", b"content")
    data = client.read_file("/test.txt")
    print(data)
```

## API Reference

### Client Methods

| Method | Description |
|--------|-------------|
| `stat(path)` | Get file metadata |
| `wstat(path, changes)` | Modify file metadata |
| `statfs(path)` | Get filesystem statistics |
| `open(path, flags)` | Open file, returns handle |
| `read(handle, offset, size)` | Read from handle |
| `write(handle, data, offset)` | Write to handle |
| `close(handle)` | Close handle |
| `readdir(path)` | List directory contents |
| `remove(path)` | Remove file or directory |

### Convenience Methods

| Method | Description |
|--------|-------------|
| `read_file(path)` | Read entire file |
| `write_file(path, data)` | Write entire file |
| `mkdir(path)` | Create directory |
| `exists(path)` | Check if path exists |
| `is_dir(path)` | Check if path is directory |
| `is_file(path)` | Check if path is file |
| `chmod(path, mode)` | Change permissions |
| `truncate(path, size)` | Truncate file |
| `rename(path, new_name)` | Rename file |

### OpenFlags

```python
OpenFlags.read_only()      # Read only
OpenFlags.write_only()     # Write only
OpenFlags.read_write()     # Read and write
OpenFlags.create_new()     # Create if not exists
OpenFlags.create_truncate() # Create and truncate
OpenFlags.append_only()    # Append mode
OpenFlags.mkdir()          # Create directory
```

### Error Handling

```python
from fs9_client import Fs2Client, NotFoundError, PermissionDeniedError

async with Fs2Client("http://localhost:8080") as client:
    try:
        await client.stat("/nonexistent")
    except NotFoundError as e:
        print(f"File not found: {e.path}")
    except PermissionDeniedError:
        print("Access denied")
```

## Development

```bash
# Install dev dependencies
pip install -e ".[dev]"

# Run tests
pytest

# Type checking
mypy fs9_client
```
