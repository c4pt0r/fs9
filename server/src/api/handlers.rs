use axum::{
    body::{Body, Bytes},
    extract::{Extension, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use fs9_sdk::{FsError, FsProvider, Handle, OpenFlags};
use futures::stream;
use futures::StreamExt;
use std::sync::Arc;

use crate::api::models::*;
use crate::auth::RequestContext;
use crate::meta_client::MetaClient;
use crate::namespace::Namespace;
use crate::state::AppState;

pub type AppResult<T> = Result<T, AppError>;

pub enum AppError {
    Fs(FsError),
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    Conflict(String),
    NotFound(String),
}

impl From<FsError> for AppError {
    fn from(err: FsError) -> Self {
        Self::Fs(err)
    }
}

impl AppError {
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Fs(e) => {
                let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let body = Json(ErrorResponse {
                    error: e.to_string(),
                    code: e.http_status(),
                });
                (status, body).into_response()
            }
            Self::Unauthorized(msg) => {
                let body = Json(ErrorResponse {
                    error: msg,
                    code: 401,
                });
                (StatusCode::UNAUTHORIZED, body).into_response()
            }
            Self::Forbidden(msg) => {
                let body = Json(ErrorResponse {
                    error: msg,
                    code: 403,
                });
                (StatusCode::FORBIDDEN, body).into_response()
            }
            Self::BadRequest(msg) => {
                let body = Json(ErrorResponse {
                    error: msg,
                    code: 400,
                });
                (StatusCode::BAD_REQUEST, body).into_response()
            }
            Self::Conflict(msg) => {
                let body = Json(ErrorResponse {
                    error: msg,
                    code: 409,
                });
                (StatusCode::CONFLICT, body).into_response()
            }
            Self::NotFound(msg) => {
                let body = Json(ErrorResponse {
                    error: msg,
                    code: 404,
                });
                (StatusCode::NOT_FOUND, body).into_response()
            }
        }
    }
}

/// Resolve the namespace for this request from the RequestContext.
/// If the namespace is not found locally but meta_client is configured,
/// lazily load it from fs9-meta (namespace + mounts).
/// If the namespace doesn't exist in meta but db9_client is configured,
/// auto-provision it with a pagefs mount.
async fn resolve_ns(state: &AppState, ctx: &RequestContext) -> Result<Arc<Namespace>, AppError> {
    // Fast path: namespace already exists in memory
    if let Some(ns) = state.namespace_manager.get(&ctx.ns).await {
        return Ok(ns);
    }

    // Slow path: try to load from meta
    let meta_client = state.meta_client.as_ref().ok_or_else(|| {
        AppError::forbidden("Namespace not found or access denied")
    })?;

    // Try to get namespace from meta
    let needs_provision = match meta_client.get_namespace(&ctx.ns).await {
        Ok(ns_info) => {
            if ns_info.status != "active" {
                return Err(AppError::forbidden("Namespace is not active"));
            }
            false
        }
        Err(_) if state.db9_client.is_some() && state.default_pagefs.is_some() => {
            // Namespace not in meta, but db9 auth is configured — auto-provision
            true
        }
        Err(e) => {
            tracing::warn!(ns = %ctx.ns, error = %e, "Failed to fetch namespace from meta");
            return Err(AppError::forbidden("Namespace not found or access denied"));
        }
    };

    if needs_provision {
        auto_provision_namespace(state, meta_client, &ctx.ns).await?;
    }

    // Create namespace locally
    let ns = state.namespace_manager.get_or_create(&ctx.ns).await;

    // Fetch and apply mounts from meta
    load_mounts_from_meta(state, meta_client, &ns, &ctx.ns).await;

    Ok(ns)
}

