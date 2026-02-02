//! FS9 Configuration System
//!
//! Provides unified YAML-based configuration for all FS9 components.
//!
//! # Configuration Loading Priority
//!
//! 1. Compiled-in defaults
//! 2. `/etc/fs9/fs9.yaml` (system-wide)
//! 3. `~/.config/fs9/fs9.yaml` (user)
//! 4. `./fs9.yaml` (project-local)
//! 5. `FS9_CONFIG=/path/to/config.yaml` (explicit)
//! 6. Environment variables (highest priority)
//!
//! # Example Configuration
//!
//! ```yaml
//! server:
//!   host: "0.0.0.0"
//!   port: 9999
//!   auth:
//!     enabled: true
//!     jwt_secret: "${FS9_JWT_SECRET}"
//!
//! mounts:
//!   - path: "/"
//!     provider: memfs
//!   - path: "/data"
//!     provider: pagefs
//!     config:
//!       backend:
//!         type: s3
//!         bucket: "my-bucket"
//! ```

#![allow(missing_docs)]

mod error;
mod loader;
mod types;

pub use error::ConfigError;
pub use loader::ConfigLoader;
pub use types::*;

/// Load configuration from default locations.
///
/// Searches for config files in order and merges them.
/// Environment variables override file values.
pub fn load() -> Result<Fs9Config, ConfigError> {
    ConfigLoader::new().load()
}

/// Load configuration from a specific file.
pub fn load_from_file(path: &str) -> Result<Fs9Config, ConfigError> {
    ConfigLoader::new().with_file(path).load()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = Fs9Config::default();
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.server.host, "0.0.0.0");
    }

    #[test]
    fn parse_minimal_yaml() {
        let yaml = r#"
server:
  port: 8080
"#;
        let config: Fs9Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.host, "0.0.0.0"); // default
    }

    #[test]
    fn parse_full_config() {
        let yaml = r#"
server:
  host: "127.0.0.1"
  port: 9000
  auth:
    enabled: true
    jwt_secret: "test-secret"

mounts:
  - path: "/"
    provider: memfs
  - path: "/data"
    provider: pagefs
    config:
      backend:
        type: s3
        bucket: "test-bucket"
        prefix: "data"

logging:
  level: debug
"#;
        let config: Fs9Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9000);
        assert!(config.server.auth.enabled);
        assert_eq!(config.mounts.len(), 2);
        assert_eq!(config.logging.level, LogLevel::Debug);
    }
}
