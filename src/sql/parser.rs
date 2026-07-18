use anyhow::{anyhow, Result};

#[derive(Debug, PartialEq)]
pub enum Statement {
    CreateTable {
        table_name: String,
        columns: Vec<String>,
    },
    Insert {
        table_name: String,
        values: Vec<String>,
    },
    Select {
        table_name: String,
    },
}

pub struct Parser;

impl Parser {
    pub fn parse(sql: &str) -> Result<Statement> {
        let sql = sql.trim().trim_end_matches(';');
        let upper_sql = sql.to_uppercase();

        if upper_sql.starts_with("CREATE TABLE") {
            Self::parse_create_table(sql)
        } else if upper_sql.starts_with("INSERT INTO") {
            Self::parse_insert(sql)
        } else if upper_sql.starts_with("SELECT * FROM") {
            Self::parse_select(sql)
        } else {
            Err(anyhow!("Syntax Error: Unrecognized statement"))
        }
    }

    fn parse_create_table(sql: &str) -> Result<Statement> {
        // e.g. CREATE TABLE users(id INT, name TEXT)
        let parts: Vec<&str> = sql.splitn(3, ' ').collect();
        if parts.len() < 3 {
            return Err(anyhow!("Syntax Error: Invalid CREATE TABLE syntax"));
        }
        
        // table_name could have the '(' right next to it: users(id...
        let table_and_cols = parts[2];
        let table_name;
        let cols_str;

        if let Some(paren_idx) = table_and_cols.find('(') {
            table_name = table_and_cols[..paren_idx].trim().to_string();
            let end_paren = table_and_cols.rfind(')').unwrap_or(table_and_cols.len());
            cols_str = table_and_cols[paren_idx + 1..end_paren].to_string();
        } else {
            return Err(anyhow!("Syntax Error: Missing column definitions"));
        }

        let columns = cols_str.split(',')
            .map(|s| s.trim().to_string())
            .collect();

        Ok(Statement::CreateTable {
            table_name,
            columns,
        })
    }

    fn parse_insert(sql: &str) -> Result<Statement> {
        // e.g. INSERT INTO users VALUES(1, 'Soham')
        let upper_sql = sql.to_uppercase();
        let values_idx = upper_sql.find("VALUES").ok_or(anyhow!("Syntax Error: Missing VALUES keyword"))?;
        
        let table_name_part = sql[11..values_idx].trim();
        let values_part = sql[values_idx + 6..].trim();

        if !values_part.starts_with('(') || !values_part.ends_with(')') {
            return Err(anyhow!("Syntax Error: VALUES must be enclosed in parentheses"));
        }

        let inner_vals = &values_part[1..values_part.len() - 1];
        let values = inner_vals.split(',')
            .map(|s| s.trim().trim_matches('\'').to_string())
            .collect();

        Ok(Statement::Insert {
            table_name: table_name_part.to_string(),
            values,
        })
    }

    fn parse_select(sql: &str) -> Result<Statement> {
        // e.g. SELECT * FROM users
        let table_name = sql[13..].trim().to_string();
        Ok(Statement::Select { table_name })
    }
}
