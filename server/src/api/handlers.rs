use axum::{
    body::Bytes,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use fs9_sdk::{FsError, FsProvider, Handle};
use std::sync::Arc;

use crate::api::models::*;
use crate::state::AppState;

pub type AppResult<T> = Result<T, AppError>;

pub struct AppError(FsError);

impl From<FsError> for AppError {
    fn from(err: FsError) -> Self {
        Self(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(ErrorResponse {
            error: self.0.to_string(),
            code: self.0.http_status(),
        });
        (status, body).into_response()
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct PathQuery {
    pub path: String,
}

pub async fn stat(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<FileInfoResponse>> {
    let info = state.vfs.stat(&query.path).await?;
    Ok(Json(info.into()))
}

pub async fn wstat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WstatRequest>,
) -> AppResult<StatusCode> {
    state.vfs.wstat(&req.path, req.changes.into()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn statfs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<FsStatsResponse>> {
    let stats = state.vfs.statfs(&query.path).await?;
    Ok(Json(stats.into()))
}

pub async fn open(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenRequest>,
) -> AppResult<Json<OpenResponse>> {
    let handle = state.vfs.open(&req.path, req.flags.into()).await?;
    let metadata = state.vfs.stat(&req.path).await?;

    let uuid = uuid::Uuid::new_v4().to_string();
    state.handle_map.write().await.insert(uuid.clone(), handle.id());

    Ok(Json(OpenResponse {
        handle_id: uuid,
        metadata: metadata.into(),
    }))
}

pub async fn read(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReadRequest>,
) -> AppResult<impl IntoResponse> {
    let handle_id = state
        .handle_map
        .read()
        .await
        .get_id(&req.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    let data = state.vfs.read(&Handle::new(handle_id), req.offset, req.size).await?;
    Ok((StatusCode::OK, data))
}

pub async fn write(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WriteQuery>,
    body: Bytes,
) -> AppResult<Json<WriteResponse>> {
    let handle_id = state
        .handle_map
        .read()
        .await
        .get_id(&query.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    let bytes_written = state.vfs.write(&Handle::new(handle_id), query.offset, body).await?;
    Ok(Json(WriteResponse { bytes_written }))
}

#[derive(Debug, serde::Deserialize)]
pub struct WriteQuery {
    pub handle_id: String,
    pub offset: u64,
}

pub async fn close(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CloseRequest>,
) -> AppResult<StatusCode> {
    let handle_id = state
        .handle_map
        .write()
        .await
        .remove_by_uuid(&req.handle_id)
        .ok_or_else(|| FsError::invalid_argument("invalid handle_id"))?;

    state.vfs.close(Handle::new(handle_id), req.sync).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn readdir(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<Vec<FileInfoResponse>>> {
    let entries = state.vfs.readdir(&query.path).await?;
    Ok(Json(entries.into_iter().map(Into::into).collect()))
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> AppResult<StatusCode> {
    state.vfs.remove(&query.path).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn capabilities(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<CapabilitiesResponse>> {
    let info = state.mount_table.get_mount_info(&query.path).await;

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
) -> Json<Vec<MountResponse>> {
    let mounts = state.mount_table.list_mounts().await;
    Json(
        mounts
            .into_iter()
            .map(|m| MountResponse {
                path: m.path,
                provider_name: m.provider_name,
            })
            .collect(),
    )
}

pub async fn health() -> &'static str {
    "OK"
}
