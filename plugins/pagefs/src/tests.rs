use super::*;
use crate::provider::PageFsProvider;
use fs9_sdk::{FileType, FsError, OpenFlags, StatChanges};
use fs9_sdk_ffi::FS9_SDK_VERSION;

trait PipeExt: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}

impl<T> PipeExt for T {}

fn create_provider() -> PageFsProvider {
    PageFsProvider::with_memory_backend()
}

#[test]
fn version_matches_sdk() {
    assert_eq!(ffi::fs9_plugin_version(), FS9_SDK_VERSION);
}

#[test]
fn vtable_not_null() {
    let vtable = ffi::fs9_plugin_vtable();
    assert!(!vtable.is_null());
    unsafe {
        assert_eq!((*vtable).sdk_version, FS9_SDK_VERSION);
    }
}

#[test]
fn root_exists() {
    let provider = create_provider();
    let info = provider.stat("/").unwrap();
    assert_eq!(info.file_type, FileType::Directory);
}

#[test]
fn create_file_allocates_one_page() {
    let provider = create_provider();

    let handle = provider
        .open("/test.txt", OpenFlags::create_file())
        .unwrap();
    provider.close(handle.id()).unwrap();

    let inode = provider.load_inode(2).unwrap();
    assert_eq!(inode.page_count, 1);

    let page = provider.read_page(2, 0).unwrap();
    assert_eq!(page.len(), PAGE_SIZE);
}

#[test]
fn create_and_read_file() {
    let provider = create_provider();

    let handle = provider
        .open("/test.txt", OpenFlags::create_file())
        .unwrap();
    provider.write(handle.id(), 0, b"hello pagefs").unwrap();
    provider.close(handle.id()).unwrap();

    let handle = provider.open("/test.txt", OpenFlags::read()).unwrap();
    let data = provider.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"hello pagefs");
    provider.close(handle.id()).unwrap();
}

#[test]
fn write_across_page_boundary() {
    let provider = create_provider();

    let handle = provider
        .open("/cross.txt", OpenFlags::create_file())
        .unwrap();

    let data: Vec<u8> = (0..PAGE_SIZE + 1000).map(|i| (i % 256) as u8).collect();
    provider.write(handle.id(), 0, &data).unwrap();
    provider.close(handle.id()).unwrap();

    let inode = provider.resolve_path("/cross.txt").unwrap().1;
    assert_eq!(inode.page_count, 2);

    let handle = provider.open("/cross.txt", OpenFlags::read()).unwrap();
    let read_data = provider.read(handle.id(), 0, data.len()).unwrap();
    assert_eq!(&read_data[..], &data[..]);
    provider.close(handle.id()).unwrap();
}

#[test]
fn read_partial_page() {
    let provider = create_provider();

    let handle = provider
        .open("/partial.txt", OpenFlags::create_file())
        .unwrap();
    let data = b"0123456789ABCDEF0123456789";
    provider.write(handle.id(), 0, data).unwrap();
    provider.close(handle.id()).unwrap();

    let handle = provider.open("/partial.txt", OpenFlags::read()).unwrap();
    let result = provider.read(handle.id(), 10, 10).unwrap();
    assert_eq!(&result[..], b"ABCDEF0123");
    provider.close(handle.id()).unwrap();
}

#[test]
fn create_directory() {
    let provider = create_provider();

    let handle = provider.open("/mydir", OpenFlags::create_dir()).unwrap();
    provider.close(handle.id()).unwrap();

    let info = provider.stat("/mydir").unwrap();
    assert_eq!(info.file_type, FileType::Directory);
}

#[test]
fn nested_directories() {
    let provider = create_provider();

    provider
        .open("/a", OpenFlags::create_dir())
        .unwrap()
        .id()
        .pipe(|h| provider.close(h).unwrap());
    provider
        .open("/a/b", OpenFlags::create_dir())
        .unwrap()
        .id()
        .pipe(|h| provider.close(h).unwrap());
    provider
        .open("/a/b/c", OpenFlags::create_dir())
        .unwrap()
        .id()
        .pipe(|h| provider.close(h).unwrap());

    let handle = provider
        .open("/a/b/c/file.txt", OpenFlags::create_file())
        .unwrap();
    provider.write(handle.id(), 0, b"deep file").unwrap();
    provider.close(handle.id()).unwrap();

    let handle = provider.open("/a/b/c/file.txt", OpenFlags::read()).unwrap();
    let data = provider.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"deep file");
}

