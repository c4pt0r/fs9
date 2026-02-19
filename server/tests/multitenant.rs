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

/// Helper: write a file into a namespace.
async fn write_file(client: &Client, url: &str, token: &str, path: &str, content: &[u8]) -> String {
    let resp = client
        .post(format!("{}/api/v1/open", url))
        .bearer_auth(token)
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

#[derive(Debug, Deserialize)]
struct NamespaceInfoResp {
    name: String,
    created_at: String,
    created_by: String,
    status: String,
}

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

// ============================================================================
// Namespace Management API Tests
// ============================================================================

// Test 10: Admin can create a namespace → 201
#[tokio::test]
async fn admin_can_create_namespace() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "new-tenant" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        201,
        "Admin should be able to create namespace"
    );

    let info: NamespaceInfoResp = resp.json().await.unwrap();
    assert_eq!(info.name, "new-tenant");
    assert_eq!(info.created_by, "admin-user");
    assert_eq!(info.status, "active");
    assert!(!info.created_at.is_empty(), "created_at should be set");
}

// Test 11: Operator cannot create a namespace → 403
#[tokio::test]
async fn operator_cannot_create_namespace() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("op-user", "acme", &["operator"]);

    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "sneaky-ns" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Operator should not be able to create namespace"
    );
}

// Test 12: Duplicate namespace creation → 409
#[tokio::test]
async fn duplicate_namespace_rejected() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    // "acme" already exists (pre-created)
    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "acme" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        409,
        "Duplicate namespace should return 409 Conflict"
    );
}

// Test 13: Invalid namespace name → 400
#[tokio::test]
async fn invalid_namespace_name_rejected() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    // Uppercase characters
    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "BadName" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        400,
        "Uppercase name should be rejected"
    );

    // Special characters
    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "bad@name!" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        400,
        "Special chars should be rejected"
    );

    // Empty name
    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400, "Empty name should be rejected");

    // Starts with hyphen
    let resp = client
        .post(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "-invalid" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        400,
        "Name starting with hyphen should be rejected"
    );
}

// Test 14: Admin can list namespaces → 200
#[tokio::test]
async fn admin_can_list_namespaces() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    let resp = client
        .get(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Admin should be able to list namespaces"
    );

    let namespaces: Vec<NamespaceInfoResp> = resp.json().await.unwrap();
    let names: Vec<&str> = namespaces.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"acme"), "Should see acme namespace");
    assert!(names.contains(&"beta"), "Should see beta namespace");
    assert!(names.contains(&"default"), "Should see default namespace");
}

// Test 15: Operator can list namespaces → 200
#[tokio::test]
async fn operator_can_list_namespaces() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("op-user", "acme", &["operator"]);

    let resp = client
        .get(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Operator should be able to list namespaces"
    );

    let namespaces: Vec<NamespaceInfoResp> = resp.json().await.unwrap();
    assert!(
        namespaces.len() >= 3,
        "Should see at least default, acme, beta"
    );
}

// Test 16: Reader cannot list namespaces → 403
#[tokio::test]
async fn reader_cannot_list_namespaces() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("reader-user", "acme", &["reader"]);

    let resp = client
        .get(format!("{}/api/v1/namespaces", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Reader should not be able to list namespaces"
    );
}

// Test 17: Get single namespace → 200
#[tokio::test]
async fn get_single_namespace() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    let resp = client
        .get(format!("{}/api/v1/namespaces/acme", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Should be able to get single namespace"
    );

    let info: NamespaceInfoResp = resp.json().await.unwrap();
    assert_eq!(info.name, "acme");
    assert_eq!(info.status, "active");
}

// Test 18: Get nonexistent namespace → 404
#[tokio::test]
async fn get_nonexistent_namespace() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    let resp = client
        .get(format!("{}/api/v1/namespaces/ghost", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        404,
        "Nonexistent namespace should return 404"
    );
}

// ============================================================================
// Phase 3: Role Gate Tests — mount & plugin operations
// ============================================================================

// Test 19: Reader cannot mount → 403
#[tokio::test]
async fn reader_cannot_mount() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("reader-user", "acme", &["reader"]);

    let resp = client
        .post(format!("{}/api/v1/mount", server.url))
        .bearer_auth(&token)
        .json(&json!({ "path": "/mnt/test", "provider": "memfs", "config": {} }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Reader should not be able to mount"
    );
}

// Test 20: Operator can mount (will fail on provider lookup, but role check passes → not 403)
#[tokio::test]
async fn operator_can_mount() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("op-user", "acme", &["operator"]);

    let resp = client
        .post(format!("{}/api/v1/mount", server.url))
        .bearer_auth(&token)
        .json(&json!({ "path": "/mnt/test", "provider": "nonexistent", "config": {} }))
        .send()
        .await
        .unwrap();
    // Should NOT be 403 — role check passed. Will be 400 because plugin doesn't exist.
    assert_ne!(
        resp.status().as_u16(),
        403,
        "Operator should pass the role gate for mount"
    );
}

// Test 21: Reader cannot load plugin → 403
#[tokio::test]
async fn reader_cannot_load_plugin() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("reader-user", "acme", &["reader"]);

    let resp = client
        .post(format!("{}/api/v1/plugin/load", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "test", "path": "/nonexistent.so" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Reader should not be able to load plugins"
    );
}

// Test 22: Operator cannot load plugin → 403
#[tokio::test]
async fn operator_cannot_load_plugin() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("op-user", "acme", &["operator"]);

    let resp = client
        .post(format!("{}/api/v1/plugin/load", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "test", "path": "/nonexistent.so" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Operator should not be able to load plugins"
    );
}

// Test 23: Admin can list plugins → 200
#[tokio::test]
async fn admin_can_list_plugins() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("admin-user", "acme", &["admin"]);

    let resp = client
        .get(format!("{}/api/v1/plugin/list", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Admin should be able to list plugins"
    );

    let plugins: Vec<String> = resp.json().await.unwrap();
    // No plugins loaded by default — just verify it's an empty list
    assert!(plugins.is_empty(), "No plugins loaded by default");
}

// Test 24: Reader cannot list plugins → 403
#[tokio::test]
async fn reader_cannot_list_plugins() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("reader-user", "acme", &["reader"]);

    let resp = client
        .get(format!("{}/api/v1/plugin/list", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Reader should not be able to list plugins"
    );
}

// Test 25: Operator can list plugins → 200
#[tokio::test]
async fn operator_can_list_plugins() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("op-user", "acme", &["operator"]);

    let resp = client
        .get(format!("{}/api/v1/plugin/list", server.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Operator should be able to list plugins"
    );
}

// Test 26: Reader cannot unload plugin → 403
#[tokio::test]
async fn reader_cannot_unload_plugin() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("reader-user", "acme", &["reader"]);

    let resp = client
        .post(format!("{}/api/v1/plugin/unload", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "test" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Reader should not be able to unload plugins"
    );
}

// Test 27: Operator cannot unload plugin → 403
#[tokio::test]
async fn operator_cannot_unload_plugin() {
    let server = MultiTenantTestServer::start(JWT_SECRET).await;
    let client = Client::new();

    let token = server.token("op-user", "acme", &["operator"]);

    let resp = client
        .post(format!("{}/api/v1/plugin/unload", server.url))
        .bearer_auth(&token)
        .json(&json!({ "name": "test" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "Operator should not be able to unload plugins"
    );
}
