//! Integration tests for fs9-meta API.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use fs9_meta::{api, AppState, MetaStore};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn create_test_app() -> Router {
    let store = MetaStore::connect("sqlite::memory:").await.unwrap();
    store.migrate().await.unwrap();
    let state = AppState::new(store, "test-secret".to_string(), None);

    Router::new()
        .nest("/api/v1", api::router())
        .with_state(state)
}

async fn request_json(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");

    let body = body.map_or_else(Body::empty, |body| {
        Body::from(serde_json::to_string(&body).unwrap())
    });

    let req = req.body(body).unwrap();
    let response = app.oneshot(req).await.unwrap();

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    (status, json)
}

#[tokio::test]
async fn test_namespace_crud() {
    let app = create_test_app().await;

    // Create namespace
    let (status, body) = request_json(
        app.clone(),
        "POST",
        "/api/v1/namespaces",
        Some(json!({"name": "test-ns"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "test-ns");
    assert_eq!(body["status"], "active");

    // List namespaces
    let (status, body) = request_json(app.clone(), "GET", "/api/v1/namespaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let namespaces = body.as_array().unwrap();
    assert_eq!(namespaces.len(), 1);
    assert_eq!(namespaces[0]["name"], "test-ns");

    // Get namespace
    let (status, body) = request_json(app.clone(), "GET", "/api/v1/namespaces/test-ns", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "test-ns");

    // Get non-existent namespace
    let (status, _) = request_json(app.clone(), "GET", "/api/v1/namespaces/nonexistent", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Delete namespace
    let (status, body) = request_json(app.clone(), "DELETE", "/api/v1/namespaces/test-ns", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);

    // Verify deleted
    let (status, _) = request_json(app, "GET", "/api/v1/namespaces/test-ns", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_namespace_duplicate() {
    let app = create_test_app().await;

    // Create namespace
    let (status, _) = request_json(
        app.clone(),
        "POST",
        "/api/v1/namespaces",
        Some(json!({"name": "dup-ns"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Try to create duplicate
    let (status, _) = request_json(
        app,
        "POST",
        "/api/v1/namespaces",
        Some(json!({"name": "dup-ns"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_user_crud() {
    let app = create_test_app().await;

    // Create user
    let (status, body) = request_json(
        app.clone(),
        "POST",
        "/api/v1/users",
        Some(json!({
            "username": "alice",
            "password": "secret123",
            "email": "alice@example.com"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "alice");
    assert_eq!(body["email"], "alice@example.com");

    // List users
    let (status, body) = request_json(app.clone(), "GET", "/api/v1/users", None).await;
    assert_eq!(status, StatusCode::OK);
    let users = body.as_array().unwrap();
    assert_eq!(users.len(), 1);

    // Get user by name
    let (status, body) = request_json(app.clone(), "GET", "/api/v1/users/by-name/alice", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "alice");

    // Delete user (by ID from create response)
    let user_id = users[0]["id"].as_str().unwrap();
    let (status, body) = request_json(
        app.clone(),
        "DELETE",
        &format!("/api/v1/users/{user_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);
}

#[tokio::test]
async fn test_token_generate_and_validate() {
    let app = create_test_app().await;

    // First create a namespace
    let (status, _) = request_json(
        app.clone(),
        "POST",
        "/api/v1/namespaces",
        Some(json!({"name": "token-test-ns"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Generate token
    let (status, body) = request_json(
        app.clone(),
        "POST",
        "/api/v1/tokens/generate",
        Some(json!({
            "user_id": "user-123",
            "namespace": "token-test-ns",
            "roles": ["read-write"],
            "ttl_seconds": 3600
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().unwrap();
    assert!(!token.is_empty());

    // Validate token
    let (status, body) = request_json(
        app.clone(),
        "POST",
        "/api/v1/tokens/validate",
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["valid"], true);
    assert_eq!(body["user_id"], "user-123");
    assert_eq!(body["namespace"], "token-test-ns");
    assert_eq!(body["roles"], json!(["read-write"]));

    // Validate invalid token
    let (status, body) = request_json(
        app,
        "POST",
        "/api/v1/tokens/validate",
        Some(json!({"token": "invalid.token.here"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["valid"], false);
}

#[tokio::test]
async fn test_token_refresh() {
    let app = create_test_app().await;

    // Create namespace
    let (_, _) = request_json(
        app.clone(),
        "POST",
        "/api/v1/namespaces",
        Some(json!({"name": "refresh-ns"})),
    )
    .await;

    // Generate token
    let (_, body) = request_json(
        app.clone(),
        "POST",
        "/api/v1/tokens/generate",
        Some(json!({
            "user_id": "user-456",
            "namespace": "refresh-ns",
            "roles": ["read-only"]
        })),
    )
    .await;
    let original_token = body["token"].as_str().unwrap().to_string();

    // Refresh token
    let (status, body) = request_json(
        app,
        "POST",
        "/api/v1/tokens/refresh",
        Some(json!({
            "token": original_token,
            "ttl_seconds": 7200
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let new_token = body["token"].as_str().unwrap();
    assert!(!new_token.is_empty());
    assert_ne!(new_token, original_token);
}

#[tokio::test]
async fn test_mount_crud() {
    let app = create_test_app().await;

    // Create namespace first
    let (status, _) = request_json(
        app.clone(),
        "POST",
        "/api/v1/namespaces",
        Some(json!({"name": "mount-test-ns"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Create mount
    let (status, body) = request_json(
        app.clone(),
        "POST",
        "/api/v1/namespaces/mount-test-ns/mounts",
        Some(json!({
            "path": "/data",
            "provider": "pagefs",
            "config": {"uid": 1000, "gid": 1000}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["path"], "/data");
    assert_eq!(body["provider"], "pagefs");

    // List mounts
    let (status, body) = request_json(
        app.clone(),
        "GET",
        "/api/v1/namespaces/mount-test-ns/mounts",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mounts = body.as_array().unwrap();
    assert_eq!(mounts.len(), 1);

    // Delete mount
    let (status, body) = request_json(
        app,
        "DELETE",
        "/api/v1/namespaces/mount-test-ns/mounts/data",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);
}