#[test]
fn readdir_lists_children() {
    let provider = create_provider();

    for name in ["c.txt", "a.txt", "b.txt"] {
        let path = format!("/{}", name);
        let handle = provider.open(&path, OpenFlags::create_file()).unwrap();
        provider.close(handle.id()).unwrap();
    }

    let entries = provider.readdir("/").unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].path, "/a.txt");
    assert_eq!(entries[1].path, "/b.txt");
    assert_eq!(entries[2].path, "/c.txt");
}

#[test]
fn remove_file_deletes_pages() {
    let provider = create_provider();

    let handle = provider
        .open("/todelete.txt", OpenFlags::create_file())
        .unwrap();
    provider.write(handle.id(), 0, b"will be deleted").unwrap();
    provider.close(handle.id()).unwrap();

    let inode_id = provider.resolve_path("/todelete.txt").unwrap().0;
    assert!(provider.read_page(inode_id, 0).is_some());

    provider.remove("/todelete.txt").unwrap();

    assert!(provider.read_page(inode_id, 0).is_none());
    assert!(matches!(
        provider.stat("/todelete.txt"),
        Err(FsError::NotFound(_))
    ));
}

#[test]
fn cannot_remove_non_empty_dir() {
    let provider = create_provider();

    provider
        .open("/parent", OpenFlags::create_dir())
        .unwrap()
        .id()
        .pipe(|h| provider.close(h).unwrap());

    let handle = provider
        .open("/parent/child.txt", OpenFlags::create_file())
        .unwrap();
    provider.close(handle.id()).unwrap();

    assert!(matches!(
        provider.remove("/parent"),
        Err(FsError::DirectoryNotEmpty(_))
    ));

    provider.remove("/parent/child.txt").unwrap();
    provider.remove("/parent").unwrap();
}

#[test]
fn truncate_file() {
    let provider = create_provider();

    let handle = provider
        .open("/trunc.txt", OpenFlags::create_file())
        .unwrap();
    provider
        .write(handle.id(), 0, b"long content here that will be truncated")
        .unwrap();
    provider.close(handle.id()).unwrap();

    provider
        .wstat("/trunc.txt", &StatChanges::truncate(10))
        .unwrap();

    let info = provider.stat("/trunc.txt").unwrap();
    assert_eq!(info.size, 10);

    let handle = provider.open("/trunc.txt", OpenFlags::read()).unwrap();
    let data = provider.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"long conte");
}

#[test]
fn extend_file_via_wstat() {
    let provider = create_provider();

    let handle = provider
        .open("/extend.txt", OpenFlags::create_file())
        .unwrap();
    provider.write(handle.id(), 0, b"short").unwrap();
    provider.close(handle.id()).unwrap();

    provider
        .wstat("/extend.txt", &StatChanges::truncate(20))
        .unwrap();

    let info = provider.stat("/extend.txt").unwrap();
    assert_eq!(info.size, 20);
}

#[test]
fn append_mode() {
    let provider = create_provider();

    let handle = provider
        .open("/append.txt", OpenFlags::create_file())
        .unwrap();
    provider.write(handle.id(), 0, b"first").unwrap();
    provider.close(handle.id()).unwrap();

    let flags = OpenFlags {
        write: true,
        append: true,
        ..Default::default()
    };
    let handle = provider.open("/append.txt", flags).unwrap();
    provider.write(handle.id(), 0, b"second").unwrap();
    provider.close(handle.id()).unwrap();

    let handle = provider.open("/append.txt", OpenFlags::read()).unwrap();
    let data = provider.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"firstsecond");
}

