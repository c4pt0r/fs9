//! `SQLite` database implementation for fs9-meta service.

#![allow(clippy::missing_errors_doc)]

use chrono::{DateTime, Utc};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use uuid::Uuid;

use super::models::{ApiKey, Mount, Namespace, User, UserRole};
use super::Result;
use crate::error::MetaError;

/// SQLite-backed metadata store.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Connect to `SQLite` database.
    pub async fn connect(dsn: &str) -> Result<Self> {
        let url = if dsn.starts_with("sqlite:") {
            dsn.to_string()
        } else {
            format!("sqlite:{dsn}")
        };

        // Ensure create-if-not-exists mode for SQLite
        let url = if url.contains("mode=") {
            url
        } else if url.contains('?') {
            format!("{url}&mode=rwc")
        } else {
            format!("{url}?mode=rwc")
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .map_err(|e| MetaError::Database(format!("Failed to connect: {e}")))?;

        Ok(Self { pool })
    }

    /// Run database migrations.
    pub async fn migrate(&self) -> Result<()> {
        // SQLx+SQLite generally expects a single statement per prepared query, so we apply each
        // migration statement individually.
        let stmts = [
            r"
            CREATE TABLE IF NOT EXISTS namespaces (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                updated_at TEXT,
                deleted_at TEXT
            )
            ",
            r"
            CREATE TABLE IF NOT EXISTS mounts (
                id TEXT PRIMARY KEY,
                namespace_id TEXT NOT NULL,
                path TEXT NOT NULL,
                provider TEXT NOT NULL,
                config TEXT,
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                FOREIGN KEY (namespace_id) REFERENCES namespaces(id) ON DELETE CASCADE,
                UNIQUE(namespace_id, path)
            )
            ",
            r"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                password_hash TEXT,
                email TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL,
                updated_at TEXT
            )
            ",
            r"
            CREATE TABLE IF NOT EXISTS user_roles (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                namespace_id TEXT NOT NULL,
                role TEXT NOT NULL,
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (namespace_id) REFERENCES namespaces(id) ON DELETE CASCADE,
                UNIQUE(user_id, namespace_id, role)
            )
            ",
            r"
            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                namespace_id TEXT NOT NULL,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL,
                roles TEXT NOT NULL,
                expires_at TEXT,
                last_used_at TEXT,
                created_at TEXT NOT NULL,
                revoked_at TEXT,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                FOREIGN KEY (namespace_id) REFERENCES namespaces(id) ON DELETE CASCADE
            )
            ",
            r"
            CREATE TABLE IF NOT EXISTS audit_logs (
                id TEXT PRIMARY KEY,
                namespace TEXT,
                user_id TEXT,
                action TEXT NOT NULL,
                resource TEXT,
                details TEXT,
                ip_address TEXT,
                created_at TEXT NOT NULL
            )
            ",
            "CREATE INDEX IF NOT EXISTS idx_namespaces_name ON namespaces(name)",
            "CREATE INDEX IF NOT EXISTS idx_mounts_namespace ON mounts(namespace_id)",
            "CREATE INDEX IF NOT EXISTS idx_users_username ON users(username)",
            "CREATE INDEX IF NOT EXISTS idx_user_roles_user ON user_roles(user_id)",
            "CREATE INDEX IF NOT EXISTS idx_user_roles_namespace ON user_roles(namespace_id)",
            "CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id)",
            "CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash)",
            "CREATE INDEX IF NOT EXISTS idx_audit_logs_namespace ON audit_logs(namespace)",
        ];

        for stmt in stmts {
            sqlx::query(stmt).execute(&self.pool).await?;
        }

        Ok(())
    }

    // ========================================================================
    // Namespace operations
    // ========================================================================

    pub async fn create_namespace(&self, name: &str, created_by: &str) -> Result<Namespace> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            r"
            INSERT INTO namespaces (id, name, status, created_at, created_by)
            VALUES (?, ?, 'active', ?, ?)
            ",
        )
        .bind(&id)
        .bind(name)
        .bind(now.to_rfc3339())
        .bind(created_by)
        .execute(&self.pool)
        .await?;

        Ok(Namespace {
            id,
            name: name.to_string(),
            status: "active".to_string(),
            created_at: now,
            created_by: created_by.to_string(),
            updated_at: None,
            deleted_at: None,
        })
    }

    pub async fn get_namespace(&self, name: &str) -> Result<Option<Namespace>> {
        let row: Option<NamespaceRow> = sqlx::query_as(
            r"
            SELECT id, name, status, created_at, created_by, updated_at, deleted_at
            FROM namespaces
            WHERE name = ? AND deleted_at IS NULL
            ",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    pub async fn get_namespace_by_id(&self, id: &str) -> Result<Option<Namespace>> {
        let row: Option<NamespaceRow> = sqlx::query_as(
            r"
            SELECT id, name, status, created_at, created_by, updated_at, deleted_at
            FROM namespaces
            WHERE id = ? AND deleted_at IS NULL
            ",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    pub async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let rows: Vec<NamespaceRow> = sqlx::query_as(
            r"
            SELECT id, name, status, created_at, created_by, updated_at, deleted_at
            FROM namespaces
            WHERE deleted_at IS NULL
            ORDER BY name
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn delete_namespace(&self, name: &str) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r"
            UPDATE namespaces
            SET deleted_at = ?, status = 'deleted'
            WHERE name = ? AND deleted_at IS NULL
            ",
        )
        .bind(now.to_rfc3339())
        .bind(name)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(MetaError::NotFound(format!("Namespace '{name}' not found")));
        }

        Ok(())
    }

    // ========================================================================
    // Mount operations
    // ========================================================================

    pub async fn create_mount(
        &self,
        namespace_id: &str,
        path: &str,
        provider: &str,
        config: Option<serde_json::Value>,
        created_by: &str,
    ) -> Result<Mount> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let config_str = config.as_ref().map(ToString::to_string);

        sqlx::query(
            r"
            INSERT INTO mounts (id, namespace_id, path, provider, config, created_at, created_by)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&id)
        .bind(namespace_id)
        .bind(path)
        .bind(provider)
        .bind(&config_str)
        .bind(now.to_rfc3339())
        .bind(created_by)
        .execute(&self.pool)
        .await?;

        Ok(Mount {
            id,
            namespace_id: namespace_id.to_string(),
            path: path.to_string(),
            provider: provider.to_string(),
            config: config_str,
            created_at: now,
            created_by: created_by.to_string(),
        })
    }

    pub async fn get_mount(&self, namespace_id: &str, path: &str) -> Result<Option<Mount>> {
        let row: Option<MountRow> = sqlx::query_as(
            r"
            SELECT id, namespace_id, path, provider, config, created_at, created_by
            FROM mounts
            WHERE namespace_id = ? AND path = ?
            ",
        )
        .bind(namespace_id)
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    pub async fn list_mounts(&self, namespace_id: &str) -> Result<Vec<Mount>> {
        let rows: Vec<MountRow> = sqlx::query_as(
            r"
            SELECT id, namespace_id, path, provider, config, created_at, created_by
            FROM mounts
            WHERE namespace_id = ?
            ORDER BY path
            ",
        )
        .bind(namespace_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn delete_mount(&self, namespace_id: &str, path: &str) -> Result<()> {
        let result = sqlx::query(
            r"
            DELETE FROM mounts
            WHERE namespace_id = ? AND path = ?
            ",
        )
        .bind(namespace_id)
        .bind(path)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(MetaError::NotFound(format!(
                "Mount '{path}' not found in namespace"
            )));
        }

        Ok(())
    }

    // ========================================================================
    // User operations
    // ========================================================================

    pub async fn create_user(
        &self,
        username: &str,
        password_hash: Option<&str>,
        email: Option<&str>,
    ) -> Result<User> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            r"
            INSERT INTO users (id, username, password_hash, email, status, created_at)
            VALUES (?, ?, ?, ?, 'active', ?)
            ",
        )
        .bind(&id)
        .bind(username)
        .bind(password_hash)
        .bind(email)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(User {
            id,
            username: username.to_string(),
            password_hash: password_hash.map(ToString::to_string),
            email: email.map(ToString::to_string),
            status: "active".to_string(),
            created_at: now,
            updated_at: None,
        })
    }

    pub async fn get_user(&self, username: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            r"
            SELECT id, username, password_hash, email, status, created_at, updated_at
            FROM users
            WHERE username = ? AND status = 'active'
            ",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let row: Option<UserRow> = sqlx::query_as(
            r"
            SELECT id, username, password_hash, email, status, created_at, updated_at
            FROM users
            WHERE id = ? AND status = 'active'
            ",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        let rows: Vec<UserRow> = sqlx::query_as(
            r"
            SELECT id, username, password_hash, email, status, created_at, updated_at
            FROM users
            WHERE status = 'active'
            ORDER BY username
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r"
            UPDATE users
            SET status = 'deleted', updated_at = ?
            WHERE id = ? AND status = 'active'
            ",
        )
        .bind(now.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(MetaError::NotFound(format!("User '{id}' not found")));
        }

        Ok(())
    }

    // ========================================================================
    // Role operations
    // ========================================================================

    pub async fn assign_role(
        &self,
        user_id: &str,
        namespace_id: &str,
        role: &str,
        assigned_by: &str,
    ) -> Result<UserRole> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            r"
            INSERT INTO user_roles (id, user_id, namespace_id, role, created_at, created_by)
            VALUES (?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&id)
        .bind(user_id)
        .bind(namespace_id)
        .bind(role)
        .bind(now.to_rfc3339())
        .bind(assigned_by)
        .execute(&self.pool)
        .await?;

        Ok(UserRole {
            id,
            user_id: user_id.to_string(),
            namespace_id: namespace_id.to_string(),
            role: role.to_string(),
            created_at: now,
            created_by: assigned_by.to_string(),
        })
    }

    pub async fn revoke_role(&self, user_id: &str, namespace_id: &str, role: &str) -> Result<()> {
        let result = sqlx::query(
            r"
            DELETE FROM user_roles
            WHERE user_id = ? AND namespace_id = ? AND role = ?
            ",
        )
        .bind(user_id)
        .bind(namespace_id)
        .bind(role)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(MetaError::NotFound("Role assignment not found".into()));
        }

        Ok(())
    }

    pub async fn get_user_roles(&self, user_id: &str) -> Result<Vec<UserRole>> {
        let rows: Vec<UserRoleRow> = sqlx::query_as(
            r"
            SELECT id, user_id, namespace_id, role, created_at, created_by
            FROM user_roles
            WHERE user_id = ?
            ORDER BY namespace_id, role
            ",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_user_roles_for_namespace(
        &self,
        user_id: &str,
        namespace_id: &str,
    ) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r"
            SELECT role
            FROM user_roles
            WHERE user_id = ? AND namespace_id = ?
            ORDER BY role
            ",
        )
        .bind(user_id)
        .bind(namespace_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(r,)| r).collect())
    }

    // ========================================================================
    // API Key operations
    // ========================================================================

    pub async fn create_api_key(
        &self,
        user_id: &str,
        namespace_id: &str,
        name: &str,
        roles: &[String],
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(ApiKey, String)> {
        let id = Uuid::new_v4().to_string();
        let raw_key = format!("fs9_{}", Uuid::new_v4().to_string().replace('-', ""));
        let key_hash = hash_api_key(&raw_key);
        let now = Utc::now();
        let roles_json = serde_json::to_string(roles).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            r"
            INSERT INTO api_keys (id, user_id, namespace_id, name, key_hash, roles, expires_at, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&id)
        .bind(user_id)
        .bind(namespace_id)
        .bind(name)
        .bind(&key_hash)
        .bind(&roles_json)
        .bind(expires_at.map(|d| d.to_rfc3339()))
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        let api_key = ApiKey {
            id,
            user_id: user_id.to_string(),
            namespace_id: namespace_id.to_string(),
            name: name.to_string(),
            key_hash,
            roles: roles_json,
            expires_at,
            last_used_at: None,
            created_at: now,
            revoked_at: None,
        };

        Ok((api_key, raw_key))
    }

    pub async fn validate_api_key(&self, key: &str) -> Result<Option<ApiKey>> {
        let key_hash = hash_api_key(key);

        let row: Option<ApiKeyRow> = sqlx::query_as(
            r"
            SELECT id, user_id, namespace_id, name, key_hash, roles, expires_at, last_used_at, created_at, revoked_at
            FROM api_keys
            WHERE key_hash = ? AND revoked_at IS NULL
            ",
        )
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            let api_key: ApiKey = row.into();

            // Check expiration
            if let Some(expires_at) = api_key.expires_at {
                if expires_at < Utc::now() {
                    return Ok(None);
                }
            }

            Ok(Some(api_key))
        } else {
            Ok(None)
        }
    }

    pub async fn list_api_keys(&self, user_id: &str) -> Result<Vec<ApiKey>> {
        let rows: Vec<ApiKeyRow> = sqlx::query_as(
            r"
            SELECT id, user_id, namespace_id, name, key_hash, roles, expires_at, last_used_at, created_at, revoked_at
            FROM api_keys
            WHERE user_id = ?
            ORDER BY created_at DESC
            ",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn revoke_api_key(&self, key_id: &str) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r"
            UPDATE api_keys
            SET revoked_at = ?
            WHERE id = ? AND revoked_at IS NULL
            ",
        )
        .bind(now.to_rfc3339())
        .bind(key_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(MetaError::NotFound(format!("API key '{key_id}' not found")));
        }

        Ok(())
    }

    pub async fn touch_api_key(&self, key_id: &str) -> Result<()> {
        let now = Utc::now();

        sqlx::query(
            r"
            UPDATE api_keys
            SET last_used_at = ?
            WHERE id = ?
            ",
        )
        .bind(now.to_rfc3339())
        .bind(key_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

/// Hash function for API keys (SHA-256).
fn hash_api_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();

    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        write!(&mut out, "{b:02x}").expect("writing into String shouldn't fail");
    }
    out
}

