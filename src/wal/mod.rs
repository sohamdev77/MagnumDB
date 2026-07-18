//! Write-Ahead Log (WAL) module.
//!
//! Provides durability by logging all mutations before they are applied to the main data files.

use std::path::{Path, PathBuf};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};

/// Represents an operation logged in the WAL.
pub enum WalEntry {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

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
            .read(true)
            .append(true)
            .open(&path)?;

        Ok(Self {
            file,
            _path: path,
        })
    }

    /// Reads all entries from the WAL for crash recovery.
    pub fn recover(&mut self) -> io::Result<Vec<WalEntry>> {
        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        
        loop {
            let mut type_buf = [0u8; 1];
            if let Err(e) = self.file.read_exact(&mut type_buf) {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    break; // Clean EOF
                }
                break; // Partial write / crash mid-write
            }
            
            match type_buf[0] {
                1 => { // PUT
                    let mut len_buf = [0u8; 4];
                    if self.file.read_exact(&mut len_buf).is_err() { break; }
                    let key_len = u32::from_le_bytes(len_buf) as usize;
                    
                    let mut key = vec![0; key_len];
                    if self.file.read_exact(&mut key).is_err() { break; }
                    
                    if self.file.read_exact(&mut len_buf).is_err() { break; }
                    let val_len = u32::from_le_bytes(len_buf) as usize;
                    
                    let mut val = vec![0; val_len];
                    if self.file.read_exact(&mut val).is_err() { break; }
                    
                    entries.push(WalEntry::Put(key, val));
                }
                2 => { // DELETE
                    let mut len_buf = [0u8; 4];
                    if self.file.read_exact(&mut len_buf).is_err() { break; }
                    let key_len = u32::from_le_bytes(len_buf) as usize;
                    
                    let mut key = vec![0; key_len];
                    if self.file.read_exact(&mut key).is_err() { break; }
                    
                    entries.push(WalEntry::Delete(key));
                }
                _ => break, // Corrupted type, stop replay
            }
        }
        
        // Seek to end so subsequent appends work correctly
        self.file.seek(SeekFrom::End(0))?;
        Ok(entries)
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
