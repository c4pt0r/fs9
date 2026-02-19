use fs9_client::{Fs9Client, Fs9Error, OpenFlags};
use fs9_tests::{generate_test_path, get_server_url};

#[tokio::test]
async fn health_check() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    assert!(client.health().await.unwrap());
}

#[tokio::test]
async fn write_and_read_file() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("write_read");
    let content = b"Hello, FS9 World!";

    client.write_file(&path, content).await.unwrap();

    let data = client.read_file(&path).await.unwrap();
    assert_eq!(&data[..], content);

    client.remove(&path).await.unwrap();
}

#[tokio::test]
async fn file_stat() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("stat");
    let content = b"test content for stat";

    client.write_file(&path, content).await.unwrap();

    let info = client.stat(&path).await.unwrap();
    assert_eq!(info.size, content.len() as u64);
    assert!(info.is_file());
    assert!(!info.is_dir());

    client.remove(&path).await.unwrap();
}

#[tokio::test]
async fn directory_operations() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let dir_path = generate_test_path("dir");
    let file_path = format!("{}/file.txt", dir_path);

    client.mkdir(&dir_path).await.unwrap();
    assert!(client.is_dir(&dir_path).await.unwrap());

    client.write_file(&file_path, b"content").await.unwrap();

    let entries = client.readdir(&dir_path).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].path.ends_with("file.txt"));

    client.remove(&file_path).await.unwrap();
    client.remove(&dir_path).await.unwrap();
}

#[tokio::test]
async fn file_handle_operations() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("handle");

    let handle = client
        .open(&path, OpenFlags::create_truncate())
        .await
        .unwrap();

    client.write(&handle, 0, b"first").await.unwrap();
    client.write(&handle, 5, b" second").await.unwrap();

    client.close(handle).await.unwrap();

    let handle = client.open(&path, OpenFlags::read()).await.unwrap();
    let data = client.read(&handle, 0, 100).await.unwrap();
    assert_eq!(&data[..], b"first second");

    let partial = client.read(&handle, 6, 6).await.unwrap();
    assert_eq!(&partial[..], b"second");

    client.close(handle).await.unwrap();

    client.remove(&path).await.unwrap();
}

#[tokio::test]
async fn chmod_operation() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("chmod");

    client.write_file(&path, b"test").await.unwrap();

    client.chmod(&path, 0o755).await.unwrap();

    let info = client.stat(&path).await.unwrap();
    assert_eq!(info.mode & 0o777, 0o755);

    client.remove(&path).await.unwrap();
}

#[tokio::test]
async fn truncate_operation() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("truncate");

    client.write_file(&path, b"hello world").await.unwrap();

    client.truncate(&path, 5).await.unwrap();

    let info = client.stat(&path).await.unwrap();
    assert_eq!(info.size, 5);

    let data = client.read_file(&path).await.unwrap();
    assert_eq!(&data[..], b"hello");

    client.remove(&path).await.unwrap();
}

#[tokio::test]
async fn exists_check() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("exists");

    assert!(!client.exists(&path).await.unwrap());

    client.write_file(&path, b"x").await.unwrap();
    assert!(client.exists(&path).await.unwrap());

    client.remove(&path).await.unwrap();
    assert!(!client.exists(&path).await.unwrap());
}

#[tokio::test]
async fn not_found_error() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = "/nonexistent_file_12345.txt";

    let result = client.stat(path).await;
    assert!(matches!(result, Err(Fs9Error::NotFound(_))));
}

#[tokio::test]
async fn list_mounts() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();

    let mounts = client.list_mounts().await.unwrap();
    assert!(!mounts.is_empty());

    let root = mounts.iter().find(|m| m.path == "/");
    assert!(root.is_some());
}

#[tokio::test]
async fn capabilities() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();

    let caps = client.capabilities("/").await.unwrap();
    assert!(caps.can_read());
    assert!(caps.can_write());
}

#[tokio::test]
async fn statfs() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();

    let stats = client.statfs("/").await.unwrap();
    assert!(stats.total_bytes > 0);
    assert!(stats.block_size > 0);
}

#[tokio::test]
async fn large_file_write_read() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("large");

    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

    client.write_file(&path, &data).await.unwrap();

    let read_data = client.read_file(&path).await.unwrap();
    assert_eq!(read_data.len(), data.len());
    assert_eq!(&read_data[..], &data[..]);

    client.remove(&path).await.unwrap();
}

#[tokio::test]
async fn nested_directories() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let base = generate_test_path("nested");
    let level1 = format!("{}/a", base);
    let level2 = format!("{}/b", level1);
    let file_path = format!("{}/file.txt", level2);

    client.mkdir(&base).await.unwrap();
    client.mkdir(&level1).await.unwrap();
    client.mkdir(&level2).await.unwrap();
    client.write_file(&file_path, b"deep").await.unwrap();

    assert!(client.is_file(&file_path).await.unwrap());
    assert!(client.is_dir(&level2).await.unwrap());

    let data = client.read_file(&file_path).await.unwrap();
    assert_eq!(&data[..], b"deep");

    client.remove(&file_path).await.unwrap();
    client.remove(&level2).await.unwrap();
    client.remove(&level1).await.unwrap();
    client.remove(&base).await.unwrap();
}

#[tokio::test]
async fn append_mode() {
    let url = get_server_url().await;
    let client = Fs9Client::new(&url).unwrap();
    let path = generate_test_path("append");

    client.write_file(&path, b"start").await.unwrap();

    let handle = client.open(&path, OpenFlags::append()).await.unwrap();
    client.write(&handle, 0, b"_appended").await.unwrap();
    client.close(handle).await.unwrap();

    let data = client.read_file(&path).await.unwrap();
    assert_eq!(&data[..], b"start_appended");

    client.remove(&path).await.unwrap();
}
