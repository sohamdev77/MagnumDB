//! The Storage Engine module.
//!
//! This module provides the core key-value storage capabilities.

pub mod buffer_pool;
pub mod btree;
pub mod pager;

use crate::config::Config;
use crate::wal::WriteAheadLog;
use btree::BTree;
use pager::Pager;
use buffer_pool::BufferPool;

/// The core Database structure.
pub struct Database {
    config: Config,
    wal: Option<WriteAheadLog>,
    index: BTree,
}

impl Database {
    /// Opens or creates a new database given the configuration.
    pub fn open(config: Config) -> anyhow::Result<Self> {
        // Ensure data directory exists
        if !config.storage.path.exists() {
            std::fs::create_dir_all(&config.storage.path)?;
        }

        let wal = if config.wal.enabled {
            Some(WriteAheadLog::open(&config.storage.path)?)
        } else {
            None
        };

        let data_path = config.storage.path.join("magnum.data");
        let pager = Pager::open(&data_path)?;
        
        let buffer_pool = BufferPool::new(pager, 1024);
        let index = BTree::new(buffer_pool)?;

        Ok(Self {
            config,
            wal,
            index,
        })
    }

    /// Inserts a key-value pair into the database.
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        if let Some(wal) = &mut self.wal {
            wal.append_put(key, value)?;
            if self.config.wal.sync_on_write {
                wal.sync()?;
            }
        }
        
        self.index.insert(key, value)?;
        Ok(())
    }

    /// Retrieves a value by key. Returns None if the key doesn't exist.
    pub fn get(&mut self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        self.index.search(key)
    }

    /// Scans all key-value pairs in the database.
    pub fn scan(&mut self) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.index.scan()
    }

    /// Deletes a key from the database.
    pub fn delete(&mut self, key: &[u8]) -> anyhow::Result<()> {
        if let Some(wal) = &mut self.wal {
            wal.append_delete(key)?;
            if self.config.wal.sync_on_write {
                wal.sync()?;
            }
        }
        
        self.index.delete(key)?;
        Ok(())
    }
}
