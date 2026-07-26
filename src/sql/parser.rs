use anyhow::{anyhow, Result};

/// Reserved internal key prefix. Table and column names must not start with this.
const RESERVED_PREFIX: &str = "__";

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

    /// Validates that an identifier (table/column name) doesn't use reserved prefixes.
    fn validate_identifier(name: &str) -> Result<()> {
        if name.starts_with(RESERVED_PREFIX) {
            return Err(anyhow!(
                "Identifier '{}' uses reserved prefix '{}'. Choose a different name.",
                name, RESERVED_PREFIX
            ));
        }
        if name.is_empty() {
            return Err(anyhow!("Identifier cannot be empty"));
        }
        Ok(())
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

        Self::validate_identifier(&table_name)?;

        let columns: Vec<String> = cols_str.split(',').map(|s| s.trim().to_string()).collect();
        for col_def in &columns {
            let col_name = col_def.split_whitespace().next().unwrap_or("");
            Self::validate_identifier(col_name)?;
        }

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

        Self::validate_identifier(&table_name)?;
        Self::validate_identifier(&column)?;

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
        let values = split_values_respecting_quotes(inner_vals)?;

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
                let wc = parse_where_equality(cond)?;
                (tname, Some(wc))
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

            let wc = parse_where_equality(cond)?;
            Ok(Statement::Select {
                table_name,
                where_clause: Some(wc),
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

        let (column, value) = parse_assignment(set_expr)?;
        let (_, pk_val) = parse_assignment(where_expr)?;

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
        let (_, pk_val) = parse_assignment(where_expr)?;

        Ok(Statement::Delete { table_name, pk_val })
    }

    fn parse_drop_table(sql: &str) -> Result<Statement> {
        let table_name = sql[10..].trim().to_string();
        Self::validate_identifier(&table_name)?;
        Ok(Statement::DropTable { table_name })
    }
}

/// Splits a comma-separated value list while respecting single-quoted strings.
/// Handles escaped quotes ('') inside quoted strings.
fn split_values_respecting_quotes(input: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '\'' {
                // Check for escaped quote ''
                if chars.peek() == Some(&'\'') {
                    current.push('\'');
                    chars.next(); // consume the second quote
                } else {
                    in_quotes = false;
                    // Don't add the closing quote to the value
                }
            } else {
                current.push(ch);
            }
        } else if ch == '\'' {
            in_quotes = true;
            // Don't add the opening quote to the value
        } else if ch == ',' {
            values.push(current.trim().to_string());
            current = String::new();
        } else {
            current.push(ch);
        }
    }

    if in_quotes {
        return Err(anyhow!("Syntax Error: Unterminated string literal"));
    }

    values.push(current.trim().to_string());
    Ok(values)
}

/// Parses a `col = value` or `col = 'value'` assignment, handling quoted values.
fn parse_assignment(expr: &str) -> Result<(String, String)> {
    let eq_idx = expr.find('=')
        .ok_or_else(|| anyhow!("Syntax Error: Expected '=' in expression '{}'", expr))?;

    let col = expr[..eq_idx].trim().to_string();
    let val = expr[eq_idx + 1..].trim().trim_matches('\'').to_string();
    Ok((col, val))
}

/// Parses a WHERE clause with equality: `col = value` or `col = 'value'`.
fn parse_where_equality(cond: &str) -> Result<(String, String)> {
    parse_assignment(cond)
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

    #[test]
    fn test_parse_insert_with_commas_in_values() {
        let stmt = Parser::parse("INSERT INTO notes VALUES(1, 'hello, world')").unwrap();
        if let Statement::Insert { values, .. } = stmt {
            assert_eq!(values.len(), 2);
            assert_eq!(values[0], "1");
            assert_eq!(values[1], "hello, world");
        } else {
            panic!("Expected Insert statement");
        }
    }

    #[test]
    fn test_parse_insert_with_escaped_quotes() {
        let stmt = Parser::parse("INSERT INTO notes VALUES(1, 'it''s a test')").unwrap();
        if let Statement::Insert { values, .. } = stmt {
            assert_eq!(values.len(), 2);
            assert_eq!(values[1], "it's a test");
        } else {
            panic!("Expected Insert statement");
        }
    }

    #[test]
    fn test_reject_reserved_table_name() {
        let result = Parser::parse("CREATE TABLE __internal__(id INT)");
        assert!(result.is_err());
    }

    #[test]
    fn test_reject_reserved_column_name() {
        let result = Parser::parse("CREATE TABLE users(__secret INT)");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unterminated_string() {
        let result = Parser::parse("INSERT INTO t VALUES(1, 'unclosed)");
        assert!(result.is_err());
    }
}
