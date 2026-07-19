//! The Storage Engine module.
//!
//! This module provides the core key-value storage capabilities.

pub mod btree;
pub mod buffer_pool;
pub mod pager;

use crate::config::Config;
use crate::wal::WriteAheadLog;
use btree::BTree;
use buffer_pool::BufferPool;
use pager::Pager;

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

        let mut wal = if config.wal.enabled {
            Some(WriteAheadLog::open(&config.storage.path)?)
        } else {
            None
        };

        let data_path = config.storage.path.join("magnum.data");
        let pager = Pager::open(&data_path)?;

        let buffer_pool = BufferPool::new(pager, 1024);
        let mut index = BTree::new(buffer_pool)?;

        // Perform WAL Recovery if enabled
        if let Some(wal_ref) = &mut wal {
            let entries = wal_ref.recover()?;
            for entry in entries {
                match entry {
                    crate::wal::WalEntry::Put(k, v) => {
                        index.insert(&k, &v)?;
                    }
                    crate::wal::WalEntry::Delete(k) => {
                        index.delete(&k)?;
                    }
                }
            }
        }

        Ok(Self { config, wal, index })
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

    /// Closes the database gracefully by flushing the buffer pool and checkpointing the WAL.
    pub fn close(mut self) -> anyhow::Result<()> {
        self.index.flush_all()?;
        self.index.sync()?;
        if let Some(wal) = &mut self.wal {
            wal.checkpoint()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn get_config(path: std::path::PathBuf, wal_enabled: bool) -> Config {
        let mut config = Config::default();
        config.storage.path = path;
        config.wal.enabled = wal_enabled;
        config
    }

    #[test]
    fn test_database_close_wal_disabled() {
        let dir = tempdir().unwrap();
        let config = get_config(dir.path().to_path_buf(), false);

        {
            let mut db = Database::open(config.clone()).unwrap();
            db.put(b"hello", b"world").unwrap();
            db.close().unwrap(); // Should flush data to btree pages
        }

        {
            let mut db = Database::open(config).unwrap();
            let val = db.get(b"hello").unwrap().unwrap();
            assert_eq!(val, b"world");
        }
    }

    #[test]
    fn test_database_wal_checkpoint() {
        let dir = tempdir().unwrap();
        let config = get_config(dir.path().to_path_buf(), true);
        let wal_path = config.storage.path.join("magnum.wal");

        {
            let mut db = Database::open(config.clone()).unwrap();
            for i in 0..100 {
                let k = format!("k{}", i);
                let v = format!("v{}", i);
                db.put(k.as_bytes(), v.as_bytes()).unwrap();
            }
            db.close().unwrap(); // Should checkpoint WAL (truncate)
        }

        // After close, WAL file size should be 0 (checkpointed)
        assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), 0);

        {
            let mut db = Database::open(config).unwrap();
            for i in 0..100 {
                let k = format!("k{}", i);
                let v = format!("v{}", i);
                let val = db.get(k.as_bytes()).unwrap().unwrap();
                assert_eq!(val, v.as_bytes());
            }
        }
    }
}
