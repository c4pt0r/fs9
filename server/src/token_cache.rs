//! Token validation cache for reducing load on fs9-meta service.
//!
//! Uses moka for high-performance concurrent caching with:
//! - Lock-free reads via sharded hash map
//! - Bounded capacity with LRU eviction
//! - Automatic TTL-based expiration
//! - Background cleanup of expired entries

use moka::future::Cache;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// Default maximum number of cached tokens.
/// For 1M users with ~10% active sessions, 100K entries is a reasonable default.
pub const DEFAULT_MAX_CAPACITY: u64 = 100_000;

/// In-memory cache for validated tokens.
///
/// This cache helps reduce load on the fs9-meta service by caching
/// successful token validation results for a configurable TTL.
///
/// Uses moka for:
/// - O(1) lock-free reads via sharded concurrent hash map
/// - Bounded capacity with LRU eviction (prevents unbounded memory growth)
/// - Automatic TTL-based expiration (no manual cleanup needed)
/// - Thread-safe operations without explicit locking
#[derive(Clone)]
pub struct TokenCache {
    cache: Cache<String, CachedToken>,
    ttl: Duration,
}

impl TokenCache {
    /// Create a new token cache with the specified TTL and default capacity.
    ///
    /// # Arguments
    /// * `ttl` - How long cached tokens remain valid
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self::with_capacity(ttl, DEFAULT_MAX_CAPACITY)
    }

    /// Create a new token cache with the specified TTL and capacity.
    ///
    /// # Arguments
    /// * `ttl` - How long cached tokens remain valid
    /// * `max_capacity` - Maximum number of tokens to cache (LRU eviction when exceeded)
    #[must_use]
    pub fn with_capacity(ttl: Duration, max_capacity: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .build();

        Self { cache, ttl }
    }

    /// Get a cached token if it exists and hasn't expired.
    ///
    /// Returns `None` if the token is not in the cache or has expired.
    pub async fn get(&self, token: &str) -> Option<CachedToken> {
        let now = now_unix_secs();
        let result = self.cache.get(token).await.and_then(|entry| {
            if now < entry.expires_at {
                Some(entry)
            } else {
                None
            }
        });
        match &result {
            Some(_) => metrics::counter!("fs9_token_cache_hits_total").increment(1),
            None => metrics::counter!("fs9_token_cache_misses_total").increment(1),
        }
        result
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
        self.cache.insert(token.to_string(), entry).await;
    }

    /// Remove a token from the cache (e.g., on logout or invalidation).
    pub async fn remove(&self, token: &str) {
        self.cache.remove(token).await;
    }

    /// Remove all expired entries from the cache.
    ///
    /// Note: Moka handles TTL-based expiration automatically in the background.
    /// This method is provided for compatibility and to force immediate cleanup
    /// of JWT-expired entries (where JWT exp < cache TTL).
    pub async fn cleanup_expired(&self) {
        self.cache.run_pending_tasks().await;
    }

    /// Get the approximate number of entries in the cache.
    ///
    /// Note: This is an approximation because moka uses eventual consistency
    /// for better performance. The actual count may be slightly different.
    pub async fn len(&self) -> usize {
        self.cache.run_pending_tasks().await;
        self.cache.entry_count() as usize
    }

    /// Check if the cache is approximately empty.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Get the TTL configured for this cache.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Get the maximum capacity of this cache.
    #[must_use]
    pub fn max_capacity(&self) -> u64 {
        self.cache.policy().max_capacity().unwrap_or(0)
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

        assert!(cache.get("token123").await.is_some());

        tokio::time::sleep(Duration::from_millis(50)).await;
        cache.cache.run_pending_tasks().await;

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

        tokio::time::sleep(Duration::from_millis(50)).await;

        cache
            .set(
                "token2",
                "u2".to_string(),
                "ns2".to_string(),
                vec![],
                now_unix() + 60,
            )
            .await;

        cache.cleanup_expired().await;

        assert!(cache.get("token1").await.is_none());
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
                now_unix(),
            )
            .await;

        assert!(cache.get("token123").await.is_none());
    }

    #[tokio::test]
    async fn test_cache_capacity_limit() {
        let cache = TokenCache::with_capacity(Duration::from_secs(60), 10);

        for i in 0..20 {
            cache
                .set(
                    &format!("token{i}"),
                    format!("u{i}"),
                    format!("ns{i}"),
                    vec![],
                    now_unix() + 60,
                )
                .await;
        }

        cache.cache.run_pending_tasks().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        cache.cache.run_pending_tasks().await;

        let count = cache.len().await;
        assert!(count <= 10, "Expected at most 10 entries, got {count}");
    }

    #[tokio::test]
    async fn test_cache_configuration() {
        let cache = TokenCache::with_capacity(Duration::from_secs(300), 50_000);

        assert_eq!(cache.ttl(), Duration::from_secs(300));
        assert_eq!(cache.max_capacity(), 50_000);
    }
}
