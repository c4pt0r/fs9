//! API Key API handlers.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{Duration, Utc};

use crate::db::models::{ApiKeyResponse, CreateApiKeyRequest, CreateApiKeyResponse, ValidateApiKeyRequest};
use crate::error::MetaError;
use crate::AppState;

/// Create a new API key.
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, MetaError> {
    // Get namespace
    let ns = state
        .store
        .get_namespace(&req.namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{}' not found", req.namespace)))?;

    // TODO: Get user from auth context
    let user_id = "system";

    let expires_at = req
        .expires_in_days
        .map(|days| Utc::now() + Duration::days(days));

    let (api_key, raw_key) = state
        .store
        .create_api_key(user_id, &ns.id, &req.name, &req.roles, expires_at)
        .await?;

    let roles: Vec<String> = serde_json::from_str(&api_key.roles).unwrap_or_default();

    Ok(Json(CreateApiKeyResponse {
        id: api_key.id,
        key: raw_key,
        name: api_key.name,
        namespace: req.namespace,
        roles,
        expires_at: api_key.expires_at,
        created_at: api_key.created_at,
    }))
}

/// List API keys for the current user.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ApiKeyResponse>>, MetaError> {
    // TODO: Get user from auth context
    let user_id = "system";

    let keys = state.store.list_api_keys(user_id).await?;

    let mut responses = Vec::new();
    for key in keys {
        // Resolve namespace name
        let namespace = if let Some(ns) = state.store.get_namespace_by_id(&key.namespace_id).await? {
            ns.name
        } else {
            key.namespace_id.clone()
        };

        let roles: Vec<String> = serde_json::from_str(&key.roles).unwrap_or_default();

        responses.push(ApiKeyResponse {
            id: key.id,
            name: key.name,
            namespace,
            roles,
            expires_at: key.expires_at,
            last_used_at: key.last_used_at,
            created_at: key.created_at,
            revoked: key.revoked_at.is_some(),
        });
    }

    Ok(Json(responses))
}

/// Validate an API key.
pub async fn validate(
    State(state): State<AppState>,
    Json(req): Json<ValidateApiKeyRequest>,
) -> Result<Json<serde_json::Value>, MetaError> {
    let api_key = state.store.validate_api_key(&req.key).await?;

    match api_key {
        Some(key) => {
            // Touch the key to update last_used_at
            let _ = state.store.touch_api_key(&key.id).await;

            // Resolve namespace name
            let namespace = if let Some(ns) = state.store.get_namespace_by_id(&key.namespace_id).await? {
                ns.name
            } else {
                key.namespace_id.clone()
            };

            let roles: Vec<String> = serde_json::from_str(&key.roles).unwrap_or_default();

            Ok(Json(serde_json::json!({
                "valid": true,
                "user_id": key.user_id,
                "namespace": namespace,
                "roles": roles,
                "expires_at": key.expires_at,
            })))
        }
        None => Ok(Json(serde_json::json!({
            "valid": false,
            "error": "Invalid or expired API key"
        }))),
    }
}

/// Revoke an API key.
pub async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, MetaError> {
    state.store.revoke_api_key(&id).await?;
    Ok(Json(serde_json::json!({"revoked": true})))
}