// ============================================================================
// Row types for SQLite queries (with String dates)
// ============================================================================

#[derive(sqlx::FromRow)]
struct NamespaceRow {
    id: String,
    name: String,
    status: String,
    created_at: String,
    created_by: String,
    updated_at: Option<String>,
    deleted_at: Option<String>,
}

impl From<NamespaceRow> for Namespace {
    fn from(row: NamespaceRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            status: row.status,
            created_at: parse_datetime(&row.created_at),
            created_by: row.created_by,
            updated_at: row.updated_at.as_ref().map(|s| parse_datetime(s)),
            deleted_at: row.deleted_at.as_ref().map(|s| parse_datetime(s)),
        }
    }
}

#[derive(sqlx::FromRow)]
struct MountRow {
    id: String,
    namespace_id: String,
    path: String,
    provider: String,
    config: Option<String>,
    created_at: String,
    created_by: String,
}

impl From<MountRow> for Mount {
    fn from(row: MountRow) -> Self {
        Self {
            id: row.id,
            namespace_id: row.namespace_id,
            path: row.path,
            provider: row.provider,
            config: row.config,
            created_at: parse_datetime(&row.created_at),
            created_by: row.created_by,
        }
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
    password_hash: Option<String>,
    email: Option<String>,
    status: String,
    created_at: String,
    updated_at: Option<String>,
}

impl From<UserRow> for User {
    fn from(row: UserRow) -> Self {
        Self {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            email: row.email,
            status: row.status,
            created_at: parse_datetime(&row.created_at),
            updated_at: row.updated_at.as_ref().map(|s| parse_datetime(s)),
        }
    }
}

#[derive(sqlx::FromRow)]
struct UserRoleRow {
    id: String,
    user_id: String,
    namespace_id: String,
    role: String,
    created_at: String,
    created_by: String,
}

impl From<UserRoleRow> for UserRole {
    fn from(row: UserRoleRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            namespace_id: row.namespace_id,
            role: row.role,
            created_at: parse_datetime(&row.created_at),
            created_by: row.created_by,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: String,
    user_id: String,
    namespace_id: String,
    name: String,
    key_hash: String,
    roles: String,
    expires_at: Option<String>,
    last_used_at: Option<String>,
    created_at: String,
    revoked_at: Option<String>,
}

impl From<ApiKeyRow> for ApiKey {
    fn from(row: ApiKeyRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            namespace_id: row.namespace_id,
            name: row.name,
            key_hash: row.key_hash,
            roles: row.roles,
            expires_at: row.expires_at.as_ref().map(|s| parse_datetime(s)),
            last_used_at: row.last_used_at.as_ref().map(|s| parse_datetime(s)),
            created_at: parse_datetime(&row.created_at),
            revoked_at: row.revoked_at.as_ref().map(|s| parse_datetime(s)),
        }
    }
}

/// Parse ISO 8601 datetime string to `DateTime<Utc>`.
fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_db() -> SqliteStore {
        let store = SqliteStore::connect(":memory:").await.unwrap();
        store.migrate().await.unwrap();
        store
    }

