mod common;

use common::{get_server_url, MountedFs};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

#[test]
#[ignore]
fn test_fuse_write_read() {
    let mountpoint = "/tmp/fs9-fuse-test-wr";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/test_write_read.txt", mountpoint);
    let content = b"Hello from FUSE test!";

    {
        let mut file = fs::File::create(&test_file).expect("Failed to create file");
        file.write_all(content).expect("Failed to write");
    }

    {
        let mut file = fs::File::open(&test_file).expect("Failed to open file");
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).expect("Failed to read");
        assert_eq!(&buf[..], content);
    }

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_mkdir_readdir() {
    let mountpoint = "/tmp/fs9-fuse-test-dir";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_dir = format!("{}/testdir", mountpoint);
    fs::create_dir(&test_dir).expect("Failed to create directory");

    let file1 = format!("{}/file1.txt", test_dir);
    let file2 = format!("{}/file2.txt", test_dir);
    fs::write(&file1, b"content1").expect("Failed to write file1");
    fs::write(&file2, b"content2").expect("Failed to write file2");

    let entries: Vec<_> = fs::read_dir(&test_dir)
        .expect("Failed to read directory")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 2);

    fs::remove_file(&file1).expect("Failed to remove file1");
    fs::remove_file(&file2).expect("Failed to remove file2");
    fs::remove_dir(&test_dir).expect("Failed to remove directory");
}

#[test]
#[ignore]
fn test_fuse_rename() {
    let mountpoint = "/tmp/fs9-fuse-test-rename";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let old_path = format!("{}/old_name.txt", mountpoint);
    let new_path = format!("{}/new_name.txt", mountpoint);

    fs::write(&old_path, b"rename test").expect("Failed to write");
    fs::rename(&old_path, &new_path).expect("Failed to rename");

    assert!(!Path::new(&old_path).exists());
    assert!(Path::new(&new_path).exists());

    let content = fs::read(&new_path).expect("Failed to read renamed file");
    assert_eq!(&content[..], b"rename test");

    fs::remove_file(&new_path).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_truncate() {
    let mountpoint = "/tmp/fs9-fuse-test-truncate";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/truncate_test.txt", mountpoint);
    fs::write(&test_file, b"hello world").expect("Failed to write");

    {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&test_file)
            .expect("Failed to open");
        file.set_len(5).expect("Failed to truncate");
    }

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"hello");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_nested_directories() {
    let mountpoint = "/tmp/fs9-fuse-test-nested";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let deep_path = format!("{}/a/b/c/d/e", mountpoint);
    fs::create_dir_all(&deep_path).expect("Failed to create nested dirs");

    let file_path = format!("{}/deep.txt", deep_path);
    fs::write(&file_path, b"deeply nested").expect("Failed to write");

    let content = fs::read(&file_path).expect("Failed to read");
    assert_eq!(&content[..], b"deeply nested");

    fs::remove_file(&file_path).expect("Failed to remove file");
    fs::remove_dir_all(&format!("{}/a", mountpoint)).expect("Failed to cleanup");
}

