from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any


class FileType(Enum):
    """File type enumeration."""

    REGULAR = "regular"
    DIRECTORY = "directory"
    SYMLINK = "symlink"

    def is_dir(self) -> bool:
        return self == FileType.DIRECTORY

    def is_file(self) -> bool:
        return self == FileType.REGULAR

    def is_symlink(self) -> bool:
        return self == FileType.SYMLINK


@dataclass
class FileInfo:
    """File metadata."""

    path: str
    size: int
    file_type: FileType
    mode: int
    uid: int
    gid: int
    atime: int
    mtime: int
    ctime: int
    etag: str = ""
    symlink_target: str | None = None

    def is_dir(self) -> bool:
        return self.file_type.is_dir()

    def is_file(self) -> bool:
        return self.file_type.is_file()

    def is_symlink(self) -> bool:
        return self.file_type.is_symlink()

    @property
    def name(self) -> str:
        return self.path.rsplit("/", 1)[-1]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FileInfo:
        file_type_str = data.get("file_type", "regular")
        try:
            file_type = FileType(file_type_str)
        except ValueError:
            file_type = FileType.REGULAR

        return cls(
            path=data["path"],
            size=data["size"],
            file_type=file_type,
            mode=data["mode"],
            uid=data["uid"],
            gid=data["gid"],
            atime=data["atime"],
            mtime=data["mtime"],
            ctime=data["ctime"],
            etag=data.get("etag", ""),
            symlink_target=data.get("symlink_target"),
        )


@dataclass
class FsStats:
    """Filesystem statistics."""

    total_bytes: int
    free_bytes: int
    total_inodes: int
    free_inodes: int
    block_size: int
    max_name_len: int

    @property
    def used_bytes(self) -> int:
        return self.total_bytes - self.free_bytes

    @property
    def usage_percent(self) -> float:
        if self.total_bytes == 0:
            return 0.0
        return (self.used_bytes / self.total_bytes) * 100.0

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FsStats:
        return cls(
            total_bytes=data["total_bytes"],
            free_bytes=data["free_bytes"],
            total_inodes=data["total_inodes"],
            free_inodes=data["free_inodes"],
            block_size=data["block_size"],
            max_name_len=data["max_name_len"],
        )


@dataclass
class OpenFlags:
    """Flags for opening files."""

    read: bool = False
    write: bool = False
    create: bool = False
    truncate: bool = False
    append: bool = False
    directory: bool = False

    def to_dict(self) -> dict[str, bool]:
        return {
            "read": self.read,
            "write": self.write,
            "create": self.create,
            "truncate": self.truncate,
            "append": self.append,
            "directory": self.directory,
        }

    @classmethod
    def read_only(cls) -> OpenFlags:
        return cls(read=True)

    @classmethod
    def write_only(cls) -> OpenFlags:
        return cls(write=True)

    @classmethod
    def read_write(cls) -> OpenFlags:
        return cls(read=True, write=True)

    @classmethod
    def create_new(cls) -> OpenFlags:
        return cls(read=True, write=True, create=True)

    @classmethod
    def create_truncate(cls) -> OpenFlags:
        return cls(read=True, write=True, create=True, truncate=True)

    @classmethod
    def append_only(cls) -> OpenFlags:
        return cls(write=True, append=True)

    @classmethod
    def mkdir(cls) -> OpenFlags:
        return cls(create=True, directory=True)


@dataclass
class StatChanges:
    """Changes to apply via wstat."""

    mode: int | None = None
    uid: int | None = None
    gid: int | None = None
    size: int | None = None
    atime: int | None = None
    mtime: int | None = None
    name: str | None = None
    symlink_target: str | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if self.mode is not None:
            result["mode"] = self.mode
        if self.uid is not None:
            result["uid"] = self.uid
        if self.gid is not None:
            result["gid"] = self.gid
        if self.size is not None:
            result["size"] = self.size
        if self.atime is not None:
            result["atime"] = self.atime
        if self.mtime is not None:
            result["mtime"] = self.mtime
        if self.name is not None:
            result["name"] = self.name
        if self.symlink_target is not None:
            result["symlink_target"] = self.symlink_target
        return result

    @classmethod
    def chmod(cls, mode: int) -> StatChanges:
        return cls(mode=mode)

    @classmethod
    def chown(cls, uid: int, gid: int) -> StatChanges:
        return cls(uid=uid, gid=gid)

    @classmethod
    def truncate(cls, size: int) -> StatChanges:
        return cls(size=size)

    @classmethod
    def rename(cls, new_name: str) -> StatChanges:
        return cls(name=new_name)


@dataclass
class FileHandle:
    """Handle to an open file."""

    id: str
    path: str
    metadata: FileInfo

    @property
    def size(self) -> int:
        return self.metadata.size

    @property
    def name(self) -> str:
        return self.metadata.name


@dataclass
class MountInfo:
    """Mount point information."""

    path: str
    provider_name: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MountInfo:
        return cls(
            path=data["path"],
            provider_name=data["provider_name"],
        )


@dataclass
class Capabilities:
    """Provider capabilities."""

    capabilities: list[str] = field(default_factory=list)
    provider_type: str = ""

    def can_read(self) -> bool:
        return "read" in self.capabilities

    def can_write(self) -> bool:
        return "write" in self.capabilities

    def can_create(self) -> bool:
        return "create" in self.capabilities

    def can_delete(self) -> bool:
        return "delete" in self.capabilities

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Capabilities:
        return cls(
            capabilities=data.get("capabilities", []),
            provider_type=data.get("provider_type", ""),
        )
