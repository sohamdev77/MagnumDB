use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// Supported SQL Data Types in MagnumDB
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    Int,
    BigInt,
    Float,
    Text,
    Boolean,
}

impl DataType {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "INT" | "INTEGER" | "INT4" => Ok(DataType::Int),
            "BIGINT" | "INT8" => Ok(DataType::BigInt),
            "FLOAT" | "FLOAT8" | "DOUBLE" | "REAL" => Ok(DataType::Float),
            "TEXT" | "VARCHAR" | "STRING" | "CHAR" => Ok(DataType::Text),
            "BOOL" | "BOOLEAN" => Ok(DataType::Boolean),
            other => Err(anyhow!("Unsupported data type: '{}'", other)),
        }
    }

    pub fn to_sql_string(&self) -> &'static str {
        match self {
            DataType::Int => "INTEGER",
            DataType::BigInt => "BIGINT",
            DataType::Float => "FLOAT",
            DataType::Text => "TEXT",
            DataType::Boolean => "BOOLEAN",
        }
    }
}

/// Strongly-typed SQL runtime value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    Null,
    Int(i32),
    BigInt(i64),
    Float(f64),
    Text(String),
    Boolean(bool),
}

impl Value {
    pub fn parse_str(raw: &str, data_type: &DataType) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("NULL") {
            return Ok(Value::Null);
        }

        match data_type {
            DataType::Int => {
                let v = trimmed.parse::<i32>()
                    .map_err(|_| anyhow!("Cannot parse '{}' as INT", raw))?;
                Ok(Value::Int(v))
            }
            DataType::BigInt => {
                let v = trimmed.parse::<i64>()
                    .map_err(|_| anyhow!("Cannot parse '{}' as BIGINT", raw))?;
                Ok(Value::BigInt(v))
            }
            DataType::Float => {
                let v = trimmed.parse::<f64>()
                    .map_err(|_| anyhow!("Cannot parse '{}' as FLOAT", raw))?;
                Ok(Value::Float(v))
            }
            DataType::Text => {
                let unquoted = if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
                    || (trimmed.starts_with('"') && trimmed.ends_with('"'))
                {
                    &trimmed[1..trimmed.len() - 1]
                } else {
                    trimmed
                };
                Ok(Value::Text(unquoted.to_string()))
            }
            DataType::Boolean => {
                let v = match trimmed.to_lowercase().as_str() {
                    "true" | "t" | "1" => true,
                    "false" | "f" | "0" => false,
                    _ => return Err(anyhow!("Cannot parse '{}' as BOOLEAN", raw)),
                };
                Ok(Value::Boolean(v))
            }
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn to_string_repr(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Int(v) => v.to_string(),
            Value::BigInt(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Text(v) => v.clone(),
            Value::Boolean(v) => v.to_string(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_repr())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::BigInt(a), Value::BigInt(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            // Cross-numeric equality
            (Value::Int(a), Value::BigInt(b)) => (*a as i64) == *b,
            (Value::BigInt(a), Value::Int(b)) => *a == (*b as i64),
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::BigInt(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::BigInt(b)) => *a == (*b as f64),
            _ => false,
        }
    }
}

impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Value::Null, Value::Null) => Some(Ordering::Equal),
            (Value::Null, _) => Some(Ordering::Less),
            (_, Value::Null) => Some(Ordering::Greater),
            (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
            (Value::BigInt(a), Value::BigInt(b)) => a.partial_cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Text(a), Value::Text(b)) => a.partial_cmp(b),
            (Value::Boolean(a), Value::Boolean(b)) => a.partial_cmp(b),
            // Cross-numeric ordering
            (Value::Int(a), Value::BigInt(b)) => (*a as i64).partial_cmp(b),
            (Value::BigInt(a), Value::Int(b)) => a.partial_cmp(&(*b as i64)),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::BigInt(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::BigInt(b)) => a.partial_cmp(&(*b as f64)),
            _ => None,
        }
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// Column definition within a table schema
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: DataType,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub default_value: Option<String>,
}

impl ColumnDef {
    pub fn new(name: String, data_type: DataType) -> Self {
        Self {
            name,
            data_type,
            is_nullable: true,
            is_primary_key: false,
            is_unique: false,
            default_value: None,
        }
    }

    pub fn with_nullable(mut self, nullable: bool) -> Self {
        self.is_nullable = nullable;
        self
    }

    pub fn with_primary_key(mut self, primary_key: bool) -> Self {
        self.is_primary_key = primary_key;
        if primary_key {
            self.is_nullable = false;
            self.is_unique = true;
        }
        self
    }
    
    pub fn with_unique(mut self, unique: bool) -> Self {
        self.is_unique = unique;
        self
    }
}

/// Full metadata definition for a database table schema
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSchema {
    pub schema_name: String,
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: Option<Vec<String>>,
}

impl TableSchema {
    pub fn new(schema_name: String, table_name: String, columns: Vec<ColumnDef>) -> Self {
        let pk_cols: Vec<String> = columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| c.name.clone())
            .collect();

        let primary_key = if !pk_cols.is_empty() {
            Some(pk_cols)
        } else {
            None
        };

        Self {
            schema_name,
            table_name,
            columns,
            primary_key,
        }
    }

    pub fn full_name(&self) -> String {
        format!("{}.{}", self.schema_name, self.table_name)
    }

    pub fn find_column(&self, col_name: &str) -> Option<(usize, &ColumnDef)> {
        self.columns
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.eq_ignore_ascii_case(col_name))
    }

    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }
}
