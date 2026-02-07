from __future__ import annotations

from collections.abc import AsyncIterator
from typing import Any

import httpx

from fs9_client import errors
from fs9_client.types import (
    Capabilities,
    FileHandle,
    FileInfo,
    FsStats,
    MountInfo,
    OpenFlags,
    StatChanges,
)


class Fs9Client:
    """Async client for FS9 distributed filesystem."""

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 30.0,
        token: str | None = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        headers: dict[str, str] = {}
        if token:
            headers["Authorization"] = f"Bearer {token}"

        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            timeout=timeout,
            headers=headers,
        )

    async def __aenter__(self) -> Fs9Client:
        return self

    async def __aexit__(self, *args: Any) -> None:
        await self.close_client()

    async def close_client(self) -> None:
        await self._client.aclose()

    @property
    def base_url(self) -> str:
        return self._base_url

    async def _handle_response(self, response: httpx.Response) -> None:
        if response.is_success:
            return
        try:
            data = response.json()
            message = data.get("error", "unknown error")
        except Exception:
            message = response.text or "unknown error"
        raise errors.from_response(response.status_code, message)

    async def health(self) -> bool:
        try:
            response = await self._client.get("/health")
            return response.is_success
        except httpx.RequestError:
            return False

    async def stat(self, path: str) -> FileInfo:
        response = await self._client.get("/api/v1/stat", params={"path": path})
        await self._handle_response(response)
        return FileInfo.from_dict(response.json())

    async def wstat(self, path: str, changes: StatChanges) -> None:
        response = await self._client.post(
            "/api/v1/wstat",
            json={"path": path, "changes": changes.to_dict()},
        )
        await self._handle_response(response)

    async def statfs(self, path: str) -> FsStats:
        response = await self._client.get("/api/v1/statfs", params={"path": path})
        await self._handle_response(response)
        return FsStats.from_dict(response.json())

    async def open(self, path: str, flags: OpenFlags | None = None) -> FileHandle:
        if flags is None:
            flags = OpenFlags.read_only()

        response = await self._client.post(
            "/api/v1/open",
            json={"path": path, "flags": flags.to_dict()},
        )
        await self._handle_response(response)
        data = response.json()
        return FileHandle(
            id=data["handle_id"],
            path=path,
            metadata=FileInfo.from_dict(data["metadata"]),
        )

    async def read(self, handle: FileHandle, offset: int = 0, size: int = 1024 * 1024) -> bytes:
        response = await self._client.post(
            "/api/v1/read",
            json={"handle_id": handle.id, "offset": offset, "size": size},
        )
        await self._handle_response(response)
        return response.content

    async def write(self, handle: FileHandle, data: bytes, offset: int = 0) -> int:
        response = await self._client.post(
            "/api/v1/write",
            params={"handle_id": handle.id, "offset": offset},
            content=data,
        )
        await self._handle_response(response)
        return response.json()["bytes_written"]

    async def close(self, handle: FileHandle, *, sync: bool = False) -> None:
        response = await self._client.post(
            "/api/v1/close",
            json={"handle_id": handle.id, "sync": sync},
        )
        await self._handle_response(response)

    async def readdir(self, path: str) -> list[FileInfo]:
        response = await self._client.get("/api/v1/readdir", params={"path": path})
        await self._handle_response(response)
        return [FileInfo.from_dict(item) for item in response.json()]

    async def remove(self, path: str) -> None:
        response = await self._client.delete("/api/v1/remove", params={"path": path})
        await self._handle_response(response)

    async def capabilities(self, path: str) -> Capabilities:
        response = await self._client.get("/api/v1/capabilities", params={"path": path})
        await self._handle_response(response)
        return Capabilities.from_dict(response.json())

    async def list_mounts(self) -> list[MountInfo]:
        response = await self._client.get("/api/v1/mounts")
        await self._handle_response(response)
        return [MountInfo.from_dict(item) for item in response.json()]

    async def read_file(self, path: str) -> bytes:
        handle = await self.open(path, OpenFlags.read_only())
        try:
            return await self.read(handle, 0, handle.size or 1024 * 1024)
        finally:
            await self.close(handle)

    async def write_file(self, path: str, data: bytes) -> None:
        handle = await self.open(path, OpenFlags.create_truncate())
        try:
            await self.write(handle, data, 0)
        finally:
            await self.close(handle)

    async def download(self, path: str) -> bytes:
        response = await self._client.get("/api/v1/download", params={"path": path})
        await self._handle_response(response)
        return response.content

    async def download_range(self, path: str, start: int, end: int) -> bytes:
        response = await self._client.get(
            "/api/v1/download",
            params={"path": path},
            headers={"Range": f"bytes={start}-{end}"},
        )
        await self._handle_response(response)
        return response.content

    async def download_stream(self, path: str) -> AsyncIterator[bytes]:
        async with self._client.stream("GET", "/api/v1/download", params={"path": path}) as response:
            if not response.is_success:
                await response.aread()
                try:
                    data = response.json()
                    message = data.get("error", "unknown error")
                except Exception:
                    message = response.text or "unknown error"
                raise errors.from_response(response.status_code, message)
            async for chunk in response.aiter_bytes():
                yield chunk

    async def upload(self, path: str, data: bytes) -> int:
        response = await self._client.put(
            "/api/v1/upload",
            params={"path": path},
            content=data,
        )
        await self._handle_response(response)
        return response.json()["bytes_written"]

    async def upload_stream(self, path: str, stream: AsyncIterator[bytes]) -> int:
        response = await self._client.put(
            "/api/v1/upload",
            params={"path": path},
            content=stream,
        )
        await self._handle_response(response)
        return response.json()["bytes_written"]

    async def mkdir(self, path: str) -> None:
        handle = await self.open(path, OpenFlags.mkdir())
        await self.close(handle)

    async def exists(self, path: str) -> bool:
        try:
            await self.stat(path)
            return True
        except errors.NotFoundError:
            return False

    async def is_dir(self, path: str) -> bool:
        try:
            info = await self.stat(path)
            return info.is_dir()
        except errors.NotFoundError:
            return False

    async def is_file(self, path: str) -> bool:
        try:
            info = await self.stat(path)
            return info.is_file()
        except errors.NotFoundError:
            return False

    async def chmod(self, path: str, mode: int) -> None:
        await self.wstat(path, StatChanges.chmod(mode))

    async def truncate(self, path: str, size: int) -> None:
        await self.wstat(path, StatChanges.truncate(size))

    async def rename(self, path: str, new_name: str) -> None:
        await self.wstat(path, StatChanges.rename(new_name))


