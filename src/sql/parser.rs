use anyhow::{anyhow, Result};

#[derive(Debug, PartialEq, Clone)]
pub enum Statement {
    CreateTable {
        table_name: String,
        columns: Vec<String>,
    },
    CreateIndex {
        index_name: String,
        table_name: String,
        column: String,
    },
    Insert {
        table_name: String,
        values: Vec<String>,
    },
    Select {
        table_name: String,
        where_clause: Option<(String, String)>,
    },
    SelectRange {
        table_name: String,
        column: String,
        op: String,
        val: String,
    },
    SelectAggregate {
        func: String,
        column: String,
        table_name: String,
        where_clause: Option<(String, String)>,
    },
    Update {
        table_name: String,
        column: String,
        value: String,
        pk_val: String,
    },
    Delete {
        table_name: String,
        pk_val: String,
    },
    DropTable {
        table_name: String,
    },
    ShowTables,
    Begin,
    Commit,
    Rollback,
}

pub struct Parser;

impl Parser {
    pub fn parse(sql: &str) -> Result<Statement> {
        let sql = sql.trim().trim_end_matches(';');
        let upper_sql = sql.to_uppercase();

        if upper_sql.starts_with("CREATE INDEX") {
            Self::parse_create_index(sql)
        } else if upper_sql.starts_with("CREATE TABLE") {
            Self::parse_create_table(sql)
        } else if upper_sql.starts_with("INSERT INTO") {
            Self::parse_insert(sql)
        } else if upper_sql.starts_with("SELECT") {
            Self::parse_select(sql)
        } else if upper_sql.starts_with("UPDATE") {
            Self::parse_update(sql)
        } else if upper_sql.starts_with("DELETE FROM") {
            Self::parse_delete(sql)
        } else if upper_sql.starts_with("DROP TABLE") {
            Self::parse_drop_table(sql)
        } else if upper_sql == "SHOW TABLES" {
            Ok(Statement::ShowTables)
        } else if upper_sql == "BEGIN" {
            Ok(Statement::Begin)
        } else if upper_sql == "COMMIT" {
            Ok(Statement::Commit)
        } else if upper_sql == "ROLLBACK" {
            Ok(Statement::Rollback)
        } else {
            Err(anyhow!("Syntax Error: Unrecognized statement"))
        }
    }

    fn parse_create_table(sql: &str) -> Result<Statement> {
        let parts: Vec<&str> = sql.splitn(3, ' ').collect();
        if parts.len() < 3 {
            return Err(anyhow!("Syntax Error: Invalid CREATE TABLE syntax"));
        }

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

        let columns = cols_str.split(',').map(|s| s.trim().to_string()).collect();

        Ok(Statement::CreateTable {
            table_name,
            columns,
        })
    }

    fn parse_create_index(sql: &str) -> Result<Statement> {
        let upper_sql = sql.to_uppercase();
        let on_idx = upper_sql
            .find(" ON ")
            .ok_or_else(|| anyhow!("Syntax Error: Missing ON keyword in CREATE INDEX"))?;

        let index_name = sql[12..on_idx].trim().to_string();
        let rest = sql[on_idx + 4..].trim();

        let paren_idx = rest
            .find('(')
            .ok_or_else(|| anyhow!("Syntax Error: Missing '(' in CREATE INDEX"))?;
        let end_paren = rest
            .rfind(')')
            .ok_or_else(|| anyhow!("Syntax Error: Missing ')' in CREATE INDEX"))?;

        let table_name = rest[..paren_idx].trim().to_string();
        let column = rest[paren_idx + 1..end_paren].trim().to_string();

        Ok(Statement::CreateIndex {
            index_name,
            table_name,
            column,
        })
    }

    fn parse_insert(sql: &str) -> Result<Statement> {
        let upper_sql = sql.to_uppercase();
        let values_idx = upper_sql
            .find("VALUES")
            .ok_or_else(|| anyhow!("Syntax Error: Missing VALUES keyword"))?;

        let table_name_part = sql[11..values_idx].trim();
        let values_part = sql[values_idx + 6..].trim();

        if !values_part.starts_with('(') || !values_part.ends_with(')') {
            return Err(anyhow!(
                "Syntax Error: VALUES must be enclosed in parentheses"
            ));
        }

        let inner_vals = &values_part[1..values_part.len() - 1];
        let values = inner_vals
            .split(',')
            .map(|s| s.trim().trim_matches('\'').to_string())
            .collect();

        Ok(Statement::Insert {
            table_name: table_name_part.to_string(),
            values,
        })
    }

