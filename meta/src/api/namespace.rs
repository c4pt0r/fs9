//! Namespace API handlers.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::db::models::{CreateNamespaceRequest, NamespaceResponse};
use crate::error::MetaError;
use crate::AppState;

/// Create a new namespace.
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateNamespaceRequest>,
) -> Result<Json<NamespaceResponse>, MetaError> {
    // TODO: Get creator from auth context
    let created_by = "system";

    let ns = state.store.create_namespace(&req.name, created_by).await?;
    Ok(Json(ns.into()))
}

/// List all namespaces.
pub async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<NamespaceResponse>>, MetaError> {
    let namespaces = state.store.list_namespaces().await?;
    Ok(Json(namespaces.into_iter().map(Into::into).collect()))
}

/// Get a namespace by name.
pub async fn get(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<NamespaceResponse>, MetaError> {
    let ns = state
        .store
        .get_namespace(&name)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{name}' not found")))?;
    Ok(Json(ns.into()))
}

/// Delete a namespace.
pub async fn delete(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, MetaError> {
    state.store.delete_namespace(&name).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}
