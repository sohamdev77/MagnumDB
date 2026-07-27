use anyhow::Result;
use std::collections::HashMap;

/// The Volcano-style Iterator Trait for query execution operators.
pub trait ExecutionPlan {
    fn open(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<Option<Vec<String>>>;
    fn close(&mut self) -> Result<()>;
}

/// Sequential Scan Operator over table rows.
pub struct SeqScanExec {
    rows: Vec<Vec<String>>,
    cursor: usize,
}

impl SeqScanExec {
    pub fn new(rows: Vec<Vec<String>>) -> Self {
        Self { rows, cursor: 0 }
    }
}

impl ExecutionPlan for SeqScanExec {
    fn open(&mut self) -> Result<()> {
        self.cursor = 0;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        if self.cursor < self.rows.len() {
            let row = self.rows[self.cursor].clone();
            self.cursor += 1;
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.cursor = self.rows.len();
        Ok(())
    }
}

/// Filter Operator (WHERE clause evaluation).
pub struct FilterExec {
    child: Box<dyn ExecutionPlan>,
    col_idx: usize,
    target_val: String,
}

impl FilterExec {
    pub fn new(child: Box<dyn ExecutionPlan>, col_idx: usize, target_val: String) -> Self {
        Self {
            child,
            col_idx,
            target_val,
        }
    }
}

impl ExecutionPlan for FilterExec {
    fn open(&mut self) -> Result<()> {
        self.child.open()
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        while let Some(row) = self.child.next()? {
            if self.col_idx < row.len() && row[self.col_idx] == self.target_val {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> Result<()> {
        self.child.close()
    }
}

/// Aggregate Function types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFunc {
    Count,
    Sum(usize),
    Avg(usize),
}

/// Aggregate Operator (`COUNT(*)`, `SUM(col)`, `AVG(col)`).
pub struct AggregateExec {
    child: Box<dyn ExecutionPlan>,
    func: AggregateFunc,
    executed: bool,
}

impl AggregateExec {
    pub fn new(child: Box<dyn ExecutionPlan>, func: AggregateFunc) -> Self {
        Self {
            child,
            func,
            executed: false,
        }
    }
}

impl ExecutionPlan for AggregateExec {
    fn open(&mut self) -> Result<()> {
        self.child.open()?;
        self.executed = false;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        if self.executed {
            return Ok(None);
        }
        self.executed = true;

        let mut count = 0u64;
        let mut sum = 0.0f64;

        while let Some(row) = self.child.next()? {
            count += 1;
            match self.func {
                AggregateFunc::Count => {}
                AggregateFunc::Sum(col_idx) | AggregateFunc::Avg(col_idx) => {
                    if col_idx < row.len() {
                        if let Ok(val) = row[col_idx].parse::<f64>() {
                            sum += val;
                        }
                    }
                }
            }
        }

        let result_str = match self.func {
            AggregateFunc::Count => count.to_string(),
            AggregateFunc::Sum(_) => sum.to_string(),
            AggregateFunc::Avg(_) => {
                if count > 0 {
                    (sum / count as f64).to_string()
                } else {
                    "0".to_string()
                }
            }
        };

        Ok(Some(vec![result_str]))
    }

    fn close(&mut self) -> Result<()> {
        self.child.close()
    }
}

/// Join type for relational join execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
}

/// Hash Join Operator (`INNER JOIN`, `LEFT JOIN`).
pub struct HashJoinExec {
    left: Box<dyn ExecutionPlan>,
    right: Box<dyn ExecutionPlan>,
    left_key_idx: usize,
    right_key_idx: usize,
    join_type: JoinType,
    right_num_cols: usize,
    result_rows: Vec<Vec<String>>,
    cursor: usize,
}

impl HashJoinExec {
    pub fn new(
        left: Box<dyn ExecutionPlan>,
        right: Box<dyn ExecutionPlan>,
        left_key_idx: usize,
        right_key_idx: usize,
        join_type: JoinType,
        right_num_cols: usize,
    ) -> Self {
        Self {
            left,
            right,
            left_key_idx,
            right_key_idx,
            join_type,
            right_num_cols,
            result_rows: Vec::new(),
            cursor: 0,
        }
    }
}

impl ExecutionPlan for HashJoinExec {
    fn open(&mut self) -> Result<()> {
        self.left.open()?;
        self.right.open()?;
        self.result_rows.clear();
        self.cursor = 0;

        let mut hash_table: HashMap<String, Vec<Vec<String>>> = HashMap::new();
        while let Some(right_row) = self.right.next()? {
            if self.right_key_idx < right_row.len() {
                let key = right_row[self.right_key_idx].clone();
                hash_table.entry(key).or_default().push(right_row);
            }
        }

        while let Some(left_row) = self.left.next()? {
            if self.left_key_idx < left_row.len() {
                let key = &left_row[self.left_key_idx];
                if let Some(matching_right_rows) = hash_table.get(key) {
                    for r_row in matching_right_rows {
                        let mut joined = left_row.clone();
                        joined.extend_from_slice(r_row);
                        self.result_rows.push(joined);
                    }
                } else if self.join_type == JoinType::Left {
                    let mut joined = left_row.clone();
                    joined.extend(vec!["NULL".to_string(); self.right_num_cols]);
                    self.result_rows.push(joined);
                }
            }
        }

        Ok(())
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        if self.cursor < self.result_rows.len() {
            let row = self.result_rows[self.cursor].clone();
            self.cursor += 1;
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.left.close()?;
        self.right.close()?;
        self.cursor = self.result_rows.len();
        Ok(())
    }
}

/// Group By & Having Hash Aggregation Operator.
pub struct HashGroupAggregateExec {
    child: Box<dyn ExecutionPlan>,
    group_col_idx: usize,
    agg_func: AggregateFunc,
    having_op_val: Option<(String, String)>,
    result_rows: Vec<Vec<String>>,
    cursor: usize,
}

impl HashGroupAggregateExec {
    pub fn new(
        child: Box<dyn ExecutionPlan>,
        group_col_idx: usize,
        agg_func: AggregateFunc,
        having_op_val: Option<(String, String)>,
    ) -> Self {
        Self {
            child,
            group_col_idx,
            agg_func,
            having_op_val,
            result_rows: Vec::new(),
            cursor: 0,
        }
    }
}

impl ExecutionPlan for HashGroupAggregateExec {
    fn open(&mut self) -> Result<()> {
        self.child.open()?;
        self.result_rows.clear();
        self.cursor = 0;

        let mut groups: HashMap<String, (u64, f64)> = HashMap::new();
        let mut group_keys = Vec::new();

        while let Some(row) = self.child.next()? {
            if self.group_col_idx < row.len() {
                let group_key = row[self.group_col_idx].clone();
                let entry = groups.entry(group_key.clone()).or_insert_with(|| {
                    group_keys.push(group_key);
                    (0, 0.0)
                });

                entry.0 += 1;
                match self.agg_func {
                    AggregateFunc::Count => {}
                    AggregateFunc::Sum(col_idx) | AggregateFunc::Avg(col_idx) => {
                        if col_idx < row.len() {
                            if let Ok(v) = row[col_idx].parse::<f64>() {
                                entry.1 += v;
                            }
                        }
                    }
                }
            }
        }

        for g_key in group_keys {
            if let Some(&(count, sum)) = groups.get(&g_key) {
                let agg_val = match self.agg_func {
                    AggregateFunc::Count => count.to_string(),
                    AggregateFunc::Sum(_) => sum.to_string(),
                    AggregateFunc::Avg(_) => {
                        if count > 0 {
                            (sum / count as f64).to_string()
                        } else {
                            "0".to_string()
                        }
                    }
                };

                let mut passes_having = true;
                if let Some((ref op, ref target_str)) = self.having_op_val {
                    if let Ok(target_num) = target_str.parse::<f64>() {
                        if let Ok(agg_num) = agg_val.parse::<f64>() {
                            passes_having = match op.as_str() {
                                ">=" => agg_num >= target_num,
                                "<=" => agg_num <= target_num,
                                ">" => agg_num > target_num,
                                "<" => agg_num < target_num,
                                "=" => (agg_num - target_num).abs() < 1e-9,
                                _ => true,
                            };
                        }
                    }
                }

                if passes_having {
                    self.result_rows.push(vec![g_key, agg_val]);
                }
            }
        }

        Ok(())
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        if self.cursor < self.result_rows.len() {
            let row = self.result_rows[self.cursor].clone();
            self.cursor += 1;
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.child.close()?;
        self.cursor = self.result_rows.len();
        Ok(())
    }
}

/// Sort Operator (`ORDER BY col [ASC|DESC]`).
pub struct SortExec {
    child: Box<dyn ExecutionPlan>,
    sort_col_idx: usize,
    is_desc: bool,
    result_rows: Vec<Vec<String>>,
    cursor: usize,
}

impl SortExec {
    pub fn new(child: Box<dyn ExecutionPlan>, sort_col_idx: usize, is_desc: bool) -> Self {
        Self {
            child,
            sort_col_idx,
            is_desc,
            result_rows: Vec::new(),
            cursor: 0,
        }
    }
}

impl ExecutionPlan for SortExec {
    fn open(&mut self) -> Result<()> {
        self.child.open()?;
        self.result_rows.clear();
        self.cursor = 0;

        while let Some(row) = self.child.next()? {
            self.result_rows.push(row);
        }

        let idx = self.sort_col_idx;
        let is_desc = self.is_desc;

        self.result_rows.sort_by(|a, b| {
            let val_a = a.get(idx).map(|s| s.as_str()).unwrap_or("");
            let val_b = b.get(idx).map(|s| s.as_str()).unwrap_or("");

            let ord = match (val_a.parse::<f64>(), val_b.parse::<f64>()) {
                (Ok(num_a), Ok(num_b)) => num_a.partial_cmp(&num_b).unwrap_or(std::cmp::Ordering::Equal),
                _ => val_a.cmp(val_b),
            };

            if is_desc {
                ord.reverse()
            } else {
                ord
            }
        });

        Ok(())
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        if self.cursor < self.result_rows.len() {
            let row = self.result_rows[self.cursor].clone();
            self.cursor += 1;
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.child.close()?;
        self.cursor = self.result_rows.len();
        Ok(())
    }
}

/// Limit & Offset Operator (`LIMIT n [OFFSET m]`).
pub struct LimitOffsetExec {
    child: Box<dyn ExecutionPlan>,
    limit: Option<usize>,
    offset: usize,
    result_rows: Vec<Vec<String>>,
    cursor: usize,
}

impl LimitOffsetExec {
    pub fn new(child: Box<dyn ExecutionPlan>, limit: Option<usize>, offset: usize) -> Self {
        Self {
            child,
            limit,
            offset,
            result_rows: Vec::new(),
            cursor: 0,
        }
    }
}

impl ExecutionPlan for LimitOffsetExec {
    fn open(&mut self) -> Result<()> {
        self.child.open()?;
        self.result_rows.clear();
        self.cursor = 0;

        let mut skipped = 0;
        let mut count = 0;

        while let Some(row) = self.child.next()? {
            if skipped < self.offset {
                skipped += 1;
                continue;
            }

            if let Some(l) = self.limit {
                if count >= l {
                    break;
                }
            }

            self.result_rows.push(row);
            count += 1;
        }

        Ok(())
    }

    fn next(&mut self) -> Result<Option<Vec<String>>> {
        if self.cursor < self.result_rows.len() {
            let row = self.result_rows[self.cursor].clone();
            self.cursor += 1;
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> Result<()> {
        self.child.close()?;
        self.cursor = self.result_rows.len();
        Ok(())
    }
}
