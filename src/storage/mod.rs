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

use parking_lot::Mutex;

/// The core Database structure.
pub struct Database {
    config: Config,
    wal: Option<Mutex<WriteAheadLog>>,
    index: BTree,
}

impl Database {
    /// Opens or creates a new database given the configuration.
    pub fn open(config: Config) -> anyhow::Result<Self> {
        // Ensure data directory exists
        if !config.storage.path.exists() {
            std::fs::create_dir_all(&config.storage.path)?;
        }

        let data_path = config.storage.path.join("magnum.data");
        let pager = Pager::open(&data_path)?;

        let checkpoint_lsn = {
            let mut tmp_pager = Pager::open(&data_path)?;
            tmp_pager.read_checkpoint_lsn().unwrap_or(0)
        };

        let buffer_pool = BufferPool::new(pager, 1024)
            .with_sync_interval(config.storage.sync_interval);
        let mut index = BTree::new(buffer_pool)?;

        let wal = if config.wal.enabled {
            Some(Mutex::new(WriteAheadLog::open(&config.storage.path)?))
        } else {
            None
        };

        // Perform WAL Recovery if enabled — only replay entries after the checkpoint LSN
        if let Some(wal_mutex) = &wal {
            let mut wal_ref = wal_mutex.lock();
            let entries = wal_ref.recover(checkpoint_lsn)?;
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
            // After recovery, flush and sync to make recovered data durable
            if !entries_is_empty_hint(&*wal_ref) {
                index.flush_and_sync()?;
            }
        }

        Ok(Self { config, wal, index })
    }

    /// Inserts a key-value pair into the database (non-transactional, tx_id=0).
    pub fn put(&self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        self.put_with_tx(0, key, value)
    }

    /// Inserts a key-value pair with an explicit transaction ID.
    pub fn put_with_tx(&self, tx_id: u64, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        if let Some(wal_mutex) = &self.wal {
            let mut wal = wal_mutex.lock();
            wal.append_tx_put(tx_id, key, value)?;
            if self.config.wal.sync_on_write {
                wal.sync()?;
            }
        }

        self.index.insert(key, value)?;
        Ok(())
    }

    /// Retrieves a value by key. Returns None if the key doesn't exist.
    pub fn get(&self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        self.index.search(key)
    }

    /// Scans all key-value pairs in the database.
    pub fn scan(&self) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.index.scan()
    }

    /// Scans key-value pairs matching a prefix efficiently.
    pub fn scan_prefix(&self, prefix: &[u8]) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.index.scan_prefix(prefix)
    }

    /// Deletes a key from the database (non-transactional, tx_id=0).
    pub fn delete(&self, key: &[u8]) -> anyhow::Result<()> {
        self.delete_with_tx(0, key)
    }

    /// Deletes a key with an explicit transaction ID.
    pub fn delete_with_tx(&self, tx_id: u64, key: &[u8]) -> anyhow::Result<()> {
        if let Some(wal_mutex) = &self.wal {
            let mut wal = wal_mutex.lock();
            wal.append_tx_delete(tx_id, key)?;
            if self.config.wal.sync_on_write {
                wal.sync()?;
            }
        }

        self.index.delete(key)?;
        Ok(())
    }

    /// Logs BEGIN transaction to WAL.
    pub fn begin_tx(&self, tx_id: u64) -> anyhow::Result<()> {
        if let Some(wal_mutex) = &self.wal {
            let mut wal = wal_mutex.lock();
            wal.append_begin(tx_id)?;
            if self.config.wal.sync_on_write {
                wal.sync()?;
            }
        }
        Ok(())
    }

    /// Logs COMMIT transaction to WAL and flushes+syncs the buffer pool.
    pub fn commit_tx(&self, tx_id: u64) -> anyhow::Result<()> {
        if let Some(wal_mutex) = &self.wal {
            let mut wal = wal_mutex.lock();
            wal.append_commit(tx_id)?;
            // Always sync the WAL on commit for durability
            wal.sync()?;
        }

        // Flush and sync the buffer pool so committed data is durable on disk
        self.index.flush_and_sync()?;
        Ok(())
    }

    /// Logs ROLLBACK transaction to WAL.
    pub fn rollback_tx(&self, tx_id: u64) -> anyhow::Result<()> {
        if let Some(wal_mutex) = &self.wal {
            let mut wal = wal_mutex.lock();
            wal.append_rollback(tx_id)?;
            if self.config.wal.sync_on_write {
                wal.sync()?;
            }
        }
        Ok(())
    }

    /// Closes the database gracefully by flushing the buffer pool and checkpointing the WAL.
    pub fn close(mut self) -> anyhow::Result<()> {
        self.index.flush_all()?;
        self.index.sync()?;

        if let Some(wal_mutex) = &self.wal {
            let mut wal = wal_mutex.lock();
            let current_lsn = wal.current_lsn();
            // Write checkpoint LSN to page 0 before truncating the WAL
            self.index
                .buffer_pool()
                .write_checkpoint_lsn(current_lsn)?;
            self.index.sync()?;
            wal.checkpoint()?;
        }
        Ok(())
    }
}

/// Helper — we can't easily check if recovery produced entries without changing
/// the WAL API, so this always returns false (recovery always flushes).
fn entries_is_empty_hint(_wal: &WriteAheadLog) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn get_config(path: std::path::PathBuf, wal_enabled: bool) -> Config {
        let mut config = Config::default();
        config.storage.path = path;
        config.wal.enabled = wal_enabled;
        config.storage.sync_interval = 0; // disable auto-sync in tests
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

    #[test]
    fn test_database_tx_commit_durable() {
        let dir = tempdir().unwrap();
        let config = get_config(dir.path().to_path_buf(), true);

        {
            let mut db = Database::open(config.clone()).unwrap();
            db.begin_tx(1).unwrap();
            db.put_with_tx(1, b"txkey", b"txval").unwrap();
            db.commit_tx(1).unwrap();
            // Don't call close — simulate crash
        }

        {
            let mut db = Database::open(config).unwrap();
            let val = db.get(b"txkey").unwrap();
            assert!(val.is_some());
            assert_eq!(val.unwrap(), b"txval");
        }
    }
}
