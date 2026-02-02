use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use fuser::FileAttr;

pub const ROOT_INO: u64 = 1;

pub struct InodeTable {
    next_ino: AtomicU64,
    path_to_ino: RwLock<HashMap<String, u64>>,
    ino_to_path: RwLock<HashMap<u64, String>>,
    ino_to_attr: RwLock<HashMap<u64, CachedAttr>>,
    cache_ttl: Duration,
}

struct CachedAttr {
    attr: FileAttr,
    cached_at: Instant,
}

impl InodeTable {
    pub fn new(cache_ttl: Duration) -> Self {
        let mut path_to_ino = HashMap::new();
        let mut ino_to_path = HashMap::new();

        path_to_ino.insert("/".to_string(), ROOT_INO);
        ino_to_path.insert(ROOT_INO, "/".to_string());

        Self {
            next_ino: AtomicU64::new(ROOT_INO + 1),
            path_to_ino: RwLock::new(path_to_ino),
            ino_to_path: RwLock::new(ino_to_path),
            ino_to_attr: RwLock::new(HashMap::new()),
            cache_ttl,
        }
    }

    pub fn get_or_create_ino(&self, path: &str) -> u64 {
        let normalized = normalize_path(path);

        if let Some(&ino) = self.path_to_ino.read().unwrap().get(&normalized) {
            return ino;
        }

        let mut path_to_ino = self.path_to_ino.write().unwrap();
        if let Some(&ino) = path_to_ino.get(&normalized) {
            return ino;
        }

        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst);
        path_to_ino.insert(normalized.clone(), ino);
        self.ino_to_path.write().unwrap().insert(ino, normalized);
        ino
    }

    pub fn get_ino(&self, path: &str) -> Option<u64> {
        let normalized = normalize_path(path);
        self.path_to_ino.read().unwrap().get(&normalized).copied()
    }

    pub fn get_path(&self, ino: u64) -> Option<String> {
        self.ino_to_path.read().unwrap().get(&ino).cloned()
    }

    pub fn remove(&self, path: &str) {
        let normalized = normalize_path(path);
        let mut path_to_ino = self.path_to_ino.write().unwrap();
        if let Some(ino) = path_to_ino.remove(&normalized) {
            self.ino_to_path.write().unwrap().remove(&ino);
            self.ino_to_attr.write().unwrap().remove(&ino);
        }
    }

    pub fn rename(&self, old_path: &str, new_path: &str) {
        let old_normalized = normalize_path(old_path);
        let new_normalized = normalize_path(new_path);

        let mut path_to_ino = self.path_to_ino.write().unwrap();
        let mut ino_to_path = self.ino_to_path.write().unwrap();

        if let Some(ino) = path_to_ino.remove(&old_normalized) {
            path_to_ino.insert(new_normalized.clone(), ino);
            ino_to_path.insert(ino, new_normalized);
        }
    }

    pub fn cache_attr(&self, ino: u64, attr: FileAttr) {
        self.ino_to_attr.write().unwrap().insert(
            ino,
            CachedAttr {
                attr,
                cached_at: Instant::now(),
            },
        );
    }

    pub fn get_cached_attr(&self, ino: u64) -> Option<FileAttr> {
        let cache = self.ino_to_attr.read().unwrap();
        cache.get(&ino).and_then(|cached| {
            if cached.cached_at.elapsed() < self.cache_ttl {
                Some(cached.attr)
            } else {
                None
            }
        })
    }

    pub fn invalidate_attr(&self, ino: u64) {
        self.ino_to_attr.write().unwrap().remove(&ino);
    }

    pub fn invalidate_all(&self) {
        self.ino_to_attr.write().unwrap().clear();
    }
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    path.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_exists() {
        let table = InodeTable::new(Duration::from_secs(10));
        assert_eq!(table.get_ino("/"), Some(ROOT_INO));
        assert_eq!(table.get_path(ROOT_INO), Some("/".to_string()));
    }

    #[test]
    fn test_get_or_create() {
        let table = InodeTable::new(Duration::from_secs(10));
        let ino1 = table.get_or_create_ino("/foo");
        let ino2 = table.get_or_create_ino("/foo");
        assert_eq!(ino1, ino2);
        assert_ne!(ino1, ROOT_INO);
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("/foo"), "/foo");
        assert_eq!(normalize_path("/foo/"), "/foo");
        assert_eq!(normalize_path("foo"), "/foo");
    }

    #[test]
    fn test_remove() {
        let table = InodeTable::new(Duration::from_secs(10));
        let ino = table.get_or_create_ino("/foo");
        assert!(table.get_ino("/foo").is_some());
        table.remove("/foo");
        assert!(table.get_ino("/foo").is_none());
        assert!(table.get_path(ino).is_none());
    }

    #[test]
    fn test_rename() {
        let table = InodeTable::new(Duration::from_secs(10));
        let ino = table.get_or_create_ino("/old");
        table.rename("/old", "/new");
        assert!(table.get_ino("/old").is_none());
        assert_eq!(table.get_ino("/new"), Some(ino));
        assert_eq!(table.get_path(ino), Some("/new".to_string()));
    }
}