#[test]
fn large_file_spanning_many_pages() {
    let provider = create_provider();

    let handle = provider
        .open("/large.bin", OpenFlags::create_file())
        .unwrap();

    let data: Vec<u8> = (0..(PAGE_SIZE * 3 + 5000))
        .map(|i| (i % 256) as u8)
        .collect();
    provider.write(handle.id(), 0, &data).unwrap();
    provider.close(handle.id()).unwrap();

    let info = provider.stat("/large.bin").unwrap();
    assert_eq!(info.size, data.len() as u64);

    let inode = provider.resolve_path("/large.bin").unwrap().1;
    assert_eq!(inode.page_count, 4);

    let handle = provider.open("/large.bin", OpenFlags::read()).unwrap();
    let read_data = provider.read(handle.id(), 0, data.len()).unwrap();
    assert_eq!(read_data.len(), data.len());
    assert_eq!(&read_data[..], &data[..]);
}

#[test]
fn sparse_write() {
    let provider = create_provider();

    let handle = provider
        .open("/sparse.txt", OpenFlags::create_file())
        .unwrap();
    provider
        .write(handle.id(), PAGE_SIZE as u64, b"sparse data")
        .unwrap();
    provider.close(handle.id()).unwrap();

    let info = provider.stat("/sparse.txt").unwrap();
    assert_eq!(info.size, PAGE_SIZE as u64 + 11);

    let inode = provider.resolve_path("/sparse.txt").unwrap().1;
    assert_eq!(inode.page_count, 2);

    let handle = provider.open("/sparse.txt", OpenFlags::read()).unwrap();
    let first_page = provider.read(handle.id(), 0, PAGE_SIZE).unwrap();
    assert!(first_page.iter().all(|&b| b == 0));

    let second_page = provider.read(handle.id(), PAGE_SIZE as u64, 11).unwrap();
    assert_eq!(&second_page[..], b"sparse data");
}

#[test]
fn kv_operations() {
    let kv = InMemoryKv::new();

    kv.set(b"key1", b"value1");
    kv.set(b"key2", b"value2");
    kv.set(b"other", b"other_value");

    assert_eq!(kv.get(b"key1"), Some(b"value1".to_vec()));
    assert_eq!(kv.get(b"missing"), None);

    let scanned = kv.scan(b"key");
    assert_eq!(scanned.len(), 2);

    kv.delete(b"key1");
    assert_eq!(kv.get(b"key1"), None);
}

#[test]
fn page_size_is_16kb() {
    assert_eq!(PAGE_SIZE, 16 * 1024);
}

#[test]
fn negative_timestamps_handled_correctly() {
    use std::time::{Duration, UNIX_EPOCH};

    let positive = timestamp_to_system_time(100);
    assert_eq!(positive, UNIX_EPOCH + Duration::from_secs(100));

    let zero = timestamp_to_system_time(0);
    assert_eq!(zero, UNIX_EPOCH);

    let negative = timestamp_to_system_time(-100);
    assert_eq!(negative, UNIX_EPOCH - Duration::from_secs(100));
}

#[test]
fn configurable_uid_gid() {
    let provider = PageFsProvider::with_config(Box::new(InMemoryKv::new()), 1000, 1001);

    let info = provider.stat("/").unwrap();
    assert_eq!(info.uid, 1000);
    assert_eq!(info.gid, 1001);

    let handle = provider
        .open("/file.txt", OpenFlags::create_file())
        .unwrap();
    provider.close(handle.id()).unwrap();

    let entries = provider.readdir("/").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].uid, 1000);
    assert_eq!(entries[0].gid, 1001);
}

#[test]
fn rename_file_same_dir() {
    let provider = PageFsProvider::with_memory_backend();

    let handle = provider.open("/old.txt", OpenFlags::create_file()).unwrap();
    provider.write(handle.id(), 0, b"content").unwrap();
    provider.close(handle.id()).unwrap();

    provider
        .wstat("/old.txt", &StatChanges::rename("new.txt"))
        .unwrap();

    assert!(provider.stat("/old.txt").is_err());
    let info = provider.stat("/new.txt").unwrap();
    assert_eq!(info.size, 7);
}

#[test]
fn rename_file_cross_dir() {
    let provider = PageFsProvider::with_memory_backend();

    provider.open("/subdir", OpenFlags::create_dir()).unwrap();
    let handle = provider
        .open("/file.txt", OpenFlags::create_file())
        .unwrap();
    provider.write(handle.id(), 0, b"data").unwrap();
    provider.close(handle.id()).unwrap();

    provider
        .wstat("/file.txt", &StatChanges::rename("/subdir/moved.txt"))
        .unwrap();

    assert!(provider.stat("/file.txt").is_err());
    let info = provider.stat("/subdir/moved.txt").unwrap();
    assert_eq!(info.size, 4);
}

