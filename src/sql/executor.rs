use crate::sql::parser::Statement;
use crate::storage::Database;
use anyhow::Result;
use std::collections::BTreeMap;

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
                let internal_val = values.join(",").into_bytes();

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
                        let v_str = String::from_utf8_lossy(&val);
                        let formatted_row = v_str.replace(",", " | ");
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