/// Auto-provision a namespace and pagefs mount in fs9-meta for a db9 tenant.
async fn auto_provision_namespace(
    state: &AppState,
    meta_client: &MetaClient,
    tenant_id: &str,
) -> Result<(), AppError> {
    let pagefs_config = state.default_pagefs.as_ref().unwrap();
    let keyspace = format!("{}{}", pagefs_config.keyspace_prefix, tenant_id);

    tracing::info!(ns = %tenant_id, keyspace = %keyspace, "Auto-provisioning namespace for db9 tenant");

    // Create namespace in meta
    if let Err(e) = meta_client.create_namespace(tenant_id).await {
        tracing::warn!(ns = %tenant_id, error = %e, "Failed to create namespace in meta (may already exist)");
    }

    // Build pagefs mount config with explicit keyspace
    let mount_config = serde_json::json!({
        "backend": {
            "type": "tikv",
            "pd_endpoints": pagefs_config.pd_endpoints,
            "ca_path": pagefs_config.ca_path,
            "cert_path": pagefs_config.cert_path,
            "key_path": pagefs_config.key_path,
            "keyspace": keyspace,
        }
    });

    // Create mount in meta
    if let Err(e) = meta_client
        .create_mount(tenant_id, "/", "pagefs", &mount_config)
        .await
    {
        tracing::warn!(ns = %tenant_id, error = %e, "Failed to create mount in meta (may already exist)");
    }

    Ok(())
}

