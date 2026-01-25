import pytest
from pytest_httpx import HTTPXMock

from fs9_client import (
    Fs9Client,
    OpenFlags,
    StatChanges,
    NotFoundError,
    AlreadyExistsError,
)


@pytest.fixture
async def client():
    async with Fs9Client("http://localhost:8080") as c:
        yield c


async def test_health(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(url="http://localhost:8080/health", text="OK")
    assert await client.health() is True


async def test_health_failure(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(url="http://localhost:8080/health", status_code=500)
    assert await client.health() is False


async def test_stat(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/stat?path=%2Ftest.txt",
        json={
            "path": "/test.txt",
            "size": 1024,
            "file_type": "regular",
            "mode": 0o644,
            "uid": 1000,
            "gid": 1000,
            "atime": 1700000000,
            "mtime": 1700000000,
            "ctime": 1700000000,
            "etag": "",
        },
    )
    info = await client.stat("/test.txt")
    assert info.path == "/test.txt"
    assert info.size == 1024
    assert info.is_file()


async def test_stat_not_found(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/stat?path=%2Fmissing.txt",
        status_code=404,
        json={"error": "not found: /missing.txt", "code": 404},
    )
    with pytest.raises(NotFoundError):
        await client.stat("/missing.txt")


async def test_open_and_close(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/open",
        json={
            "handle_id": "uuid-123",
            "metadata": {
                "path": "/test.txt",
                "size": 100,
                "file_type": "regular",
                "mode": 0o644,
                "uid": 0,
                "gid": 0,
                "atime": 0,
                "mtime": 0,
                "ctime": 0,
            },
        },
    )
    httpx_mock.add_response(url="http://localhost:8080/api/v1/close", status_code=204)

    handle = await client.open("/test.txt", OpenFlags.read_only())
    assert handle.id == "uuid-123"
    assert handle.path == "/test.txt"
    await client.close(handle)


async def test_read(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/open",
        json={
            "handle_id": "uuid-456",
            "metadata": {
                "path": "/data.bin",
                "size": 5,
                "file_type": "regular",
                "mode": 0o644,
                "uid": 0,
                "gid": 0,
                "atime": 0,
                "mtime": 0,
                "ctime": 0,
            },
        },
    )
    httpx_mock.add_response(url="http://localhost:8080/api/v1/read", content=b"hello")
    httpx_mock.add_response(url="http://localhost:8080/api/v1/close", status_code=204)

    handle = await client.open("/data.bin")
    data = await client.read(handle, 0, 100)
    assert data == b"hello"
    await client.close(handle)


async def test_write(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/open",
        json={
            "handle_id": "uuid-789",
            "metadata": {
                "path": "/output.txt",
                "size": 0,
                "file_type": "regular",
                "mode": 0o644,
                "uid": 0,
                "gid": 0,
                "atime": 0,
                "mtime": 0,
                "ctime": 0,
            },
        },
    )
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/write?handle_id=uuid-789&offset=0",
        json={"bytes_written": 11},
    )
    httpx_mock.add_response(url="http://localhost:8080/api/v1/close", status_code=204)

    handle = await client.open("/output.txt", OpenFlags.create_new())
    written = await client.write(handle, b"hello world", 0)
    assert written == 11
    await client.close(handle)


async def test_readdir(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/readdir?path=%2F",
        json=[
            {
                "path": "/file1.txt",
                "size": 100,
                "file_type": "regular",
                "mode": 0o644,
                "uid": 0,
                "gid": 0,
                "atime": 0,
                "mtime": 0,
                "ctime": 0,
            },
            {
                "path": "/subdir",
                "size": 0,
                "file_type": "directory",
                "mode": 0o755,
                "uid": 0,
                "gid": 0,
                "atime": 0,
                "mtime": 0,
                "ctime": 0,
            },
        ],
    )
    entries = await client.readdir("/")
    assert len(entries) == 2
    assert entries[0].path == "/file1.txt"
    assert entries[0].is_file()
    assert entries[1].path == "/subdir"
    assert entries[1].is_dir()


async def test_remove(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/remove?path=%2Fto_delete.txt",
        status_code=204,
    )
    await client.remove("/to_delete.txt")


async def test_wstat_chmod(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(url="http://localhost:8080/api/v1/wstat", status_code=204)
    await client.chmod("/test.txt", 0o755)


async def test_exists(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/stat?path=%2Fexists.txt",
        json={
            "path": "/exists.txt",
            "size": 0,
            "file_type": "regular",
            "mode": 0o644,
            "uid": 0,
            "gid": 0,
            "atime": 0,
            "mtime": 0,
            "ctime": 0,
        },
    )
    assert await client.exists("/exists.txt") is True

    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/stat?path=%2Fmissing.txt",
        status_code=404,
        json={"error": "not found", "code": 404},
    )
    assert await client.exists("/missing.txt") is False


async def test_mkdir(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/open",
        json={
            "handle_id": "dir-handle",
            "metadata": {
                "path": "/newdir",
                "size": 0,
                "file_type": "directory",
                "mode": 0o755,
                "uid": 0,
                "gid": 0,
                "atime": 0,
                "mtime": 0,
                "ctime": 0,
            },
        },
    )
    httpx_mock.add_response(url="http://localhost:8080/api/v1/close", status_code=204)
    await client.mkdir("/newdir")


async def test_list_mounts(client: Fs9Client, httpx_mock: HTTPXMock):
    httpx_mock.add_response(
        url="http://localhost:8080/api/v1/mounts",
        json=[
            {"path": "/", "provider_name": "memfs"},
            {"path": "/data", "provider_name": "localfs"},
        ],
    )
    mounts = await client.list_mounts()
    assert len(mounts) == 2
    assert mounts[0].path == "/"
    assert mounts[0].provider_name == "memfs"
