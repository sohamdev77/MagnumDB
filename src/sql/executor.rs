use crate::sql::parser::Statement;
use crate::sql::volcano::{
    AggregateExec, AggregateFunc, ExecutionPlan, FilterExec, HashGroupAggregateExec, HashJoinExec,
    JoinType, SeqScanExec,
};
use crate::storage::Database;
use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static TX_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Encodes row values with optional MVCC header `[xmin: 8B][xmax: 8B]`.
#[allow(dead_code)]
fn encode_values(values: &[String]) -> Vec<u8> {
    encode_values_mvcc(values, 1, 0)
}

fn encode_values_mvcc(values: &[String], xmin: u64, xmax: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    // MVCC Header: 16 bytes (xmin: 8B, xmax: 8B)
    buf.extend_from_slice(&xmin.to_le_bytes());
    buf.extend_from_slice(&xmax.to_le_bytes());

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

    // Check if data contains 16-byte MVCC header
    if data.len() >= 16 {
        // Skip xmin (8B) and xmax (8B) header for string value decoding
        offset = 16;
    }

    while offset < data.len() {
        if offset + 4 > data.len() {
            // Fallback for legacy format without MVCC header
            offset = 0;
            values.clear();
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
            return Ok(values);
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

/// Separator used to store multiple PKs in a secondary index entry.
const SEC_IDX_PK_SEP: &str = "\x1F"; // ASCII Unit Separator

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
                let tx_id = self.current_tx_id;
                for (key, value_opt) in &self.write_buffer {
                    match value_opt {
                        Some(val) => self.db.put_with_tx(tx_id, key, val)?,
                        None => self.db.delete_with_tx(tx_id, key)?,
                    }
                }
                self.db.commit_tx(tx_id)?;
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
                columns,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let idx_meta_key = format!("__index__:{}:{}", table_name, index_name);
                if self.db.get(idx_meta_key.as_bytes())?.is_some() {
                    return Err(anyhow::anyhow!("Index '{}' already exists", index_name));
                }

                let cols_joined = columns.join(",");
                self.db.put(idx_meta_key.as_bytes(), cols_joined.as_bytes())?;

                // Populate secondary index from existing records
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let mut col_indices = Vec::new();
                for col in &columns {
                    let idx = col_names.iter().position(|c| c == col).ok_or_else(|| {
                        anyhow::anyhow!("Column '{}' not found in table '{}'", col, table_name)
                    })?;
                    col_indices.push(idx);
                }

                let prefix = format!("{}:", table_name);
                let records = self.db.scan_prefix(prefix.as_bytes())?;

                for (key, val) in records {
                    let decoded = decode_values(&val)?;
                    let mut col_vals = Vec::new();
                    for &c_idx in &col_indices {
                        if c_idx < decoded.len() {
                            col_vals.push(decoded[c_idx].clone());
                        }
                    }
                    if col_vals.len() == columns.len() {
                        let composite_val = col_vals.join("+");
                        let pk = String::from_utf8_lossy(&key[prefix.len()..]).to_string();
                        let column_key = columns.join("+");
                        self.sec_idx_add(&table_name, &column_key, &composite_val, &pk)?;
                    }
                }

                Ok(format!(
                    "Query OK, index '{}' created on '{}({})'.",
                    index_name, table_name, cols_joined
                ))
            }
            Statement::Insert { table_name, values } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);
                let col_defs: Vec<&str> = schema_val.split(',').collect();

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
                let active_tx = if self.in_transaction { self.current_tx_id } else { 1 };
                let internal_val = encode_values_mvcc(&values, active_tx, 0);

                if self.in_transaction {
                    self.write_buffer.insert(internal_key, Some(internal_val));
                } else {
                    self.db.put(&internal_key, &internal_val)?;
                }

                // Sync secondary index entries if any exist
                self.sync_secondary_indexes_on_insert(&table_name, &col_names, &values, pk)?;

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
                let col_names = Self::extract_col_names(&schema_val);

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
            Statement::SelectJoin {
                left_table,
                right_table,
                left_col,
                right_col,
                is_left_join,
            } => {
                let left_schema_bytes = self
                    .get_schema_bytes(&left_table)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", left_table))?;
                let right_schema_bytes = self
                    .get_schema_bytes(&right_table)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", right_table))?;

                let left_schema_val = String::from_utf8(left_schema_bytes)?;
                let right_schema_val = String::from_utf8(right_schema_bytes)?;

                let left_cols = Self::extract_col_names(&left_schema_val);
                let right_cols = Self::extract_col_names(&right_schema_val);

                let left_key_idx = left_cols
                    .iter()
                    .position(|c| c == &left_col)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found in '{}'", left_col, left_table))?;
                let right_key_idx = right_cols
                    .iter()
                    .position(|c| c == &right_col)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found in '{}'", right_col, right_table))?;

                let left_prefix = format!("{}:", left_table);
                let right_prefix = format!("{}:", right_table);

                let left_records = self.db.scan_prefix(left_prefix.as_bytes())?;
                let right_records = self.db.scan_prefix(right_prefix.as_bytes())?;

                let left_rows: Result<Vec<Vec<String>>> =
                    left_records.iter().map(|(_, v)| decode_values(v)).collect();
                let right_rows: Result<Vec<Vec<String>>> =
                    right_records.iter().map(|(_, v)| decode_values(v)).collect();

                let left_exec: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(left_rows?));
                let right_exec: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(right_rows?));

                let join_type = if is_left_join { JoinType::Left } else { JoinType::Inner };
                let mut join_plan = HashJoinExec::new(
                    left_exec,
                    right_exec,
                    left_key_idx,
                    right_key_idx,
                    join_type,
                    right_cols.len(),
                );

                join_plan.open()?;

                let mut all_headers = Vec::new();
                for c in &left_cols {
                    all_headers.push(format!("{}.{}", left_table, c));
                }
                for c in &right_cols {
                    all_headers.push(format!("{}.{}", right_table, c));
                }

                let header_str = all_headers.join(" | ");
                let mut output =
                    format!("+-----------------+\n| {} |\n+-----------------+\n", header_str);
                let mut count = 0;

                while let Some(joined_row) = join_plan.next()? {
                    output.push_str(&format!("| {} |\n", joined_row.join(" | ")));
                    count += 1;
                }

                join_plan.close()?;

                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} row(s) in set.", count));

                Ok(output)
            }
            Statement::SelectGroupAggregate {
                group_col,
                func,
                agg_col,
                table_name,
                where_clause,
                having_clause,
            } => {
                let schema_bytes = self
                    .get_schema_bytes(&table_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let group_col_idx = col_names
                    .iter()
                    .position(|c| c == &group_col)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found in '{}'", group_col, table_name))?;

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
                        let col_idx = col_names.iter().position(|name| name == &agg_col).ok_or_else(
                            || anyhow::anyhow!("Column '{}' not found", agg_col),
                        )?;
                        AggregateFunc::Sum(col_idx)
                    }
                    "AVG" => {
                        let col_idx = col_names.iter().position(|name| name == &agg_col).ok_or_else(
                            || anyhow::anyhow!("Column '{}' not found", agg_col),
                        )?;
                        AggregateFunc::Avg(col_idx)
                    }
                    _ => return Err(anyhow::anyhow!("Unsupported aggregate function {}", func)),
                };

                let mut group_plan = HashGroupAggregateExec::new(
                    filtered_op,
                    group_col_idx,
                    agg_func,
                    having_clause,
                );

                group_plan.open()?;

                let header = format!("{} | {}({})", group_col, func, agg_col);
                let mut output =
                    format!("+-----------------+\n| {} |\n+-----------------+\n", header);
                let mut count = 0;

                while let Some(res_row) = group_plan.next()? {
                    output.push_str(&format!("| {} |\n", res_row.join(" | ")));
                    count += 1;
                }

                group_plan.close()?;

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
                let col_names = Self::extract_col_names(&schema_val);

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
                let col_names = Self::extract_col_names(&schema_val);

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
                let col_names = Self::extract_col_names(&schema_val);

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
                    self.write_buffer.get(&internal_key).cloned().flatten()
                } else {
                    self.db.get(&internal_key)?
                };

                let existing_bytes = existing_bytes
                    .ok_or_else(|| anyhow::anyhow!("Row with primary key '{}' not found", pk_val))?;

                let mut row_values = decode_values(&existing_bytes)?;
                if col_idx >= row_values.len() {
                    return Err(anyhow::anyhow!("Corrupted row length"));
                }

                // Remove old secondary index entries for the changed column
                let old_value = row_values[col_idx].clone();
                self.sec_idx_remove_for_column(&table_name, &column, &old_value, &pk_val)?;

                row_values[col_idx] = value.clone();

                // Add new secondary index entries for the updated column
                self.sec_idx_add_for_column(&table_name, &column, &value, &pk_val)?;

                let active_tx = if self.in_transaction { self.current_tx_id } else { 1 };
                let updated_bytes = encode_values_mvcc(&row_values, active_tx, 0);

                if self.in_transaction {
                    self.write_buffer.insert(internal_key, Some(updated_bytes));
                } else {
                    self.db.put(&internal_key, &updated_bytes)?;
                }

                Ok("Query OK, 1 row updated.".to_string())
            }
            Statement::Delete { table_name, pk_val } => {
                let internal_key = format!("{}:{}", table_name, pk_val).into_bytes();

                // Read the row before deleting to clean up secondary indexes
                let existing_bytes = if self.in_transaction
                    && self.write_buffer.contains_key(&internal_key)
                {
                    self.write_buffer.get(&internal_key).cloned().flatten()
                } else {
                    self.db.get(&internal_key)?
                };

                if let Some(row_data) = existing_bytes {
                    // Clean up secondary indexes
                    if let Ok(Some(sb)) = self.get_schema_bytes(&table_name) {
                        if let Ok(schema_val) = String::from_utf8(sb) {
                            let col_names = Self::extract_col_names(&schema_val);
                            if let Ok(decoded) = decode_values(&row_data) {
                                self.remove_all_secondary_indexes(
                                    &table_name, &col_names, &decoded, &pk_val,
                                )?;
                            }
                        }
                    }
                }

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

                // Clean up secondary indexes and index metadata
                let idx_prefix = format!("__index__:{}:", table_name);
                let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
                for (idx_key, _) in &indices {
                    self.db.delete(idx_key)?;
                }
                // Clean up all secidx entries for this table
                let sec_prefix = format!("__secidx__:{}:", table_name);
                let sec_entries = self.db.scan_prefix(sec_prefix.as_bytes())?;
                for (sec_key, _) in sec_entries {
                    self.db.delete(&sec_key)?;
                }

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

    // ---- Helper methods ----

    fn get_schema_bytes(&mut self, table_name: &str) -> Result<Option<Vec<u8>>> {
        let schema_key = format!("__schema__:{}", table_name);
        if self.in_transaction && self.write_buffer.contains_key(schema_key.as_bytes()) {
            Ok(self.write_buffer.get(schema_key.as_bytes()).cloned().flatten())
        } else {
            self.db.get(schema_key.as_bytes())
        }
    }

    /// Extracts column names from a schema string like "id INT,name TEXT,age INT".
    fn extract_col_names(schema_val: &str) -> Vec<String> {
        schema_val
            .split(',')
            .map(|c| {
                c.split_whitespace()
                    .next()
                    .unwrap_or(c.trim())
                    .to_string()
            })
            .collect()
    }

    // ---- Secondary Index Helpers ----

    /// Adds a PK to a secondary index entry (multi-value safe).
    fn sec_idx_add(&mut self, table: &str, column: &str, col_val: &str, pk: &str) -> Result<()> {
        let sec_key = format!("__secidx__:{}:{}:{}", table, column, col_val);
        let existing = self.db.get(sec_key.as_bytes())?;

        let new_val = match existing {
            Some(data) => {
                let existing_str = String::from_utf8_lossy(&data).to_string();
                // Check if PK already exists
                let pks: Vec<&str> = existing_str.split(SEC_IDX_PK_SEP).collect();
                if pks.contains(&pk) {
                    return Ok(()); // Already indexed
                }
                format!("{}{}{}", existing_str, SEC_IDX_PK_SEP, pk)
            }
            None => pk.to_string(),
        };

        if self.in_transaction {
            self.write_buffer
                .insert(sec_key.into_bytes(), Some(new_val.into_bytes()));
        } else {
            self.db.put(sec_key.as_bytes(), new_val.as_bytes())?;
        }
        Ok(())
    }

    /// Removes a PK from a secondary index entry (multi-value safe).
    fn sec_idx_remove(&mut self, table: &str, column: &str, col_val: &str, pk: &str) -> Result<()> {
        let sec_key = format!("__secidx__:{}:{}:{}", table, column, col_val);
        let existing = self.db.get(sec_key.as_bytes())?;

        if let Some(data) = existing {
            let existing_str = String::from_utf8_lossy(&data).to_string();
            let pks: Vec<&str> = existing_str
                .split(SEC_IDX_PK_SEP)
                .filter(|p| *p != pk)
                .collect();

            if pks.is_empty() {
                if self.in_transaction {
                    self.write_buffer.insert(sec_key.into_bytes(), None);
                } else {
                    self.db.delete(sec_key.as_bytes())?;
                }
            } else {
                let new_val = pks.join(SEC_IDX_PK_SEP);
                if self.in_transaction {
                    self.write_buffer
                        .insert(sec_key.into_bytes(), Some(new_val.into_bytes()));
                } else {
                    self.db.put(sec_key.as_bytes(), new_val.as_bytes())?;
                }
            }
        }
        Ok(())
    }

    /// Syncs secondary indexes when inserting a new row.
    fn sync_secondary_indexes_on_insert(
        &mut self,
        table_name: &str,
        col_names: &[String],
        values: &[String],
        pk: &str,
    ) -> Result<()> {
        let idx_prefix = format!("__index__:{}:", table_name);
        let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
        for (_idx_key, col_val_bytes) in indices {
            let col_names_spec = String::from_utf8_lossy(&col_val_bytes).to_string();
            let cols: Vec<&str> = col_names_spec.split(',').collect();

            let mut col_vals = Vec::new();
            for col in &cols {
                if let Some(col_idx) = col_names.iter().position(|c| c == col) {
                    if col_idx < values.len() {
                        col_vals.push(values[col_idx].clone());
                    }
                }
            }

            if col_vals.len() == cols.len() {
                let composite_val = col_vals.join("+");
                let column_key = cols.join("+");
                self.sec_idx_add(table_name, &column_key, &composite_val, pk)?;
            }
        }
        Ok(())
    }

    /// Removes secondary index entries for a specific column value.
    fn sec_idx_remove_for_column(
        &mut self,
        table_name: &str,
        column: &str,
        col_val: &str,
        pk: &str,
    ) -> Result<()> {
        let idx_prefix = format!("__index__:{}:", table_name);
        let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
        for (_idx_key, col_val_bytes) in indices {
            let indexed_col = String::from_utf8_lossy(&col_val_bytes).to_string();
            if indexed_col == column {
                self.sec_idx_remove(table_name, column, col_val, pk)?;
            }
        }
        Ok(())
    }

    /// Adds secondary index entries for a specific column value.
    fn sec_idx_add_for_column(
        &mut self,
        table_name: &str,
        column: &str,
        col_val: &str,
        pk: &str,
    ) -> Result<()> {
        let idx_prefix = format!("__index__:{}:", table_name);
        let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
        for (_idx_key, col_val_bytes) in indices {
            let indexed_col = String::from_utf8_lossy(&col_val_bytes).to_string();
            if indexed_col == column {
                self.sec_idx_add(table_name, column, col_val, pk)?;
            }
        }
        Ok(())
    }

    /// Removes all secondary index entries for a row being deleted.
    fn remove_all_secondary_indexes(
        &mut self,
        table_name: &str,
        col_names: &[String],
        row_values: &[String],
        pk: &str,
    ) -> Result<()> {
        let idx_prefix = format!("__index__:{}:", table_name);
        let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
        for (_idx_key, col_val_bytes) in indices {
            let col_spec = String::from_utf8_lossy(&col_val_bytes).to_string();
            let cols: Vec<&str> = col_spec.split(',').collect();

            let mut col_vals = Vec::new();
            for col in &cols {
                if let Some(col_idx) = col_names.iter().position(|c| c == col) {
                    if col_idx < row_values.len() {
                        col_vals.push(row_values[col_idx].clone());
                    }
                }
            }

            if col_vals.len() == cols.len() {
                let composite_val = col_vals.join("+");
                let column_key = cols.join("+");
                self.sec_idx_remove(table_name, &column_key, &composite_val, pk)?;
            }
        }
        Ok(())
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
        config.storage.sync_interval = 0;
        Database::open(config).unwrap()
    }

    #[test]
    fn test_executor_join_inner_and_left() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "users".to_string(),
            columns: vec!["id INT".to_string(), "name TEXT".to_string()],
        }).unwrap();

        exec.execute(Statement::CreateTable {
            table_name: "orders".to_string(),
            columns: vec!["id INT".to_string(), "user_id INT".to_string(), "amount INT".to_string()],
        }).unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["1".to_string(), "Alice".to_string()],
        }).unwrap();
        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["2".to_string(), "Bob".to_string()],
        }).unwrap();

        exec.execute(Statement::Insert {
            table_name: "orders".to_string(),
            values: vec!["100".to_string(), "1".to_string(), "500".to_string()],
        }).unwrap();

        // Inner Join
        let res_inner = exec.execute(Statement::SelectJoin {
            left_table: "users".to_string(),
            right_table: "orders".to_string(),
            left_col: "id".to_string(),
            right_col: "user_id".to_string(),
            is_left_join: false,
        }).unwrap();

        assert!(res_inner.contains("Alice"));
        assert!(res_inner.contains("500"));
        assert!(!res_inner.contains("Bob"));

        // Left Join
        let res_left = exec.execute(Statement::SelectJoin {
            left_table: "users".to_string(),
            right_table: "orders".to_string(),
            left_col: "id".to_string(),
            right_col: "user_id".to_string(),
            is_left_join: true,
        }).unwrap();

        assert!(res_left.contains("Alice"));
        assert!(res_left.contains("Bob"));
        assert!(res_left.contains("NULL"));
    }

    #[test]
    fn test_executor_group_by_having() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "employees".to_string(),
            columns: vec!["id INT".to_string(), "dept TEXT".to_string(), "salary INT".to_string()],
        }).unwrap();

        exec.execute(Statement::Insert {
            table_name: "employees".to_string(),
            values: vec!["1".to_string(), "Eng".to_string(), "100".to_string()],
        }).unwrap();
        exec.execute(Statement::Insert {
            table_name: "employees".to_string(),
            values: vec!["2".to_string(), "Eng".to_string(), "200".to_string()],
        }).unwrap();
        exec.execute(Statement::Insert {
            table_name: "employees".to_string(),
            values: vec!["3".to_string(), "HR".to_string(), "50".to_string()],
        }).unwrap();

        let res = exec.execute(Statement::SelectGroupAggregate {
            group_col: "dept".to_string(),
            func: "COUNT".to_string(),
            agg_col: "*".to_string(),
            table_name: "employees".to_string(),
            where_clause: None,
            having_clause: Some((">".to_string(), "1".to_string())),
        }).unwrap();

        assert!(res.contains("Eng"));
        assert!(!res.contains("HR"));
    }

    #[test]
    fn test_executor_secondary_index() {
        let mut db = setup_db();
        {
            let mut exec = Executor::new(&mut db);
            exec.execute(Statement::CreateTable {
                table_name: "users".to_string(),
                columns: vec!["id INT".to_string(), "name TEXT".to_string()],
            }).unwrap();

            exec.execute(Statement::CreateIndex {
                index_name: "idx_name".to_string(),
                table_name: "users".to_string(),
                columns: vec!["name".to_string()],
            }).unwrap();

            exec.execute(Statement::Insert {
                table_name: "users".to_string(),
                values: vec!["1".to_string(), "Charlie".to_string()],
            }).unwrap();
        }

        let sec_key = b"__secidx__:users:name:Charlie";
        let pk = db.get(sec_key).unwrap().unwrap();
        assert_eq!(pk, b"1");
    }

    #[test]
    fn test_executor_secondary_index_multi_pk() {
        let mut db = setup_db();
        {
            let mut exec = Executor::new(&mut db);
            exec.execute(Statement::CreateTable {
                table_name: "users".to_string(),
                columns: vec!["id INT".to_string(), "name TEXT".to_string()],
            }).unwrap();

            exec.execute(Statement::CreateIndex {
                index_name: "idx_name".to_string(),
                table_name: "users".to_string(),
                columns: vec!["name".to_string()],
            }).unwrap();

            exec.execute(Statement::Insert {
                table_name: "users".to_string(),
                values: vec!["1".to_string(), "Alice".to_string()],
            }).unwrap();

            exec.execute(Statement::Insert {
                table_name: "users".to_string(),
                values: vec!["2".to_string(), "Alice".to_string()],
            }).unwrap();
        }

        let sec_key = b"__secidx__:users:name:Alice";
        let pks = db.get(sec_key).unwrap().unwrap();
        let pks_str = String::from_utf8(pks).unwrap();
        assert!(pks_str.contains("1"));
        assert!(pks_str.contains("2"));
    }

    #[test]
    fn test_executor_delete_cleans_secondary_index() {
        let mut db = setup_db();
        {
            let mut exec = Executor::new(&mut db);
            exec.execute(Statement::CreateTable {
                table_name: "users".to_string(),
                columns: vec!["id INT".to_string(), "name TEXT".to_string()],
            }).unwrap();

            exec.execute(Statement::CreateIndex {
                index_name: "idx_name".to_string(),
                table_name: "users".to_string(),
                columns: vec!["name".to_string()],
            }).unwrap();

            exec.execute(Statement::Insert {
                table_name: "users".to_string(),
                values: vec!["1".to_string(), "Alice".to_string()],
            }).unwrap();
        }

        assert!(db.get(b"__secidx__:users:name:Alice").unwrap().is_some());

        {
            let mut exec = Executor::new(&mut db);
            exec.execute(Statement::Delete {
                table_name: "users".to_string(),
                pk_val: "1".to_string(),
            }).unwrap();
        }

        assert!(db.get(b"__secidx__:users:name:Alice").unwrap().is_none());
    }

    #[test]
    fn test_executor_update_updates_secondary_index() {
        let mut db = setup_db();
        {
            let mut exec = Executor::new(&mut db);
            exec.execute(Statement::CreateTable {
                table_name: "users".to_string(),
                columns: vec!["id INT".to_string(), "name TEXT".to_string()],
            }).unwrap();

            exec.execute(Statement::CreateIndex {
                index_name: "idx_name".to_string(),
                table_name: "users".to_string(),
                columns: vec!["name".to_string()],
            }).unwrap();

            exec.execute(Statement::Insert {
                table_name: "users".to_string(),
                values: vec!["1".to_string(), "Alice".to_string()],
            }).unwrap();

            exec.execute(Statement::Update {
                table_name: "users".to_string(),
                column: "name".to_string(),
                value: "Bob".to_string(),
                pk_val: "1".to_string(),
            }).unwrap();
        }

        assert!(db.get(b"__secidx__:users:name:Alice").unwrap().is_none());
        assert!(db.get(b"__secidx__:users:name:Bob").unwrap().is_some());
    }

    #[test]
    fn test_executor_select_range() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "users".to_string(),
            columns: vec!["id INT".to_string(), "age INT".to_string()],
        }).unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["1".to_string(), "20".to_string()],
        }).unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values: vec!["2".to_string(), "30".to_string()],
        }).unwrap();

        let res = exec.execute(Statement::SelectRange {
            table_name: "users".to_string(),
            column: "age".to_string(),
            op: ">=".to_string(),
            val: "25".to_string(),
        }).unwrap();

        assert!(res.contains("30"));
        assert!(!res.contains("20"));
    }
}
