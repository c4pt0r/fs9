use crate::{ConfigError, Fs9Config};
use regex::Regex;
use std::path::PathBuf;

pub struct ConfigLoader {
    explicit_file: Option<PathBuf>,
    search_paths: Vec<PathBuf>,
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigLoader {
    pub fn new() -> Self {
        let mut search_paths = Vec::new();

        if let Some(home) = dirs::home_dir() {
            search_paths.push(home.join(".config/fs9/fs9.yaml"));
        }
        search_paths.push(PathBuf::from("./fs9.yaml"));

        #[cfg(unix)]
        search_paths.insert(0, PathBuf::from("/etc/fs9/fs9.yaml"));

        Self {
            explicit_file: None,
            search_paths,
        }
    }

    pub fn with_file(mut self, path: &str) -> Self {
        self.explicit_file = Some(PathBuf::from(path));
        self
    }

    pub fn load(&self) -> Result<Fs9Config, ConfigError> {
        let mut config = Fs9Config::default();

        if let Ok(env_path) = std::env::var("FS9_CONFIG") {
            let content =
                std::fs::read_to_string(&env_path).map_err(|e| ConfigError::ReadFile {
                    path: PathBuf::from(&env_path),
                    source: e,
                })?;
            config = self.parse_yaml(&content)?;
        } else if let Some(ref explicit) = self.explicit_file {
            let content = std::fs::read_to_string(explicit).map_err(|e| ConfigError::ReadFile {
                path: explicit.clone(),
                source: e,
            })?;
            config = self.parse_yaml(&content)?;
        } else {
            for path in &self.search_paths {
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        config = self.merge_yaml(&config, &content)?;
                    }
                }
            }
        }