#[test]
fn rename_replaces_existing_file() {
    let provider = PageFsProvider::with_memory_backend();

    let h1 = provider.open("/src.txt", OpenFlags::create_file()).unwrap();
    provider.write(h1.id(), 0, b"source").unwrap();
    provider.close(h1.id()).unwrap();

    let h2 = provider.open("/dst.txt", OpenFlags::create_file()).unwrap();
    provider.write(h2.id(), 0, b"old content").unwrap();
    provider.close(h2.id()).unwrap();

    provider
        .wstat("/src.txt", &StatChanges::rename("dst.txt"))
        .unwrap();

    assert!(provider.stat("/src.txt").is_err());
    let info = provider.stat("/dst.txt").unwrap();
    assert_eq!(info.size, 6);

    let handle = provider.open("/dst.txt", OpenFlags::read()).unwrap();
    let data = provider.read(handle.id(), 0, 100).unwrap();
    assert_eq!(&data[..], b"source");
}

#[test]
fn rename_file_to_dir_fails() {
    let provider = PageFsProvider::with_memory_backend();

    let handle = provider
        .open("/file.txt", OpenFlags::create_file())
        .unwrap();
    provider.close(handle.id()).unwrap();

    provider.open("/dir", OpenFlags::create_dir()).unwrap();

    let result = provider.wstat("/file.txt", &StatChanges::rename("dir"));
    assert!(matches!(result, Err(FsError::IsDirectory(_))));
}

#[test]
fn rename_dir_to_file_fails() {
    let provider = PageFsProvider::with_memory_backend();

    provider.open("/dir", OpenFlags::create_dir()).unwrap();

    let handle = provider.open("/file", OpenFlags::create_file()).unwrap();
    provider.close(handle.id()).unwrap();

    let result = provider.wstat("/dir", &StatChanges::rename("file"));
    assert!(matches!(result, Err(FsError::NotDirectory(_))));
}

#[test]
fn rename_dir_to_nonempty_dir_fails() {
    let provider = PageFsProvider::with_memory_backend();

    provider.open("/src", OpenFlags::create_dir()).unwrap();
    provider.open("/dst", OpenFlags::create_dir()).unwrap();

    let handle = provider
        .open("/dst/child.txt", OpenFlags::create_file())
        .unwrap();
    provider.close(handle.id()).unwrap();

    let result = provider.wstat("/src", &StatChanges::rename("dst"));
    assert!(matches!(result, Err(FsError::DirectoryNotEmpty(_))));
}

#[cfg(feature = "s3")]
mod s3_tests {
    use super::*;

