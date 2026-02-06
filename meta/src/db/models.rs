//! Database models for fs9-meta service.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Namespace (tenant) record.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Namespace {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Mount configuration record.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Mount {
    pub id: String,
    pub namespace_id: String,
    pub path: String,
    pub provider: String,
    pub config: Option<String>, // JSON string
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// User record.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: Option<String>,
    pub email: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// User-Namespace role mapping.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserRole {
    pub id: String,
    pub user_id: String,
    pub namespace_id: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// API Key record.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub namespace_id: String,
    pub name: String,
    pub key_hash: String,
    pub roles: String, // JSON array
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    pub id: String,
    pub namespace: Option<String>,
    pub user_id: Option<String>,
    pub action: String,
    pub resource: Option<String>,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Request/Response DTOs
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateNamespaceRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct NamespaceResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

impl From<Namespace> for NamespaceResponse {
    fn from(ns: Namespace) -> Self {
        Self {
            id: ns.id,
            name: ns.name,
            status: ns.status,
            created_at: ns.created_at,
            created_by: ns.created_by,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateMountRequest {
    pub path: String,
    pub provider: String,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct MountResponse {
    pub id: String,
    pub namespace_id: String,
    pub path: String,
    pub provider: String,
    pub config: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

impl From<Mount> for MountResponse {
    fn from(m: Mount) -> Self {
        Self {
            id: m.id,
            namespace_id: m.namespace_id,
            path: m.path,
            provider: m.provider,
            config: m.config.and_then(|s| serde_json::from_str(&s).ok()),
            created_at: m.created_at,
            created_by: m.created_by,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl From<User> for UserResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            email: u.email,
            status: u.status,
            created_at: u.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AssignRoleRequest {
    pub namespace: String,
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct UserRoleResponse {
    pub namespace: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub namespace: String,
    pub name: String,
    #[serde(default)]
    pub roles: Vec<String>,
    pub expires_in_days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub key: String, // Only returned on creation
    pub name: String,
    pub namespace: String,
    pub roles: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub roles: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
}

#[derive(Debug, Deserialize)]
pub struct ValidateTokenRequest {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct ValidateTokenResponse {
    pub valid: bool,
    pub user_id: Option<String>,
    pub namespace: Option<String>,
    pub roles: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateTokenRequest {
    pub user_id: String,
    pub namespace: String,
    #[serde(default)]
    pub roles: Vec<String>,
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct GenerateTokenResponse {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateApiKeyRequest {
    pub key: String,
}
