use crate::sql::parser::Statement;
use crate::storage::Database;
use anyhow::Result;

/// Cost-Based Query Optimizer (CBO) module.
pub struct Optimizer;

#[derive(Debug, Clone)]
pub enum ExecutionStrategy {
    SeqScan,
    IndexScan(String), // Index Name
}

impl Optimizer {
    /// Estimates cost and selects the best execution strategy for a query.
    pub fn plan_select(
        db: &mut Database,
        table_name: &str,
        filter_col: Option<&str>,
    ) -> Result<ExecutionStrategy> {
        if let Some(col) = filter_col {
            let idx_key = format!("__index__:{}:{}", table_name, col);
            if db.get(idx_key.as_bytes())?.is_some() {
                // Secondary index exists for column!
                return Ok(ExecutionStrategy::IndexScan(col.to_string()));
            }
        }

        Ok(ExecutionStrategy::SeqScan)
    }

    /// Optimizes a SQL statement.
    pub fn optimize(_db: &mut Database, stmt: Statement) -> Statement {
        stmt
    }
}
