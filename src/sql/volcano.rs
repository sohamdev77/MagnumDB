use anyhow::Result;

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
