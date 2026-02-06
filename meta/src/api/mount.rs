//! Mount API handlers.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::db::models::{CreateMountRequest, MountResponse};
use crate::error::MetaError;
use crate::AppState;

/// Create a new mount.
pub async fn create(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    Json(req): Json<CreateMountRequest>,
) -> Result<Json<MountResponse>, MetaError> {
    // Get namespace ID
    let ns = state
        .store
        .get_namespace(&namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{namespace}' not found")))?;

    // TODO: Get creator from auth context
    let created_by = "system";

    let mount = state
        .store
        .create_mount(&ns.id, &req.path, &req.provider, req.config, created_by)
        .await?;
    Ok(Json(mount.into()))
}

/// List mounts in a namespace.
pub async fn list(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
) -> Result<Json<Vec<MountResponse>>, MetaError> {
    let ns = state
        .store
        .get_namespace(&namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{namespace}' not found")))?;

    let mounts = state.store.list_mounts(&ns.id).await?;
    Ok(Json(mounts.into_iter().map(Into::into).collect()))
}

/// Get a mount by path.
pub async fn get(
    State(state): State<AppState>,
    Path((namespace, path)): Path<(String, String)>,
) -> Result<Json<MountResponse>, MetaError> {
    let ns = state
        .store
        .get_namespace(&namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{namespace}' not found")))?;

    let mount_path = format!("/{path}");
    let mount = state
        .store
        .get_mount(&ns.id, &mount_path)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Mount '{mount_path}' not found")))?;
    Ok(Json(mount.into()))
}

/// Delete a mount.
pub async fn delete(
    State(state): State<AppState>,
    Path((namespace, path)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, MetaError> {
    let ns = state
        .store
        .get_namespace(&namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{namespace}' not found")))?;

    let mount_path = format!("/{path}");
    state.store.delete_mount(&ns.id, &mount_path).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}
