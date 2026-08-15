use crate::sql::catalog::{CatalogManager, DEFAULT_SCHEMA};
use crate::sql::parser::Statement;
use crate::sql::types::{ColumnDef, DataType, TableSchema, Value};
use crate::sql::volcano::{
    AggregateExec, AggregateFunc, ExecutionPlan, FilterExec, HashGroupAggregateExec, HashJoinExec,
    JoinType, LimitOffsetExec, SeqScanExec, SortExec,
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

    if data.len() >= 16 {
        offset = 16;
    }

    while offset < data.len() {
        if offset + 4 > data.len() {
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
    db: &'a Database,
    in_transaction: bool,
    current_tx_id: u64,
    write_buffer: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl<'a> Executor<'a> {
    pub fn new(db: &'a Database) -> Self {
        let _ = CatalogManager::init_default_schema(db);
        Self {
            db,
            in_transaction: false,
            current_tx_id: 0,
            write_buffer: BTreeMap::new(),
        }
    }

    pub fn execute(&mut self, stmt: Statement) -> Result<String> {
        match stmt {
            Statement::CreateUser { username, password } => {
                let pwd_hash = password.map(|p| {
                    let digest = md5::compute(format!("{}{}", p, username));
                    format!("md5{:x}", digest)
                });
                crate::sql::catalog::CatalogManager::create_user(self.db, &username, pwd_hash, false)?;
                Ok(format!("CREATE ROLE"))
            }
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
            Statement::CreateSchema { schema_name } => {
                CatalogManager::create_schema(self.db, &schema_name)?;
                Ok(format!("Query OK, schema '{}' created.", schema_name))
            }
            Statement::ShowSchemas => {
                let schemas = CatalogManager::list_schemas(self.db)?;
                let mut output =
                    "+-----------------+\n| Schemas |\n+-----------------+\n".to_string();
                for s in &schemas {
                    output.push_str(&format!("| {} |\n", s));
                }
                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} schema(s) in set.", schemas.len()));
                Ok(output)
            }
            Statement::CreateTable {
                table_name,
                columns,
            } => {
                let (schema_name, t_name) = parse_qualified_table_name(&table_name);
                let schema_key = format!("__schema__:{}", t_name);

                if self.get_schema_bytes(&t_name)?.is_some()
                    || CatalogManager::get_table_schema(self.db, &schema_name, &t_name)?.is_some()
                {
                    return Err(anyhow::anyhow!("Table '{}' already exists", table_name));
                }

                let col_defs: Vec<ColumnDef> = columns.iter().map(|c| parse_column_def(c)).collect();
                let table_schema = TableSchema::new(schema_name.clone(), t_name.clone(), col_defs);
                CatalogManager::save_table_schema(self.db, &table_schema)?;

                let schema_val = columns.join(",");
                if self.in_transaction {
                    self.write_buffer
                        .insert(schema_key.into_bytes(), Some(schema_val.into_bytes()));
                } else {
                    self.db.put(schema_key.as_bytes(), schema_val.as_bytes())?;
                }

                let msg = if schema_name == DEFAULT_SCHEMA {
                    format!("Query OK, table '{}' created.", t_name)
                } else {
                    format!("Query OK, table '{}.{}' created.", schema_name, t_name)
                };
                Ok(msg)
            }
            Statement::CreateIndex {
                index_name,
                table_name,
                columns,
            } => {
                let (schema_name, t_name) = parse_qualified_table_name(&table_name);
                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let idx_meta_key = format!("__index__:{}:{}", t_name, index_name);
                if self.db.get(idx_meta_key.as_bytes())?.is_some() {
                    return Err(anyhow::anyhow!("Index '{}' already exists", index_name));
                }

                let cols_joined = columns.join(",");
                self.db.put(idx_meta_key.as_bytes(), cols_joined.as_bytes())?;

                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let mut col_indices = Vec::new();
                for col in &columns {
                    let idx = col_names.iter().position(|c| c == col).ok_or_else(|| {
                        anyhow::anyhow!("Column '{}' not found in table '{}'", col, table_name)
                    })?;
                    col_indices.push(idx);
                }

                let prefix = format!("{}:", t_name);
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
                        self.sec_idx_add(&t_name, &column_key, &composite_val, &pk)?;
                    }
                }

                Ok(format!(
                    "Query OK, index '{}' created on '{}.{}({})'.",
                    index_name, schema_name, t_name, cols_joined
                ))
            }
            Statement::Insert { table_name, values_list } => {
                let (schema_name, t_name) = parse_qualified_table_name(&table_name);
                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                // Fetch full table schema if available to validate types and constraints
                let catalog_schema = CatalogManager::get_table_schema(self.db, &schema_name, &t_name)?;

                let active_tx = if self.in_transaction { self.current_tx_id } else { 1 };
                let mut inserted_count = 0;

                for values in values_list {
                    if let Some(schema) = &catalog_schema {
                        if values.len() != schema.columns.len() {
                            return Err(anyhow::anyhow!(
                                "Column count mismatch: expected {}, got {}",
                                schema.columns.len(),
                                values.len()
                            ));
                        }

                        // Validate typed values & NOT NULL constraints
                        for (i, col) in schema.columns.iter().enumerate() {
                            let val_str = &values[i];
                            let parsed_val = Value::parse_str(val_str, &col.data_type)?;
                            if !col.is_nullable && parsed_val.is_null() {
                                return Err(anyhow::anyhow!(
                                    "Constraint Violation: Column '{}' cannot be NULL",
                                    col.name
                                ));
                            }
                        }
                    } else if values.len() != col_names.len() {
                        return Err(anyhow::anyhow!(
                            "Column count mismatch: expected {}, got {}",
                            col_names.len(),
                            values.len()
                        ));
                    }

                    if values.is_empty() {
                        return Err(anyhow::anyhow!("No values provided for insertion"));
                    }

                    let pk = &values[0];
                    let internal_key = format!("{}:{}", t_name, pk).into_bytes();
                    let internal_val = encode_values_mvcc(&values, active_tx, 0);

                    if self.in_transaction {
                        self.write_buffer.insert(internal_key, Some(internal_val));
                    } else {
                        self.db.put(&internal_key, &internal_val)?;
                    }

                    self.sync_secondary_indexes_on_insert(&t_name, &col_names, &values, pk)?;
                    inserted_count += 1;
                }

                if inserted_count == 1 {
                    Ok("Query OK, 1 row inserted.".to_string())
                } else {
                    Ok(format!("Query OK, {} rows inserted.", inserted_count))
                }
            }
            Statement::Select {
                table_name,
                where_clause,
                order_by,
                limit_offset,
            } => {
                let (schema_name, t_name) = parse_qualified_table_name(&table_name);

                // Handle virtual system catalog queries
                if t_name.eq_ignore_ascii_case("information_schema.tables")
                    || (schema_name.eq_ignore_ascii_case("information_schema")
                        && t_name.eq_ignore_ascii_case("tables"))
                {
                    let rows = CatalogManager::query_information_schema_tables(self.db)?;
                    return Ok(format_table_output(&["table_catalog", "table_schema", "table_name", "table_type"], &rows));
                }

                if t_name.eq_ignore_ascii_case("information_schema.columns")
                    || (schema_name.eq_ignore_ascii_case("information_schema")
                        && t_name.eq_ignore_ascii_case("columns"))
                {
                    let rows = CatalogManager::query_information_schema_columns(self.db, None)?;
                    return Ok(format_table_output(
                        &["table_catalog", "table_schema", "table_name", "column_name", "ordinal_position", "data_type", "is_nullable", "column_default"],
                        &rows,
                    ));
                }

                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;

                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let mut rows = Vec::new();

                // Check index scan optimizations
                let mut used_index = false;
                if let Some((ref filter_col, ref filter_val)) = where_clause {
                    let sec_key = format!("__secidx__:{}:{}:{}", t_name, filter_col, filter_val);
                    if let Some(pks_bytes) = self.db.get(sec_key.as_bytes())? {
                        used_index = true;
                        let pks_str = String::from_utf8_lossy(&pks_bytes);
                        let pks: Vec<&str> = pks_str.split(SEC_IDX_PK_SEP).collect();
                        for pk in pks {
                            let k = format!("{}:{}", t_name, pk).into_bytes();
                            let v_opt = if self.in_transaction && self.write_buffer.contains_key(&k) {
                                self.write_buffer.get(&k).cloned().flatten()
                            } else {
                                self.db.get(&k)?
                            };
                            if let Some(v) = v_opt {
                                rows.push(decode_values(&v)?);
                            }
                        }
                    }
                }

                if !used_index {
                    rows = self.scan_table_rows(&t_name)?;
                }

                let scan_op: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(rows));

                let filtered_op: Box<dyn ExecutionPlan> = if !used_index {
                    if let Some((ref filter_col, ref filter_val)) = where_clause {
                        let col_idx = col_names.iter().position(|name| name == filter_col).ok_or_else(
                            || anyhow::anyhow!("Column '{}' not found", filter_col),
                        )?;
                        Box::new(FilterExec::new(scan_op, col_idx, filter_val.clone()))
                    } else {
                        scan_op
                    }
                } else {
                    scan_op
                };

                let sorted_op: Box<dyn ExecutionPlan> = if let Some((ref sort_col, is_desc)) = order_by {
                    let col_idx = col_names
                        .iter()
                        .position(|name| name == sort_col)
                        .ok_or_else(|| anyhow::anyhow!("Column '{}' not found for ORDER BY", sort_col))?;
                    Box::new(SortExec::new(filtered_op, col_idx, is_desc))
                } else {
                    filtered_op
                };

                let mut plan: Box<dyn ExecutionPlan> = if let Some((limit, offset)) = limit_offset {
                    Box::new(LimitOffsetExec::new(sorted_op, Some(limit), offset))
                } else {
                    sorted_op
                };

                plan.open()?;

                let header = col_names.join(" | ");
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
                let (_, l_tname) = parse_qualified_table_name(&left_table);
                let (_, r_tname) = parse_qualified_table_name(&right_table);

                let left_sb = self
                    .get_schema_bytes(&l_tname)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", left_table))?;
                let right_sb = self
                    .get_schema_bytes(&r_tname)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", right_table))?;

                let left_schema = String::from_utf8(left_sb)?;
                let right_schema = String::from_utf8(right_sb)?;

                let left_cols = Self::extract_col_names(&left_schema);
                let right_cols = Self::extract_col_names(&right_schema);

                let left_key_idx = left_cols
                    .iter()
                    .position(|c| c == &left_col)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found in '{}'", left_col, left_table))?;
                let right_key_idx = right_cols
                    .iter()
                    .position(|c| c == &right_col)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Column '{}' not found in '{}'", right_col, right_table)
                    })?;

                let l_rows = self.scan_table_rows(&l_tname)?;
                let r_rows = self.scan_table_rows(&r_tname)?;

                let left_plan: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(l_rows));
                let right_plan: Box<dyn ExecutionPlan> = Box::new(SeqScanExec::new(r_rows));

                let join_type = if is_left_join {
                    JoinType::Left
                } else {
                    JoinType::Inner
                };

                let mut plan = HashJoinExec::new(
                    left_plan,
                    right_plan,
                    left_key_idx,
                    right_key_idx,
                    join_type,
                    right_cols.len(),
                );

                plan.open()?;

                let mut combined_cols = left_cols;
                combined_cols.extend(right_cols);
                let header = combined_cols.join(" | ");

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
            Statement::SelectGroupAggregate {
                group_col,
                func,
                agg_col,
                table_name,
                where_clause,
                having_clause,
            } => {
                let (_, t_name) = parse_qualified_table_name(&table_name);
                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let group_col_idx = col_names
                    .iter()
                    .position(|c| c == &group_col)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", group_col))?;

                let rows = self.scan_table_rows(&t_name)?;
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
                        let col_idx = col_names
                            .iter()
                            .position(|name| name == &agg_col)
                            .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", agg_col))?;
                        AggregateFunc::Sum(col_idx)
                    }
                    "AVG" => {
                        let col_idx = col_names
                            .iter()
                            .position(|name| name == &agg_col)
                            .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", agg_col))?;
                        AggregateFunc::Avg(col_idx)
                    }
                    _ => return Err(anyhow::anyhow!("Unsupported aggregate function {}", func)),
                };

                let mut plan =
                    HashGroupAggregateExec::new(filtered_op, group_col_idx, agg_func, having_clause);
                plan.open()?;

                let header = format!("{} | {}({})", group_col, func, agg_col);
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
                let (_, t_name) = parse_qualified_table_name(&table_name);
                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let col_idx = col_names
                    .iter()
                    .position(|name| name == &column)
                    .ok_or_else(|| anyhow::anyhow!("Column '{}' not found", column))?;

                let rows = self.scan_table_rows(&t_name)?;
                let header = col_names.join(" | ");
                let mut output =
                    format!("+-----------------+\n| {} |\n+-----------------+\n", header);
                let mut count = 0;

                for decoded in rows {
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
                let (_, t_name) = parse_qualified_table_name(&table_name);
                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
                    .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table_name))?;
                let schema_val = String::from_utf8(schema_bytes)?;
                let col_names = Self::extract_col_names(&schema_val);

                let rows = self.scan_table_rows(&t_name)?;
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
                where_clause,
            } => {
                let (schema_name, t_name) = parse_qualified_table_name(&table_name);
                let schema_bytes = self
                    .get_schema_bytes(&t_name)?
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

                // Validate updated type & constraints if schema exists
                if let Some(schema) = CatalogManager::get_table_schema(self.db, &schema_name, &t_name)? {
                    if let Some(col_def) = schema.columns.get(col_idx) {
                        let parsed_val = Value::parse_str(&value, &col_def.data_type)?;
                        if !col_def.is_nullable && parsed_val.is_null() {
                            return Err(anyhow::anyhow!(
                                "Constraint Violation: Column '{}' cannot be NULL",
                                col_def.name
                            ));
                        }
                    }
                }

                let mut rows_to_update = Vec::new();
                
                // Fetch rows
                let rows = self.scan_table_rows(&t_name)?;
                
                if let Some((filter_col, filter_val)) = &where_clause {
                    let filter_idx = col_names.iter().position(|name| name == filter_col).ok_or_else(
                        || anyhow::anyhow!("Column '{}' not found", filter_col),
                    )?;
                    for row in rows {
                        if row[filter_idx] == *filter_val {
                            rows_to_update.push(row);
                        }
                    }
                } else {
                    rows_to_update = rows;
                }

                let mut updated_count = 0;
                let active_tx = if self.in_transaction { self.current_tx_id } else { 1 };

                for mut row_values in rows_to_update {
                    if col_idx >= row_values.len() {
                        return Err(anyhow::anyhow!("Corrupted row length"));
                    }

                    let pk_val = row_values[0].clone();
                    let old_value = row_values[col_idx].clone();
                    self.sec_idx_remove_for_column(&t_name, &column, &old_value, &pk_val)?;

                    row_values[col_idx] = value.clone();

                    self.sec_idx_add_for_column(&t_name, &column, &value, &pk_val)?;

                    let internal_key = format!("{}:{}", t_name, pk_val).into_bytes();
                    let updated_bytes = encode_values_mvcc(&row_values, active_tx, 0);

                    if self.in_transaction {
                        self.write_buffer.insert(internal_key, Some(updated_bytes));
                    } else {
                        self.db.put(&internal_key, &updated_bytes)?;
                    }
                    updated_count += 1;
                }

                if updated_count == 1 {
                    Ok("Query OK, 1 row updated.".to_string())
                } else {
                    Ok(format!("Query OK, {} rows updated.", updated_count))
                }
            }
            Statement::Delete { table_name, where_clause } => {
                let (_, t_name) = parse_qualified_table_name(&table_name);
                
                let schema_bytes = self.get_schema_bytes(&t_name)?;
                if schema_bytes.is_none() {
                    return Err(anyhow::anyhow!("Table '{}' does not exist", table_name));
                }
                
                let schema_val = String::from_utf8(schema_bytes.unwrap())?;
                let col_names = Self::extract_col_names(&schema_val);

                let mut rows_to_delete = Vec::new();
                let rows = self.scan_table_rows(&t_name)?;
                
                if let Some((filter_col, filter_val)) = &where_clause {
                    let filter_idx = col_names.iter().position(|name| name == filter_col).ok_or_else(
                        || anyhow::anyhow!("Column '{}' not found", filter_col),
                    )?;
                    for row in rows {
                        if row[filter_idx] == *filter_val {
                            rows_to_delete.push(row);
                        }
                    }
                } else {
                    rows_to_delete = rows;
                }

                let mut deleted_count = 0;

                for row_data in rows_to_delete {
                    let pk_val = row_data[0].clone();
                    let internal_key = format!("{}:{}", t_name, pk_val).into_bytes();

                    self.remove_all_secondary_indexes(
                        &t_name, &col_names, &row_data, &pk_val,
                    )?;

                    if self.in_transaction {
                        self.write_buffer.insert(internal_key, None);
                    } else {
                        self.db.delete(&internal_key)?;
                    }
                    
                    deleted_count += 1;
                }

                if deleted_count == 1 {
                    Ok("Query OK, 1 row deleted.".to_string())
                } else {
                    Ok(format!("Query OK, {} rows deleted.", deleted_count))
                }
            }
            Statement::DropTable { table_name } => {
                let (schema_name, t_name) = parse_qualified_table_name(&table_name);
                let schema_key = format!("__schema__:{}", t_name);

                if self.get_schema_bytes(&t_name)?.is_none() {
                    return Err(anyhow::anyhow!("Table '{}' does not exist", table_name));
                }

                let prefix = format!("{}:", t_name);
                let records = self.db.scan_prefix(prefix.as_bytes())?;

                let idx_prefix = format!("__index__:{}:", t_name);
                let indices = self.db.scan_prefix(idx_prefix.as_bytes())?;
                for (idx_key, _) in &indices {
                    self.db.delete(idx_key)?;
                }
                let sec_prefix = format!("__secidx__:{}:", t_name);
                let sec_entries = self.db.scan_prefix(sec_prefix.as_bytes())?;
                for (sec_key, _) in sec_entries {
                    self.db.delete(&sec_key)?;
                }

                // Delete catalog entry
                let catalog_key = CatalogManager::table_key(&schema_name, &t_name);
                self.db.delete(catalog_key.as_bytes())?;

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
                let tables = CatalogManager::list_tables(self.db, DEFAULT_SCHEMA)?;

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

    fn scan_table_rows(&mut self, table_name: &str) -> Result<Vec<Vec<String>>> {
        let prefix = format!("{}:", table_name);
        let records = self.db.scan_prefix(prefix.as_bytes())?;

        let mut row_map: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for (k, v) in records {
            row_map.insert(k, v);
        }

        if self.in_transaction {
            let p_bytes = prefix.as_bytes();
            for (k, v_opt) in &self.write_buffer {
                if k.starts_with(p_bytes) {
                    match v_opt {
                        Some(v) => {
                            row_map.insert(k.clone(), v.clone());
                        }
                        None => {
                            row_map.remove(k);
                        }
                    }
                }
            }
        }

        let mut rows = Vec::new();
        for (_k, v) in row_map {
            rows.push(decode_values(&v)?);
        }
        Ok(rows)
    }

    fn get_schema_bytes(&mut self, table_name: &str) -> Result<Option<Vec<u8>>> {
        let schema_key = format!("__schema__:{}", table_name);
        if self.in_transaction && self.write_buffer.contains_key(schema_key.as_bytes()) {
            Ok(self.write_buffer.get(schema_key.as_bytes()).cloned().flatten())
        } else {
            self.db.get(schema_key.as_bytes())
        }
    }

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

    fn sec_idx_add(&mut self, table: &str, column: &str, col_val: &str, pk: &str) -> Result<()> {
        let sec_key = format!("__secidx__:{}:{}:{}", table, column, col_val);
        let existing = self.db.get(sec_key.as_bytes())?;

        let new_val = match existing {
            Some(data) => {
                let existing_str = String::from_utf8_lossy(&data).to_string();
                let pks: Vec<&str> = existing_str.split(SEC_IDX_PK_SEP).collect();
                if pks.contains(&pk) {
                    return Ok(());
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

fn parse_column_def(raw: &str) -> ColumnDef {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let col_name = tokens.first().cloned().unwrap_or("col").to_string();

    let mut dtype = DataType::Text;
    let mut is_nullable = true;
    let mut is_pk = false;

    let upper_raw = raw.to_uppercase();
    if upper_raw.contains("PRIMARY KEY") {
        is_pk = true;
        is_nullable = false;
    }
    if upper_raw.contains("NOT NULL") {
        is_nullable = false;
    }

    if tokens.len() > 1 {
        let type_str = tokens[1].trim_matches(|c| c == ',' || c == '(' || c == ')');
        if let Ok(dt) = DataType::from_str(type_str) {
            dtype = dt;
        }
    }

    ColumnDef::new(col_name, dtype)
        .with_nullable(is_nullable)
        .with_primary_key(is_pk)
}

fn parse_qualified_table_name(raw: &str) -> (String, String) {
    if let Some(dot_idx) = raw.find('.') {
        let s_name = raw[..dot_idx].trim().to_string();
        let t_name = raw[dot_idx + 1..].trim().to_string();
        (s_name, t_name)
    } else {
        (DEFAULT_SCHEMA.to_string(), raw.trim().to_string())
    }
}

fn format_table_output(headers: &[&str], rows: &[Vec<String>]) -> String {
    let header_str = headers.join(" | ");
    let mut output = format!("+-----------------+\n| {} |\n+-----------------+\n", header_str);
    for row in rows {
        output.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    output.push_str("+-----------------+\n");
    output.push_str(&format!("{} row(s) in set.", rows.len()));
    output
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
    fn test_executor_order_by_limit_offset() {
        let mut db = setup_db();
        let mut exec = Executor::new(&mut db);

        exec.execute(Statement::CreateTable {
            table_name: "users".to_string(),
            columns: vec!["id INT".to_string(), "age INT".to_string()],
        }).unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values_list: vec![vec!["1".to_string(), "20".to_string()]],
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values_list: vec![vec!["2".to_string(), "30".to_string()]],
        })
        .unwrap();

        exec.execute(Statement::Insert {
            table_name: "users".to_string(),
            values_list: vec![vec!["3".to_string(), "25".to_string()]],
        })
        .unwrap();

        let res = exec.execute(Statement::Select {
            table_name: "users".to_string(),
            where_clause: None,
            order_by: Some(("age".to_string(), true)),
            limit_offset: Some((2, 0)),
        }).unwrap();

        assert!(res.contains("30"));
        assert!(res.contains("25"));
        assert!(!res.contains("20"));
    }
}
