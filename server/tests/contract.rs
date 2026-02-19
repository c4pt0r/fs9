//! Contract tests for FS9 HTTP API.
//!
//! These tests verify the core filesystem semantics are correctly implemented.
//! The test suite is designed to be reusable with different backends.

mod harness;

use harness::TestServer;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

/// Helper to generate unique test paths
fn test_path(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "/test_{}_{}",
        prefix,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, Deserialize)]
struct FileInfo {
    path: String,
    size: u64,
    mode: u32,
    is_dir: bool,
}

#[derive(Debug, Deserialize)]
struct OpenResponse {
    handle_id: String,
}

#[derive(Debug, Deserialize)]
struct WriteResponse {
    bytes_written: usize,
}

// ============================================================================
// Contract Test Suite
// ============================================================================

/// Core Contract #1: open(create) → write → close → stat(size) → read
#[tokio::test]
async fn contract_write_close_stat_read() {
    let server = TestServer::start().await;
    let client = Client::new();
    let path = test_path("write_read");

    // Open with create + write
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({
            "path": path,
            "flags": 0x242  // O_CREAT | O_RDWR | O_TRUNC
        }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "open failed: {:?}",
        resp.text().await
    );
    let open_resp: OpenResponse = resp.json().await.unwrap();

    // Write data using query params + raw body
    let content = b"Hello, FS9!";
    let resp = client
        .post(format!(
            "{}/api/v1/write?handle_id={}&offset=0",
            server.url, open_resp.handle_id
        ))
        .body(content.to_vec())
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "write failed: {:?}",
        resp.text().await
    );
    let write_resp: WriteResponse = resp.json().await.unwrap();
    assert_eq!(write_resp.bytes_written, content.len());

    // Close
    let resp = client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "close failed: {:?}",
        resp.text().await
    );

    // Stat - verify size
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "stat failed: {:?}",
        resp.text().await
    );
    let info: FileInfo = resp.json().await.unwrap();
    assert_eq!(info.size, content.len() as u64);
    assert!(!info.is_dir);

    // Open for read
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({
            "path": path,
            "flags": 0x00  // O_RDONLY
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let open_resp: OpenResponse = resp.json().await.unwrap();

    // Read and verify content
    let resp = client
        .post(format!("{}/api/v1/read", server.url))
        .json(&json!({
            "handle_id": open_resp.handle_id,
            "offset": 0,
            "size": 100
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let data = resp.bytes().await.unwrap();
    assert_eq!(&data[..], content);

    // Close
    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Cleanup
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// Core Contract #2: read/write after close must return invalid handle error
#[tokio::test]
async fn contract_closed_handle_rejected() {
    let server = TestServer::start().await;
    let client = Client::new();
    let path = test_path("closed_handle");

    // Create file
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    // Close
    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Try to read with closed handle - should fail
    let resp = client
        .post(format!("{}/api/v1/read", server.url))
        .json(&json!({
            "handle_id": open_resp.handle_id,
            "offset": 0,
            "size": 10
        }))
        .send()
        .await
        .unwrap();
    assert!(
        !resp.status().is_success(),
        "read should fail with closed handle"
    );

    // Cleanup
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// Core Contract #3: readdir("/") must see newly created files
#[tokio::test]
async fn contract_readdir_sees_new_file() {
    let server = TestServer::start().await;
    let client = Client::new();
    let path = test_path("visible");

    // Create file
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Readdir root
    let resp = client
        .get(format!("{}/api/v1/readdir?path=/", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let entries: Vec<FileInfo> = resp.json().await.unwrap();

    // File should be visible
    let found = entries
        .iter()
        .any(|e| e.path == path || e.path.ends_with(&path[1..]));
    assert!(
        found,
        "New file should be visible in readdir. Entries: {:?}",
        entries
    );

    // Cleanup
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// Core Contract #4: stat after remove must return not_found
#[tokio::test]
async fn contract_remove_then_stat_not_found() {
    let server = TestServer::start().await;
    let client = Client::new();
    let path = test_path("to_remove");

    // Create file
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Verify exists
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "file should exist before remove"
    );

    // Remove
    let resp = client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "remove should succeed");

    // Stat should fail with not found
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    assert!(!resp.status().is_success(), "stat should fail after remove");
}

/// Core Contract #5: health endpoint
#[tokio::test]
async fn contract_health() {
    let server = TestServer::start().await;
    let client = Client::new();

    let resp = client
        .get(format!("{}/health", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

/// Core Contract #6: list mounts shows root
#[tokio::test]
async fn contract_list_mounts() {
    let server = TestServer::start().await;
    let client = Client::new();

    #[derive(Debug, Deserialize)]
    struct MountInfo {
        path: String,
        name: String,
    }

    let resp = client
        .get(format!("{}/api/v1/mounts", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let mounts: Vec<MountInfo> = resp.json().await.unwrap();
    assert!(!mounts.is_empty(), "should have at least root mount");
    assert!(
        mounts.iter().any(|m| m.path == "/"),
        "should have root mount"
    );
}

/// Core Contract #7: capabilities returns valid flags
#[tokio::test]
async fn contract_capabilities() {
    let server = TestServer::start().await;
    let client = Client::new();

    #[derive(Debug, Deserialize)]
    struct Caps {
        flags: u64,
    }

    let resp = client
        .get(format!("{}/api/v1/capabilities?path=/", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let caps: Caps = resp.json().await.unwrap();
    // MemoryFs should at least support read/write
    assert!(caps.flags > 0, "should have some capabilities");
}

/// Core Contract #8: wstat chmod
#[tokio::test]
async fn contract_wstat_chmod() {
    let server = TestServer::start().await;
    let client = Client::new();
    let path = test_path("chmod");

    // Create file
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Change mode
    let resp = client
        .post(format!("{}/api/v1/wstat", server.url))
        .json(&json!({ "path": path, "mode": 0o755 }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "wstat chmod failed: {:?}",
        resp.text().await
    );

    // Verify mode changed
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    let info: FileInfo = resp.json().await.unwrap();
    assert_eq!(info.mode & 0o777, 0o755, "mode should be updated");

    // Cleanup
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// Core Contract #9: wstat truncate
#[tokio::test]
async fn contract_wstat_truncate() {
    let server = TestServer::start().await;
    let client = Client::new();
    let path = test_path("truncate");

    // Create file with content
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    let content = b"hello world";
    client
        .post(format!(
            "{}/api/v1/write?handle_id={}&offset=0",
            server.url, open_resp.handle_id
        ))
        .body(content.to_vec())
        .send()
        .await
        .unwrap();

    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Truncate to 5 bytes
    let resp = client
        .post(format!("{}/api/v1/wstat", server.url))
        .json(&json!({ "path": path, "size": 5 }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "wstat truncate failed");

    // Verify size
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    let info: FileInfo = resp.json().await.unwrap();
    assert_eq!(info.size, 5, "size should be truncated");

    // Cleanup
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// Core Contract #10: statfs returns valid info
#[tokio::test]
async fn contract_statfs() {
    let server = TestServer::start().await;
    let client = Client::new();

    #[derive(Debug, Deserialize)]
    struct StatFs {
        total_bytes: u64,
        free_bytes: u64,
        block_size: u64,
    }

    let resp = client
        .get(format!("{}/api/v1/statfs?path=/", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let stats: StatFs = resp.json().await.unwrap();
    assert!(stats.total_bytes > 0, "should have total bytes");
    assert!(stats.block_size > 0, "should have block size");
}

// ============================================================================
// PageFS Plugin Tests
// Run the same contract suite with PageFS backend
// ============================================================================

/// PageFS Contract #1: write/close/stat/read
#[tokio::test]
async fn pagefs_write_close_stat_read() {
    let server = TestServer::start_with_pagefs().await;
    let client = Client::new();
    let path = test_path("pfs_write");

    // Open with create
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "open failed: {:?}",
        resp.text().await
    );
    let open_resp: OpenResponse = resp.json().await.unwrap();

    // Write
    let content = b"PageFS test content!";
    let resp = client
        .post(format!(
            "{}/api/v1/write?handle_id={}&offset=0",
            server.url, open_resp.handle_id
        ))
        .body(content.to_vec())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "write failed");
    let write_resp: WriteResponse = resp.json().await.unwrap();
    assert_eq!(write_resp.bytes_written, content.len());

    // Close
    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Stat
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let info: FileInfo = resp.json().await.unwrap();
    assert_eq!(info.size, content.len() as u64);

    // Read back
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x00 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    let resp = client
        .post(format!("{}/api/v1/read", server.url))
        .json(&json!({
            "handle_id": open_resp.handle_id,
            "offset": 0,
            "size": 100
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let data = resp.bytes().await.unwrap();
    assert_eq!(&data[..], content);

    // Cleanup
    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// PageFS Contract #2: readdir sees files
#[tokio::test]
async fn pagefs_readdir() {
    let server = TestServer::start_with_pagefs().await;
    let client = Client::new();
    let path = test_path("pfs_readdir");

    // Create file
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    let open_resp: OpenResponse = resp.json().await.unwrap();

    client
        .post(format!("{}/api/v1/close", server.url))
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    // Readdir
    let resp = client
        .get(format!("{}/api/v1/readdir?path=/", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let entries: Vec<FileInfo> = resp.json().await.unwrap();

    let found = entries
        .iter()
        .any(|e| e.path == path || e.path.ends_with(&path[1..]));
    assert!(found, "File should be visible. Entries: {:?}", entries);

    // Cleanup
    client
        .delete(format!("{}/api/v1/remove?path={}", server.url, path))
        .send()
        .await
        .unwrap();
}

/// PageFS Contract #3: capabilities
#[tokio::test]
async fn pagefs_capabilities() {
    let server = TestServer::start_with_pagefs().await;
    let client = Client::new();

    #[derive(Debug, Deserialize)]
    struct Caps {
        flags: u64,
    }

    let resp = client
        .get(format!("{}/api/v1/capabilities?path=/", server.url))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let caps: Caps = resp.json().await.unwrap();
    assert!(caps.flags > 0, "PageFS should have capabilities");
}
