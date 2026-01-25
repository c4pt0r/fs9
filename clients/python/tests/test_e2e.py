"""
End-to-end tests for the Python FS9 client.

These tests require a running FS9 server.
Run with: pytest tests/test_e2e.py -v

To start the server:
    cargo run -p fs9-server

Or use the test harness (starts server automatically):
    FS9_SERVER_URL=http://localhost:3000 pytest tests/test_e2e.py -v
"""

import os
import random
import subprocess
import socket
import time
from contextlib import contextmanager

import pytest

from fs9_client import Fs9Client, SyncFs9Client, OpenFlags, NotFoundError


def find_free_port() -> int:
    """Find an available port."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def find_server_binary() -> str:
    """Find the fs9-server binary."""
    # Get the project root: clients/python/tests -> clients/python -> clients -> fs2/
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
    
    candidates = [
        os.path.join(project_root, "target", "debug", "fs9-server"),
        os.path.join(project_root, "target", "release", "fs9-server"),
    ]
    
    for candidate in candidates:
        if os.path.exists(candidate):
            return candidate
    
    raise FileNotFoundError(
        f"fs9-server binary not found. Run 'cargo build -p fs9-server' first. Checked: {candidates}"
    )


@contextmanager
def start_server():
    """Start a test server and return its URL."""
    # Check if server URL is provided externally
    external_url = os.environ.get("FS9_SERVER_URL")
    if external_url:
        yield external_url
        return
    
    # Start our own server
    server_bin = find_server_binary()
    port = find_free_port()
    url = f"http://127.0.0.1:{port}"
    
    process = subprocess.Popen(
        [server_bin],
        env={
            **os.environ,
            "FS9_PORT": str(port),
            "FS9_HOST": "127.0.0.1",
            "RUST_LOG": "warn",
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    
    try:
        # Wait for server to be ready
        import httpx
        client = httpx.Client(timeout=1.0)
        for _ in range(50):
            time.sleep(0.1)
            try:
                response = client.get(f"{url}/health")
                if response.is_success:
                    break
            except httpx.RequestError:
                pass
        else:
            raise RuntimeError("Server failed to start within 5 seconds")
        client.close()
        
        yield url
    finally:
        process.terminate()
        process.wait(timeout=5)


@pytest.fixture(scope="module")
def server_url():
    """Provide server URL for all tests in the module."""
    with start_server() as url:
        yield url


@pytest.fixture
async def client(server_url):
    """Provide an async client for tests."""
    async with Fs9Client(server_url) as c:
        yield c


@pytest.fixture
def sync_client(server_url):
    """Provide a sync client for tests."""
    with SyncFs9Client(server_url) as c:
        yield c


def generate_test_path(prefix: str) -> str:
    """Generate a unique test path."""
    suffix = random.randint(0, 2**31)
    return f"/{prefix}_{suffix}"


# Async client tests

@pytest.mark.asyncio
async def test_health(client: Fs9Client):
    assert await client.health() is True


@pytest.mark.asyncio
async def test_write_and_read_file(client: Fs9Client):
    path = generate_test_path("write_read")
    content = b"Hello, FS9 from Python!"
    
    await client.write_file(path, content)
    data = await client.read_file(path)
    assert data == content
    
    await client.remove(path)


@pytest.mark.asyncio
async def test_file_stat(client: Fs9Client):
    path = generate_test_path("stat")
    content = b"test content for stat"
    
    await client.write_file(path, content)
    
    info = await client.stat(path)
    assert info.size == len(content)
    assert info.is_file()
    assert not info.is_dir()
    
    await client.remove(path)


@pytest.mark.asyncio
async def test_directory_operations(client: Fs9Client):
    dir_path = generate_test_path("dir")
    file_path = f"{dir_path}/file.txt"
    
    await client.mkdir(dir_path)
    assert await client.is_dir(dir_path)
    
    await client.write_file(file_path, b"content")
    
    entries = await client.readdir(dir_path)
    assert len(entries) == 1
    assert entries[0].path.endswith("file.txt")
    
    await client.remove(file_path)
    await client.remove(dir_path)


@pytest.mark.asyncio
async def test_file_handle_operations(client: Fs9Client):
    path = generate_test_path("handle")
    
    handle = await client.open(path, OpenFlags.create_truncate())
    
    await client.write(handle, b"first", 0)
    await client.write(handle, b" second", 5)
    
    await client.close(handle)
    
    handle = await client.open(path, OpenFlags.read_only())
    data = await client.read(handle, 0, 100)
    assert data == b"first second"
    
    partial = await client.read(handle, 6, 6)
    assert partial == b"second"
    
    await client.close(handle)
    await client.remove(path)


@pytest.mark.asyncio
async def test_chmod_operation(client: Fs9Client):
    path = generate_test_path("chmod")
    
    await client.write_file(path, b"test")
    await client.chmod(path, 0o755)
    
    info = await client.stat(path)
    assert (info.mode & 0o777) == 0o755
    
    await client.remove(path)


@pytest.mark.asyncio
async def test_truncate_operation(client: Fs9Client):
    path = generate_test_path("truncate")
    
    await client.write_file(path, b"hello world")
    await client.truncate(path, 5)
    
    info = await client.stat(path)
    assert info.size == 5
    
    data = await client.read_file(path)
    assert data == b"hello"
    
    await client.remove(path)


@pytest.mark.asyncio
async def test_exists_check(client: Fs9Client):
    path = generate_test_path("exists")
    
    assert await client.exists(path) is False
    
    await client.write_file(path, b"x")
    assert await client.exists(path) is True
    
    await client.remove(path)
    assert await client.exists(path) is False


@pytest.mark.asyncio
async def test_not_found_error(client: Fs9Client):
    path = "/nonexistent_file_12345.txt"
    
    with pytest.raises(NotFoundError):
        await client.stat(path)


@pytest.mark.asyncio
async def test_list_mounts(client: Fs9Client):
    mounts = await client.list_mounts()
    assert len(mounts) > 0
    
    root = next((m for m in mounts if m.path == "/"), None)
    assert root is not None


@pytest.mark.asyncio
async def test_capabilities(client: Fs9Client):
    caps = await client.capabilities("/")
    assert caps.can_read
    assert caps.can_write


@pytest.mark.asyncio
async def test_statfs(client: Fs9Client):
    stats = await client.statfs("/")
    assert stats.total_bytes > 0
    assert stats.block_size > 0


@pytest.mark.asyncio
async def test_large_file(client: Fs9Client):
    path = generate_test_path("large")
    data = bytes(i % 256 for i in range(100_000))
    
    await client.write_file(path, data)
    read_data = await client.read_file(path)
    
    assert len(read_data) == len(data)
    assert read_data == data
    
    await client.remove(path)


@pytest.mark.asyncio
async def test_nested_directories(client: Fs9Client):
    base = generate_test_path("nested")
    level1 = f"{base}/a"
    level2 = f"{level1}/b"
    file_path = f"{level2}/file.txt"
    
    await client.mkdir(base)
    await client.mkdir(level1)
    await client.mkdir(level2)
    await client.write_file(file_path, b"deep")
    
    assert await client.is_file(file_path)
    assert await client.is_dir(level2)
    
    data = await client.read_file(file_path)
    assert data == b"deep"
    
    await client.remove(file_path)
    await client.remove(level2)
    await client.remove(level1)
    await client.remove(base)


# Sync client tests

def test_sync_health(sync_client: SyncFs9Client):
    assert sync_client.health() is True


def test_sync_write_and_read_file(sync_client: SyncFs9Client):
    path = generate_test_path("sync_wr")
    content = b"Hello from sync client!"
    
    sync_client.write_file(path, content)
    data = sync_client.read_file(path)
    assert data == content
    
    sync_client.remove(path)


def test_sync_mkdir_and_readdir(sync_client: SyncFs9Client):
    dir_path = generate_test_path("sync_dir")
    file_path = f"{dir_path}/test.txt"
    
    sync_client.mkdir(dir_path)
    sync_client.write_file(file_path, b"test")
    
    entries = sync_client.readdir(dir_path)
    assert len(entries) == 1
    
    sync_client.remove(file_path)
    sync_client.remove(dir_path)


def test_sync_exists(sync_client: SyncFs9Client):
    path = generate_test_path("sync_exists")
    
    assert sync_client.exists(path) is False
    sync_client.write_file(path, b"x")
    assert sync_client.exists(path) is True
    sync_client.remove(path)
