//! Client for validating db9 API tokens against the db9 backend.
//!
//! Validates bearer tokens by calling the db9 customer API and checking
//! that the requested tenant_id belongs to the authenticated customer.
//! Results are cached with a 5-minute TTL using moka.
//!
//! Protections against amplification attacks:
//! - **Negative caching**: failed validations are cached (30s TTL) to avoid
//!   repeated backend calls for the same invalid token.
//! - **Request coalescing**: concurrent validations for the same token share
//!   a single in-flight backend request (via tokio broadcast).
//! - **Backend rate limiting**: a semaphore caps concurrent backend calls to
//!   prevent a flood of unique invalid tokens from overwhelming db9.

use moka::future::Cache;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

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

    #[error("token validation failed (cached rejection)")]
    CachedRejection,

    #[error("too many concurrent validation requests")]
    RateLimited,
}

/// Client for validating db9 API bearer tokens.
#[derive(Clone)]
pub struct Db9Client {
    client: Client,
    base_url: String,
    /// Positive cache: successful token → auth info (5 min TTL)
    cache: Cache<String, CachedAuth>,
    /// Negative cache: failed token hash → () (30s TTL)
    neg_cache: Cache<String, ()>,
    /// In-flight request coalescing: token hash → broadcast sender
    in_flight:
        Arc<Mutex<HashMap<String, tokio::sync::broadcast::Sender<Result<CachedAuth, String>>>>>,
    /// Semaphore to limit concurrent backend validation calls
    backend_semaphore: Arc<Semaphore>,
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

        let neg_cache = Cache::builder()
            .max_capacity(50_000)
            .time_to_live(Duration::from_secs(30))
            .build();

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            cache,
            neg_cache,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            // Allow at most 20 concurrent backend validation calls
            backend_semaphore: Arc::new(Semaphore::new(20)),
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

        // 1. Check positive cache
        if let Some(cached) = self.cache.get(&key).await {
            if cached.tenant_ids.iter().any(|id| id == tenant_id) {
                return Ok(cached.customer_id);
            }
            // Tenant not in cached list — might be newly created.
            // Invalidate and re-fetch below.
            self.cache.invalidate(&key).await;
        }

        // 2. Check negative cache — reject immediately if recently failed
        if self.neg_cache.get(&key).await.is_some() {
            return Err(Db9AuthError::CachedRejection);
        }

        // 3. Request coalescing: check if another task is already validating this token
        let mut in_flight = self.in_flight.lock().await;
        if let Some(tx) = in_flight.get(&key) {
            // Another request is in-flight for this token — subscribe and wait
            let mut rx = tx.subscribe();
            drop(in_flight); // release lock while waiting

            match rx.recv().await {
                Ok(Ok(cached)) => {
                    if cached.tenant_ids.iter().any(|id| id == tenant_id) {
                        return Ok(cached.customer_id);
                    }
                    return Err(Db9AuthError::TenantNotAuthorized(tenant_id.to_string()));
                }
                _ => return Err(Db9AuthError::CachedRejection),
            }
        }

        // We are the first — register a broadcast channel
        let (tx, _) = tokio::sync::broadcast::channel(1);
        in_flight.insert(key.clone(), tx.clone());
        drop(in_flight);

        // 4. Acquire semaphore permit (rate limit backend calls)
        let _permit = match self.backend_semaphore.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                self.cleanup_in_flight(&key).await;
                self.neg_cache.insert(key, ()).await;
                let _ = tx.send(Err("rate limited".to_string()));
                return Err(Db9AuthError::RateLimited);
            }
        };

        // 5. Fetch from backend
        let result = self.fetch_from_backend(token).await;

        match result {
            Ok(cached) => {
                // Positive cache
                self.cache.insert(key.clone(), cached.clone()).await;
                self.cleanup_in_flight(&key).await;
                let _ = tx.send(Ok(cached.clone()));

                if cached.tenant_ids.iter().any(|id| id == tenant_id) {
                    Ok(cached.customer_id)
                } else {
                    Err(Db9AuthError::TenantNotAuthorized(tenant_id.to_string()))
                }
            }
            Err(e) => {
                // Negative cache for auth failures (401, 403)
                self.neg_cache.insert(key.clone(), ()).await;
                self.cleanup_in_flight(&key).await;
                let _ = tx.send(Err(e.to_string()));
                Err(e)
            }
        }
    }

    async fn cleanup_in_flight(&self, key: &str) {
        let mut in_flight = self.in_flight.lock().await;
        in_flight.remove(key);
    }

    async fn fetch_from_backend(&self, token: &str) -> Result<CachedAuth, Db9AuthError> {
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

        Ok(CachedAuth {
            customer_id: customer.id,
            tenant_ids,
        })
    }
}
