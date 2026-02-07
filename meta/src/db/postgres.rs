//! PostgreSQL database implementation for fs9-meta service.

#![allow(clippy::missing_errors_doc)]

use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

use super::models::{ApiKey, Mount, Namespace, User, UserRole};
use super::Result;
use crate::error::MetaError;

/// PostgreSQL-backed metadata store.
#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connect to PostgreSQL database.
    pub async fn connect(dsn: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(dsn)
            .await
            .map_err(|e| MetaError::Database(format!("Failed to connect to PostgreSQL: {e}")))?;

        Ok(Self { pool })
    }

    /// Run database migrations.
    pub async fn migrate(&self) -> Result<()> {
        // PostgreSQL supports TIMESTAMPTZ natively — no need for TEXT date columns.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS namespaces (
                id TEXT PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                created_by TEXT NOT NULL,
                updated_at TIMESTAMPTZ,
                deleted_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS mounts (
                id TEXT PRIMARY KEY,
                namespace_id TEXT NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                provider TEXT NOT NULL,
                config JSONB,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                created_by TEXT NOT NULL,
                UNIQUE(namespace_id, path)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                password_hash TEXT,
                email TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS user_roles (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                namespace_id TEXT NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                created_by TEXT NOT NULL,
                UNIQUE(user_id, namespace_id, role)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                namespace_id TEXT NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL,
                roles JSONB NOT NULL DEFAULT '[]',
                expires_at TIMESTAMPTZ,
                last_used_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                revoked_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS audit_logs (
                id TEXT PRIMARY KEY,
                namespace TEXT,
                user_id TEXT,
                action TEXT NOT NULL,
                resource TEXT,
                details JSONB,
                ip_address TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Create indexes (IF NOT EXISTS is PG 9.5+)
        let indexes = [
            "CREATE INDEX IF NOT EXISTS idx_pg_namespaces_name ON namespaces(name)",
            "CREATE INDEX IF NOT EXISTS idx_pg_mounts_namespace ON mounts(namespace_id)",
            "CREATE INDEX IF NOT EXISTS idx_pg_users_username ON users(username)",
            "CREATE INDEX IF NOT EXISTS idx_pg_user_roles_user ON user_roles(user_id)",
            "CREATE INDEX IF NOT EXISTS idx_pg_user_roles_namespace ON user_roles(namespace_id)",
            "CREATE INDEX IF NOT EXISTS idx_pg_api_keys_user ON api_keys(user_id)",
            "CREATE INDEX IF NOT EXISTS idx_pg_api_keys_hash ON api_keys(key_hash)",
            "CREATE INDEX IF NOT EXISTS idx_pg_audit_logs_namespace ON audit_logs(namespace)",
        ];

        for idx in indexes {
            sqlx::query(idx).execute(&self.pool).await?;
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
            r#"
            INSERT INTO namespaces (id, name, status, created_at, created_by)
            VALUES ($1, $2, 'active', $3, $4)
            "#,
        )
        .bind(&id)
        .bind(name)
        .bind(now)
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
        let row: Option<Namespace> = sqlx::query_as(
            r#"
            SELECT id, name, status, created_at, created_by, updated_at, deleted_at
            FROM namespaces
            WHERE name = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn get_namespace_by_id(&self, id: &str) -> Result<Option<Namespace>> {
        let row: Option<Namespace> = sqlx::query_as(
            r#"
            SELECT id, name, status, created_at, created_by, updated_at, deleted_at
            FROM namespaces
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let rows: Vec<Namespace> = sqlx::query_as(
            r#"
            SELECT id, name, status, created_at, created_by, updated_at, deleted_at
            FROM namespaces
            WHERE deleted_at IS NULL
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn delete_namespace(&self, name: &str) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE namespaces
            SET deleted_at = $1, status = 'deleted'
            WHERE name = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(now)
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
            r#"
            INSERT INTO mounts (id, namespace_id, path, provider, config, created_at, created_by)
            VALUES ($1, $2, $3, $4, $5::jsonb, $6, $7)
            "#,
        )
        .bind(&id)
        .bind(namespace_id)
        .bind(path)
        .bind(provider)
        .bind(&config_str)
        .bind(now)
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
        let row: Option<PgMountRow> = sqlx::query_as(
            r#"
            SELECT id, namespace_id, path, provider, config::text, created_at, created_by
            FROM mounts
            WHERE namespace_id = $1 AND path = $2
            "#,
        )
        .bind(namespace_id)
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    pub async fn list_mounts(&self, namespace_id: &str) -> Result<Vec<Mount>> {
        let rows: Vec<PgMountRow> = sqlx::query_as(
            r#"
            SELECT id, namespace_id, path, provider, config::text, created_at, created_by
            FROM mounts
            WHERE namespace_id = $1
            ORDER BY path
            "#,
        )
        .bind(namespace_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn delete_mount(&self, namespace_id: &str, path: &str) -> Result<()> {
        let result = sqlx::query(
            r#"
            DELETE FROM mounts
            WHERE namespace_id = $1 AND path = $2
            "#,
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
            r#"
            INSERT INTO users (id, username, password_hash, email, status, created_at)
            VALUES ($1, $2, $3, $4, 'active', $5)
            "#,
        )
        .bind(&id)
        .bind(username)
        .bind(password_hash)
        .bind(email)
        .bind(now)
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
        let row: Option<User> = sqlx::query_as(
            r#"
            SELECT id, username, password_hash, email, status, created_at, updated_at
            FROM users
            WHERE username = $1 AND status = 'active'
            "#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let row: Option<User> = sqlx::query_as(
            r#"
            SELECT id, username, password_hash, email, status, created_at, updated_at
            FROM users
            WHERE id = $1 AND status = 'active'
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        let rows: Vec<User> = sqlx::query_as(
            r#"
            SELECT id, username, password_hash, email, status, created_at, updated_at
            FROM users
            WHERE status = 'active'
            ORDER BY username
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE users
            SET status = 'deleted', updated_at = $1
            WHERE id = $2 AND status = 'active'
            "#,
        )
        .bind(now)
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
            r#"
            INSERT INTO user_roles (id, user_id, namespace_id, role, created_at, created_by)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(namespace_id)
        .bind(role)
        .bind(now)
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
            r#"
            DELETE FROM user_roles
            WHERE user_id = $1 AND namespace_id = $2 AND role = $3
            "#,
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
        let rows: Vec<UserRole> = sqlx::query_as(
            r#"
            SELECT id, user_id, namespace_id, role, created_at, created_by
            FROM user_roles
            WHERE user_id = $1
            ORDER BY namespace_id, role
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn get_user_roles_for_namespace(
        &self,
        user_id: &str,
        namespace_id: &str,
    ) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT role
            FROM user_roles
            WHERE user_id = $1 AND namespace_id = $2
            ORDER BY role
            "#,
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
            r#"
            INSERT INTO api_keys (id, user_id, namespace_id, name, key_hash, roles, expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8)
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(namespace_id)
        .bind(name)
        .bind(&key_hash)
        .bind(&roles_json)
        .bind(expires_at)
        .bind(now)
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

        let row: Option<PgApiKeyRow> = sqlx::query_as(
            r#"
            SELECT id, user_id, namespace_id, name, key_hash, roles::text, expires_at, last_used_at, created_at, revoked_at
            FROM api_keys
            WHERE key_hash = $1 AND revoked_at IS NULL
            "#,
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
        let rows: Vec<PgApiKeyRow> = sqlx::query_as(
            r#"
            SELECT id, user_id, namespace_id, name, key_hash, roles::text, expires_at, last_used_at, created_at, revoked_at
            FROM api_keys
            WHERE user_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn revoke_api_key(&self, key_id: &str) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE api_keys
            SET revoked_at = $1
            WHERE id = $2 AND revoked_at IS NULL
            "#,
        )
        .bind(now)
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
            r#"
            UPDATE api_keys
            SET last_used_at = $1
            WHERE id = $2
            "#,
        )
        .bind(now)
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
// Row types for PostgreSQL queries
// ============================================================================

// Mount needs a custom row type because `config` is JSONB cast to text
#[derive(sqlx::FromRow)]
struct PgMountRow {
    id: String,
    namespace_id: String,
    path: String,
    provider: String,
    config: Option<String>,
    created_at: DateTime<Utc>,
    created_by: String,
}

impl From<PgMountRow> for Mount {
    fn from(row: PgMountRow) -> Self {
        Self {
            id: row.id,
            namespace_id: row.namespace_id,
            path: row.path,
            provider: row.provider,
            config: row.config,
            created_at: row.created_at,
            created_by: row.created_by,
        }
    }
}

// ApiKey needs custom row because `roles` is JSONB cast to text
#[derive(sqlx::FromRow)]
struct PgApiKeyRow {
    id: String,
    user_id: String,
    namespace_id: String,
    name: String,
    key_hash: String,
    roles: String,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

impl From<PgApiKeyRow> for ApiKey {
    fn from(row: PgApiKeyRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            namespace_id: row.namespace_id,
            name: row.name,
            key_hash: row.key_hash,
            roles: row.roles,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            created_at: row.created_at,
            revoked_at: row.revoked_at,
        }
    }
}
