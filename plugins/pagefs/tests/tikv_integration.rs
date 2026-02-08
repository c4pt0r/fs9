#![cfg(feature = "tikv")]

use fs9_plugin_pagefs::provider::PageFsProvider;
use fs9_plugin_pagefs::{KvBackend, TikvKvBackend, PAGE_SIZE};
use fs9_sdk::{FileType, FsError, OpenFlags, StatChanges};

fn fresh_tikv_provider(test_name: &str) -> PageFsProvider {
    let ns = format!("fs9_test_{test_name}");
    let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns));
    wipe_all_keys(&backend);
    PageFsProvider::new(Box::new(backend))
}

fn fresh_tikv_provider_with_uid(test_name: &str, uid: u32, gid: u32) -> PageFsProvider {
    let ns = format!("fs9_test_{test_name}");
    let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns));
    wipe_all_keys(&backend);
    PageFsProvider::with_config(Box::new(backend), uid, gid)
}

fn wipe_all_keys(backend: &TikvKvBackend) {
    for prefix in [b"S".as_slice(), b"I", b"D", b"P"] {
        let keys: Vec<_> = backend.scan(prefix).into_iter().map(|(k, _)| k).collect();
        for key in keys {
            backend.delete(&key);
        }
    }
}

// ---------------------------------------------------------------------------
// Basic CRUD
// ---------------------------------------------------------------------------

#[test]
fn tikv_write_read_roundtrip() {
    let p = fresh_tikv_provider("rw");

    let (handle, _info) = p.open("/hello.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"hello tikv").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, info) = p.open("/hello.txt", OpenFlags::read()).unwrap();
    assert_eq!(info.size, 10);
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"hello tikv");
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_mkdir_readdir() {
    let p = fresh_tikv_provider("readdir");

    let (handle, _) = p.open("/mydir", OpenFlags::create_dir()).unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/mydir/a.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"aaa").unwrap();
    p.close(handle.id()).unwrap();

    let entries = p.readdir("/mydir").unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].path.ends_with("a.txt"));
}

