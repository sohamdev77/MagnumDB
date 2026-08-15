use crate::sql::types::{ColumnDef, DataType, TableSchema};
use crate::storage::Database;
use anyhow::{anyhow, Result};
use std::collections::HashSet;

pub const DEFAULT_SCHEMA: &str = "public";

/// Manages database catalogs, schemas (namespaces), and table definitions.
pub struct CatalogManager;

impl CatalogManager {
    /// Formats catalog key for schema registration.
    pub fn schema_key(schema_name: &str) -> String {
        format!("__catalog__:schema:{}", schema_name.to_lowercase())
    }

    /// Formats catalog key for table schema metadata storage.
    pub fn table_key(schema_name: &str, table_name: &str) -> String {
        format!(
            "__catalog__:table:{}:{}",
            schema_name.to_lowercase(),
            table_name.to_lowercase()
        )
    }

    /// Ensures the default 'public' schema exists in the database catalog.
    pub fn init_default_schema(db: &Database) -> Result<()> {
        let key = Self::schema_key(DEFAULT_SCHEMA);
        if db.get(key.as_bytes())?.is_none() {
            db.put(key.as_bytes(), b"registered")?;
        }
        Ok(())
    }

    /// Registers a new schema namespace (e.g. `CREATE SCHEMA my_schema`).
    pub fn create_schema(db: &Database, schema_name: &str) -> Result<()> {
        let key = Self::schema_key(schema_name);
        if db.get(key.as_bytes())?.is_some() {
            return Err(anyhow!("Schema '{}' already exists", schema_name));
        }
        db.put(key.as_bytes(), b"registered")?;
        Ok(())
    }

    /// Lists all registered schema namespaces in the database.
    pub fn list_schemas(db: &Database) -> Result<Vec<String>> {
        let prefix = "__catalog__:schema:";
        let records = db.scan_prefix(prefix.as_bytes())?;
        let mut schemas = HashSet::new();

        schemas.insert(DEFAULT_SCHEMA.to_string());

        for (k, _) in records {
            let key_str = String::from_utf8_lossy(&k);
            if let Some(s_name) = key_str.strip_prefix(prefix) {
                schemas.insert(s_name.to_string());
            }
        }

        let mut sorted: Vec<String> = schemas.into_iter().collect();
        sorted.sort();
        Ok(sorted)
    }

    /// Saves a `TableSchema` into the persistent metadata catalog.
    pub fn save_table_schema(db: &Database, schema: &TableSchema) -> Result<()> {
        let key = Self::table_key(&schema.schema_name, &schema.table_name);
        let serialized = serde_json::to_vec(schema)?;
        db.put(key.as_bytes(), &serialized)?;

        // Also save legacy key for backward compatibility
        let legacy_key = format!("__schema__:{}", schema.table_name);
        let legacy_val = schema
            .columns
            .iter()
            .map(|c| format!("{} {}", c.name, c.data_type.to_sql_string()))
            .collect::<Vec<_>>()
            .join(",");
        db.put(legacy_key.as_bytes(), legacy_val.as_bytes())?;
        Ok(())
    }

    /// Retrieves a `TableSchema` from the catalog.
    pub fn get_table_schema(
        db: &Database,
        schema_name: &str,
        table_name: &str,
    ) -> Result<Option<TableSchema>> {
        let key = Self::table_key(schema_name, table_name);
        if let Some(bytes) = db.get(key.as_bytes())? {
            let schema: TableSchema = serde_json::from_slice(&bytes)?;
            return Ok(Some(schema));
        }

        // Fallback for legacy key if table was created under old schema representation
        if schema_name.eq_ignore_ascii_case(DEFAULT_SCHEMA) {
            let legacy_key = format!("__schema__:{}", table_name);
            if let Some(bytes) = db.get(legacy_key.as_bytes())? {
                let raw_val = String::from_utf8(bytes)?;
                let cols: Vec<ColumnDef> = raw_val
                    .split(',')
                    .map(|part| {
                        let tokens: Vec<&str> = part.trim().split_whitespace().collect();
                        let col_name = tokens.first().cloned().unwrap_or("col").to_string();
                        let dtype = if tokens.len() > 1 {
                            DataType::from_str(tokens[1]).unwrap_or(DataType::Text)
                        } else {
                            DataType::Text
                        };
                        ColumnDef::new(col_name, dtype)
                    })
                    .collect();

                let schema = TableSchema::new(DEFAULT_SCHEMA.to_string(), table_name.to_string(), cols);
                return Ok(Some(schema));
            }
        }

        Ok(None)
    }

    /// Lists all tables in a given schema namespace.
    pub fn list_tables(db: &Database, schema_name: &str) -> Result<Vec<String>> {
        let prefix = format!("__catalog__:table:{}:", schema_name.to_lowercase());
        let records = db.scan_prefix(prefix.as_bytes())?;
        let mut tables = Vec::new();

        for (k, _) in records {
            let key_str = String::from_utf8_lossy(&k);
            if let Some(t_name) = key_str.strip_prefix(&prefix) {
                tables.push(t_name.to_string());
            }
        }

        if schema_name.eq_ignore_ascii_case(DEFAULT_SCHEMA) {
            let legacy_prefix = "__schema__:";
            let legacy_records = db.scan_prefix(legacy_prefix.as_bytes())?;
            for (k, _) in legacy_records {
                let key_str = String::from_utf8_lossy(&k);
                if let Some(t_name) = key_str.strip_prefix(legacy_prefix) {
                    if !tables.iter().any(|t| t.eq_ignore_ascii_case(t_name)) {
                        tables.push(t_name.to_string());
                    }
                }
            }
        }

        tables.sort();
        Ok(tables)
    }

    /// Queries `information_schema.tables` virtual table catalog.
    pub fn query_information_schema_tables(db: &Database) -> Result<Vec<Vec<String>>> {
        let mut rows = Vec::new();
        let schemas = Self::list_schemas(db)?;

        for s in schemas {
            let tables = Self::list_tables(db, &s)?;
            for t in tables {
                rows.push(vec![
                    "magnumdb".to_string(),
                    s.clone(),
                    t,
                    "BASE TABLE".to_string(),
                ]);
            }
        }
        Ok(rows)
    }

    /// Queries `information_schema.columns` virtual table catalog.
    pub fn query_information_schema_columns(
        db: &Database,
        target_table: Option<&str>,
    ) -> Result<Vec<Vec<String>>> {
        let mut rows = Vec::new();
        let schemas = Self::list_schemas(db)?;

        for s in schemas {
            let tables = Self::list_tables(db, &s)?;
            for t in tables {
                if let Some(filter_t) = target_table {
                    if !t.eq_ignore_ascii_case(filter_t) {
                        continue;
                    }
                }

                if let Some(schema) = Self::get_table_schema(db, &s, &t)? {
                    for (idx, col) in schema.columns.iter().enumerate() {
                        rows.push(vec![
                            "magnumdb".to_string(),
                            s.clone(),
                            t.clone(),
                            col.name.clone(),
                            (idx + 1).to_string(),
                            col.data_type.to_sql_string().to_string(),
                            if col.is_nullable { "YES" } else { "NO" }.to_string(),
                            col.default_value.clone().unwrap_or_else(|| "NULL".to_string()),
                        ]);
                    }
                }
            }
        }
        Ok(rows)
    }
}
