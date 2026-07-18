use magnumdb::storage::Database;
use magnumdb::config::Config;
use tempfile::tempdir;

#[test]
fn test_wal_crash_recovery() {
    let dir = tempdir().unwrap();
    let mut config = Config::default();
    config.storage.path = dir.path().to_path_buf();
    config.wal.enabled = true;
    config.wal.sync_on_write = true;

    // Phase 1: Write and "Crash"
    {
        let mut db = Database::open(config.clone()).expect("Failed to open db");
        db.put(b"key1", b"value1").unwrap();
        db.put(b"key2", b"value2").unwrap();
        db.put(b"key1", b"value1_updated").unwrap();
        // We drop db here. The buffer pool does NOT automatically flush all pages on drop
        // in our implementation right now, so the data is only safely in the WAL.
    }

    // Phase 2: Recover
    {
        let mut db = Database::open(config.clone()).expect("Failed to recover db");
        
        // key1 was updated
        assert_eq!(db.get(b"key1").unwrap(), Some(b"value1_updated".to_vec()));
        // key2 should exist
        assert_eq!(db.get(b"key2").unwrap(), Some(b"value2".to_vec()));
    }
}