#[test]
fn tikv_remove() {
    let p = fresh_tikv_provider("rm");

    let (handle, _) = p.open("/todelete.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"bye").unwrap();
    p.close(handle.id()).unwrap();

    p.remove("/todelete.txt").unwrap();
    assert!(p.stat("/todelete.txt").is_err());
}

#[test]
fn tikv_large_file_multi_page() {
    let p = fresh_tikv_provider("bigfile");

    let data = vec![0xABu8; 32 * 1024];
    let (handle, _) = p.open("/bigfile.bin", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, &data).unwrap();
    p.close(handle.id()).unwrap();

    let (handle, info) = p.open("/bigfile.bin", OpenFlags::read()).unwrap();
    assert_eq!(info.size, 32 * 1024);
    let read_back = p.read(handle.id(), 0, 32 * 1024).unwrap();
    assert_eq!(read_back.len(), 32 * 1024);
    assert!(read_back.iter().all(|&b| b == 0xAB));
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Root & stat
// ---------------------------------------------------------------------------

#[test]
fn tikv_root_exists() {
    let p = fresh_tikv_provider("root");
    let info = p.stat("/").unwrap();
    assert_eq!(info.file_type, FileType::Directory);
    assert_eq!(info.mode, 0o755);
}

#[test]
fn tikv_stat_nonexistent_returns_not_found() {
    let p = fresh_tikv_provider("stat_nf");
    assert!(matches!(
        p.stat("/nonexistent.txt"),
        Err(FsError::NotFound(_))
    ));
}

// ---------------------------------------------------------------------------
// Page-boundary writes
// ---------------------------------------------------------------------------

#[test]
fn tikv_write_across_page_boundary() {
    let p = fresh_tikv_provider("cross_page");

    let (handle, _) = p.open("/cross.txt", OpenFlags::create_file()).unwrap();
    let data: Vec<u8> = (0..PAGE_SIZE + 1000).map(|i| (i % 256) as u8).collect();
    p.write(handle.id(), 0, &data).unwrap();
    p.close(handle.id()).unwrap();

    let info = p.stat("/cross.txt").unwrap();
    assert_eq!(info.size, data.len() as u64);

    let (handle, _) = p.open("/cross.txt", OpenFlags::read()).unwrap();
    let read_data = p.read(handle.id(), 0, data.len()).unwrap();
    assert_eq!(&read_data[..], &data[..]);
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_large_file_spanning_many_pages() {
    let p = fresh_tikv_provider("many_pages");

    let (handle, _) = p.open("/large.bin", OpenFlags::create_file()).unwrap();
    let data: Vec<u8> = (0..(PAGE_SIZE * 3 + 5000))
        .map(|i| (i % 256) as u8)
        .collect();
    p.write(handle.id(), 0, &data).unwrap();
    p.close(handle.id()).unwrap();

    let info = p.stat("/large.bin").unwrap();
    assert_eq!(info.size, data.len() as u64);

    let (handle, _) = p.open("/large.bin", OpenFlags::read()).unwrap();
    let read_data = p.read(handle.id(), 0, data.len()).unwrap();
    assert_eq!(read_data.len(), data.len());
    assert_eq!(&read_data[..], &data[..]);
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Partial / offset reads
// ---------------------------------------------------------------------------

#[test]
fn tikv_read_partial() {
    let p = fresh_tikv_provider("partial_read");

    let (handle, _) = p.open("/partial.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"0123456789ABCDEF0123456789")
        .unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/partial.txt", OpenFlags::read()).unwrap();
    let result = p.read(handle.id(), 10, 10).unwrap();
    assert_eq!(&result[..], b"ABCDEF0123");
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_read_beyond_eof_returns_empty() {
    let p = fresh_tikv_provider("read_eof");

    let (handle, _) = p.open("/small.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"tiny").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/small.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 1000, 100).unwrap();
    assert!(data.is_empty());
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Sparse writes
// ---------------------------------------------------------------------------

#[test]
fn tikv_sparse_write() {
    let p = fresh_tikv_provider("sparse");

    let (handle, _) = p.open("/sparse.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), PAGE_SIZE as u64, b"sparse data")
        .unwrap();
    p.close(handle.id()).unwrap();

    let info = p.stat("/sparse.txt").unwrap();
    assert_eq!(info.size, PAGE_SIZE as u64 + 11);

    let (handle, _) = p.open("/sparse.txt", OpenFlags::read()).unwrap();
    // First page should be zeros
    let first_page = p.read(handle.id(), 0, PAGE_SIZE).unwrap();
    assert!(first_page.iter().all(|&b| b == 0));
    // Data at offset
    let sparse_data = p.read(handle.id(), PAGE_SIZE as u64, 11).unwrap();
    assert_eq!(&sparse_data[..], b"sparse data");
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Append mode
// ---------------------------------------------------------------------------

#[test]
fn tikv_append_mode() {
    let p = fresh_tikv_provider("append");

    let (handle, _) = p.open("/append.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"first").unwrap();
    p.close(handle.id()).unwrap();

    let flags = OpenFlags {
        write: true,
        append: true,
        ..Default::default()
    };
    let (handle, _) = p.open("/append.txt", flags).unwrap();
    p.write(handle.id(), 0, b"second").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/append.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"firstsecond");
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Truncate
// ---------------------------------------------------------------------------

#[test]
fn tikv_truncate_file() {
    let p = fresh_tikv_provider("truncate");

    let (handle, _) = p.open("/trunc.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"long content here that will be truncated")
        .unwrap();
    p.close(handle.id()).unwrap();

    p.wstat("/trunc.txt", &StatChanges::truncate(10)).unwrap();

    let info = p.stat("/trunc.txt").unwrap();
    assert_eq!(info.size, 10);

    let (handle, _) = p.open("/trunc.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"long conte");
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_truncate_to_zero() {
    let p = fresh_tikv_provider("trunc_zero");

    let (handle, _) = p.open("/z.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"some data").unwrap();
    p.close(handle.id()).unwrap();

    p.wstat("/z.txt", &StatChanges::truncate(0)).unwrap();

    let info = p.stat("/z.txt").unwrap();
    assert_eq!(info.size, 0);

    let (handle, _) = p.open("/z.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert!(data.is_empty());
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_extend_file_via_wstat() {
    let p = fresh_tikv_provider("extend");

    let (handle, _) = p.open("/extend.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"short").unwrap();
    p.close(handle.id()).unwrap();

    p.wstat("/extend.txt", &StatChanges::truncate(20)).unwrap();

    let info = p.stat("/extend.txt").unwrap();
    assert_eq!(info.size, 20);

    // Extended region should be zeros
    let (handle, _) = p.open("/extend.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 20).unwrap();
    assert_eq!(&data[..5], b"short");
    assert!(data[5..].iter().all(|&b| b == 0));
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_open_with_truncate_flag() {
    let p = fresh_tikv_provider("open_trunc");

    let (handle, _) = p.open("/ot.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"original content").unwrap();
    p.close(handle.id()).unwrap();

    let flags = OpenFlags {
        write: true,
        truncate: true,
        ..Default::default()
    };
    let (handle, info) = p.open("/ot.txt", flags).unwrap();
    assert_eq!(info.size, 0);
    p.write(handle.id(), 0, b"new").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/ot.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"new");
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Nested directories
// ---------------------------------------------------------------------------

#[test]
fn tikv_nested_directories() {
    let p = fresh_tikv_provider("nested_dirs");

    let (h, _) = p.open("/a", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();
    let (h, _) = p.open("/a/b", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();
    let (h, _) = p.open("/a/b/c", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let (handle, _) = p.open("/a/b/c/deep.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"deep file").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/a/b/c/deep.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"deep file");
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Readdir ordering and multiple entries
// ---------------------------------------------------------------------------

#[test]
fn tikv_readdir_sorted_multiple_entries() {
    let p = fresh_tikv_provider("readdir_sort");

    for name in ["c.txt", "a.txt", "b.txt"] {
        let path = format!("/{name}");
        let (handle, _) = p.open(&path, OpenFlags::create_file()).unwrap();
        p.close(handle.id()).unwrap();
    }

    let entries = p.readdir("/").unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].path, "/a.txt");
    assert_eq!(entries[1].path, "/b.txt");
    assert_eq!(entries[2].path, "/c.txt");
}

#[test]
fn tikv_readdir_mixed_files_and_dirs() {
    let p = fresh_tikv_provider("readdir_mixed");

    let (h, _) = p.open("/dir1", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();
    let (h, _) = p.open("/file1.txt", OpenFlags::create_file()).unwrap();
    p.close(h.id()).unwrap();
    let (h, _) = p.open("/dir2", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let entries = p.readdir("/").unwrap();
    assert_eq!(entries.len(), 3);

    let dirs: Vec<_> = entries
        .iter()
        .filter(|e| e.file_type == FileType::Directory)
        .collect();
    let files: Vec<_> = entries
        .iter()
        .filter(|e| e.file_type == FileType::Regular)
        .collect();
    assert_eq!(dirs.len(), 2);
    assert_eq!(files.len(), 1);
}

// ---------------------------------------------------------------------------
// Remove edge cases
// ---------------------------------------------------------------------------

#[test]
fn tikv_cannot_remove_nonempty_dir() {
    let p = fresh_tikv_provider("rm_nonempty");

    let (h, _) = p.open("/parent", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let (handle, _) = p
        .open("/parent/child.txt", OpenFlags::create_file())
        .unwrap();
    p.close(handle.id()).unwrap();

    assert!(matches!(
        p.remove("/parent"),
        Err(FsError::DirectoryNotEmpty(_))
    ));

    // Clean up child first, then dir
    p.remove("/parent/child.txt").unwrap();
    p.remove("/parent").unwrap();
    assert!(matches!(p.stat("/parent"), Err(FsError::NotFound(_))));
}

#[test]
fn tikv_cannot_remove_root() {
    let p = fresh_tikv_provider("rm_root");
    assert!(p.remove("/").is_err());
}

#[test]
fn tikv_remove_empty_directory() {
    let p = fresh_tikv_provider("rm_empty_dir");

    let (h, _) = p.open("/emptydir", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    p.remove("/emptydir").unwrap();
    assert!(matches!(p.stat("/emptydir"), Err(FsError::NotFound(_))));
}

// ---------------------------------------------------------------------------
// Rename / wstat
// ---------------------------------------------------------------------------

#[test]
fn tikv_rename_file_same_dir() {
    let p = fresh_tikv_provider("rename_same");

    let (handle, _) = p.open("/old.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"content").unwrap();
    p.close(handle.id()).unwrap();

    p.wstat("/old.txt", &StatChanges::rename("new.txt"))
        .unwrap();

    assert!(p.stat("/old.txt").is_err());
    let info = p.stat("/new.txt").unwrap();
    assert_eq!(info.size, 7);
}

#[test]
fn tikv_rename_file_cross_dir() {
    let p = fresh_tikv_provider("rename_cross");

    let (h, _) = p.open("/subdir", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let (handle, _) = p.open("/file.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"data").unwrap();
    p.close(handle.id()).unwrap();

    p.wstat("/file.txt", &StatChanges::rename("/subdir/moved.txt"))
        .unwrap();

    assert!(p.stat("/file.txt").is_err());
    let info = p.stat("/subdir/moved.txt").unwrap();
    assert_eq!(info.size, 4);
}

#[test]
fn tikv_rename_replaces_existing_file() {
    let p = fresh_tikv_provider("rename_replace");

    let (h1, _) = p.open("/src.txt", OpenFlags::create_file()).unwrap();
    p.write(h1.id(), 0, b"source").unwrap();
    p.close(h1.id()).unwrap();

    let (h2, _) = p.open("/dst.txt", OpenFlags::create_file()).unwrap();
    p.write(h2.id(), 0, b"old content").unwrap();
    p.close(h2.id()).unwrap();

    p.wstat("/src.txt", &StatChanges::rename("dst.txt"))
        .unwrap();

    assert!(p.stat("/src.txt").is_err());
    let info = p.stat("/dst.txt").unwrap();
    assert_eq!(info.size, 6);

    let (handle, _) = p.open("/dst.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"source");
    p.close(handle.id()).unwrap();
}

#[test]
fn tikv_rename_file_to_dir_fails() {
    let p = fresh_tikv_provider("rename_f2d");

    let (handle, _) = p.open("/file.txt", OpenFlags::create_file()).unwrap();
    p.close(handle.id()).unwrap();

    let (h, _) = p.open("/dir", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let result = p.wstat("/file.txt", &StatChanges::rename("dir"));
    assert!(matches!(result, Err(FsError::IsDirectory(_))));
}

#[test]
fn tikv_rename_dir_to_file_fails() {
    let p = fresh_tikv_provider("rename_d2f");

    let (h, _) = p.open("/dir", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let (handle, _) = p.open("/file", OpenFlags::create_file()).unwrap();
    p.close(handle.id()).unwrap();

    let result = p.wstat("/dir", &StatChanges::rename("file"));
    assert!(matches!(result, Err(FsError::NotDirectory(_))));
}

#[test]
fn tikv_rename_dir_to_nonempty_dir_fails() {
    let p = fresh_tikv_provider("rename_d2ned");

    let (h, _) = p.open("/src", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();
    let (h, _) = p.open("/dst", OpenFlags::create_dir()).unwrap();
    p.close(h.id()).unwrap();

    let (handle, _) = p.open("/dst/child.txt", OpenFlags::create_file()).unwrap();
    p.close(handle.id()).unwrap();

    let result = p.wstat("/src", &StatChanges::rename("dst"));
    assert!(matches!(result, Err(FsError::DirectoryNotEmpty(_))));
}

// ---------------------------------------------------------------------------
// chmod via wstat
// ---------------------------------------------------------------------------

#[test]
fn tikv_chmod() {
    let p = fresh_tikv_provider("chmod");

    let (handle, _) = p.open("/ch.txt", OpenFlags::create_file()).unwrap();
    p.close(handle.id()).unwrap();

    let info = p.stat("/ch.txt").unwrap();
    assert_eq!(info.mode, 0o644);

    p.wstat(
        "/ch.txt",
        &StatChanges {
            mode: Some(0o755),
            ..Default::default()
        },
    )
    .unwrap();

    let info = p.stat("/ch.txt").unwrap();
    assert_eq!(info.mode, 0o755);
}

// ---------------------------------------------------------------------------
// uid/gid configuration
// ---------------------------------------------------------------------------

#[test]
fn tikv_configurable_uid_gid() {
    let p = fresh_tikv_provider_with_uid("uidgid", 1000, 1001);

    let info = p.stat("/").unwrap();
    assert_eq!(info.uid, 1000);
    assert_eq!(info.gid, 1001);

    let (handle, _) = p.open("/file.txt", OpenFlags::create_file()).unwrap();
    p.close(handle.id()).unwrap();

    let entries = p.readdir("/").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].uid, 1000);
    assert_eq!(entries[0].gid, 1001);
}

// ---------------------------------------------------------------------------
// Handle semantics
// ---------------------------------------------------------------------------

#[test]
fn tikv_close_invalid_handle_fails() {
    let p = fresh_tikv_provider("bad_handle");
    assert!(matches!(p.close(99999), Err(FsError::InvalidHandle(_))));
}

#[test]
fn tikv_read_invalid_handle_fails() {
    let p = fresh_tikv_provider("bad_read");
    assert!(matches!(
        p.read(99999, 0, 100),
        Err(FsError::InvalidHandle(_))
    ));
}

#[test]
fn tikv_write_invalid_handle_fails() {
    let p = fresh_tikv_provider("bad_write");
    assert!(matches!(
        p.write(99999, 0, b"data"),
        Err(FsError::InvalidHandle(_))
    ));
}

// ---------------------------------------------------------------------------
// Overwrite (write at offset within existing data)
// ---------------------------------------------------------------------------

#[test]
fn tikv_overwrite_in_place() {
    let p = fresh_tikv_provider("overwrite");

    let (handle, _) = p.open("/ow.txt", OpenFlags::create_file()).unwrap();
    p.write(handle.id(), 0, b"AAAAAAAAAA").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p
        .open(
            "/ow.txt",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    p.write(handle.id(), 3, b"BBB").unwrap();
    p.close(handle.id()).unwrap();

    let (handle, _) = p.open("/ow.txt", OpenFlags::read()).unwrap();
    let data = p.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"AAABBBAAAA");
    p.close(handle.id()).unwrap();
}

// ---------------------------------------------------------------------------
// Multiple files in same directory
// ---------------------------------------------------------------------------

#[test]
fn tikv_multiple_files_independent() {
    let p = fresh_tikv_provider("multi_files");

    for i in 0..5 {
        let path = format!("/file_{i}.txt");
        let content = format!("content of file {i}");
        let (handle, _) = p.open(&path, OpenFlags::create_file()).unwrap();
        p.write(handle.id(), 0, content.as_bytes()).unwrap();
        p.close(handle.id()).unwrap();
    }

    for i in 0..5 {
        let path = format!("/file_{i}.txt");
        let expected = format!("content of file {i}");
        let (handle, _) = p.open(&path, OpenFlags::read()).unwrap();
        let data = p.read(handle.id(), 0, 200).unwrap();
        assert_eq!(&data[..], expected.as_bytes());
        p.close(handle.id()).unwrap();
    }

    let entries = p.readdir("/").unwrap();
    assert_eq!(entries.len(), 5);
}

// ---------------------------------------------------------------------------
// KvBackend direct operations (TiKV-specific)
// ---------------------------------------------------------------------------

#[test]
fn tikv_kv_backend_get_set_delete() {
    let ns = "fs9_test_kv_basic".to_string();
    let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns));

    backend.set(b"test_key_1", b"value_1");
    backend.set(b"test_key_2", b"value_2");

    assert_eq!(backend.get(b"test_key_1"), Some(b"value_1".to_vec()));
    assert_eq!(backend.get(b"test_key_2"), Some(b"value_2".to_vec()));
    assert_eq!(backend.get(b"test_key_missing"), None);

    backend.delete(b"test_key_1");
    assert_eq!(backend.get(b"test_key_1"), None);

    // Cleanup
    backend.delete(b"test_key_2");
}

#[test]
fn tikv_kv_backend_scan_prefix() {
    let ns = "fs9_test_kv_scan".to_string();
    let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns));

    // Clean first
    for prefix in [b"pfx:".as_slice(), b"other:"] {
        let keys: Vec<_> = backend.scan(prefix).into_iter().map(|(k, _)| k).collect();
        for key in keys {
            backend.delete(&key);
        }
    }

    backend.set(b"pfx:a", b"va");
    backend.set(b"pfx:b", b"vb");
    backend.set(b"pfx:c", b"vc");
    backend.set(b"other:x", b"vx");

    let results = backend.scan(b"pfx:");
    assert_eq!(results.len(), 3);
    for (k, _) in &results {
        assert!(k.starts_with(b"pfx:"));
    }

    let other = backend.scan(b"other:");
    assert_eq!(other.len(), 1);

    // Cleanup
    backend.delete(b"pfx:a");
    backend.delete(b"pfx:b");
    backend.delete(b"pfx:c");
    backend.delete(b"other:x");
}

// ---------------------------------------------------------------------------
// Persistence across provider instances (data survives reconnect)
// ---------------------------------------------------------------------------

#[test]
fn tikv_data_persists_across_reconnect() {
    let ns = "fs9_test_persist";

    // First connection: write data
    {
        let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns.to_string()));
        wipe_all_keys(&backend);
        let p = PageFsProvider::new(Box::new(backend));

        let (handle, _) = p.open("/persist.txt", OpenFlags::create_file()).unwrap();
        p.write(handle.id(), 0, b"persistent data").unwrap();
        p.close(handle.id()).unwrap();
    }

    // Second connection: read data back (no wipe!)
    {
        let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns.to_string()));
        let p = PageFsProvider::new(Box::new(backend));

        let info = p.stat("/persist.txt").unwrap();
        assert_eq!(info.size, 15);

        let (handle, _) = p.open("/persist.txt", OpenFlags::read()).unwrap();
        let data = p.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"persistent data");
        p.close(handle.id()).unwrap();
    }

    // Cleanup
    {
        let backend = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns.to_string()));
        wipe_all_keys(&backend);
    }
}

// ---------------------------------------------------------------------------
// Keyspace isolation (different namespaces see different data)
// ---------------------------------------------------------------------------

#[test]
fn tikv_keyspace_isolation() {
    let ns_a = "fs9_test_iso_a";
    let ns_b = "fs9_test_iso_b";

    // Setup: wipe both
    let backend_a = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns_a.to_string()));
    wipe_all_keys(&backend_a);
    let backend_b = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns_b.to_string()));
    wipe_all_keys(&backend_b);

    let pa = PageFsProvider::new(Box::new(backend_a));
    let pb = PageFsProvider::new(Box::new(backend_b));

    // Write in ns_a
    let (h, _) = pa.open("/secret.txt", OpenFlags::create_file()).unwrap();
    pa.write(h.id(), 0, b"ns_a data").unwrap();
    pa.close(h.id()).unwrap();

    // Write different content in ns_b
    let (h, _) = pb.open("/secret.txt", OpenFlags::create_file()).unwrap();
    pb.write(h.id(), 0, b"ns_b data").unwrap();
    pb.close(h.id()).unwrap();

    // Read back — each sees their own
    let (h, _) = pa.open("/secret.txt", OpenFlags::read()).unwrap();
    let data_a = pa.read(h.id(), 0, 100).unwrap();
    assert_eq!(&data_a[..], b"ns_a data");
    pa.close(h.id()).unwrap();

    let (h, _) = pb.open("/secret.txt", OpenFlags::read()).unwrap();
    let data_b = pb.read(h.id(), 0, 100).unwrap();
    assert_eq!(&data_b[..], b"ns_b data");
    pb.close(h.id()).unwrap();

    // Cleanup
    let backend_a = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns_a.to_string()));
    wipe_all_keys(&backend_a);
    let backend_b = TikvKvBackend::new(vec!["127.0.0.1:2379".to_string()], Some(ns_b.to_string()));
    wipe_all_keys(&backend_b);
}
