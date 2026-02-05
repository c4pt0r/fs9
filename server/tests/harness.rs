//! Test harness for in-process server testing.
//!
//! Starts the FS9 server in the same process with a random port,
//! allowing fast, reliable integration tests without external processes.

use fs9_core::{HandleRegistry, MemoryFs, MountTable, PluginManager, VfsRouter};
use fs9_sdk::FsProvider;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A test server instance running in the background.
pub struct TestServer {
    pub url: String,
    pub addr: SocketAddr,
    pub jwt_secret: Option<String>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TestServer {
    /// Start a new test server with MemoryFs mounted at root (no auth).
    pub async fn start() -> Self {
        Self::start_with_provider(Arc::new(MemoryFs::new())).await
    }

    /// Start a test server with PageFS plugin (in-memory KV backend).
    pub async fn start_with_pagefs() -> Self {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().unwrap();

        let plugin_path = workspace_root.join("target/debug/libfs9_plugin_pagefs.so");
        if !plugin_path.exists() {
            panic!(
                "PageFS plugin not found at {:?}. Run `cargo build -p fs9-plugin-pagefs` first.",
                plugin_path
            );
        }

        let plugin_manager = Arc::new(PluginManager::new());
        plugin_manager
            .load("pagefs", &plugin_path)
            .expect("Failed to load PageFS plugin");

        let provider = Arc::new(
            plugin_manager
                .create_provider("pagefs", r#"{"uid": 1000, "gid": 1000}"#)
                .expect("Failed to create PageFS provider"),
        );

        Self::start_with_provider_and_plugin_manager(provider, Some(plugin_manager)).await
    }

    /// Start a test server with auth enabled + multi-tenant support.
    pub async fn start_with_auth(jwt_secret: &str) -> Self {
        Self::start_with_auth_and_provider(jwt_secret, Arc::new(MemoryFs::new())).await
    }

    /// Start with auth + custom provider for default namespace.
    pub async fn start_with_auth_and_provider(
        jwt_secret: &str,
        provider: Arc<dyn FsProvider>,
    ) -> Self {
        Self::start_internal(provider, None, Some(jwt_secret.to_string())).await
    }

    /// Start a test server with a custom provider mounted at root.
    pub async fn start_with_provider(provider: Arc<dyn FsProvider>) -> Self {
        Self::start_with_provider_and_plugin_manager(provider, None).await
    }

    /// Start a test server with a custom provider and optional plugin manager.
    async fn start_with_provider_and_plugin_manager(
        provider: Arc<dyn FsProvider>,
        plugin_manager: Option<Arc<PluginManager>>,
    ) -> Self {
        Self::start_internal(provider, plugin_manager, None).await
    }

    async fn start_internal(
        provider: Arc<dyn FsProvider>,
        _plugin_manager: Option<Arc<PluginManager>>,
        jwt_secret: Option<String>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);

        let mount_table = Arc::new(MountTable::new());
        let handle_registry = Arc::new(HandleRegistry::new(Duration::from_secs(300)));
        let vfs = Arc::new(VfsRouter::new(mount_table.clone(), handle_registry.clone()));

        mount_table.mount("/", "test", provider).await.unwrap();

        let state = Arc::new(TestAppState {
            vfs,
            mount_table,
            handle_registry,
            handle_map: Arc::new(tokio::sync::RwLock::new(HandleMap::new())),
        });

        let auth_enabled = jwt_secret.is_some();
        let jwt_secret_clone = jwt_secret.clone();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let app = if auth_enabled {
            let secret = jwt_secret_clone.clone().unwrap();
            build_test_router_with_auth(state, &secret)
        } else {
            build_test_router(state)
        };

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        Self::wait_ready(&url, jwt_secret.as_deref()).await;

        Self {
            url,
            addr,
            jwt_secret,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Generate a JWT token for a given namespace and subject.
    pub fn token(&self, subject: &str, namespace: &str, roles: &[&str]) -> String {
        let secret = self
            .jwt_secret
            .as_ref()
            .expect("Server was not started with auth enabled");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = serde_json::json!({
            "sub": subject,
            "ns": namespace,
            "roles": roles,
            "exp": now + 3600,
            "iat": now,
        });

        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    async fn wait_ready(url: &str, _jwt_secret: Option<&str>) {
        let client = reqwest::Client::new();
        for _ in 0..50 {
            if client.get(format!("{}/health", url)).send().await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("Server failed to start within 500ms");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

// Simplified app state for testing (no namespace manager, single namespace)
pub struct TestAppState {
    pub vfs: Arc<VfsRouter>,
    pub mount_table: Arc<MountTable>,
    pub handle_registry: Arc<HandleRegistry>,
    pub handle_map: Arc<tokio::sync::RwLock<HandleMap>>,
}

pub struct HandleMap {
    uuid_to_id: std::collections::HashMap<String, u64>,
    id_to_uuid: std::collections::HashMap<u64, String>,
}

impl HandleMap {
    pub fn new() -> Self {
        Self {
            uuid_to_id: std::collections::HashMap::new(),
            id_to_uuid: std::collections::HashMap::new(),
        }
    }

    pub fn insert(&mut self, uuid: String, id: u64) {
        self.uuid_to_id.insert(uuid.clone(), id);
        self.id_to_uuid.insert(id, uuid);
    }

    pub fn get_id(&self, uuid: &str) -> Option<u64> {
        self.uuid_to_id.get(uuid).copied()
    }

    pub fn remove_by_uuid(&mut self, uuid: &str) -> Option<u64> {
        if let Some(id) = self.uuid_to_id.remove(uuid) {
            self.id_to_uuid.remove(&id);
            Some(id)
        } else {
            None
        }
    }
}

// ============================================================================
// Multi-tenant test state: uses real namespace manager
// ============================================================================

use fs9_server::namespace::{NamespaceManager, DEFAULT_NAMESPACE};
use fs9_server::auth::RequestContext;

/// Multi-tenant test server that uses the real NamespaceManager.
pub struct MultiTenantTestServer {
    pub url: String,
    pub addr: SocketAddr,
    pub jwt_secret: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

pub struct MultiTenantAppState {
    pub namespace_manager: Arc<NamespaceManager>,
}

impl MultiTenantTestServer {
    pub async fn start(jwt_secret: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);

        let ns_manager = Arc::new(NamespaceManager::new(Duration::from_secs(300)));

        // Create default namespace with a root memfs mount
        let default_ns = ns_manager.create(DEFAULT_NAMESPACE, "system").await.unwrap();
        default_ns
            .mount_table
            .mount("/", "memfs", Arc::new(MemoryFs::new()))
            .await
            .unwrap();

        // Pre-create tenant namespaces used in tests
        for ns_name in &["acme", "beta"] {
            let ns = ns_manager.create(ns_name, "system").await.unwrap();
            ns.mount_table
                .mount("/", "memfs", Arc::new(MemoryFs::new()))
                .await
                .unwrap();
        }

        let state = Arc::new(MultiTenantAppState {
            namespace_manager: ns_manager,
        });

        let secret = jwt_secret.to_string();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let app = build_multitenant_router(state, &secret);

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let client = reqwest::Client::new();
        for _ in 0..50 {
            if client.get(format!("{}/health", &url)).send().await.is_ok() {
                return Self {
                    url,
                    addr,
                    jwt_secret: jwt_secret.to_string(),
                    shutdown_tx: Some(shutdown_tx),
                };
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("Multi-tenant server failed to start");
    }

    /// Generate a JWT token for a given namespace.
    pub fn token(&self, subject: &str, namespace: &str, roles: &[&str]) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = serde_json::json!({
            "sub": subject,
            "ns": namespace,
            "roles": roles,
            "exp": now + 3600,
            "iat": now,
        });

        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .unwrap()
    }
}

impl Drop for MultiTenantTestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn build_multitenant_router(
    state: Arc<MultiTenantAppState>,
    jwt_secret: &str,
) -> axum::Router {
    use axum::routing::{delete, get, post};
    use axum::{middleware, Router};

    let secret = jwt_secret.to_string();

    Router::new()
        .route("/health", get(mt_health))
        // Namespace management API
        .route(
            "/api/v1/namespaces",
            post(mt_create_namespace).get(mt_list_namespaces),
        )
        .route("/api/v1/namespaces/{ns}", get(mt_get_namespace))
        // Filesystem API
        .route("/api/v1/stat", get(mt_stat))
        .route("/api/v1/open", post(mt_open))
        .route("/api/v1/read", post(mt_read))
        .route("/api/v1/write", post(mt_write))
        .route("/api/v1/close", post(mt_close))
        .route("/api/v1/readdir", get(mt_readdir))
        .route("/api/v1/remove", delete(mt_remove))
        .route("/api/v1/mounts", get(mt_list_mounts))
        .layer(middleware::from_fn(move |mut req: axum::extract::Request, next: axum::middleware::Next| {
            let secret = secret.clone();
            async move {
                let path = req.uri().path().to_string();
                if path == "/health" {
                    req.extensions_mut().insert(RequestContext {
                        ns: DEFAULT_NAMESPACE.to_string(),
                        user_id: "anonymous".to_string(),
                        roles: Vec::new(),
                    });
                    return next.run(req).await;
                }

                let auth_header = req.headers().get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());

                let token = match auth_header {
                    Some(ref h) if h.starts_with("Bearer ") => &h[7..],
                    _ => {
                        return axum::response::Response::builder()
                            .status(401)
                            .body(axum::body::Body::from("Missing or invalid Authorization"))
                            .unwrap();
                    }
                };

                let validation = jsonwebtoken::Validation::default();
                let key = jsonwebtoken::DecodingKey::from_secret(secret.as_bytes());

                match jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation) {
                    Ok(data) => {
                        let claims = data.claims;
                        let ns = match claims.get("ns").and_then(|v| v.as_str()) {
                            Some(ns) => ns.to_string(),
                            None => {
                                return axum::response::Response::builder()
                                    .status(401)
                                    .header("content-type", "application/json")
                                    .body(axum::body::Body::from(
                                        r#"{"error":"Token missing required 'ns' claim","code":401}"#
                                    ))
                                    .unwrap();
                            }
                        };
                        let user_id = claims.get("sub")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let roles: Vec<String> = claims.get("roles")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();

                        req.extensions_mut().insert(RequestContext { ns, user_id, roles });
                        next.run(req).await
                    }
                    Err(e) => {
                        axum::response::Response::builder()
                            .status(401)
                            .body(axum::body::Body::from(format!("Invalid token: {e}")))
                            .unwrap()
                    }
                }
            }
        }))
        .with_state(state)
}

// ============================================================================
// Multi-tenant handlers
// ============================================================================

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::Json;
use fs9_sdk::{FsError, Handle, OpenFlags, StatChanges};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

async fn mt_health() -> &'static str { "ok" }

#[derive(Deserialize)]
struct PathQuery { path: String }

#[derive(Serialize)]
struct FileInfoResp {
    path: String,
    size: u64,
    mode: u32,
    is_dir: bool,
}

type MtResult<T> = Result<T, (StatusCode, String)>;
fn mt_err(e: FsError) -> (StatusCode, String) {
    let s = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (s, e.to_string())
}

/// Resolve namespace — unknown namespaces are rejected with 403.
async fn mt_resolve_ns(state: &MultiTenantAppState, ctx: &RequestContext) -> Result<Arc<fs9_server::namespace::Namespace>, (StatusCode, String)> {
    state.namespace_manager.get(&ctx.ns).await
        .ok_or_else(|| (StatusCode::FORBIDDEN, format!("Namespace '{}' not found or access denied", ctx.ns)))
}

async fn mt_stat(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<PathQuery>,
) -> MtResult<Json<FileInfoResp>> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let info = ns.vfs.stat(&q.path).await.map_err(mt_err)?;
    let is_dir = info.is_dir();
    Ok(Json(FileInfoResp { path: info.path, size: info.size, mode: info.mode, is_dir }))
}

#[derive(Serialize)]
struct OpenResp { handle_id: String }

#[derive(Deserialize)]
struct OpenReq { path: String, #[serde(default)] flags: u32 }

fn parse_flags(bits: u32) -> OpenFlags {
    let read = (bits & 0x03) != 0x01;
    let write = (bits & 0x03) != 0x00;
    let create = (bits & 0x40) != 0 || (bits & 0x200) != 0;
    let truncate = (bits & 0x200) != 0;
    OpenFlags { read, write, create, truncate, append: false, directory: false }
}

async fn mt_open(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<OpenReq>,
) -> MtResult<Json<OpenResp>> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let flags = parse_flags(req.flags);
    let handle = ns.vfs.open(&req.path, flags).await.map_err(mt_err)?;
    let uuid = uuid::Uuid::new_v4().to_string();
    ns.handle_map.write().await.insert(uuid.clone(), handle.id());
    Ok(Json(OpenResp { handle_id: uuid }))
}

#[derive(Deserialize)]
struct ReadReq { handle_id: String, offset: u64, size: usize }

async fn mt_read(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<ReadReq>,
) -> MtResult<Vec<u8>> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let hid = ns.handle_map.read().await.get_id(&req.handle_id)
        .ok_or((StatusCode::BAD_REQUEST, "Invalid handle".into()))?;
    let data = ns.vfs.read(&Handle::new(hid), req.offset, req.size).await.map_err(mt_err)?;
    Ok(data.to_vec())
}

#[derive(Deserialize)]
struct WriteQ { handle_id: String, offset: u64 }
#[derive(Serialize)]
struct WriteResp { bytes_written: usize }

async fn mt_write(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<WriteQ>,
    body: axum::body::Bytes,
) -> MtResult<Json<WriteResp>> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let hid = ns.handle_map.read().await.get_id(&q.handle_id)
        .ok_or((StatusCode::BAD_REQUEST, "Invalid handle".into()))?;
    let bw = ns.vfs.write(&Handle::new(hid), q.offset, body).await.map_err(mt_err)?;
    Ok(Json(WriteResp { bytes_written: bw }))
}

#[derive(Deserialize)]
struct CloseReq { handle_id: String, #[serde(default)] sync: bool }

async fn mt_close(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CloseReq>,
) -> MtResult<StatusCode> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let hid = ns.handle_map.write().await.remove_by_uuid(&req.handle_id)
        .ok_or((StatusCode::BAD_REQUEST, "Invalid handle".into()))?;
    ns.vfs.close(Handle::new(hid), req.sync).await.map_err(mt_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mt_readdir(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<PathQuery>,
) -> MtResult<Json<Vec<FileInfoResp>>> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let entries = ns.vfs.readdir(&q.path).await.map_err(mt_err)?;
    Ok(Json(entries.into_iter().map(|i| {
        let is_dir = i.is_dir();
        FileInfoResp { path: i.path, size: i.size, mode: i.mode, is_dir }
    }).collect()))
}

async fn mt_remove(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<PathQuery>,
) -> MtResult<StatusCode> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    ns.vfs.remove(&q.path).await.map_err(mt_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct MountInfoResp { path: String, name: String }

async fn mt_list_mounts(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
) -> MtResult<Json<Vec<MountInfoResp>>> {
    let ns = mt_resolve_ns(&state, &ctx).await?;
    let mounts = ns.mount_table.list_mounts().await;
    Ok(Json(mounts.into_iter().map(|m| MountInfoResp { path: m.path, name: m.provider_name }).collect()))
}

// ============================================================================
// Multi-tenant namespace management handlers
// ============================================================================

#[derive(Deserialize)]
struct CreateNsReq { name: String }

#[derive(Serialize)]
struct NsInfoResp {
    name: String,
    created_at: String,
    created_by: String,
    status: String,
}

fn mt_require_role(ctx: &RequestContext, allowed: &[&str]) -> Result<(), (StatusCode, String)> {
    if ctx.roles.iter().any(|r| allowed.contains(&r.as_str())) {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, r#"{"error":"Insufficient permissions","code":403}"#.to_string()))
    }
}

async fn mt_create_namespace(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateNsReq>,
) -> Result<(StatusCode, Json<NsInfoResp>), (StatusCode, String)> {
    mt_require_role(&ctx, &["admin"])?;

    match state.namespace_manager.create(&req.name, &ctx.user_id).await {
        Ok(_ns) => {
            let info = state.namespace_manager.get_info(&req.name).await.unwrap();
            Ok((StatusCode::CREATED, Json(NsInfoResp {
                name: info.name,
                created_at: info.created_at,
                created_by: info.created_by,
                status: info.status,
            })))
        }
        Err(e) if e.contains("already exists") => {
            Err((StatusCode::CONFLICT, format!(r#"{{"error":"{}","code":409}}"#, e)))
        }
        Err(e) => {
            Err((StatusCode::BAD_REQUEST, format!(r#"{{"error":"{}","code":400}}"#, e)))
        }
    }
}

async fn mt_list_namespaces(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
) -> MtResult<Json<Vec<NsInfoResp>>> {
    mt_require_role(&ctx, &["admin", "operator"])?;

    let infos = state.namespace_manager.list_info().await;
    Ok(Json(infos.into_iter().map(|info| NsInfoResp {
        name: info.name,
        created_at: info.created_at,
        created_by: info.created_by,
        status: info.status,
    }).collect()))
}

async fn mt_get_namespace(
    State(state): State<Arc<MultiTenantAppState>>,
    Extension(ctx): Extension<RequestContext>,
    axum::extract::Path(ns_name): axum::extract::Path<String>,
) -> MtResult<Json<NsInfoResp>> {
    mt_require_role(&ctx, &["admin", "operator"])?;

    match state.namespace_manager.get_info(&ns_name).await {
        Some(info) => Ok(Json(NsInfoResp {
            name: info.name,
            created_at: info.created_at,
            created_by: info.created_by,
            status: info.status,
        })),
        None => Err((StatusCode::NOT_FOUND, format!(r#"{{"error":"Namespace '{}' not found","code":404}}"#, ns_name)))
    }
}

// ============================================================================
// Legacy single-tenant router (for existing contract tests)
// ============================================================================

fn build_test_router(state: Arc<TestAppState>) -> axum::Router {
    use axum::routing::{delete, get, post};
    use axum::Router;

    Router::new()
        .route("/health", get(health))
        .route("/api/v1/stat", get(stat))
        .route("/api/v1/wstat", post(wstat))
        .route("/api/v1/statfs", get(statfs))
        .route("/api/v1/open", post(open))
        .route("/api/v1/read", post(read))
        .route("/api/v1/write", post(write))
        .route("/api/v1/close", post(close))
        .route("/api/v1/readdir", get(readdir))
        .route("/api/v1/remove", delete(remove))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/mounts", get(list_mounts))
        .with_state(state)
}

fn build_test_router_with_auth(state: Arc<TestAppState>, _jwt_secret: &str) -> axum::Router {
    // For now, same as non-auth (existing contract tests don't use auth)
    build_test_router(state)
}

// ============================================================================
// Legacy single-tenant handlers
// ============================================================================

async fn health() -> &'static str { "ok" }

fn system_time_to_epoch(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn map_err(e: FsError) -> (StatusCode, String) {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, e.to_string())
}

#[derive(Serialize)]
struct FileInfoResponse {
    path: String,
    size: u64,
    mode: u32,
    is_dir: bool,
    mtime: Option<u64>,
}

type AppResult<T> = Result<T, (StatusCode, String)>;

async fn stat(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<FileInfoResponse>> {
    let info = state.vfs.stat(&q.path).await.map_err(map_err)?;
    let is_dir = info.is_dir();
    Ok(Json(FileInfoResponse {
        path: info.path, size: info.size, mode: info.mode, is_dir,
        mtime: Some(system_time_to_epoch(info.mtime)),
    }))
}

#[derive(Deserialize)]
struct WstatRequest { path: String, #[serde(default)] mode: Option<u32>, #[serde(default)] size: Option<u64> }

async fn wstat(State(state): State<Arc<TestAppState>>, Json(req): Json<WstatRequest>) -> AppResult<StatusCode> {
    let changes = StatChanges { mode: req.mode, size: req.size, ..Default::default() };
    state.vfs.wstat(&req.path, changes).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct FsStatsResponse { total_bytes: u64, free_bytes: u64, block_size: u64 }

async fn statfs(State(state): State<Arc<TestAppState>>, Query(q): Query<PathQuery>) -> AppResult<Json<FsStatsResponse>> {
    let stats = state.vfs.statfs(&q.path).await.map_err(map_err)?;
    Ok(Json(FsStatsResponse { total_bytes: stats.total_bytes, free_bytes: stats.free_bytes, block_size: u64::from(stats.block_size) }))
}

#[derive(Deserialize)]
struct LegacyOpenReq { path: String, #[serde(default)] flags: u32 }
#[derive(Serialize)]
struct LegacyOpenResp { handle_id: String }

async fn open(State(state): State<Arc<TestAppState>>, Json(req): Json<LegacyOpenReq>) -> AppResult<Json<LegacyOpenResp>> {
    let flags = parse_flags(req.flags);
    let handle = state.vfs.open(&req.path, flags).await.map_err(map_err)?;
    let uuid = uuid::Uuid::new_v4().to_string();
    state.handle_map.write().await.insert(uuid.clone(), handle.id());
    Ok(Json(LegacyOpenResp { handle_id: uuid }))
}

#[derive(Deserialize)]
struct LegacyReadReq { handle_id: String, offset: u64, size: usize }

async fn read(State(state): State<Arc<TestAppState>>, Json(req): Json<LegacyReadReq>) -> AppResult<Vec<u8>> {
    let hid = state.handle_map.read().await.get_id(&req.handle_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid handle".to_string()))?;
    let data = state.vfs.read(&Handle::new(hid), req.offset, req.size).await.map_err(map_err)?;
    Ok(data.to_vec())
}

#[derive(Deserialize)]
struct LegacyWriteQ { handle_id: String, offset: u64 }
#[derive(Serialize)]
struct LegacyWriteResp { bytes_written: usize }

async fn write(State(state): State<Arc<TestAppState>>, Query(q): Query<LegacyWriteQ>, body: axum::body::Bytes) -> AppResult<Json<LegacyWriteResp>> {
    let hid = state.handle_map.read().await.get_id(&q.handle_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid handle".to_string()))?;
    let bw = state.vfs.write(&Handle::new(hid), q.offset, body).await.map_err(map_err)?;
    Ok(Json(LegacyWriteResp { bytes_written: bw }))
}

#[derive(Deserialize)]
struct LegacyCloseReq { handle_id: String, #[serde(default)] sync: bool }

async fn close(State(state): State<Arc<TestAppState>>, Json(req): Json<LegacyCloseReq>) -> AppResult<StatusCode> {
    let hid = state.handle_map.write().await.remove_by_uuid(&req.handle_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid handle".to_string()))?;
    state.vfs.close(Handle::new(hid), req.sync).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn readdir(State(state): State<Arc<TestAppState>>, Query(q): Query<PathQuery>) -> AppResult<Json<Vec<FileInfoResponse>>> {
    let entries = state.vfs.readdir(&q.path).await.map_err(map_err)?;
    Ok(Json(entries.into_iter().map(|info| {
        let is_dir = info.is_dir();
        FileInfoResponse { path: info.path, size: info.size, mode: info.mode, is_dir, mtime: Some(system_time_to_epoch(info.mtime)) }
    }).collect()))
}

async fn remove(State(state): State<Arc<TestAppState>>, Query(q): Query<PathQuery>) -> AppResult<StatusCode> {
    state.vfs.remove(&q.path).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct CapabilitiesResponse { flags: u64 }

async fn capabilities(State(state): State<Arc<TestAppState>>, Query(q): Query<PathQuery>) -> AppResult<Json<CapabilitiesResponse>> {
    let caps = state.mount_table.get_mount_info(&q.path).await.map(|(_, caps)| caps.bits()).unwrap_or(0);
    Ok(Json(CapabilitiesResponse { flags: caps }))
}

#[derive(Serialize)]
struct MountInfoResponse { path: String, name: String }

async fn list_mounts(State(state): State<Arc<TestAppState>>) -> Json<Vec<MountInfoResponse>> {
    let mounts = state.mount_table.list_mounts().await;
    Json(mounts.into_iter().map(|m| MountInfoResponse { path: m.path, name: m.provider_name }).collect())
}
