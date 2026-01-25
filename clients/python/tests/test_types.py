from fs9_client import (
    FileInfo,
    FileType,
    FsStats,
    OpenFlags,
    StatChanges,
)


def test_file_type_checks():
    assert FileType.DIRECTORY.is_dir()
    assert not FileType.DIRECTORY.is_file()
    assert FileType.REGULAR.is_file()
    assert not FileType.REGULAR.is_dir()
    assert FileType.SYMLINK.is_symlink()


def test_file_info_from_dict():
    data = {
        "path": "/test/file.txt",
        "size": 1024,
        "file_type": "regular",
        "mode": 0o644,
        "uid": 1000,
        "gid": 1000,
        "atime": 1700000000,
        "mtime": 1700000100,
        "ctime": 1700000050,
        "etag": "abc123",
        "symlink_target": None,
    }
    info = FileInfo.from_dict(data)
    assert info.path == "/test/file.txt"
    assert info.size == 1024
    assert info.file_type == FileType.REGULAR
    assert info.is_file()
    assert not info.is_dir()
    assert info.name == "file.txt"


def test_file_info_directory():
    data = {
        "path": "/mydir",
        "size": 0,
        "file_type": "directory",
        "mode": 0o755,
        "uid": 0,
        "gid": 0,
        "atime": 0,
        "mtime": 0,
        "ctime": 0,
    }
    info = FileInfo.from_dict(data)
    assert info.is_dir()
    assert not info.is_file()


def test_fs_stats():
    stats = FsStats(
        total_bytes=1000,
        free_bytes=400,
        total_inodes=100,
        free_inodes=50,
        block_size=4096,
        max_name_len=255,
    )
    assert stats.used_bytes == 600
    assert abs(stats.usage_percent - 60.0) < 0.01


def test_fs_stats_empty():
    stats = FsStats(
        total_bytes=0,
        free_bytes=0,
        total_inodes=0,
        free_inodes=0,
        block_size=4096,
        max_name_len=255,
    )
    assert stats.usage_percent == 0.0


def test_open_flags_constructors():
    read = OpenFlags.read_only()
    assert read.read
    assert not read.write

    write = OpenFlags.write_only()
    assert write.write
    assert not write.read

    create = OpenFlags.create_new()
    assert create.read
    assert create.write
    assert create.create

    mkdir = OpenFlags.mkdir()
    assert mkdir.create
    assert mkdir.directory


def test_open_flags_to_dict():
    flags = OpenFlags.create_truncate()
    d = flags.to_dict()
    assert d["read"] is True
    assert d["write"] is True
    assert d["create"] is True
    assert d["truncate"] is True
    assert d["append"] is False


def test_stat_changes_constructors():
    chmod = StatChanges.chmod(0o644)
    assert chmod.mode == 0o644
    assert chmod.size is None

    truncate = StatChanges.truncate(100)
    assert truncate.size == 100
    assert truncate.mode is None

    rename = StatChanges.rename("newname")
    assert rename.name == "newname"


def test_stat_changes_to_dict():
    changes = StatChanges(mode=0o755, size=1024)
    d = changes.to_dict()
    assert d["mode"] == 0o755
    assert d["size"] == 1024
    assert "uid" not in d
    assert "name" not in d
