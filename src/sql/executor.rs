use crate::sql::parser::Statement;
use crate::storage::Database;
use anyhow::Result;
use std::collections::BTreeMap;

fn encode_values(values: &[String]) -> Vec<u8> {
    let mut buf = Vec::new();
    for v in values {
        let bytes = v.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(bytes);
    }
    buf
}

fn decode_values(data: &[u8]) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("Corrupted row encoding"));
        }
        let len = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
        offset += 4;
        if offset + len > data.len() {
            return Err(anyhow::anyhow!("Corrupted row encoding"));
        }
        let s = String::from_utf8(data[offset..offset+len].to_vec())?;
        offset += len;
        values.push(s);
    }
    Ok(values)
}

pub struct Executor<'a> {
    db: &'a mut Database,
    in_transaction: bool,
    write_buffer: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl<'a> Executor<'a> {
    pub fn new(db: &'a mut Database) -> Self {
        Self { 
            db,
            in_transaction: false,
            write_buffer: BTreeMap::new(),
        }
    }

    pub fn execute(&mut self, stmt: Statement) -> Result<String> {
        match stmt {
            Statement::Begin => {
                if self.in_transaction {
                    return Err(anyhow::anyhow!("Transaction already in progress"));
                }
                self.in_transaction = true;
                self.write_buffer.clear();
                Ok("Query OK, transaction started.".to_string())
            }
            Statement::Commit => {
                if !self.in_transaction {
                    return Err(anyhow::anyhow!("No transaction in progress"));
                }
                for (key, value_opt) in &self.write_buffer {
                    match value_opt {
                        Some(val) => self.db.put(key, val)?,
                        None => self.db.delete(key)?,
                    }
                }
                self.write_buffer.clear();
                self.in_transaction = false;
                Ok("Query OK, transaction committed.".to_string())
            }
            Statement::Rollback => {
                if !self.in_transaction {
                    return Err(anyhow::anyhow!("No transaction in progress"));
                }
                self.write_buffer.clear();
                self.in_transaction = false;
                Ok("Query OK, transaction rolled back.".to_string())
            }
            Statement::CreateTable { table_name, columns } => {
                let schema_key = format!("__schema__:{}", table_name);
                
                if self.db.get(schema_key.as_bytes())?.is_some() {
                    return Err(anyhow::anyhow!("Table '{}' already exists", table_name));
                }
                
                let schema_val = columns.join(",");
                if self.in_transaction {
                    self.write_buffer.insert(schema_key.into_bytes(), Some(schema_val.into_bytes()));
                } else {
                    self.db.put(schema_key.as_bytes(), schema_val.as_bytes())?;
                }
                
                Ok(format!("Query OK, table '{}' created.", table_name))
            }
            Statement::Insert { table_name, values } => {
                let schema_key = format!("__schema__:{}", table_name);
                let schema_bytes = if self.in_transaction && self.write_buffer.contains_key(schema_key.as_bytes()) {
                    self.write_buffer.get(schema_key.as_bytes()).unwrap().clone()
                } else {
                    self.db.get(schema_key.as_bytes())?
                };

                let schema_bytes = schema_bytes
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                
                let schema_val = String::from_utf8(schema_bytes)?;
                let columns: Vec<&str> = schema_val.split(',').collect();
                
                if values.len() != columns.len() {
                    return Err(anyhow::anyhow!("Column count mismatch: expected {}, got {}", columns.len(), values.len()));
                }
                
                if values.is_empty() {
                    return Err(anyhow::anyhow!("No values provided for insertion"));
                }
                
                let pk = &values[0];
                let internal_key = format!("{}:{}", table_name, pk).into_bytes();
                let internal_val = encode_values(&values);

                if self.in_transaction {
                    self.write_buffer.insert(internal_key, Some(internal_val));
                } else {
                    self.db.put(&internal_key, &internal_val)?;
                }
                
                Ok("Query OK, 1 row inserted.".to_string())
            }
            Statement::Select { table_name } => {
                let schema_key = format!("__schema__:{}", table_name);
                let schema_bytes = if self.in_transaction && self.write_buffer.contains_key(schema_key.as_bytes()) {
                    self.write_buffer.get(schema_key.as_bytes()).unwrap().clone()
                } else {
                    self.db.get(schema_key.as_bytes())?
                };

                let schema_bytes = schema_bytes
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let header = schema_val.replace(",", " | ");

                let mut records = self.db.scan()?;
                let prefix = format!("{}:", table_name);
                
                // Overlay uncommitted writes
                if self.in_transaction {
                    let prefix_bytes = prefix.as_bytes();
                    for (k, v_opt) in &self.write_buffer {
                        if k.starts_with(prefix_bytes) {
                            // Remove from records if exists to update/delete
                            records.retain(|(rk, _)| rk != k);
                            if let Some(v) = v_opt {
                                records.push((k.clone(), v.clone()));
                            }
                        }
                    }
                }
                
                let mut output = format!("+-----------------+\n| {} |\n+-----------------+\n", header);
                let mut count = 0;
                
                for (key, val) in records {
                    let k_str = String::from_utf8_lossy(&key);
                    if k_str.starts_with(&prefix) {
                        let decoded = decode_values(&val)?;
                        let formatted_row = decoded.join(" | ");
                        output.push_str(&format!("| {} |\n", formatted_row));
                        count += 1;
                    }
                }
                
                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} row(s) in set.", count));
                
                Ok(output)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::tempdir;

    fn setup_db() -> Database {
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        config.storage.path = dir.path().to_path_buf();
        config.wal.enabled = false;
        Database::open(config).unwrap()
    }

    #[test]
    fn test_executor_comma_delimiter() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        // Create table
        exec.execute(Statement::CreateTable {
            table_name: "users".to_string(),
            columns: vec!["id".to_string(), "name".to_string(), "bio".to_string()],
        }).unwrap();

        // Insert row with comma
        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["1".to_string(), "Alice".to_string(), "Hello, world!".to_string()],
        }).unwrap();

        // Select and assert roundtrip
        let result = exec.execute(Statement::Select { table_name: "users".to_string() }).unwrap();
        
        assert!(result.contains("Hello, world!"));
        assert!(!result.contains("Hello |  world!")); // Should not be replaced
    }
}