/// Load mounts from meta and mount them into the namespace.
async fn load_mounts_from_meta(
    state: &AppState,
    meta_client: &MetaClient,
    ns: &Arc<Namespace>,
    ns_name: &str,
) {
    match meta_client.get_namespace_mounts(ns_name).await {
        Ok(mounts) => {
            for mount in mounts {
                let mut config: serde_json::Value = mount.config.clone();
                if let Some(obj) = config.as_object_mut() {
                    obj.insert(
                        "ns".to_string(),
                        serde_json::json!(ns_name),
                    );
                }
                let config_json = serde_json::to_string(&config).unwrap_or_default();

                match state
                    .plugin_manager
                    .create_provider(&mount.provider, &config_json)
                {
                    Ok(p) => {
                        let provider: Arc<dyn fs9_sdk::FsProvider> = Arc::new(p);
                        if let Err(e) = ns
                            .mount_table
                            .mount(&mount.path, &mount.provider, provider)
                            .await
                        {
                            tracing::error!(
                                ns = %ns_name, path = %mount.path,
                                error = %e, "Failed to mount from meta config"
                            );
                        } else {
                            tracing::info!(
                                ns = %ns_name, path = %mount.path,
                                provider = %mount.provider,
                                "Lazily mounted from meta"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            ns = %ns_name, provider = %mount.provider,
                            error = %e, "Failed to create provider from meta config"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(ns = %ns_name, error = %e, "Failed to fetch mounts from meta");
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct PathQuery {
    pub path: String,
}

pub async fn stat(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<FileInfoResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let info = ns.vfs.stat(&query.path).await?;
    Ok(Json(info.into()))
}

pub async fn wstat(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<WstatRequest>,
) -> AppResult<StatusCode> {
    let ns = resolve_ns(&state, &ctx).await?;
    ns.vfs.wstat(&req.path, req.changes.into()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn statfs(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<FsStatsResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let stats = ns.vfs.statfs(&query.path).await?;
    Ok(Json(stats.into()))
}

pub async fn open(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<OpenRequest>,
) -> AppResult<Json<OpenResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let (handle, metadata) = ns.vfs.open(&req.path, req.flags.into()).await?;

    let handle_id = handle.id();
    ns.handle_map.write().await.insert(handle_id);

    Ok(Json(OpenResponse {
        handle_id: handle_id.to_string(),
        metadata: metadata.into(),
    }))
}

const STREAM_CHUNK_SIZE: usize = 256 * 1024; // 256KB

pub async fn read(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<ReadRequest>,
) -> AppResult<Response> {
    let ns = resolve_ns(&state, &ctx).await?;
    let handle_id = ns
        .handle_map
        .read()
        .await
        .get_id(&req.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    let total_size = req.size;

    if total_size <= 1024 * 1024 {
        let data = ns
            .vfs
            .read(&Handle::new(handle_id), req.offset, total_size)
            .await?;
        return Ok((StatusCode::OK, data).into_response());
    }

    let vfs = ns.vfs.clone();
    let handle = Handle::new(handle_id);
    let offset = req.offset;
    let end_offset = req.offset + total_size as u64;

    let body_stream = stream::unfold(
        (vfs, handle, offset, end_offset),
        move |(vfs, handle, mut offset, end_offset)| async move {
            if offset >= end_offset {
                return None;
            }
            let remaining = (end_offset - offset) as usize;
            let chunk_size = remaining.min(STREAM_CHUNK_SIZE);

            match vfs.read(&handle, offset, chunk_size).await {
                Ok(data) => {
                    if data.is_empty() {
                        return None;
                    }
                    offset += data.len() as u64;
                    Some((Ok::<_, std::io::Error>(data), (vfs, handle, offset, end_offset)))
                }
                Err(_) => None,
            }
        },
    );

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Transfer-Encoding", "chunked")
        .body(Body::from_stream(body_stream))
        .unwrap())
}

pub async fn write(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<WriteQuery>,
    body: Body,
) -> AppResult<Json<WriteResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let handle_id = ns
        .handle_map
        .read()
        .await
        .get_id(&query.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    let handle = Handle::new(handle_id);
    let mut offset = query.offset;
    let mut total_written: usize = 0;
    let mut stream = body.into_data_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| FsError::internal(e.to_string()))?;
        if chunk.is_empty() {
            continue;
        }
        let written = ns.vfs.write(&handle, offset, chunk).await?;
        offset += written as u64;
        total_written += written;
    }

    Ok(Json(WriteResponse {
        bytes_written: total_written,
    }))
}

#[derive(Debug, serde::Deserialize)]
pub struct WriteQuery {
    pub handle_id: String,
    pub offset: u64,
}

pub async fn close(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CloseRequest>,
) -> AppResult<StatusCode> {
    let ns = resolve_ns(&state, &ctx).await?;
    let handle_id = ns
        .handle_map
        .write()
        .await
        .remove(&req.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    ns.vfs.close(Handle::new(handle_id), req.sync).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Stateless streaming endpoints: download (GET) and upload (PUT)
// =============================================================================

/// Parse an HTTP Range header value.
/// Supports: `bytes=start-end`, `bytes=start-`, `bytes=-suffix`.
fn parse_range_header(range: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    if let Some(suffix) = range.strip_prefix('-') {
        // bytes=-500  →  last 500 bytes
        let suffix_len: u64 = suffix.parse().ok()?;
        if suffix_len == 0 || suffix_len > file_size {
            return None;
        }
        Some((file_size - suffix_len, file_size - 1))
    } else if let Some((start_s, end_s)) = range.split_once('-') {
        let start: u64 = start_s.parse().ok()?;
        if start >= file_size {
            return None;
        }
        if end_s.is_empty() {
            // bytes=100-  →  from 100 to end
            Some((start, file_size - 1))
        } else {
            let end: u64 = end_s.parse().ok()?;
            if end < start || end >= file_size {
                return None;
            }
            Some((start, end))
        }
    } else {
        None
    }
}

/// GET /api/v1/download?path=/foo — stateless file download with Range support.
///
/// Opens the file, streams it in chunks, closes the handle when done.
/// Supports `Range: bytes=start-end` for partial content (206).
pub async fn download(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let ns = resolve_ns(&state, &ctx).await?;

    // Stat to get file size
    let info = ns.vfs.stat(&query.path).await?;
    let file_size = info.size;

    // Open for reading
    let (handle, _metadata) = ns.vfs.open(&query.path, OpenFlags {
        read: true,
        ..Default::default()
    }).await?;
    let handle_id = handle.id();
    ns.handle_map.write().await.insert(handle_id);

    // Parse Range header
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range_header(v, file_size));

    let (start, end, status) = match range {
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => {
            if file_size == 0 {
                // Empty file — close handle and return empty body
                ns.handle_map.write().await.remove(&handle_id.to_string());
                let _ = ns.vfs.close(Handle::new(handle_id), false).await;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, "0")
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(Body::empty())
                    .unwrap());
            }
            (0, file_size - 1, StatusCode::OK)
        }
    };

    let content_length = end - start + 1;

    // Build streaming body
    let vfs = ns.vfs.clone();
    let vfs_close = ns.vfs.clone();
    let handle_map = ns.handle_map.clone();
    let fh = Handle::new(handle_id);
    let end_offset = end + 1;

    let body_stream = stream::unfold(
        (vfs, fh, start, end_offset),
        move |(vfs, handle, offset, end_off)| async move {
            if offset >= end_off {
                return None;
            }
            let remaining = (end_off - offset) as usize;
            let chunk_size = remaining.min(STREAM_CHUNK_SIZE);

            match vfs.read(&handle, offset, chunk_size).await {
                Ok(data) => {
                    if data.is_empty() {
                        return None;
                    }
                    let new_offset = offset + data.len() as u64;
                    Some((Ok::<_, std::io::Error>(data), (vfs, handle, new_offset, end_off)))
                }
                Err(_) => None,
            }
        },
    );

    // Wrap the stream to close handle when done
    let cleanup_handle_id = handle_id;
    let body_stream = body_stream.chain(stream::once(async move {
        // Cleanup: close handle after streaming completes
        handle_map.write().await.remove(&cleanup_handle_id.to_string());
        let _ = vfs_close.close(Handle::new(cleanup_handle_id), false).await;
        // Yield nothing — this is just cleanup
        Ok::<Bytes, std::io::Error>(Bytes::new())
    }).filter(|r| {
        let is_empty = matches!(r, Ok(b) if b.is_empty());
        async move { !is_empty }
    }));

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .header(header::ACCEPT_RANGES, "bytes");

    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end, file_size),
        );
    }

    Ok(builder.body(Body::from_stream(body_stream)).unwrap())
}

/// PUT /api/v1/upload?path=/foo — stateless streaming file upload.
///
/// Creates/truncates the file, streams the request body in chunks, closes when done.
pub async fn upload(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
    body: Body,
) -> AppResult<Json<UploadResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;

    // Open for create+truncate+write
    let (handle, _metadata) = ns.vfs.open(&query.path, OpenFlags {
        read: true,
        write: true,
        create: true,
        truncate: true,
        ..Default::default()
    }).await?;
    let handle_id = handle.id();
    ns.handle_map.write().await.insert(handle_id);

    // Stream body chunks into provider
    let fh = Handle::new(handle_id);
    let mut offset: u64 = 0;
    let mut total_written: usize = 0;
    let mut stream = body.into_data_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            // On error, try to close the handle
            FsError::internal(e.to_string())
        })?;
        if chunk.is_empty() {
            continue;
        }
        let written = ns.vfs.write(&fh, offset, chunk).await?;
        offset += written as u64;
        total_written += written;
    }

    // Close handle
    ns.handle_map.write().await.remove(&handle_id.to_string());
    ns.vfs.close(Handle::new(handle_id), true).await?;

    Ok(Json(UploadResponse {
        path: query.path,
        bytes_written: total_written,
    }))
}

