//! Token validation cache for reducing load on fs9-meta service.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

/// Cached token validation result.
#[derive(Debug, Clone)]
pub struct CachedToken {
    pub user_id: String,
    pub namespace: String,
    pub roles: Vec<String>,
    /// JWT expiration (seconds since Unix epoch). Cache entries never outlive this.
    pub expires_at: u64,
    pub cached_at: Instant,
}

/// In-memory cache for validated tokens.
///
/// This cache helps reduce load on the fs9-meta service by caching
/// successful token validation results for a configurable TTL.
#[derive(Clone)]
pub struct TokenCache {
    cache: Arc<RwLock<HashMap<String, CachedToken>>>,
    ttl: Duration,
}

impl TokenCache {
    /// Create a new token cache with the specified TTL.
    ///
    /// # Arguments
    /// * `ttl` - How long cached tokens remain valid
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Get a cached token if it exists and hasn't expired.
    ///
    /// Returns `None` if the token is not in the cache or has expired.
    pub async fn get(&self, token: &str) -> Option<CachedToken> {
        let cache = self.cache.read().await;
        let now = now_unix_secs();
        cache.get(token).and_then(|entry| {
            // Never authorize past the JWT's own expiry, even if cache TTL hasn't elapsed yet.
            if entry.cached_at.elapsed() < self.ttl && now < entry.expires_at {
                Some(entry.clone())
            } else {
                None
            }
        })
    }

    /// Store a validated token in the cache.
    pub async fn set(
        &self,
        token: &str,
        user_id: String,
        namespace: String,
        roles: Vec<String>,
        expires_at: u64,
    ) {
        let entry = CachedToken {
            user_id,
            namespace,
            roles,
            expires_at,
            cached_at: Instant::now(),
        };
        let mut cache = self.cache.write().await;
        cache.insert(token.to_string(), entry);
    }

    /// Remove a token from the cache (e.g., on logout or invalidation).
    pub async fn remove(&self, token: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(token);
    }

    /// Remove all expired entries from the cache.
    ///
    /// This can be called periodically to prevent memory growth.
    pub async fn cleanup_expired(&self) {
        let now = now_unix_secs();
        let mut cache = self.cache.write().await;
        cache.retain(|_, entry| entry.cached_at.elapsed() < self.ttl && now < entry.expires_at);
    }

    /// Get the number of entries in the cache.
    pub async fn len(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Check if the cache is empty.
    pub async fn is_empty(&self) -> bool {
        self.cache.read().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs()
    }

    #[tokio::test]
    async fn test_cache_set_and_get() {
        let cache = TokenCache::new(Duration::from_secs(60));

        cache
            .set(
                "token123",
                "user1".to_string(),
                "ns1".to_string(),
                vec!["admin".to_string()],
                now_unix() + 60,
            )
            .await;

        let entry = cache.get("token123").await;
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.user_id, "user1");
        assert_eq!(entry.namespace, "ns1");
        assert_eq!(entry.roles, vec!["admin"]);
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = TokenCache::new(Duration::from_secs(60));
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = TokenCache::new(Duration::from_millis(10));

        cache
            .set(
                "token123",
                "user1".to_string(),
                "ns1".to_string(),
                vec![],
                now_unix() + 60,
            )
            .await;

        // Entry should exist immediately
        assert!(cache.get("token123").await.is_some());

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Entry should be expired
        assert!(cache.get("token123").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_remove() {
        let cache = TokenCache::new(Duration::from_secs(60));

        cache
            .set(
                "token123",
                "user1".to_string(),
                "ns1".to_string(),
                vec![],
                now_unix() + 60,
            )
            .await;
        assert!(cache.get("token123").await.is_some());

        cache.remove("token123").await;
        assert!(cache.get("token123").await.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let cache = TokenCache::new(Duration::from_millis(10));

        cache
            .set(
                "token1",
                "u1".to_string(),
                "ns1".to_string(),
                vec![],
                now_unix() + 60,
            )
            .await;

        tokio::time::sleep(Duration::from_millis(20)).await;

        cache
            .set(
                "token2",
                "u2".to_string(),
                "ns2".to_string(),
                vec![],
                now_unix() + 60,
            )
            .await;

        assert_eq!(cache.len().await, 2);

        cache.cleanup_expired().await;

        assert_eq!(cache.len().await, 1);
        assert!(cache.get("token2").await.is_some());
    }

    #[tokio::test]
    async fn test_cache_does_not_outlive_jwt_exp() {
        let cache = TokenCache::new(Duration::from_secs(60));

        cache
            .set(
                "token123",
                "user1".to_string(),
                "ns1".to_string(),
                vec![],
                now_unix(), // Already expired
            )
            .await;

        assert!(cache.get("token123").await.is_none());
    }
}
