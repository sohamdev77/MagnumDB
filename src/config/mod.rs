use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::fs;

/// The main configuration structure for MagnumDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub storage: StorageConfig,
    pub wal: WalConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub path: PathBuf,
    pub cache_size_mb: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    pub enabled: bool,
    pub sync_on_write: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: StorageConfig {
                path: PathBuf::from("./data"),
                cache_size_mb: 256,
            },
            wal: WalConfig {
                enabled: true,
                sync_on_write: false,
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
