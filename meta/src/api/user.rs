//! User API handlers.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::db::models::{AssignRoleRequest, CreateUserRequest, UserResponse, UserRoleResponse};
use crate::error::MetaError;
use crate::AppState;

/// Create a new user.
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserResponse>, MetaError> {
    // Hash password if provided
    let password_hash = req.password.as_ref().map(|p| hash_password(p));

    let user = state
        .store
        .create_user(
            &req.username,
            password_hash.as_deref(),
            req.email.as_deref(),
        )
        .await?;
    Ok(Json(user.into()))
}

/// List all users.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<UserResponse>>, MetaError> {
    let users = state.store.list_users().await?;
    Ok(Json(users.into_iter().map(Into::into).collect()))
}

/// Get a user by username.
pub async fn get(
    State(state): State<AppState>,
    Path(username): Path<String>,
) -> Result<Json<UserResponse>, MetaError> {
    let user = state
        .store
        .get_user(&username)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("User '{username}' not found")))?;
    Ok(Json(user.into()))
}

/// Delete a user.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, MetaError> {
    state.store.delete_user(&id).await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// Assign a role to a user.
pub async fn assign_role(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    Json(req): Json<AssignRoleRequest>,
) -> Result<Json<UserRoleResponse>, MetaError> {
    // Verify user exists
    state
        .store
        .get_user_by_id(&user_id)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("User '{user_id}' not found")))?;

    // Get namespace ID
    let ns = state
        .store
        .get_namespace(&req.namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{}' not found", req.namespace)))?;

    // TODO: Get assigner from auth context
    let assigned_by = "system";

    let role = state
        .store
        .assign_role(&user_id, &ns.id, &req.role, assigned_by)
        .await?;

    Ok(Json(UserRoleResponse {
        namespace: req.namespace,
        role: role.role,
        created_at: role.created_at,
    }))
}

/// Get roles for a user.
pub async fn get_roles(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Result<Json<Vec<UserRoleResponse>>, MetaError> {
    // Verify user exists
    state
        .store
        .get_user_by_id(&user_id)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("User '{user_id}' not found")))?;

    let roles = state.store.get_user_roles(&user_id).await?;

    // Resolve namespace names
    let mut responses = Vec::new();
    for role in roles {
        if let Some(ns) = state.store.get_namespace_by_id(&role.namespace_id).await? {
            responses.push(UserRoleResponse {
                namespace: ns.name,
                role: role.role,
                created_at: role.created_at,
            });
        }
    }

    Ok(Json(responses))
}

/// Revoke a role from a user.
pub async fn revoke_role(
    State(state): State<AppState>,
    Path((user_id, namespace, role)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, MetaError> {
    // Get namespace ID
    let ns = state
        .store
        .get_namespace(&namespace)
        .await?
        .ok_or_else(|| MetaError::NotFound(format!("Namespace '{namespace}' not found")))?;

    state.store.revoke_role(&user_id, &ns.id, &role).await?;
    Ok(Json(serde_json::json!({"revoked": true})))
}

/// Password hashing using Argon2id.
fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    use rand::rngs::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("password hashing should not fail")
        .to_string()
}