pub async fn readdir(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<Vec<FileInfoResponse>>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let entries = ns.vfs.readdir(&query.path).await?;
    Ok(Json(entries.into_iter().map(Into::into).collect()))
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
) -> AppResult<StatusCode> {
    let ns = resolve_ns(&state, &ctx).await?;
    ns.vfs.remove(&query.path).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn capabilities(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<CapabilitiesResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let info = ns.mount_table.get_mount_info(&query.path).await;

    match info {
        Some((mount, caps)) => {
            let mut cap_list = Vec::new();
            if caps.supports_read() { cap_list.push("read".to_string()); }
            if caps.supports_write() { cap_list.push("write".to_string()); }
            if caps.supports_create() { cap_list.push("create".to_string()); }
            if caps.supports_delete() { cap_list.push("delete".to_string()); }
            if caps.supports_rename() { cap_list.push("rename".to_string()); }
            if caps.supports_truncate() { cap_list.push("truncate".to_string()); }
            if caps.supports_chmod() { cap_list.push("chmod".to_string()); }
            if caps.supports_chown() { cap_list.push("chown".to_string()); }
            if caps.supports_symlink() { cap_list.push("symlink".to_string()); }
            if caps.supports_directories() { cap_list.push("directory".to_string()); }

            Ok(Json(CapabilitiesResponse {
                capabilities: cap_list,
                provider_type: mount.provider_name,
            }))
        }
        None => Err(FsError::not_found(&query.path).into()),
    }
}

pub async fn list_mounts(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
) -> AppResult<Json<Vec<MountResponse>>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let mounts = ns.mount_table.list_mounts().await;
    Ok(Json(
        mounts
            .into_iter()
            .map(|m| MountResponse {
                path: m.path,
                provider_name: m.provider_name,
            })
            .collect(),
    ))
}

