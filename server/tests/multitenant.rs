//! Multi-tenant isolation tests for FS9.
//!
//! Verifies that namespaces provide proper data, handle, and mount isolation.

mod harness;

use harness::MultiTenantTestServer;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

const JWT_SECRET: &str = "test-multitenant-secret-key-12345";

fn test_path(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("/test_{}_{}", prefix, COUNTER.fetch_add(1, Ordering::Relaxed))
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

/// Helper: write a file into a namespace.
async fn write_file(client: &Client, url: &str, token: &str, path: &str, content: &[u8]) -> String {
    let resp = client
        .post(format!("{}/api/v1/open", url))
        .bearer_auth(token)
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "open failed: {:?}", resp.text().await);
    let open_resp: OpenResponse = resp.json().await.unwrap();

    let resp = client
        .post(format!(
            "{}/api/v1/write?handle_id={}&offset=0",
            url, open_resp.handle_id
        ))
        .bearer_auth(token)
        .body(content.to_vec())
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "write failed");

    client
        .post(format!("{}/api/v1/close", url))
        .bearer_auth(token)
        .json(&json!({ "handle_id": open_resp.handle_id }))
        .send()
        .await
        .unwrap();

    open_resp.handle_id
}

// ============================================================================
// Test 1: Data Isolation — tenant A writes, tenant B cannot see it
// ============================================================================

#[tokio::test]
async fn data_isolation_between_namespaces() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();
    let path = test_path("data_iso");

    let token_a = server.token("user_a", "acme", &["operator"]);
    let token_b = server.token("user_b", "beta", &["operator"]);

    // Tenant A writes a file
    write_file(&client, &server.url, &token_a, &path, b"acme secret data").await;

    // Tenant A can see it
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "tenant A should see own file");
    let info: FileInfo = resp.json().await.unwrap();
    assert_eq!(info.size, 16); // "acme secret data".len()

    // Tenant B CANNOT see it
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert!(
        !resp.status().is_success(),
        "tenant B should NOT see tenant A's file"
    );
}

// ============================================================================
// Test 2: Handle Isolation — tenant A's handle rejected by tenant B
// ============================================================================

#[tokio::test]
async fn handle_isolation_between_namespaces() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();
    let path = test_path("handle_iso");

    let token_a = server.token("user_a", "acme", &["operator"]);
    let token_b = server.token("user_b", "beta", &["operator"]);

    // Tenant A opens a file
    let resp = client
        .post(format!("{}/api/v1/open", server.url))
        .bearer_auth(&token_a)
        .json(&json!({ "path": path, "flags": 0x242 }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let open_resp: OpenResponse = resp.json().await.unwrap();
    let handle_a = open_resp.handle_id;

    // Tenant B tries to use tenant A's handle — must fail
    let resp = client
        .post(format!("{}/api/v1/read", server.url))
        .bearer_auth(&token_b)
        .json(&json!({
            "handle_id": handle_a,
            "offset": 0,
            "size": 100
        }))
        .send()
        .await
        .unwrap();
    assert!(
        !resp.status().is_success(),
        "tenant B must NOT be able to use tenant A's handle"
    );

    // Cleanup: close with proper tenant
    client
        .post(format!("{}/api/v1/close", server.url))
        .bearer_auth(&token_a)
        .json(&json!({ "handle_id": handle_a }))
        .send()
        .await
        .unwrap();
}

// ============================================================================
// Test 3: Mount Isolation — tenant A's mounts invisible to tenant B
// ============================================================================

#[tokio::test]
async fn mount_isolation_between_namespaces() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token_a = server.token("user_a", "acme", &["operator"]);
    let token_b = server.token("user_b", "beta", &["operator"]);

    // Both tenants list mounts — each should see their own root
    let resp = client
        .get(format!("{}/api/v1/mounts", server.url))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let mounts_a: Vec<serde_json::Value> = resp.json().await.unwrap();

    let resp = client
        .get(format!("{}/api/v1/mounts", server.url))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let mounts_b: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Both should have a root mount (auto-created per namespace)
    assert!(
        mounts_a.iter().any(|m| m["path"] == "/"),
        "Tenant A should have root mount"
    );
    assert!(
        mounts_b.iter().any(|m| m["path"] == "/"),
        "Tenant B should have root mount"
    );
}

