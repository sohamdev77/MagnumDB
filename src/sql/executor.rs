use crate::sql::parser::Statement;
use crate::sql::volcano::{AggregateExec, AggregateFunc, ExecutionPlan, FilterExec, SeqScanExec};
use crate::storage::Database;
use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static TX_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

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
        let len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        if offset + len > data.len() {
            return Err(anyhow::anyhow!("Corrupted row encoding"));
        }
        let s = String::from_utf8(data[offset..offset + len].to_vec())?;
        offset += len;
        values.push(s);
    }
    Ok(values)
}

pub struct Executor<'a> {
    db: &'a mut Database,
    in_transaction: bool,
    current_tx_id: u64,
    write_buffer: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl<'a> Executor<'a> {
    pub fn new(db: &'a mut Database) -> Self {
        Self {
            db,
            in_transaction: false,
            current_tx_id: 0,
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
                self.current_tx_id = TX_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
                self.write_buffer.clear();
                self.db.begin_tx(self.current_tx_id)?;
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
                self.db.commit_tx(self.current_tx_id)?;
                self.write_buffer.clear();
                self.in_transaction = false;
                self.current_tx_id = 0;
                Ok("Query OK, transaction committed.".to_string())
            }
            Statement::Rollback => {
                if !self.in_transaction {
                    return Err(anyhow::anyhow!("No transaction in progress"));
                }
                self.db.rollback_tx(self.current_tx_id)?;
                self.write_buffer.clear();
                self.in_transaction = false;
                self.current_tx_id = 0;
                Ok("Query OK, transaction rolled back.".to_string())
            }
            Statement::CreateTable {
                table_name,
                columns,
            } => {
                let schema_key = format!("__schema__:{}", table_name);

                if self.get_schema_bytes(&table_name)?.is_some() {
                    return Err(anyhow::anyhow!("Table '{}' already exists", table_name));
                }

                let schema_val = columns.join(",");
                if self.in_transaction {
                    self.write_buffer
                        .insert(schema_key.into_bytes(), Some(schema_val.into_bytes()));
                } else {
                    self.db.put(schema_key.as_bytes(), schema_val.as_bytes())?;
                }

                Ok(format!("Query OK, table '{}' created.", table_name))
            }
            Statement::CreateIndex {
                index_name,
                table_name,
                column,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let idx_meta_key = format!("__index__:{}:{}", table_name, index_name);
                if self.db.get(idx_meta_key.as_bytes())?.is_some() {
                    return Err(anyhow::anyhow!("Index '{}' already exists", index_name));
                }

                self.db.put(idx_meta_key.as_bytes(), column.as_bytes())?;

                // Populate secondary index from existing records
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_defs: Vec<&str> = schema_val.split(',').collect();
                let col_names: Vec<&str> = col_defs
                    .iter()
                    .map(|c| c.split_whitespace().next().unwrap_or(c.trim()))
                    .collect();

                let col_idx = col_names.iter().position(|c| c == &column).ok_or_else(|| {
                    anyhow::anyhow!("Column '{}' not found in table '{}'", column, table_name)
                })?;

                let prefix = format!("{}:", table_name);
                let records = self.db.scan_prefix(prefix.as_bytes())?;

                for (key, val) in records {
                    let decoded = decode_values(&val)?;
                    if col_idx < decoded.len() {
                        let col_val = &decoded[col_idx];
                        let pk = String::from_utf8_lossy(&key[prefix.len()..]).to_string();
                        let sec_key =
                            format!("__secidx__:{}:{}:{}", table_name, column, col_val);
                        self.db.put(sec_key.as_bytes(), pk.as_bytes())?;
                    }
                }

                Ok(format!(
                    "Query OK, index '{}' created on '{}({})'.",
                    index_name, table_name, column
                ))
            }
            Statement::Insert { table_name, values } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let schema_val = String::from_utf8(schema_bytes)?;
                let col_defs: Vec<&str> = schema_val.split(',').collect();
                let col_names: Vec<&str> = col_defs
                    .iter()
                    .map(|c| c.split_whitespace().next().unwrap_or(c.trim()))
                    .collect();

                if values.len() != col_defs.len() {
                    return Err(anyhow::anyhow!(
                        "Column count mismatch: expected {}, got {}",
                        col_defs.len(),
                        values.len()
                    ));
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

                // Sync secondary index entries if any exist
                let idx_prefix = format!("__index__:{}:", table_name);
                let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
                for (idx_key, col_val_bytes) in indices {
                    let col_name = String::from_utf8_lossy(&col_val_bytes).to_string();
                    if let Some(col_idx) = col_names.iter().position(|c| c == &col_name) {
                        if col_idx < values.len() {
                            let sec_val = &values[col_idx];
                            let sec_key =
                                format!("__secidx__:{}:{}:{}", table_name, col_name, sec_val);
                            if self.in_transaction {
                                self.write_buffer
                                    .insert(sec_key.into_bytes(), Some(pk.as_bytes().to_vec()));
                            } else {
                                self.db.put(sec_key.as_bytes(), pk.as_bytes())?;
                            }
                        }
                    }
                    let _ = idx_key;
                }

                Ok("Query OK, 1 row inserted.".to_string())
            }
            Statement::Select {
                table_name,
                where_clause,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_defs: Vec<&str> = schema_val.split(',').collect();
                let col_names: Vec<&str> = col_defs
                    .iter()
                    .map(|c| c.split_whitespace().next().unwrap_or(c.trim()))
                    .collect();

                let header = col_names.join(" | ");

                let prefix = format!("{}:", table_name);
                let mut records = self.db.scan_prefix(prefix.as_bytes())?;

                if self.in_transaction {
                    let prefix_bytes = prefix.as_bytes();
                    for (k, v_opt) in &self.write_buffer {
                        if k.starts_with(prefix_bytes) {
                            records.retain(|(rk, _)| rk != k);
                            if let Some(v) = v_opt {
                                records.push((k.clone(), v.clone()));
                            }
                        }
                    }
                }

                let rows: Result<Vec<Vec<String>>> =
                    records.iter().map(|(_, val)| decode_values(val)).collect();
                let rows = rows?;

                let scan_op: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(rows));

                let mut plan: Box<dyn ExecutionPlan> = if let Some((ref filter_col, ref filter_val)) =
                    where_clause
                {
                    let col_idx = col_names.iter().position(|name| name == filter_col).ok_or_else(
                        || anyhow::anyhow!("Column '{}' not found", filter_col),
                    )?;
                    Box::new(FilterExec::new(scan_op, col_idx, filter_val.clone()))
                } else {
                    scan_op
                };

                plan.open()?;

                let mut output =
                    format!("+-----------------+\n| {} |\n+-----------------+\n", header);
                let mut count = 0;

                while let Some(row) = plan.next()? {
                    let formatted_row = row.join(" | ");
                    output.push_str(&format!("| {} |\n", formatted_row));
                    count += 1;
                }

                plan.close()?;

                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} row(s) in set.", count));

                Ok(output)
            }
            Statement::SelectRange {
                table_name,
                column,
                op,
                val,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_defs: Vec<&str> = schema_val.split(',').collect();
                let col_names: Vec<&str> = col_defs
                    .iter()
                    .map(|c| c.split_whitespace().next().unwrap_or(c.trim()))
                    .collect();

                let col_idx = col_names
                    .iter()
                    .position(|c| c == &column)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", column))?;

                let header = col_names.join(" | ");
                let prefix = format!("{}:", table_name);
                let records = self.db.scan_prefix(prefix.as_bytes())?;

                let mut output =
                    format!("+-----------------+\n| {} |\n+-----------------+\n", header);
                let mut count = 0;

                for (_key, rval) in records {
                    let decoded = decode_values(&rval)?;
                    if col_idx < decoded.len() {
                        let row_v = &decoded[col_idx];
                        let num_row = row_v.parse::<f64>();
                        let num_target = val.parse::<f64>();

                        let matches = if let (Ok(r_num), Ok(t_num)) = (num_row, num_target) {
                            match op.as_str() {
                                ">=" => r_num >= t_num,
                                "<=" => r_num <= t_num,
                                ">" => r_num > t_num,
                                "<" => r_num < t_num,
                                _ => false,
                            }
                        } else {
                            match op.as_str() {
                                ">=" => row_v >= &val,
                                "<=" => row_v <= &val,
                                ">" => row_v > &val,
                                "<" => row_v < &val,
                                _ => false,
                            }
                        };

                        if matches {
                            let formatted_row = decoded.join(" | ");
                            output.push_str(&format!("| {} |\n", formatted_row));
                            count += 1;
                        }
                    }
                }

                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} row(s) in set.", count));

                Ok(output)
            }
            Statement::SelectAggregate {
                func,
                column,
                table_name,
                where_clause,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_defs: Vec<&str> = schema_val.split(',').collect();
                let col_names: Vec<&str> = col_defs
                    .iter()
                    .map(|c| c.split_whitespace().next().unwrap_or(c.trim()))
                    .collect();

                let prefix = format!("{}:", table_name);
                let records = self.db.scan_prefix(prefix.as_bytes())?;
                let rows: Result<Vec<Vec<String>>> =
                    records.iter().map(|(_, val)| decode_values(val)).collect();
                let rows = rows?;

                let scan_op: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(rows));

                let filtered_op: Box<dyn ExecutionPlan> =
                    if let Some((ref filter_col, ref filter_val)) = where_clause {
                        let col_idx = col_names.iter().position(|name| name == filter_col).ok_or_else(
                            || anyhow::anyhow!("Column '{}' not found", filter_col),
                        )?;
                        Box::new(FilterExec::new(scan_op, col_idx, filter_val.clone()))
                    } else {
                        scan_op
                    };

                let agg_func = match func.as_str() {
                    "COUNT" => AggregateFunc::Count,
                    "SUM" => {
                        let col_idx = col_names.iter().position(|name| name == &column).ok_or_else(
                            || anyhow::anyhow!("Column '{}' not found", column),
                        )?;
                        AggregateFunc::Sum(col_idx)
                    }
                    "AVG" => {
                        let col_idx = col_names.iter().position(|name| name == &column).ok_or_else(
                            || anyhow::anyhow!("Column '{}' not found", column),
                        )?;
                        AggregateFunc::Avg(col_idx)
                    }
                    _ => return Err(anyhow::anyhow!("Unsupported aggregate function {}", func)),
                };

                let mut plan = AggregateExec::new(filtered_op, agg_func);
                plan.open()?;

                let res_row = plan.next()?.unwrap_or_else(|| vec!["0".to_string()]);
                plan.close()?;

                let header = format!("{}({})", func, column);
                let mut output =
                    format!("+-----------------+\n| {} |\n+-----------------+\n", header);
                output.push_str(&format!("| {} |\n", res_row.join(" | ")));
                output.push_str("+-----------------+\n1 row in set.");

                Ok(output)
            }
            Statement::Update {
                table_name,
                column,
                value,
                pk_val,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_defs: Vec<&str> = schema_val.split(',').collect();
                let col_names: Vec<&str> = col_defs
                    .iter()
                    .map(|c| c.split_whitespace().next().unwrap_or(c.trim()))
                    .collect();

                let col_idx = col_names
                    .iter()
                    .position(|name| name == &column)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Column '{}' not found in table '{}'",
                            column,
                            table_name
                        )
                    })?;

                let internal_key = format!("{}:{}", table_name, pk_val).into_bytes();
                let existing_bytes = if self.in_transaction
                    && self.write_buffer.contains_key(&internal_key)
                {
                    self.write_buffer.get(&internal_key).unwrap().clone()
                } else {
                    self.db.get(&internal_key)?
                };

                let existing_bytes = existing_bytes
                    .ok_or_else(|| anyhow::anyhow!("Row with primary key '{}' not found", pk_val))?;

                let mut row_values = decode_values(&existing_bytes)?;
                if col_idx >= row_values.len() {
                    return Err(anyhow::anyhow!("Corrupted row length"));
                }
                row_values[col_idx] = value;

                let updated_bytes = encode_values(&row_values);

                if self.in_transaction {
                    self.write_buffer.insert(internal_key, Some(updated_bytes));
                } else {
                    self.db.put(&internal_key, &updated_bytes)?;
                }

                Ok("Query OK, 1 row updated.".to_string())
            }
            Statement::Delete { table_name, pk_val } => {
                let internal_key = format!("{}:{}", table_name, pk_val).into_bytes();

                if self.in_transaction {
                    self.write_buffer.insert(internal_key, None);
                } else {
                    self.db.delete(&internal_key)?;
                }

                Ok("Query OK, 1 row deleted.".to_string())
            }
            Statement::DropTable { table_name } => {
                let schema_key = format!("__schema__:{}", table_name);

                if self.get_schema_bytes(&table_name)?.is_none() {
                    return Err(anyhow::anyhow!("Table '{}' does not exist", table_name));
                }

                let prefix = format!("{}:", table_name);
                let records = self.db.scan_prefix(prefix.as_bytes())?;

                if self.in_transaction {
                    self.write_buffer.insert(schema_key.into_bytes(), None);
                    for (k, _) in records {
                        self.write_buffer.insert(k, None);
                    }
                } else {
                    self.db.delete(schema_key.as_bytes())?;
                    for (k, _) in records {
                        self.db.delete(&k)?;
                    }
                }

                Ok(format!("Query OK, table '{}' dropped.", table_name))
            }
            Statement::ShowTables => {
                let prefix = b"__schema__:";
                let records = self.db.scan_prefix(prefix)?;

                let mut tables = Vec::new();
                for (key, _) in records {
                    let k_str = String::from_utf8_lossy(&key);
                    if let Some(tname) = k_str.strip_prefix("__schema__:") {
                        tables.push(tname.to_string());
                    }
                }

                let mut output =
                    "+-----------------+\n| Tables |\n+-----------------+\n".to_string();
                for t in &tables {
                    output.push_str(&format!("| {} |\n", t));
                }
                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} table(s) in set.", tables.len()));

                Ok(output)
            }
        }
    }

    fn get_schema_bytes(&mut self, table_name: &str) -> Result<Option<Vec<u8>>> {
        let schema_key = format!("__schema__:{}", table_name);
        if self.in_transaction && self.write_buffer.contains_key(schema_key.as_bytes()) {
            Ok(self.write_buffer.get(schema_key.as_bytes()).unwrap().clone())
        } else {
            self.db.get(schema_key.as_bytes())
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
    fn test_executor_volcano_aggregates() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "employees".to_string(),
            columns: vec!["id INT".to_string(), "name TEXT".to_string(), "salary INT".to_string()],
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "employees".to_string(),
            values: vec!["1".to_string(), "Alice".to_string(), "100".to_string()],
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "employees".to_string(),
            values: vec!["2".to_string(), "Bob".to_string(), "200".to_string()],
        })
        .unwrap();

        let count_res = exec
            .execute(Statement::SelectAggregate {
                func: "COUNT".to_string(),
                column: "*".to_string(),
                table_name: "employees".to_string(),
                where_clause: None,
            })
            .unwrap();
        assert!(count_res.contains("2"));

        let sum_res = exec
            .execute(Statement::SelectAggregate {
                func: "SUM".to_string(),
                column: "salary".to_string(),
                table_name: "employees".to_string(),
                where_clause: None,
            })
            .unwrap();
        assert!(sum_res.contains("300"));
    }

    #[test]
    fn test_executor_secondary_index() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "users".to_string(),
            columns: vec!["id INT".to_string(), "name TEXT".to_string()],
        })
        .unwrap();

        exec.execute(Statement::CreateIndex {
            index_name: "idx_users_name".to_string(),
            table_name: "users".to_string(),
            column: "name".to_string(),
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["1".to_string(), "Charlie".to_string()],
        })
        .unwrap();

        let sec_key = b"__secidx__:users:name:Charlie";
        let pk = db.get(sec_key).unwrap().unwrap();
        assert_eq!(pk, b"1");
    }

    #[test]
    fn test_executor_select_range() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "users".to_string(),
            columns: vec!["id INT".to_string(), "age INT".to_string()],
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["1".to_string(), "20".to_string()],
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["2".to_string(), "30".to_string()],
        })
        .unwrap();

        let res = exec
            .execute(Statement::SelectRange {
                table_name: "users".to_string(),
                column: "age".to_string(),
                op: ">=".to_string(),
                val: "25".to_string(),
            })
            .unwrap();

        assert!(res.contains("30"));
        assert!(!res.contains("20"));
    }
}