        self.apply_env_overrides(&mut config);
        Ok(config)
    }

    fn parse_yaml(&self, content: &str) -> Result<Fs9Config, ConfigError> {
        let expanded = self.expand_env_vars(content);
        Ok(serde_yaml::from_str(&expanded)?)
    }

    fn merge_yaml(&self, base: &Fs9Config, content: &str) -> Result<Fs9Config, ConfigError> {
        let expanded = self.expand_env_vars(content);
        let overlay: Fs9Config = serde_yaml::from_str(&expanded)?;
        Ok(self.merge_configs(base, &overlay))
    }

    fn merge_configs(&self, base: &Fs9Config, overlay: &Fs9Config) -> Fs9Config {
        let mut result = base.clone();

        if overlay.server.host != Fs9Config::default().server.host {
            result.server.host = overlay.server.host.clone();
        }
        if overlay.server.port != Fs9Config::default().server.port {
            result.server.port = overlay.server.port;
        }
        if overlay.server.auth.enabled {
            result.server.auth = overlay.server.auth.clone();
        }
        if !overlay.server.plugins.directories.is_empty() {
            result.server.plugins = overlay.server.plugins.clone();
        }
        if overlay.server.meta_url.is_some() {
            result.server.meta_url = overlay.server.meta_url.clone();
        }
        if !overlay.mounts.is_empty() && overlay.mounts != Fs9Config::default().mounts {
            result.mounts = overlay.mounts.clone();
        }
        if overlay.fuse.server != Fs9Config::default().fuse.server {
            result.fuse = overlay.fuse.clone();
        }
        if overlay.shell.server != Fs9Config::default().shell.server {
            result.shell = overlay.shell.clone();
        }
        if overlay.logging.level != Fs9Config::default().logging.level {
            result.logging = overlay.logging.clone();
        }

        result
    }

    fn expand_env_vars(&self, content: &str) -> String {
        let re = Regex::new(r"\$\{([^}]+)\}").unwrap();
        re.replace_all(content, |caps: &regex::Captures| {
            let var_name = &caps[1];
            std::env::var(var_name).unwrap_or_default()
        })
        .to_string()
    }

    fn apply_env_overrides(&self, config: &mut Fs9Config) {
        if let Ok(host) = std::env::var("FS9_HOST") {
            config.server.host = host;
        }
        if let Ok(port) = std::env::var("FS9_PORT") {
            if let Ok(p) = port.parse() {
                config.server.port = p;
            }
        }
        if let Ok(secret) = std::env::var("FS9_JWT_SECRET") {
            if !secret.is_empty() {
                config.server.auth.enabled = true;
            }
            config.server.auth.jwt_secret = secret;
        }
        if let Ok(dir) = std::env::var("FS9_PLUGIN_DIR") {
            config.server.plugins.directories.insert(0, dir);
        }
        if let Ok(server) = std::env::var("FS9_SERVER_ENDPOINTS") {
            config.fuse.server = server.clone();
            config.shell.server = server;
        }
        if let Ok(token) = std::env::var("FS9_TOKEN") {
            config.fuse.token = token.clone();
            config.shell.token = token;
        }
        if let Ok(level) = std::env::var("FS9_LOG_LEVEL") {
            if let Ok(l) = serde_yaml::from_str(&level) {
                config.logging.level = l;
            }
        }
        if let Ok(meta_url) = std::env::var("FS9_META_ENDPOINTS") {
            if !meta_url.is_empty() {
                config.server.meta_url = Some(meta_url);
            }
        }
        if let Ok(meta_key) = std::env::var("FS9_META_KEY") {
            if !meta_key.is_empty() {
                config.server.meta_key = Some(meta_key);
            }
        }
        if let Ok(db9_api_url) = std::env::var("FS9_DB9_API_URL") {
            if !db9_api_url.is_empty() {
                config.server.db9_api_url = Some(db9_api_url);
            }
        }

        // Default pagefs config from env vars
        if let Ok(pd_endpoints) = std::env::var("FS9_PAGEFS_PD_ENDPOINTS") {
            if !pd_endpoints.is_empty() {
                let pagefs = config
                    .server
                    .default_pagefs
                    .get_or_insert_with(|| crate::DefaultPagefsConfig {
                        pd_endpoints: Vec::new(),
                        ca_path: None,
                        cert_path: None,
                        key_path: None,
                        keyspace_prefix: "tipg_fs_".to_string(),
                    });
                pagefs.pd_endpoints =
                    pd_endpoints.split(',').map(|s| s.trim().to_string()).collect();
            }
        }
        if let Ok(ca) = std::env::var("FS9_PAGEFS_CA_PATH") {
            if let Some(ref mut pagefs) = config.server.default_pagefs {
                pagefs.ca_path = Some(ca);
            }
        }
        if let Ok(cert) = std::env::var("FS9_PAGEFS_CERT_PATH") {
            if let Some(ref mut pagefs) = config.server.default_pagefs {
                pagefs.cert_path = Some(cert);
            }
        }
        if let Ok(key) = std::env::var("FS9_PAGEFS_KEY_PATH") {
            if let Some(ref mut pagefs) = config.server.default_pagefs {
                pagefs.key_path = Some(key);
            }
        }
        if let Ok(prefix) = std::env::var("FS9_PAGEFS_KEYSPACE_PREFIX") {
            if let Some(ref mut pagefs) = config.server.default_pagefs {
                pagefs.keyspace_prefix = prefix;
            }
        }
    }
}

impl PartialEq for crate::MountConfig {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.provider == other.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_env_vars_works() {
        std::env::set_var("TEST_VAR_123", "hello");
        let loader = ConfigLoader::new();
        let result = loader.expand_env_vars("value: ${TEST_VAR_123}");
        assert_eq!(result, "value: hello");
        std::env::remove_var("TEST_VAR_123");
    }

    #[test]
    fn missing_env_var_becomes_empty() {
        let loader = ConfigLoader::new();
        let result = loader.expand_env_vars("value: ${NONEXISTENT_VAR_XYZ}");
        assert_eq!(result, "value: ");
    }

    #[test]
    fn env_overrides_config() {
        std::env::set_var("FS9_PORT", "8888");
        let mut config = Fs9Config::default();
        let loader = ConfigLoader::new();
        loader.apply_env_overrides(&mut config);
        assert_eq!(config.server.port, 8888);
        std::env::remove_var("FS9_PORT");
    }
}
