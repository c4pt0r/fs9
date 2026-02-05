use axum::{
    body::Bytes,
    extract::{Extension, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use fs9_sdk::{FsError, FsProvider, Handle};
use std::sync::Arc;

use crate::api::models::*;
use crate::auth::RequestContext;
use crate::namespace::Namespace;
use crate::state::AppState;

pub type AppResult<T> = Result<T, AppError>;

pub enum AppError {
    Fs(FsError),
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
async fn resolve_ns(state: &AppState, ctx: &RequestContext) -> Result<Arc<Namespace>, AppError> {
    state.namespace_manager.get(&ctx.ns).await
        .ok_or_else(|| AppError::forbidden("Namespace not found or access denied"))
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
    let handle = ns.vfs.open(&req.path, req.flags.into()).await?;
    let metadata = ns.vfs.stat(&req.path).await?;

    let uuid = uuid::Uuid::new_v4().to_string();
    ns.handle_map.write().await.insert(uuid.clone(), handle.id());

    Ok(Json(OpenResponse {
        handle_id: uuid,
        metadata: metadata.into(),
    }))
}

pub async fn read(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<ReadRequest>,
) -> AppResult<impl IntoResponse> {
    let ns = resolve_ns(&state, &ctx).await?;
    let handle_id = ns
        .handle_map
        .read()
        .await
        .get_id(&req.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    let data = ns.vfs.read(&Handle::new(handle_id), req.offset, req.size).await?;
    Ok((StatusCode::OK, data))
}

pub async fn write(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<WriteQuery>,
    body: Bytes,
) -> AppResult<Json<WriteResponse>> {
    let ns = resolve_ns(&state, &ctx).await?;
    let handle_id = ns
        .handle_map
        .read()
        .await
        .get_id(&query.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    let bytes_written = ns.vfs.write(&Handle::new(handle_id), query.offset, body).await?;
    Ok(Json(WriteResponse { bytes_written }))
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
        .remove_by_uuid(&req.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    ns.vfs.close(Handle::new(handle_id), req.sync).await?;
    Ok(StatusCode::NO_CONTENT)
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

pub async fn health() -> &'static str {
    "OK"
}

/// POST /api/v1/plugin/load — load a plugin (admin only).
pub async fn load_plugin(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<LoadPluginRequest>,
) -> AppResult<Json<LoadPluginResponse>> {
    require_role(&ctx, &["admin"])?;

    use std::path::Path;

    state
        .plugin_manager
        .load(&req.name, Path::new(&req.path))
        .map_err(|e| FsError::internal(e.to_string()))?;

    Ok(Json(LoadPluginResponse {
        name: req.name,
        status: "loaded".to_string(),
    }))
}

/// POST /api/v1/plugin/unload — unload a plugin (admin only).
pub async fn unload_plugin(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<UnloadPluginRequest>,
) -> AppResult<StatusCode> {
    require_role(&ctx, &["admin"])?;

    state
        .plugin_manager
        .unload(&req.name)
        .map_err(|e| FsError::internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/plugin/list — list loaded plugins (operator or admin).
pub async fn list_plugins(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
) -> AppResult<Json<Vec<String>>> {
    require_role(&ctx, &["operator", "admin"])?;

    Ok(Json(state.plugin_manager.loaded_plugins()))
}

/// POST /api/v1/mount — mount a plugin into a namespace (operator or admin).
pub async fn mount_plugin(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<MountPluginRequest>,
) -> AppResult<Json<MountResponse>> {
    require_role(&ctx, &["operator", "admin"])?;

    let ns = resolve_ns(&state, &ctx).await?;
    let config = serde_json::to_string(&req.config).unwrap_or_default();

    let provider = state
        .plugin_manager
        .create_provider(&req.provider, &config)
        .map_err(|e| FsError::internal(e.to_string()))?;

    ns.mount_table
        .mount(&req.path, &req.provider, std::sync::Arc::new(provider))
        .await
        .map_err(|e| FsError::internal(e.to_string()))?;

    Ok(Json(MountResponse {
        path: req.path,
        provider_name: req.provider,
    }))
}

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