    #[test]
    fn s3_make_key_without_prefix() {
        let backend = S3KvBackend {
            client: create_mock_client(),
            bucket: "test-bucket".to_string(),
            prefix: String::new(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
        };

        assert_eq!(backend.make_key(b"S"), "53");
        assert_eq!(
            backend.make_key(b"I\x00\x00\x00\x00\x00\x00\x00\x01"),
            "490000000000000001"
        );
        assert_eq!(backend.make_key(b"hello"), "68656c6c6f");
    }

    #[test]
    fn s3_make_key_with_prefix() {
        let backend = S3KvBackend {
            client: create_mock_client(),
            bucket: "test-bucket".to_string(),
            prefix: "pagefs".to_string(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
        };

        assert_eq!(backend.make_key(b"S"), "pagefs/53");
        assert_eq!(backend.make_key(b"hello"), "pagefs/68656c6c6f");
    }

    #[test]
    fn s3_parse_key_without_prefix() {
        let backend = S3KvBackend {
            client: create_mock_client(),
            bucket: "test-bucket".to_string(),
            prefix: String::new(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
        };

        assert_eq!(backend.parse_key("53"), Some(vec![0x53]));
        assert_eq!(backend.parse_key("68656c6c6f"), Some(b"hello".to_vec()));
    }

    #[test]
    fn s3_parse_key_with_prefix() {
        let backend = S3KvBackend {
            client: create_mock_client(),
            bucket: "test-bucket".to_string(),
            prefix: "pagefs".to_string(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
        };

        assert_eq!(backend.parse_key("pagefs/53"), Some(vec![0x53]));
        assert_eq!(
            backend.parse_key("pagefs/68656c6c6f"),
            Some(b"hello".to_vec())
        );
        assert_eq!(backend.parse_key("other/53"), None);
    }

    #[test]
    fn s3_key_roundtrip() {
        let backend = S3KvBackend {
            client: create_mock_client(),
            bucket: "test-bucket".to_string(),
            prefix: "test".to_string(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
        };

        let test_keys: Vec<&[u8]> = vec![
            b"S",
            b"I\x00\x00\x00\x00\x00\x00\x00\x01",
            b"D\x00\x00\x00\x00\x00\x00\x00\x01:file.txt",
            b"P\x00\x00\x00\x00\x00\x00\x00\x02:\x00\x00\x00\x00\x00\x00\x00\x00",
        ];

        for key in test_keys {
            let encoded = backend.make_key(key);
            let decoded = backend.parse_key(&encoded);
            assert_eq!(
                decoded,
                Some(key.to_vec()),
                "Roundtrip failed for {:?}",
                key
            );
        }
    }

    fn create_mock_client() -> aws_sdk_s3::Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }

    #[test]
    #[ignore]
    fn s3_integration_basic_operations() {
        let bucket = std::env::var("FS9_TEST_S3_BUCKET")
            .expect("FS9_TEST_S3_BUCKET env var required for S3 integration tests");
        let prefix = std::env::var("FS9_TEST_S3_PREFIX")
            .unwrap_or_else(|_| format!("pagefs-test-{}", std::process::id()));

        let backend = S3KvBackend::new(bucket, prefix);

        let test_key = b"test-key";
        let test_value = b"test-value-12345";

        backend.set(test_key, test_value);

        let retrieved = backend.get(test_key);
        assert_eq!(retrieved, Some(test_value.to_vec()));

        backend.delete(test_key);

        let after_delete = backend.get(test_key);
        assert_eq!(after_delete, None);
    }

    #[test]
    #[ignore]
    fn s3_integration_scan() {
        let bucket = std::env::var("FS9_TEST_S3_BUCKET")
            .expect("FS9_TEST_S3_BUCKET env var required for S3 integration tests");
        let prefix = std::env::var("FS9_TEST_S3_PREFIX")
            .unwrap_or_else(|_| format!("pagefs-test-{}", std::process::id()));

        let backend = S3KvBackend::new(bucket, prefix);

        backend.set(b"prefix:a", b"value-a");
        backend.set(b"prefix:b", b"value-b");
        backend.set(b"prefix:c", b"value-c");
        backend.set(b"other:x", b"value-x");

        let results = backend.scan(b"prefix:");
        assert_eq!(results.len(), 3);

        for (k, _) in &results {
            assert!(k.starts_with(b"prefix:"));
        }

        backend.delete(b"prefix:a");
        backend.delete(b"prefix:b");
        backend.delete(b"prefix:c");
        backend.delete(b"other:x");
    }

    #[test]
    #[ignore]
    fn s3_integration_full_pagefs() {
        let bucket = std::env::var("FS9_TEST_S3_BUCKET")
            .expect("FS9_TEST_S3_BUCKET env var required for S3 integration tests");
        let prefix = std::env::var("FS9_TEST_S3_PREFIX")
            .unwrap_or_else(|_| format!("pagefs-test-{}", std::process::id()));

        let backend = Box::new(S3KvBackend::new(bucket, prefix));
        let provider = PageFsProvider::new(backend);

        let info = provider.stat("/").unwrap();
        assert_eq!(info.file_type, FileType::Directory);

        let handle = provider
            .open("/test.txt", OpenFlags::create_file())
            .unwrap();
        provider.write(handle.id(), 0, b"Hello S3 PageFS!").unwrap();
        provider.close(handle.id()).unwrap();

        let handle = provider.open("/test.txt", OpenFlags::read()).unwrap();
        let data = provider.read(handle.id(), 0, 100).unwrap();
        assert_eq!(&data[..], b"Hello S3 PageFS!");
        provider.close(handle.id()).unwrap();

        provider.remove("/test.txt").unwrap();
        assert!(provider.stat("/test.txt").is_err());
    }
}
