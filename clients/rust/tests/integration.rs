use fs9_client::{Fs9Client, OpenFlags, StatChanges};

fn get_test_url() -> String {
    std::env::var("FS9_TEST_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

#[tokio::test]
#[ignore = "requires running server"]
async fn health_check() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    assert!(client.health().await.unwrap());
}

#[tokio::test]
#[ignore = "requires running server"]
async fn create_read_delete_file() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let test_path = "/integration_test_file.txt";
    let test_data = b"Hello from integration test!";

    client.write_file(test_path, test_data).await.unwrap();

    let data = client.read_file(test_path).await.unwrap();
    assert_eq!(&data[..], test_data);

    let info = client.stat(test_path).await.unwrap();
    assert_eq!(info.size, test_data.len() as u64);
    assert!(info.is_file());

    client.remove(test_path).await.unwrap();

    assert!(!client.exists(test_path).await.unwrap());
}

#[tokio::test]
#[ignore = "requires running server"]
async fn create_and_list_directory() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let dir_path = "/integration_test_dir";
    let file_path = "/integration_test_dir/file.txt";

    if client.exists(dir_path).await.unwrap() {
        if client.exists(file_path).await.unwrap() {
            client.remove(file_path).await.unwrap();
        }
        client.remove(dir_path).await.unwrap();
    }

    client.mkdir(dir_path).await.unwrap();
    assert!(client.is_dir(dir_path).await.unwrap());

    client.write_file(file_path, b"test content").await.unwrap();

    let entries = client.readdir(dir_path).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].path.ends_with("file.txt"));

    client.remove(file_path).await.unwrap();
    client.remove(dir_path).await.unwrap();
}

#[tokio::test]
#[ignore = "requires running server"]
async fn file_handle_operations() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let test_path = "/handle_test.bin";

    let handle = client.open(test_path, OpenFlags::create_truncate()).await.unwrap();
    
    let written = client.write(&handle, 0, b"first chunk").await.unwrap();
    assert_eq!(written, 11);
    
    let written = client.write(&handle, 11, b" second chunk").await.unwrap();
    assert_eq!(written, 13);
    
    client.close(handle).await.unwrap();

    let handle = client.open(test_path, OpenFlags::read()).await.unwrap();
    let data = client.read(&handle, 0, 100).await.unwrap();
    assert_eq!(&data[..], b"first chunk second chunk");
    
    let partial = client.read(&handle, 6, 5).await.unwrap();
    assert_eq!(&partial[..], b"chunk");
    
    client.close(handle).await.unwrap();

    client.remove(test_path).await.unwrap();
}

#[tokio::test]
#[ignore = "requires running server"]
async fn wstat_operations() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let test_path = "/wstat_test.txt";

    client.write_file(test_path, b"some content here").await.unwrap();

    client.chmod(test_path, 0o755).await.unwrap();
    let info = client.stat(test_path).await.unwrap();
    assert_eq!(info.mode & 0o777, 0o755);

    client.truncate(test_path, 4).await.unwrap();
    let info = client.stat(test_path).await.unwrap();
    assert_eq!(info.size, 4);

    let data = client.read_file(test_path).await.unwrap();
    assert_eq!(&data[..], b"some");

    client.remove(test_path).await.unwrap();
}

#[tokio::test]
#[ignore = "requires running server"]
async fn list_mounts() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let mounts = client.list_mounts().await.unwrap();
    assert!(!mounts.is_empty());
    
    let root_mount = mounts.iter().find(|m| m.path == "/");
    assert!(root_mount.is_some());
}

#[tokio::test]
#[ignore = "requires running server"]
async fn capabilities() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let caps = client.capabilities("/").await.unwrap();
    
    assert!(caps.can_read());
    assert!(!caps.provider_type.is_empty());
}

#[tokio::test]
#[ignore = "requires running server"]
async fn statfs() {
    let client = Fs9Client::new(&get_test_url()).unwrap();
    let stats = client.statfs("/").await.unwrap();
    
    assert!(stats.total_bytes > 0);
    assert!(stats.block_size > 0);
}
