//! Write-Ahead Log (WAL) module.
//!
//! Provides durability by logging all mutations before they are applied to the main data files.

use std::path::{Path, PathBuf};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};

/// The Write-Ahead Log structure.
pub struct WriteAheadLog {
    file: File,
    _path: PathBuf,
}

impl WriteAheadLog {
    /// Opens or creates the WAL file in the specified data directory.
    pub fn open<P: AsRef<Path>>(data_dir: P) -> io::Result<Self> {
        let path = data_dir.as_ref().join("magnum.wal");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        Ok(Self {
            file,
            _path: path,
        })
    }

    /// Appends a PUT operation to the WAL.
    pub fn append_put(&mut self, key: &[u8], value: &[u8]) -> io::Result<()> {
        // Simple binary format: [Type: 1 byte][Key Len: 4 bytes][Key][Value Len: 4 bytes][Value]
        self.file.write_all(&[1])?; // 1 = PUT
        self.file.write_all(&(key.len() as u32).to_le_bytes())?;
        self.file.write_all(key)?;
        self.file.write_all(&(value.len() as u32).to_le_bytes())?;
        self.file.write_all(value)?;
        Ok(())
    }

    /// Appends a DELETE operation to the WAL.
    pub fn append_delete(&mut self, key: &[u8]) -> io::Result<()> {
        // Format: [Type: 1 byte][Key Len: 4 bytes][Key]
        self.file.write_all(&[2])?; // 2 = DELETE
        self.file.write_all(&(key.len() as u32).to_le_bytes())?;
        self.file.write_all(key)?;
        Ok(())
    }

    /// Syncs the WAL to disk to ensure durability.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }
}
