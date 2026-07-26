//! Write-Ahead Log (WAL) module.
//!
//! Provides durability by logging all mutations and transactions before they are applied to the main data files.
//! Features LSN tracking, TxID framing, and CRC32 checksum integrity checks.

use crc32fast::Hasher;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Represents an operation logged in the WAL.
#[derive(Debug, Clone, PartialEq)]
pub enum WalEntry {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

/// Operation type tags in the WAL payload.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalOpType {
    Put = 1,
    Delete = 2,
    Begin = 3,
    Commit = 4,
    Rollback = 5,
}

/// The Write-Ahead Log structure.
pub struct WriteAheadLog {
    file: File,
    _path: PathBuf,
    lsn: u64,
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
            lsn: 0,
        })
    }

    /// Returns the current LSN.
    pub fn current_lsn(&self) -> u64 {
        self.lsn
    }

    /// Reads all committed entries from the WAL for crash recovery, verifying CRC32 checksums.
    /// Only replays entries with LSN > checkpoint_lsn, skipping already-checkpointed data.
    pub fn recover(&mut self, checkpoint_lsn: u64) -> io::Result<Vec<WalEntry>> {
        let mut committed_entries = Vec::new();
        let mut active_txs: HashMap<u64, Vec<WalEntry>> = HashMap::new();
        self.file.seek(SeekFrom::Start(0))?;

        let mut max_lsn = 0u64;

        loop {
            // Header: [LSN: 8B][TxID: 8B][Type: 1B][Len: 4B] -> Total 21 bytes
            let mut header_buf = [0u8; 21];
            if let Err(e) = self.file.read_exact(&mut header_buf) {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    break; // Clean EOF
                }
                break; // Corrupted / partial write at EOF
            }

            let lsn_bytes: [u8; 8] = match header_buf[0..8].try_into() {
                Ok(b) => b,
                Err(_) => break,
            };
            let lsn = u64::from_le_bytes(lsn_bytes);

            let tx_id_bytes: [u8; 8] = match header_buf[8..16].try_into() {
                Ok(b) => b,
                Err(_) => break,
            };
            let tx_id = u64::from_le_bytes(tx_id_bytes);

            let op_code = header_buf[16];

            let len_bytes: [u8; 4] = match header_buf[17..21].try_into() {
                Ok(b) => b,
                Err(_) => break,
            };
            let payload_len = u32::from_le_bytes(len_bytes) as usize;

            let mut payload = vec![0u8; payload_len];
            if self.file.read_exact(&mut payload).is_err() {
                break; // Corrupted / partial write
            }

            let mut crc_buf = [0u8; 4];
            if self.file.read_exact(&mut crc_buf).is_err() {
                break;
            }
            let expected_crc = u32::from_le_bytes(crc_buf);

            // Calculate checksum over header + payload
            let mut hasher = Hasher::new();
            hasher.update(&header_buf);
            hasher.update(&payload);
            let computed_crc = hasher.finalize();

            if computed_crc != expected_crc {
                // Checksum mismatch -> log corruption or incomplete write
                break;
            }

            if lsn > max_lsn {
                max_lsn = lsn;
            }

            // Skip entries that were already checkpointed
            if lsn <= checkpoint_lsn {
                // Still need to process BEGIN/COMMIT/ROLLBACK markers to track tx state
                match op_code {
                    3 => {
                        if tx_id > 0 {
                            active_txs.insert(tx_id, Vec::new());
                        }
                    }
                    4 => { active_txs.remove(&tx_id); }
                    5 => { active_txs.remove(&tx_id); }
                    _ => {}
                }
                continue;
            }

            match op_code {
                1 => {
                    // PUT: [KeyLen: 4B][Key][ValLen: 4B][Val]
                    if payload.len() < 8 {
                        break;
                    }
                    let klen_bytes: [u8; 4] = match payload[0..4].try_into() {
                        Ok(b) => b,
                        Err(_) => break,
                    };
                    let klen = u32::from_le_bytes(klen_bytes) as usize;
                    if payload.len() < 4 + klen + 4 {
                        break;
                    }
                    let key = payload[4..4 + klen].to_vec();
                    let vlen_offset = 4 + klen;
                    let vlen_bytes: [u8; 4] = match payload[vlen_offset..vlen_offset + 4].try_into() {
                        Ok(b) => b,
                        Err(_) => break,
                    };
                    let vlen = u32::from_le_bytes(vlen_bytes) as usize;
                    if payload.len() < vlen_offset + 4 + vlen {
                        break;
                    }
                    let val = payload[vlen_offset + 4..vlen_offset + 4 + vlen].to_vec();

                    let entry = WalEntry::Put(key, val);
                    if tx_id == 0 {
                        committed_entries.push(entry);
                    } else {
                        active_txs.entry(tx_id).or_default().push(entry);
                    }
                }
                2 => {
                    // DELETE: [KeyLen: 4B][Key]
                    if payload.len() < 4 {
                        break;
                    }
                    let klen_bytes: [u8; 4] = match payload[0..4].try_into() {
                        Ok(b) => b,
                        Err(_) => break,
                    };
                    let klen = u32::from_le_bytes(klen_bytes) as usize;
                    if payload.len() < 4 + klen {
                        break;
                    }
                    let key = payload[4..4 + klen].to_vec();

                    let entry = WalEntry::Delete(key);
                    if tx_id == 0 {
                        committed_entries.push(entry);
                    } else {
                        active_txs.entry(tx_id).or_default().push(entry);
                    }
                }
                3 => {
                    // BEGIN
                    if tx_id > 0 {
                        active_txs.insert(tx_id, Vec::new());
                    }
                }
                4 => {
                    // COMMIT
                    if let Some(entries) = active_txs.remove(&tx_id) {
                        committed_entries.extend(entries);
                    }
                }
                5 => {
                    // ROLLBACK / ABORT
                    active_txs.remove(&tx_id);
                }
                _ => break, // Unknown opcode
            }
        }

        self.lsn = max_lsn;
        self.file.seek(SeekFrom::End(0))?;
        Ok(committed_entries)
    }

    fn write_record(&mut self, tx_id: u64, op_type: WalOpType, payload: &[u8]) -> io::Result<u64> {
        self.lsn += 1;
        let lsn = self.lsn;

        let mut header = [0u8; 21];
        header[0..8].copy_from_slice(&lsn.to_le_bytes());
        header[8..16].copy_from_slice(&tx_id.to_le_bytes());
        header[16] = op_type as u8;
        header[17..21].copy_from_slice(&(payload.len() as u32).to_le_bytes());

        let mut hasher = Hasher::new();
        hasher.update(&header);
        hasher.update(payload);
        let crc = hasher.finalize();

        self.file.write_all(&header)?;
        self.file.write_all(payload)?;
        self.file.write_all(&crc.to_le_bytes())?;

        Ok(lsn)
    }

    /// Appends a PUT operation to the WAL (non-transactional).
    pub fn append_put(&mut self, key: &[u8], value: &[u8]) -> io::Result<u64> {
        self.append_tx_put(0, key, value)
    }

    /// Appends a transactional PUT operation to the WAL.
    pub fn append_tx_put(&mut self, tx_id: u64, key: &[u8], value: &[u8]) -> io::Result<u64> {
        let mut payload = Vec::with_capacity(4 + key.len() + 4 + value.len());
        payload.extend_from_slice(&(key.len() as u32).to_le_bytes());
        payload.extend_from_slice(key);
        payload.extend_from_slice(&(value.len() as u32).to_le_bytes());
        payload.extend_from_slice(value);

        self.write_record(tx_id, WalOpType::Put, &payload)
    }

    /// Appends a DELETE operation to the WAL (non-transactional).
    pub fn append_delete(&mut self, key: &[u8]) -> io::Result<u64> {
        self.append_tx_delete(0, key)
    }

    /// Appends a transactional DELETE operation to the WAL.
    pub fn append_tx_delete(&mut self, tx_id: u64, key: &[u8]) -> io::Result<u64> {
        let mut payload = Vec::with_capacity(4 + key.len());
        payload.extend_from_slice(&(key.len() as u32).to_le_bytes());
        payload.extend_from_slice(key);

        self.write_record(tx_id, WalOpType::Delete, &payload)
    }

    /// Logs BEGIN transaction record.
    pub fn append_begin(&mut self, tx_id: u64) -> io::Result<u64> {
        self.write_record(tx_id, WalOpType::Begin, &[])
    }

    /// Logs COMMIT transaction record.
    pub fn append_commit(&mut self, tx_id: u64) -> io::Result<u64> {
        self.write_record(tx_id, WalOpType::Commit, &[])
    }

    /// Logs ROLLBACK transaction record.
    pub fn append_rollback(&mut self, tx_id: u64) -> io::Result<u64> {
        self.write_record(tx_id, WalOpType::Rollback, &[])
    }

    /// Syncs the WAL to disk to ensure durability.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Checkpoints the WAL by clearing all entries.
    pub fn checkpoint(&mut self) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.sync_data()?;
        self.lsn = 0;
        Ok(())
    }
}