class SyncFs9Client:
    """Synchronous wrapper for Fs9Client."""

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 30.0,
        token: str | None = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        headers: dict[str, str] = {}
        if token:
            headers["Authorization"] = f"Bearer {token}"

        self._client = httpx.Client(
            base_url=self._base_url,
            timeout=timeout,
            headers=headers,
        )

    def __enter__(self) -> SyncFs9Client:
        return self

    def __exit__(self, *args: Any) -> None:
        self.close_client()

    def close_client(self) -> None:
        self._client.close()

    @property
    def base_url(self) -> str:
        return self._base_url

    def _handle_response(self, response: httpx.Response) -> None:
        if response.is_success:
            return
        try:
            data = response.json()
            message = data.get("error", "unknown error")
        except Exception:
            message = response.text or "unknown error"
        raise errors.from_response(response.status_code, message)

    def health(self) -> bool:
        try:
            response = self._client.get("/health")
            return response.is_success
        except httpx.RequestError:
            return False

    def stat(self, path: str) -> FileInfo:
        response = self._client.get("/api/v1/stat", params={"path": path})
        self._handle_response(response)
        return FileInfo.from_dict(response.json())

    def wstat(self, path: str, changes: StatChanges) -> None:
        response = self._client.post(
            "/api/v1/wstat",
            json={"path": path, "changes": changes.to_dict()},
        )
        self._handle_response(response)

    def statfs(self, path: str) -> FsStats:
        response = self._client.get("/api/v1/statfs", params={"path": path})
        self._handle_response(response)
        return FsStats.from_dict(response.json())

    def open(self, path: str, flags: OpenFlags | None = None) -> FileHandle:
        if flags is None:
            flags = OpenFlags.read_only()

        response = self._client.post(
            "/api/v1/open",
            json={"path": path, "flags": flags.to_dict()},
        )
        self._handle_response(response)
        data = response.json()
        return FileHandle(
            id=data["handle_id"],
            path=path,
            metadata=FileInfo.from_dict(data["metadata"]),
        )

    def read(self, handle: FileHandle, offset: int = 0, size: int = 1024 * 1024) -> bytes:
        response = self._client.post(
            "/api/v1/read",
            json={"handle_id": handle.id, "offset": offset, "size": size},
        )
        self._handle_response(response)
        return response.content

    def write(self, handle: FileHandle, data: bytes, offset: int = 0) -> int:
        response = self._client.post(
            "/api/v1/write",
            params={"handle_id": handle.id, "offset": offset},
            content=data,
        )
        self._handle_response(response)
        return response.json()["bytes_written"]

    def close(self, handle: FileHandle, *, sync: bool = False) -> None:
        response = self._client.post(
            "/api/v1/close",
            json={"handle_id": handle.id, "sync": sync},
        )
        self._handle_response(response)

    def readdir(self, path: str) -> list[FileInfo]:
        response = self._client.get("/api/v1/readdir", params={"path": path})
        self._handle_response(response)
        return [FileInfo.from_dict(item) for item in response.json()]

    def remove(self, path: str) -> None:
        response = self._client.delete("/api/v1/remove", params={"path": path})
        self._handle_response(response)

    def read_file(self, path: str) -> bytes:
        handle = self.open(path, OpenFlags.read_only())
        try:
            return self.read(handle, 0, handle.size or 1024 * 1024)
        finally:
            self.close(handle)

    def write_file(self, path: str, data: bytes) -> None:
        handle = self.open(path, OpenFlags.create_truncate())
        try:
            self.write(handle, data, 0)
        finally:
            self.close(handle)

    def download(self, path: str) -> bytes:
        response = self._client.get("/api/v1/download", params={"path": path})
        self._handle_response(response)
        return response.content

    def download_range(self, path: str, start: int, end: int) -> bytes:
        response = self._client.get(
            "/api/v1/download",
            params={"path": path},
            headers={"Range": f"bytes={start}-{end}"},
        )
        self._handle_response(response)
        return response.content

    def upload(self, path: str, data: bytes) -> int:
        response = self._client.put(
            "/api/v1/upload",
            params={"path": path},
            content=data,
        )
        self._handle_response(response)
        return response.json()["bytes_written"]

    def mkdir(self, path: str) -> None:
        handle = self.open(path, OpenFlags.mkdir())
        self.close(handle)

    def exists(self, path: str) -> bool:
        try:
            self.stat(path)
            return True
        except errors.NotFoundError:
            return False