pub async fn health() -> Json<HealthResponse> {
    use std::sync::LazyLock;

    static INSTANCE_ID: LazyLock<String> =
        LazyLock::new(|| uuid::Uuid::new_v4().to_string()[..8].to_string());

    Json(HealthResponse {
        status: "ok".to_string(),
        instance_id: INSTANCE_ID.clone(),
    })
}

/// POST /api/v1/auth/refresh — refresh a JWT token
/// Accepts an expired token (within grace period) and returns a new token.
/// When meta service is configured, delegates to meta for refresh.
pub async fn refresh_token(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> AppResult<Json<RefreshTokenResponse>> {
    use crate::auth::{Claims, JwtConfig};

    let ttl_secs: u64 = 86400; // 24 hours default
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| AppError::Unauthorized("Missing Authorization header".to_string()))?;

    // If meta_client is configured, try to use it for refresh
    if let Some(meta_client) = &state.meta_client {
        match meta_client.refresh_token(token, Some(ttl_secs)).await {
            Ok(resp) => {
                // Invalidate the old token from cache
                state.token_cache.remove(token).await;

                // Parse expires_at to calculate expires_in
                // The meta service returns ISO8601 timestamp
                let expires_in = ttl_secs; // Use requested TTL as fallback

                return Ok(Json(RefreshTokenResponse {
                    token: resp.token,
                    expires_in,
                }));
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Meta service refresh failed, falling back to local refresh"
                );
                // Fall through to local refresh
            }
        }
    }

    // Fallback: local JWT refresh
    let jwt_secret = state.jwt_secret.read().await;
    if jwt_secret.is_empty() {
        return Err(AppError::BadRequest(
            "Token refresh not configured".to_string(),
        ));
    }

    let config = JwtConfig::new(jwt_secret.clone());
    let old_claims = config
        .decode_allow_expired(token)
        .map_err(|e| AppError::Unauthorized(format!("Invalid token: {e}")))?;
    let ns = old_claims
        .ns
        .clone()
        .ok_or_else(|| AppError::Unauthorized("Token missing required 'ns' claim".to_string()))?;
    let new_claims = Claims::with_namespace(&old_claims.sub, &ns, old_claims.roles.clone(), ttl_secs);
    let new_token = config
        .encode(&new_claims)
        .map_err(|e| AppError::BadRequest(format!("Failed to generate token: {}", e)))?;

    Ok(Json(RefreshTokenResponse {
        token: new_token,
        expires_in: ttl_secs,
    }))
}

/// POST /api/v1/auth/revoke — revoke a token (admin only).
pub async fn revoke_token(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<RevokeTokenRequest>,
) -> AppResult<StatusCode> {
    require_role(&ctx, &["admin"])?;

    state.revocation_set.revoke(&req.token).await;
    state.token_cache.remove(&req.token).await;

    tracing::info!(user = %ctx.user_id, "Token revoked");
    Ok(StatusCode::NO_CONTENT)
}

// NOTE: plugin/load, plugin/unload, plugin/list, and mount endpoints
// have been removed for security reasons. The handler code has been deleted
// and the routes removed from api_v1_routes().

