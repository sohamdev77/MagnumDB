use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The main configuration structure for MagnumDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub storage: StorageConfig,
    pub wal: WalConfig,
    pub network: NetworkConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub path: PathBuf,
    pub cache_size_mb: usize,
    /// Number of writes between automatic buffer pool syncs. 0 = disabled.
    #[serde(default)]
    pub sync_interval: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    pub enabled: bool,
    pub sync_on_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub host: String,
    pub port: u16,
    /// Maximum number of concurrent client connections.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// Idle timeout in seconds before a connection is dropped. 0 = no timeout.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
}

fn default_max_connections() -> usize {
    128
}

fn default_idle_timeout() -> u64 {
    30
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: StorageConfig {
                path: PathBuf::from("./data"),
                cache_size_mb: 256,
                sync_interval: 1000,
            },
            wal: WalConfig {
                enabled: true,
                sync_on_write: false,
            },
            network: NetworkConfig {
                host: "127.0.0.1".to_string(),
                port: 7432,
                max_connections: 128,
                idle_timeout_secs: 30,
            },
        }
    }
}

impl Config {
    /// Overrides the storage path. Useful for testing or programmatic setup.
    pub fn with_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.storage.path = path.as_ref().to_path_buf();
        self
    }

    /// Loads the configuration from a TOML file.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}