// ============================================================================
// Test 4: Same path, different data per namespace
// ============================================================================

#[tokio::test]
async fn same_path_different_data_per_namespace() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();
    let path = "/shared_name.txt";

    let token_a = server.token("user_a", "acme", &["operator"]);
    let token_b = server.token("user_b", "beta", &["operator"]);

    // Both tenants write to the same path
    write_file(&client, &server.url, &token_a, path, b"ACME data").await;
    write_file(&client, &server.url, &token_b, path, b"BETA data longer").await;

    // Read back from A
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let info_a: FileInfo = resp.json().await.unwrap();
    assert_eq!(info_a.size, 9, "ACME data should be 9 bytes");

    // Read back from B
    let resp = client
        .get(format!("{}/api/v1/stat?path={}", server.url, path))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    let info_b: FileInfo = resp.json().await.unwrap();
    assert_eq!(info_b.size, 16, "BETA data longer should be 16 bytes");
}

// ============================================================================
// Test 5: Readdir isolation — tenant A's files don't leak into tenant B
// ============================================================================

#[tokio::test]
async fn readdir_isolation() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token_a = server.token("user_a", "acme", &["operator"]);
    let token_b = server.token("user_b", "beta", &["operator"]);

    // Tenant A creates 3 files
    write_file(&client, &server.url, &token_a, "/a1.txt", b"a1").await;
    write_file(&client, &server.url, &token_a, "/a2.txt", b"a2").await;
    write_file(&client, &server.url, &token_a, "/a3.txt", b"a3").await;

    // Tenant B creates 1 file
    write_file(&client, &server.url, &token_b, "/b1.txt", b"b1").await;

    // Tenant A readdir should see 3 files
    let resp = client
        .get(format!("{}/api/v1/readdir?path=/", server.url))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let entries_a: Vec<FileInfo> = resp.json().await.unwrap();
    assert_eq!(entries_a.len(), 3, "Tenant A should see exactly 3 files");

    // Tenant B readdir should see 1 file
    let resp = client
        .get(format!("{}/api/v1/readdir?path=/", server.url))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    let entries_b: Vec<FileInfo> = resp.json().await.unwrap();
    assert_eq!(entries_b.len(), 1, "Tenant B should see exactly 1 file");
}

// ============================================================================
// Test 6: No auth token → rejected
// ============================================================================

#[tokio::test]
async fn no_token_rejected() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    // No bearer token → should be 401
    let resp = client
        .get(format!("{}/api/v1/stat?path=/", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "Request without token should be rejected"
    );
}

// ============================================================================
// Test 7: Wrong secret → rejected  
// ============================================================================

#[tokio::test]
async fn wrong_secret_rejected() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    // Generate token with wrong secret
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let claims = json!({
        "sub": "evil",
        "ns": "acme",
        "roles": ["operator"],
        "exp": now + 3600,
        "iat": now,
    });

    let bad_token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"wrong-secret"),
    )
    .unwrap();

    let resp = client
        .get(format!("{}/api/v1/stat?path=/", server.url))
        .bearer_auth(&bad_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "Token with wrong secret should be rejected"
    );
}

// ============================================================================
// Test 8: Unknown namespace → 403
// ============================================================================

#[tokio::test]
async fn unknown_namespace_rejected() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    // Valid token but namespace "ghost" was never created
    let token = server.token("user_x", "ghost", &["operator"]);

    let resp = client
        .get(format!("{}/api/v1/stat?path=/", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Unknown namespace should be rejected with 403"
    );
}

// ============================================================================
// Test 9: Token missing 'ns' claim → 401
// ============================================================================

#[tokio::test]
async fn token_missing_ns_rejected() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    // Generate a valid token WITHOUT the ns field
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let claims = json!({
        "sub": "user_no_ns",
        "roles": ["operator"],
        "exp": now + 3600,
        "iat": now,
    });

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap();

    let resp = client
        .get(format!("{}/api/v1/stat?path=/", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "Token without 'ns' claim should be rejected with 401"
    );
}
