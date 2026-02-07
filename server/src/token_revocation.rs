use moka::future::Cache;
use sha2::{Digest, Sha256};
use std::time::Duration;

pub struct RevocationSet {
    revoked: Cache<String, ()>,
}

impl RevocationSet {
    pub fn new(max_capacity: u64) -> Self {
        Self {
            revoked: Cache::builder()
                .max_capacity(max_capacity)
                .time_to_live(Duration::from_secs(25 * 3600))
                .build(),
        }
    }

    pub async fn revoke(&self, token: &str) {
        let hash = token_hash(token);
        self.revoked.insert(hash, ()).await;
    }

    pub async fn is_revoked(&self, token: &str) -> bool {
        let hash = token_hash(token);
        self.revoked.get(&hash).await.is_some()
    }

    pub async fn count(&self) -> u64 {
        self.revoked.run_pending_tasks().await;
        self.revoked.entry_count()
    }
}

pub fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..16])
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revoke_and_check() {
        let set = RevocationSet::new(1000);

        assert!(!set.is_revoked("token123").await);

        set.revoke("token123").await;
        assert!(set.is_revoked("token123").await);
        assert!(!set.is_revoked("token456").await);
    }

    #[tokio::test]
    async fn count_revoked() {
        let set = RevocationSet::new(1000);

        set.revoke("a").await;
        set.revoke("b").await;
        set.revoke("c").await;

        assert_eq!(set.count().await, 3);
    }

    #[test]
    fn token_hash_deterministic() {
        let h1 = token_hash("test-token");
        let h2 = token_hash("test-token");
        assert_eq!(h1, h2);
        assert_ne!(token_hash("other"), h1);
    }
}