// ============================================================================
// Namespace management API
// ============================================================================

/// Check that the request context has one of the allowed roles.
fn require_role(ctx: &RequestContext, allowed: &[&str]) -> Result<(), AppError> {
    if ctx.roles.iter().any(|r| allowed.contains(&r.as_str())) {
        Ok(())
    } else {
        Err(AppError::forbidden("Insufficient permissions"))
    }
}

/// POST /api/v1/namespaces — create a new namespace (admin only).
pub async fn create_namespace(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateNamespaceRequest>,
) -> AppResult<impl IntoResponse> {
    require_role(&ctx, &["admin"])?;

    match state.namespace_manager.create(&req.name, &ctx.user_id).await {
        Ok(_ns) => {
            let info = state.namespace_manager.get_info(&req.name).await.unwrap();
            Ok((
                StatusCode::CREATED,
                Json(NamespaceInfoResponse {
                    name: info.name,
                    created_at: info.created_at,
                    created_by: info.created_by,
                    status: info.status,
                }),
            ))
        }
        Err(e) if e.contains("already exists") => Err(AppError::Conflict(e)),
        Err(e) => Err(AppError::BadRequest(e)),
    }
}

/// GET /api/v1/namespaces — list all namespaces (admin or operator).
pub async fn list_namespaces(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
) -> AppResult<Json<Vec<NamespaceInfoResponse>>> {
    require_role(&ctx, &["admin", "operator"])?;

    let infos = state.namespace_manager.list_info().await;
    Ok(Json(
        infos
            .into_iter()
            .map(|info| NamespaceInfoResponse {
                name: info.name,
                created_at: info.created_at,
                created_by: info.created_by,
                status: info.status,
            })
            .collect(),
    ))
}

/// GET /api/v1/namespaces/:ns — get a single namespace's info (admin or operator).
pub async fn get_namespace(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    axum::extract::Path(ns_name): axum::extract::Path<String>,
) -> AppResult<Json<NamespaceInfoResponse>> {
    require_role(&ctx, &["admin", "operator"])?;

    match state.namespace_manager.get_info(&ns_name).await {
        Some(info) => Ok(Json(NamespaceInfoResponse {
            name: info.name,
            created_at: info.created_at,
            created_by: info.created_by,
            status: info.status,
        })),
        None => Err(AppError::NotFound(format!(
            "Namespace '{}' not found",
            ns_name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_full() {
        assert_eq!(parse_range_header("bytes=0-499", 1000), Some((0, 499)));
    }

    #[test]
    fn parse_range_open_end() {
        assert_eq!(parse_range_header("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn parse_range_suffix() {
        assert_eq!(parse_range_header("bytes=-200", 1000), Some((800, 999)));
    }

    #[test]
    fn parse_range_entire_file() {
        assert_eq!(parse_range_header("bytes=0-999", 1000), Some((0, 999)));
    }

    #[test]
    fn parse_range_single_byte() {
        assert_eq!(parse_range_header("bytes=0-0", 1000), Some((0, 0)));
    }

    #[test]
    fn parse_range_invalid_start_past_end() {
        assert_eq!(parse_range_header("bytes=1000-", 1000), None);
    }

    #[test]
    fn parse_range_invalid_end_past_file() {
        assert_eq!(parse_range_header("bytes=0-1000", 1000), None);
    }

    #[test]
    fn parse_range_invalid_reversed() {
        assert_eq!(parse_range_header("bytes=500-100", 1000), None);
    }

    #[test]
    fn parse_range_invalid_format() {
        assert_eq!(parse_range_header("chars=0-100", 1000), None);
    }

    #[test]
    fn parse_range_suffix_zero() {
        assert_eq!(parse_range_header("bytes=-0", 1000), None);
    }

    #[test]
    fn parse_range_suffix_too_large() {
        assert_eq!(parse_range_header("bytes=-2000", 1000), None);
    }

    #[test]
    fn parse_range_empty_file() {
        assert_eq!(parse_range_header("bytes=0-", 0), None);
    }
}
