//! Database layer for fs9-meta service.

#![allow(clippy::missing_errors_doc)]

pub mod models;

#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "postgres")]
mod postgres;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;

use crate::error::MetaError;
pub use models::*;

pub type Result<T> = std::result::Result<T, MetaError>;

/// Database store abstraction.
#[derive(Clone)]
pub enum MetaStore {
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteStore),
    #[cfg(feature = "postgres")]
    Postgres(PostgresStore),
}

impl MetaStore {
    /// Connect to database using DSN.
    pub async fn connect(dsn: &str) -> Result<Self> {
        if dsn.starts_with("sqlite:") {
            #[cfg(feature = "sqlite")]
            {
                let store = SqliteStore::connect(dsn).await?;
                return Ok(Self::Sqlite(store));
            }
            #[cfg(not(feature = "sqlite"))]
            {
                return Err(MetaError::Config("SQLite support not enabled".into()));
            }
        }

        if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
            #[cfg(feature = "postgres")]
            {
                let store = PostgresStore::connect(dsn).await?;
                return Ok(Self::Postgres(store));
            }
            #[cfg(not(feature = "postgres"))]
            {
                return Err(MetaError::Config("PostgreSQL support not enabled".into()));
            }
        }

        Err(MetaError::Config(format!("Unsupported DSN: {dsn}")))
    }

    /// Run database migrations.
    pub async fn migrate(&self) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.migrate().await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.migrate().await,
        }
    }

    // ========================================================================
    // Namespace operations
    // ========================================================================

    pub async fn create_namespace(&self, name: &str, created_by: &str) -> Result<Namespace> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.create_namespace(name, created_by).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.create_namespace(name, created_by).await,
        }
    }

    pub async fn get_namespace(&self, name: &str) -> Result<Option<Namespace>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_namespace(name).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_namespace(name).await,
        }
    }

    pub async fn get_namespace_by_id(&self, id: &str) -> Result<Option<Namespace>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_namespace_by_id(id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_namespace_by_id(id).await,
        }
    }

    pub async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.list_namespaces().await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.list_namespaces().await,
        }
    }

    pub async fn delete_namespace(&self, name: &str) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.delete_namespace(name).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.delete_namespace(name).await,
        }
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
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => {
                store
                    .create_mount(namespace_id, path, provider, config, created_by)
                    .await
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                store
                    .create_mount(namespace_id, path, provider, config, created_by)
                    .await
            }
        }
    }

    pub async fn get_mount(&self, namespace_id: &str, path: &str) -> Result<Option<Mount>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_mount(namespace_id, path).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_mount(namespace_id, path).await,
        }
    }

    pub async fn list_mounts(&self, namespace_id: &str) -> Result<Vec<Mount>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.list_mounts(namespace_id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.list_mounts(namespace_id).await,
        }
    }

    pub async fn delete_mount(&self, namespace_id: &str, path: &str) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.delete_mount(namespace_id, path).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.delete_mount(namespace_id, path).await,
        }
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
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.create_user(username, password_hash, email).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.create_user(username, password_hash, email).await,
        }
    }

    pub async fn get_user(&self, username: &str) -> Result<Option<User>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_user(username).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_user(username).await,
        }
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_user_by_id(id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_user_by_id(id).await,
        }
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.list_users().await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.list_users().await,
        }
    }

    pub async fn delete_user(&self, id: &str) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.delete_user(id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.delete_user(id).await,
        }
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
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => {
                store
                    .assign_role(user_id, namespace_id, role, assigned_by)
                    .await
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                store
                    .assign_role(user_id, namespace_id, role, assigned_by)
                    .await
            }
        }
    }

    pub async fn revoke_role(&self, user_id: &str, namespace_id: &str, role: &str) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.revoke_role(user_id, namespace_id, role).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.revoke_role(user_id, namespace_id, role).await,
        }
    }

    pub async fn get_user_roles(&self, user_id: &str) -> Result<Vec<UserRole>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_user_roles(user_id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_user_roles(user_id).await,
        }
    }

    pub async fn get_user_roles_for_namespace(
        &self,
        user_id: &str,
        namespace_id: &str,
    ) -> Result<Vec<String>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.get_user_roles_for_namespace(user_id, namespace_id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                store
                    .get_user_roles_for_namespace(user_id, namespace_id)
                    .await
            }
        }
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
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(ApiKey, String)> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => {
                store
                    .create_api_key(user_id, namespace_id, name, roles, expires_at)
                    .await
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                store
                    .create_api_key(user_id, namespace_id, name, roles, expires_at)
                    .await
            }
        }
    }

    pub async fn validate_api_key(&self, key: &str) -> Result<Option<ApiKey>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.validate_api_key(key).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.validate_api_key(key).await,
        }
    }

    pub async fn list_api_keys(&self, user_id: &str) -> Result<Vec<ApiKey>> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.list_api_keys(user_id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.list_api_keys(user_id).await,
        }
    }

    pub async fn revoke_api_key(&self, key_id: &str) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.revoke_api_key(key_id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.revoke_api_key(key_id).await,
        }
    }

    pub async fn touch_api_key(&self, key_id: &str) -> Result<()> {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => store.touch_api_key(key_id).await,
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.touch_api_key(key_id).await,
        }
    }
}
