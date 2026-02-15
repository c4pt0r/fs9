//! Client for validating db9 API tokens against the db9 backend.
//!
//! Validates bearer tokens by calling the db9 customer API and checking
//! that the requested tenant_id belongs to the authenticated customer.
//! Results are cached with a 5-minute TTL using moka.

use moka::future::Cache;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct CustomerResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseResponse {
    id: String,
}

/// Cached result of a db9 token validation.
#[derive(Clone, Debug)]
struct CachedAuth {
    customer_id: String,
    tenant_ids: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Db9AuthError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("db9 backend returned {0}: {1}")]
    Backend(u16, String),

    #[error("tenant {0} not authorized for this customer")]
    TenantNotAuthorized(String),
}

/// Client for validating db9 API bearer tokens.
#[derive(Clone)]
pub struct Db9Client {
    client: Client,
    base_url: String,
    cache: Cache<String, CachedAuth>,
}

impl Db9Client {
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        let cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_secs(300))
            .build();

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            cache,
        }
    }

    fn cache_key(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Validate a db9 bearer token and check that `tenant_id` belongs to the customer.
    ///
    /// Returns the customer_id on success.
    pub async fn validate_token(
        &self,
        token: &str,
        tenant_id: &str,
    ) -> Result<String, Db9AuthError> {
        let key = Self::cache_key(token);

        if let Some(cached) = self.cache.get(&key).await {
            if cached.tenant_ids.iter().any(|id| id == tenant_id) {
                return Ok(cached.customer_id);
            }
            return Err(Db9AuthError::TenantNotAuthorized(tenant_id.to_string()));
        }

        // Fetch customer info and databases in parallel
        let me_url = format!("{}/customer/me", self.base_url);
        let dbs_url = format!("{}/customer/databases", self.base_url);

        let auth_header = format!("Bearer {token}");

        let (me_resp, dbs_resp) = tokio::join!(
            self.client
                .get(&me_url)
                .header("Authorization", &auth_header)
                .send(),
            self.client
                .get(&dbs_url)
                .header("Authorization", &auth_header)
                .send(),
        );

        let me_resp = me_resp?;
        if !me_resp.status().is_success() {
            let status = me_resp.status().as_u16();
            let body = me_resp.text().await.unwrap_or_default();
            return Err(Db9AuthError::Backend(status, body));
        }

        let dbs_resp = dbs_resp?;
        if !dbs_resp.status().is_success() {
            let status = dbs_resp.status().as_u16();
            let body = dbs_resp.text().await.unwrap_or_default();
            return Err(Db9AuthError::Backend(status, body));
        }

        let customer: CustomerResponse = me_resp.json().await?;
        let databases: Vec<DatabaseResponse> = dbs_resp.json().await?;

        let tenant_ids: Vec<String> = databases.into_iter().map(|d| d.id).collect();

        // Cache the result
        self.cache
            .insert(
                key,
                CachedAuth {
                    customer_id: customer.id.clone(),
                    tenant_ids: tenant_ids.clone(),
                },
            )
            .await;

        if tenant_ids.iter().any(|id| id == tenant_id) {
            Ok(customer.id)
        } else {
            Err(Db9AuthError::TenantNotAuthorized(tenant_id.to_string()))
        }
    }
}
