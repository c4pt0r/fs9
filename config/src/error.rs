use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to parse YAML: {0}")]
    ParseYaml(#[from] serde_yaml::Error),

    #[error("Failed to parse JSON: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("Environment variable '{name}' not found")]
    EnvVarNotFound { name: String },

    #[error("Invalid duration format: {0}")]
    InvalidDuration(String),

    #[error("Invalid config value: {0}")]
    InvalidValue(String),
}
