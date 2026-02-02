use std::collections::HashMap;
use std::sync::Arc;

use fs9_sdk::{FsError, FsProvider, FsResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    #[serde(flatten)]
    pub options: HashMap<String, serde_json::Value>,
}

impl ProviderConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with<T: Serialize>(mut self, key: &str, value: T) -> Self {
        self.options.insert(
            key.to_string(),
            serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
        );
        self
    }

    pub fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        self.options
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key)
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key)
    }

    pub fn get_usize(&self, key: &str) -> Option<usize> {
        self.get(key)
    }
}

pub type ProviderFactory = fn(ProviderConfig) -> FsResult<Arc<dyn FsProvider>>;

pub struct ProviderRegistry {
    factories: HashMap<String, ProviderFactory>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: &str, factory: ProviderFactory) {
        self.factories.insert(name.to_string(), factory);
    }

    pub fn create(&self, name: &str, config: ProviderConfig) -> FsResult<Arc<dyn FsProvider>> {
        let factory = self
            .factories
            .get(name)
            .ok_or_else(|| FsError::not_found(&format!("provider '{}' not registered", name)))?;
        factory(config)
    }

    pub fn list(&self) -> Vec<&str> {
        self.factories.keys().map(|s| s.as_str()).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }
}

pub fn default_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    registry.register("memfs", |_config| {
        Ok(Arc::new(super::memfs::MemoryFs::new()))
    });

    registry.register("localfs", |config| {
        let root = config.get_str("root").unwrap_or_else(|| "/tmp".to_string());
        let fs = super::localfs::LocalFs::new(root)?;
        Ok(Arc::new(fs))
    });

    registry.register("proxyfs", |config| {
        let upstream = config
            .get_str("upstream")
            .ok_or_else(|| FsError::invalid_argument("proxyfs requires 'upstream' config"))?;
        let max_hops = config.get_usize("max_hops").unwrap_or(10);
        let token = config.get_str("token");

        let mut proxy = super::proxyfs::ProxyFs::new(&upstream).with_max_hops(max_hops);
        if let Some(t) = token {
            proxy = proxy.with_token(t);
        }
        Ok(Arc::new(proxy) as Arc<dyn FsProvider>)
    });

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config() {
        let config = ProviderConfig::new()
            .with("root", "/data")
            .with("size", 1024u64);

        assert_eq!(config.get_str("root"), Some("/data".to_string()));
        assert_eq!(config.get_u64("size"), Some(1024));
        assert_eq!(config.get_str("missing"), None);
    }

    #[test]
    fn test_registry_list() {
        let registry = default_registry();
        let providers = registry.list();
        assert!(providers.contains(&"memfs"));
        assert!(providers.contains(&"localfs"));
        assert!(providers.contains(&"proxyfs"));
    }

    #[test]
    fn test_create_memfs() {
        let registry = default_registry();
        let provider = registry.create("memfs", ProviderConfig::new());
        assert!(provider.is_ok());
    }

    #[test]
    fn test_create_localfs() {
        let registry = default_registry();
        let config = ProviderConfig::new().with("root", "/tmp");
        let provider = registry.create("localfs", config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_unknown_provider() {
        let registry = default_registry();
        let result = registry.create("unknown", ProviderConfig::new());
        assert!(result.is_err());
    }
}