#[test]
#[ignore]
fn test_fuse_large_file() {
    let mountpoint = "/tmp/fs9-fuse-test-large";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/large.bin", mountpoint);

    let data: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();

    fs::write(&test_file, &data).expect("Failed to write large file");

    let read_data = fs::read(&test_file).expect("Failed to read large file");
    assert_eq!(read_data.len(), data.len());
    assert_eq!(read_data, data);

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_append() {
    let mountpoint = "/tmp/fs9-fuse-test-append";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/append.txt", mountpoint);

    fs::write(&test_file, b"line1\n").expect("Failed to write");

    {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&test_file)
            .expect("Failed to open for append");
        file.write_all(b"line2\n").expect("Failed to append");
        file.write_all(b"line3\n").expect("Failed to append");
    }

    let content = fs::read_to_string(&test_file).expect("Failed to read");
    assert_eq!(content, "line1\nline2\nline3\n");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_many_files() {
    let mountpoint = "/tmp/fs9-fuse-test-many";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let dir = format!("{}/manyfiles", mountpoint);
    fs::create_dir(&dir).expect("Failed to create dir");

    for i in 0..100 {
        let path = format!("{}/file{}.txt", dir, i);
        fs::write(&path, format!("content {}", i)).expect("Failed to write");
    }

    let entries: Vec<_> = fs::read_dir(&dir)
        .expect("Failed to read dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 100);

    for i in 0..100 {
        let path = format!("{}/file{}.txt", dir, i);
        let content = fs::read_to_string(&path).expect("Failed to read");
        assert_eq!(content, format!("content {}", i));
    }

    fs::remove_dir_all(&dir).expect("Failed to cleanup");
}

#[test]
#[ignore]
fn test_fuse_unicode_filename() {
    let mountpoint = "/tmp/fs9-fuse-test-unicode";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/unicode_file.txt", mountpoint);
    fs::write(&test_file, b"unicode content").expect("Failed to write");

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"unicode content");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_file_with_spaces() {
    let mountpoint = "/tmp/fs9-fuse-test-spaces";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/file with spaces.txt", mountpoint);
    fs::write(&test_file, b"spaces content").expect("Failed to write");

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"spaces content");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_move_between_dirs() {
    let mountpoint = "/tmp/fs9-fuse-test-move";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let src_dir = format!("{}/src", mountpoint);
    let dst_dir = format!("{}/dst", mountpoint);
    fs::create_dir(&src_dir).expect("Failed to create src");
    fs::create_dir(&dst_dir).expect("Failed to create dst");

    let src_file = format!("{}/file.txt", src_dir);
    let dst_file = format!("{}/file.txt", dst_dir);

    fs::write(&src_file, b"moving").expect("Failed to write");
    fs::rename(&src_file, &dst_file).expect("Failed to move");

    assert!(!Path::new(&src_file).exists());
    assert!(Path::new(&dst_file).exists());

    let content = fs::read(&dst_file).expect("Failed to read");
    assert_eq!(&content[..], b"moving");

    fs::remove_file(&dst_file).unwrap();
    fs::remove_dir(&src_dir).unwrap();
    fs::remove_dir(&dst_dir).unwrap();
}

#[test]
#[ignore]
fn test_fuse_chmod() {
    let mountpoint = "/tmp/fs9-fuse-test-chmod";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/chmod.txt", mountpoint);
    fs::write(&test_file, b"chmod test").expect("Failed to write");

    use std::os::unix::fs::PermissionsExt;

    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(&test_file, perms).expect("Failed to chmod");

    let meta = fs::metadata(&test_file).expect("Failed to stat");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_error_nonexistent() {
    let mountpoint = "/tmp/fs9-fuse-test-err";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let result = fs::read(&format!("{}/nonexistent.txt", mountpoint));
    assert!(result.is_err());
}

#[test]
#[ignore]
fn test_fuse_error_rmdir_nonempty() {
    let mountpoint = "/tmp/fs9-fuse-test-err2";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let dir = format!("{}/notempty", mountpoint);
    fs::create_dir(&dir).expect("Failed to create dir");
    fs::write(&format!("{}/file.txt", dir), b"content").expect("Failed to write");

    let result = fs::remove_dir(&dir);
    assert!(result.is_err());

    fs::remove_file(&format!("{}/file.txt", dir)).unwrap();
    fs::remove_dir(&dir).unwrap();
}

#[test]
#[ignore]
fn test_fuse_binary_with_nulls() {
    let mountpoint = "/tmp/fs9-fuse-test-bin";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/binary.bin", mountpoint);
    let data: Vec<u8> = vec![0x00, 0x01, 0x02, 0x00, 0xFF, 0x00, 0xFE];

    fs::write(&test_file, &data).expect("Failed to write");

    let read_data = fs::read(&test_file).expect("Failed to read");
    assert_eq!(read_data, data);

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_hidden_file() {
    let mountpoint = "/tmp/fs9-fuse-test-hidden";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/.hidden", mountpoint);
    fs::write(&test_file, b"hidden content").expect("Failed to write");

    assert!(Path::new(&test_file).exists());

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"hidden content");

    fs::remove_file(&test_file).expect("Failed to remove file");
}

#[test]
#[ignore]
fn test_fuse_overwrite_larger_with_smaller() {
    let mountpoint = "/tmp/fs9-fuse-test-overwrite";
    let _mount = MountedFs::mount(&get_server_url(), mountpoint).expect("Failed to mount");

    let test_file = format!("{}/overwrite.txt", mountpoint);

    fs::write(&test_file, b"this is a long content string").expect("Failed to write");
    fs::write(&test_file, b"short").expect("Failed to overwrite");

    let content = fs::read(&test_file).expect("Failed to read");
    assert_eq!(&content[..], b"short");

    fs::remove_file(&test_file).expect("Failed to remove file");
}