    fn parse_select(sql: &str) -> Result<Statement> {
        let upper_sql = sql.to_uppercase();
        let from_idx = upper_sql
            .find("FROM")
            .ok_or_else(|| anyhow!("Syntax Error: Missing FROM keyword"))?;

        let target = sql[6..from_idx].trim();
        let upper_target = target.to_uppercase();

        let rest = sql[from_idx + 4..].trim();
        let upper_rest = rest.to_uppercase();

        if upper_target.starts_with("COUNT(")
            || upper_target.starts_with("SUM(")
            || upper_target.starts_with("AVG(")
        {
            let func_end = target.find('(').unwrap();
            let func = target[..func_end].to_uppercase();
            let col_end = target.rfind(')').unwrap_or(target.len());
            let col = target[func_end + 1..col_end].trim().to_string();

            let (table_name, where_clause) = if let Some(where_idx) = upper_rest.find("WHERE") {
                let tname = rest[..where_idx].trim().to_string();
                let cond = rest[where_idx + 5..].trim();
                let cond_parts: Vec<&str> = cond.split('=').collect();
                if cond_parts.len() != 2 {
                    return Err(anyhow!("Syntax Error: Invalid WHERE clause"));
                }
                let col = cond_parts[0].trim().to_string();
                let val = cond_parts[1].trim().trim_matches('\'').to_string();
                (tname, Some((col, val)))
            } else {
                (rest.to_string(), None)
            };

            return Ok(Statement::SelectAggregate {
                func,
                column: col,
                table_name,
                where_clause,
            });
        }

        if let Some(where_idx) = upper_rest.find("WHERE") {
            let table_name = rest[..where_idx].trim().to_string();
            let cond = rest[where_idx + 5..].trim();

            for op in &[">=", "<=", ">", "<"] {
                if let Some(op_idx) = cond.find(op) {
                    let col = cond[..op_idx].trim().to_string();
                    let val = cond[op_idx + op.len()..].trim().trim_matches('\'').to_string();
                    return Ok(Statement::SelectRange {
                        table_name,
                        column: col,
                        op: (*op).to_string(),
                        val,
                    });
                }
            }

            let cond_parts: Vec<&str> = cond.split('=').collect();
            if cond_parts.len() != 2 {
                return Err(anyhow!("Syntax Error: Invalid WHERE clause"));
            }
            let col = cond_parts[0].trim().to_string();
            let val = cond_parts[1].trim().trim_matches('\'').to_string();

            Ok(Statement::Select {
                table_name,
                where_clause: Some((col, val)),
            })
        } else {
            Ok(Statement::Select {
                table_name: rest.to_string(),
                where_clause: None,
            })
        }
    }

    fn parse_update(sql: &str) -> Result<Statement> {
        let upper_sql = sql.to_uppercase();
        let set_idx = upper_sql
            .find("SET")
            .ok_or_else(|| anyhow!("Syntax Error: Missing SET keyword"))?;
        let where_idx = upper_sql
            .find("WHERE")
            .ok_or_else(|| anyhow!("Syntax Error: Missing WHERE keyword in UPDATE"))?;

        let table_name = sql[6..set_idx].trim().to_string();
        let set_expr = sql[set_idx + 3..where_idx].trim();
        let where_expr = sql[where_idx + 5..].trim();

        let set_parts: Vec<&str> = set_expr.split('=').collect();
        if set_parts.len() != 2 {
            return Err(anyhow!("Syntax Error: Invalid SET clause in UPDATE"));
        }

        let column = set_parts[0].trim().to_string();
        let value = set_parts[1].trim().trim_matches('\'').to_string();

        let where_parts: Vec<&str> = where_expr.split('=').collect();
        if where_parts.len() != 2 {
            return Err(anyhow!("Syntax Error: Invalid WHERE clause in UPDATE"));
        }
        let pk_val = where_parts[1].trim().trim_matches('\'').to_string();

        Ok(Statement::Update {
            table_name,
            column,
            value,
            pk_val,
        })
    }

    fn parse_delete(sql: &str) -> Result<Statement> {
        let upper_sql = sql.to_uppercase();
        let where_idx = upper_sql
            .find("WHERE")
            .ok_or_else(|| anyhow!("Syntax Error: Missing WHERE clause in DELETE"))?;

        let table_name = sql[11..where_idx].trim().to_string();
        let where_expr = sql[where_idx + 5..].trim();
        let where_parts: Vec<&str> = where_expr.split('=').collect();
        if where_parts.len() != 2 {
            return Err(anyhow!("Syntax Error: Invalid WHERE clause in DELETE"));
        }
        let pk_val = where_parts[1].trim().trim_matches('\'').to_string();

        Ok(Statement::Delete { table_name, pk_val })
    }

    fn parse_drop_table(sql: &str) -> Result<Statement> {
        let table_name = sql[10..].trim().to_string();
        Ok(Statement::DropTable { table_name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_range_query() {
        let range_gt = Parser::parse("SELECT * FROM users WHERE age >= 21").unwrap();
        assert_eq!(
            range_gt,
            Statement::SelectRange {
                table_name: "users".to_string(),
                column: "age".to_string(),
                op: ">=".to_string(),
                val: "21".to_string(),
            }
        );
    }
}
