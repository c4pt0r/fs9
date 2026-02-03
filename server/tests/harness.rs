//! Test harness for in-process server testing.
//!
//! Starts the FS9 server in the same process with a random port,
//! allowing fast, reliable integration tests without external processes.

use fs9_core::{HandleRegistry, MemoryFs, MountTable, VfsRouter};
use fs9_sdk::FsProvider;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// A test server instance running in the background.
pub struct TestServer {
    pub url: String,
    pub addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TestServer {
    /// Start a new test server with MemoryFs mounted at root.
    pub async fn start() -> Self {
        Self::start_with_provider(Arc::new(MemoryFs::new())).await
    }

    /// Start a test server with a custom provider mounted at root.
    pub async fn start_with_provider(provider: Arc<dyn FsProvider>) -> Self {
        // Bind to random port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);

        // Create state using the same structure as the real server
        let mount_table = Arc::new(MountTable::new());
        let handle_registry = Arc::new(HandleRegistry::new(Duration::from_secs(300)));
        let vfs = Arc::new(VfsRouter::new(mount_table.clone(), handle_registry.clone()));

        // Mount provider at root
        mount_table.mount("/", "test", provider).await.unwrap();

        // Build app state
        let state = Arc::new(TestAppState {
            vfs,
            mount_table,
            handle_registry,
            handle_map: Arc::new(tokio::sync::RwLock::new(HandleMap::new())),
        });

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Build router - minimal version for testing
        let app = build_test_router(state);

        // Spawn server
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        // Wait for server to be ready
        Self::wait_ready(&url).await;

        Self {
            url,
            addr,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    async fn wait_ready(url: &str) {
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

// Simplified app state for testing
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

// Build a minimal router for testing
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

// Handlers
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use fs9_sdk::{FsError, Handle, OpenFlags, StatChanges};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

async fn health() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

#[derive(Serialize)]
struct FileInfoResponse {
    path: String,
    size: u64,
    mode: u32,
    is_dir: bool,
    mtime: Option<u64>,
}

fn system_time_to_epoch(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

type AppResult<T> = Result<T, (StatusCode, String)>;

fn map_err(e: FsError) -> (StatusCode, String) {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, e.to_string())
}

async fn stat(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<FileInfoResponse>> {
    let info = state.vfs.stat(&q.path).await.map_err(map_err)?;
    let is_dir = info.is_dir();
    Ok(Json(FileInfoResponse {
        path: info.path,
        size: info.size,
        mode: info.mode,
        is_dir,
        mtime: Some(system_time_to_epoch(info.mtime)),
    }))
}

#[derive(Deserialize)]
struct WstatRequest {
    path: String,
    #[serde(default)]
    mode: Option<u32>,
    #[serde(default)]
    size: Option<u64>,
}

async fn wstat(
    State(state): State<Arc<TestAppState>>,
    Json(req): Json<WstatRequest>,
) -> AppResult<StatusCode> {
    let changes = StatChanges {
        mode: req.mode,
        size: req.size,
        ..Default::default()
    };
    state.vfs.wstat(&req.path, changes).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct FsStatsResponse {
    total_bytes: u64,
    free_bytes: u64,
    block_size: u64,
}

async fn statfs(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<FsStatsResponse>> {
    let stats = state.vfs.statfs(&q.path).await.map_err(map_err)?;
    Ok(Json(FsStatsResponse {
        total_bytes: stats.total_bytes,
        free_bytes: stats.free_bytes,
        block_size: u64::from(stats.block_size),
    }))
}

#[derive(Deserialize)]
struct OpenRequest {
    path: String,
    #[serde(default)]
    flags: u32,
}

#[derive(Serialize)]
struct OpenResponse {
    handle_id: String,
}

fn parse_open_flags(bits: u32) -> OpenFlags {
    // Simplified parsing: 
    // 0x00 = read, 0x01 = write, 0x02 = rdwr
    // 0x40 = creat, 0x200 = trunc
    let read = (bits & 0x03) != 0x01;  // not write-only
    let write = (bits & 0x03) != 0x00; // not read-only
    let create = (bits & 0x40) != 0 || (bits & 0x200) != 0;
    let truncate = (bits & 0x200) != 0;
    
    OpenFlags {
        read,
        write,
        create,
        truncate,
        append: false,
        directory: false,
    }
}

async fn open(
    State(state): State<Arc<TestAppState>>,
    Json(req): Json<OpenRequest>,
) -> AppResult<Json<OpenResponse>> {
    let flags = parse_open_flags(req.flags);
    let handle = state.vfs.open(&req.path, flags).await.map_err(map_err)?;

    let uuid = uuid::Uuid::new_v4().to_string();
    state.handle_map.write().await.insert(uuid.clone(), handle.id());

    Ok(Json(OpenResponse { handle_id: uuid }))
}

#[derive(Deserialize)]
struct ReadRequest {
    handle_id: String,
    offset: u64,
    size: usize,
}

async fn read(
    State(state): State<Arc<TestAppState>>,
    Json(req): Json<ReadRequest>,
) -> AppResult<Vec<u8>> {
    let handle_id = state
        .handle_map
        .read()
        .await
        .get_id(&req.handle_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid handle".to_string()))?;

    let data = state
        .vfs
        .read(&Handle::new(handle_id), req.offset, req.size)
        .await
        .map_err(map_err)?;

    Ok(data.to_vec())
}

#[derive(Deserialize)]
struct WriteQuery {
    handle_id: String,
    offset: u64,
}

#[derive(Serialize)]
struct WriteResponse {
    bytes_written: usize,
}

async fn write(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<WriteQuery>,
    body: axum::body::Bytes,
) -> AppResult<Json<WriteResponse>> {
    let handle_id = state
        .handle_map
        .read()
        .await
        .get_id(&q.handle_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid handle".to_string()))?;

    let bytes_written = state
        .vfs
        .write(&Handle::new(handle_id), q.offset, body)
        .await
        .map_err(map_err)?;

    Ok(Json(WriteResponse { bytes_written }))
}

#[derive(Deserialize)]
struct CloseRequest {
    handle_id: String,
    #[serde(default)]
    sync: bool,
}

async fn close(
    State(state): State<Arc<TestAppState>>,
    Json(req): Json<CloseRequest>,
) -> AppResult<StatusCode> {
    let handle_id = state
        .handle_map
        .write()
        .await
        .remove_by_uuid(&req.handle_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid handle".to_string()))?;

    state
        .vfs
        .close(Handle::new(handle_id), req.sync)
        .await
        .map_err(map_err)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn readdir(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<Vec<FileInfoResponse>>> {
    let entries = state.vfs.readdir(&q.path).await.map_err(map_err)?;
    Ok(Json(
        entries
            .into_iter()
            .map(|info| {
                let is_dir = info.is_dir();
                FileInfoResponse {
                    path: info.path,
                    size: info.size,
                    mode: info.mode,
                    is_dir,
                    mtime: Some(system_time_to_epoch(info.mtime)),
                }
            })
            .collect(),
    ))
}

async fn remove(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<PathQuery>,
) -> AppResult<StatusCode> {
    state.vfs.remove(&q.path).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct CapabilitiesResponse {
    flags: u64,
}

async fn capabilities(
    State(state): State<Arc<TestAppState>>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<CapabilitiesResponse>> {
    // Get capabilities from the mount for this path
    let caps = state
        .mount_table
        .get_mount_info(&q.path)
        .await
        .map(|(_, caps)| caps.bits())
        .unwrap_or(0);
    Ok(Json(CapabilitiesResponse { flags: caps }))
}

#[derive(Serialize)]
struct MountInfoResponse {
    path: String,
    name: String,
}

async fn list_mounts(State(state): State<Arc<TestAppState>>) -> Json<Vec<MountInfoResponse>> {
    let mounts = state.mount_table.list_mounts().await;
    Json(
        mounts
            .into_iter()
            .map(|m| MountInfoResponse {
                path: m.path,
                name: m.provider_name,
            })
            .collect(),
    )
}
