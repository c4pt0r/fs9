use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Fs9Config {
    pub server: ServerConfig,
    pub mounts: Vec<MountConfig>,
    pub fuse: FuseConfig,
    pub shell: ShellConfig,
    pub logging: LoggingConfig,
}

impl Default for Fs9Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            mounts: vec![MountConfig {
                path: "/".to_string(),
                provider: "memfs".to_string(),
                config: None,
            }],
            fuse: FuseConfig::default(),
            shell: ShellConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub auth: AuthConfig,
    pub plugins: PluginsConfig,
    /// URL of the fs9-meta service for token validation (e.g., "http://localhost:9998")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_url: Option<String>,
    /// Optional admin key for fs9-meta (sent as `x-fs9-meta-key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_key: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 9999,
            auth: AuthConfig::default(),
            plugins: PluginsConfig::default(),
            meta_url: None,
            meta_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub enabled: bool,
    pub jwt_secret: String,
    pub issuer: String,
    pub audience: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            jwt_secret: String::new(),
            issuer: "fs9".to_string(),
            audience: "fs9-clients".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    pub directories: Vec<String>,
    pub preload: Vec<PluginEntry>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            directories: vec!["./plugins".to_string()],
            preload: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    pub path: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FuseConfig {
    pub server: String,
    pub token: String,
    pub options: FuseOptions,
    pub cache: CacheConfig,
}

impl Default for FuseConfig {
    fn default() -> Self {
        Self {
            server: "http://localhost:9999".to_string(),
            token: String::new(),
            options: FuseOptions::default(),
            cache: CacheConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FuseOptions {
    pub allow_other: bool,
    pub allow_root: bool,
    pub auto_unmount: bool,
    pub read_only: bool,
}

impl Default for FuseOptions {
    fn default() -> Self {
        Self {
            allow_other: false,
            allow_root: false,
            auto_unmount: true,
            read_only: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub attr_ttl: String,
    pub entry_ttl: String,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            attr_ttl: "1s".to_string(),
            entry_ttl: "1s".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub server: String,
    pub token: String,
    pub prompt: String,
    pub history: HistoryConfig,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            server: "http://localhost:9999".to_string(),
            token: String::new(),
            prompt: "sh9:{cwd}> ".to_string(),
            history: HistoryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub file: String,
    pub max_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            file: "~/.fs9_history".to_string(),
            max_entries: 10000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: LogLevel,
    pub format: LogFormat,
    pub filter: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: LogFormat::Pretty,
            filter: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
    Compact,
}