    #[tokio::test]
    async fn test_namespace_crud() {
        let store = setup_test_db().await;

        // Create
        let ns = store.create_namespace("test-ns", "admin").await.unwrap();
        assert_eq!(ns.name, "test-ns");
        assert_eq!(ns.status, "active");

        // Get by name
        let ns2 = store.get_namespace("test-ns").await.unwrap().unwrap();
        assert_eq!(ns2.id, ns.id);

        // Get by id
        let ns3 = store.get_namespace_by_id(&ns.id).await.unwrap().unwrap();
        assert_eq!(ns3.name, "test-ns");

        // List
        let list = store.list_namespaces().await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete
        store.delete_namespace("test-ns").await.unwrap();
        let deleted = store.get_namespace("test-ns").await.unwrap();
        assert!(deleted.is_none());
    }

    #[tokio::test]
    async fn test_namespace_already_exists() {
        let store = setup_test_db().await;

        store.create_namespace("test-ns", "admin").await.unwrap();
        let result = store.create_namespace("test-ns", "admin").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mount_crud() {
        let store = setup_test_db().await;

        let ns = store.create_namespace("test-ns", "admin").await.unwrap();

        // Create mount
        let mount = store
            .create_mount(&ns.id, "/data", "pagefs", Some(serde_json::json!({"uid": 1000})), "admin")
            .await
            .unwrap();
        assert_eq!(mount.path, "/data");
        assert_eq!(mount.provider, "pagefs");

        // Get mount
        let mount2 = store.get_mount(&ns.id, "/data").await.unwrap().unwrap();
        assert_eq!(mount2.id, mount.id);

        // List mounts
        let list = store.list_mounts(&ns.id).await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete mount
        store.delete_mount(&ns.id, "/data").await.unwrap();
        let deleted = store.get_mount(&ns.id, "/data").await.unwrap();
        assert!(deleted.is_none());
    }

    #[tokio::test]
    async fn test_user_crud() {
        let store = setup_test_db().await;

        // Create user
        let user = store
            .create_user("alice", Some("hashed_pw"), Some("alice@example.com"))
            .await
            .unwrap();
        assert_eq!(user.username, "alice");

        // Get by username
        let user2 = store.get_user("alice").await.unwrap().unwrap();
        assert_eq!(user2.id, user.id);

        // Get by id
        let user3 = store.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(user3.username, "alice");

        // List users
        let list = store.list_users().await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete user
        store.delete_user(&user.id).await.unwrap();
        let deleted = store.get_user("alice").await.unwrap();
        assert!(deleted.is_none());
    }

    #[tokio::test]
    async fn test_role_operations() {
        let store = setup_test_db().await;

        let ns = store.create_namespace("test-ns", "admin").await.unwrap();
        let user = store.create_user("alice", None, None).await.unwrap();

        // Assign role
        let role = store
            .assign_role(&user.id, &ns.id, "operator", "admin")
            .await
            .unwrap();
        assert_eq!(role.role, "operator");

        // Get user roles
        let roles = store.get_user_roles(&user.id).await.unwrap();
        assert_eq!(roles.len(), 1);

        // Get roles for namespace
        let ns_roles = store
            .get_user_roles_for_namespace(&user.id, &ns.id)
            .await
            .unwrap();
        assert_eq!(ns_roles, vec!["operator"]);

        // Revoke role
        store.revoke_role(&user.id, &ns.id, "operator").await.unwrap();
        let roles_after = store.get_user_roles(&user.id).await.unwrap();
        assert!(roles_after.is_empty());
    }

    #[tokio::test]
    async fn test_api_key_operations() {
        let store = setup_test_db().await;

        let ns = store.create_namespace("test-ns", "admin").await.unwrap();
        let user = store.create_user("alice", None, None).await.unwrap();

        // Create API key
        let (api_key, raw_key) = store
            .create_api_key(&user.id, &ns.id, "my-key", &["read-only".to_string()], None)
            .await
            .unwrap();
        assert_eq!(api_key.name, "my-key");
        assert!(raw_key.starts_with("fs9_"));

        // Validate API key
        let validated = store.validate_api_key(&raw_key).await.unwrap().unwrap();
        assert_eq!(validated.id, api_key.id);

        // Touch API key
        store.touch_api_key(&api_key.id).await.unwrap();

        // List API keys
        let list = store.list_api_keys(&user.id).await.unwrap();
        assert_eq!(list.len(), 1);

        // Revoke API key
        store.revoke_api_key(&api_key.id).await.unwrap();
        let revoked = store.validate_api_key(&raw_key).await.unwrap();
        assert!(revoked.is_none());
    }
}
