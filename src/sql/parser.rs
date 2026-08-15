use anyhow::{anyhow, Result};

/// Reserved internal key prefix. Table and column names must not start with this.
const RESERVED_PREFIX: &str = "__";

#[derive(Debug, PartialEq, Clone)]
pub enum Statement {
    CreateSchema {
        schema_name: String,
    },
    CreateTable {
        table_name: String,
        columns: Vec<String>,
    },
    CreateIndex {
        index_name: String,
        table_name: String,
        columns: Vec<String>,
    },
    Insert {
        table_name: String,
        values: Vec<String>,
    },
    Select {
        table_name: String,
        where_clause: Option<(String, String)>,
        order_by: Option<(String, bool)>,       // (col_name, is_desc)
        limit_offset: Option<(usize, usize)>,  // (limit, offset)
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
    SelectGroupAggregate {
        group_col: String,
        func: String,
        agg_col: String,
        table_name: String,
        where_clause: Option<(String, String)>,
        having_clause: Option<(String, String)>,
    },
    SelectJoin {
        left_table: String,
        right_table: String,
        left_col: String,
        right_col: String,
        is_left_join: bool,
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
    ShowSchemas,
    Begin,
    Commit,
    Rollback,
}

pub struct Parser;

impl Parser {
    pub fn parse(sql: &str) -> Result<Statement> {
        let sql = sql.trim().trim_end_matches(';');
        let upper_sql = sql.to_uppercase();

        if upper_sql.starts_with("CREATE SCHEMA") {
            Self::parse_create_schema(sql)
        } else if upper_sql.starts_with("CREATE INDEX") {
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
        } else if upper_sql == "SHOW SCHEMAS" {
            Ok(Statement::ShowSchemas)
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
        let clean = name.trim();
        if clean.starts_with(RESERVED_PREFIX) {
            return Err(anyhow!(
                "Identifier '{}' uses reserved prefix '{}'. Choose a different name.",
                clean, RESERVED_PREFIX
            ));
        }
        if clean.is_empty() {
            return Err(anyhow!("Identifier cannot be empty"));
        }
        Ok(())
    }

    fn parse_create_schema(sql: &str) -> Result<Statement> {
        let schema_name = sql[13..].trim().to_string();
        Self::validate_identifier(&schema_name)?;
        Ok(Statement::CreateSchema { schema_name })
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
        let cols_raw = rest[paren_idx + 1..end_paren].trim();
        let columns: Vec<String> = cols_raw.split(',').map(|c| c.trim().to_string()).collect();

        Self::validate_identifier(&table_name)?;
        for col in &columns {
            Self::validate_identifier(col)?;
        }

        Ok(Statement::CreateIndex {
            index_name,
            table_name,
            columns,
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
        let rest = sql[from_idx + 4..].trim();
        let upper_rest = rest.to_uppercase();

        // Check for JOIN query: SELECT * FROM t1 [LEFT] JOIN t2 ON t1.col1 = t2.col2
        if upper_rest.contains(" JOIN ") {
            let is_left_join = upper_rest.contains("LEFT JOIN");
            let join_kw = if is_left_join { "LEFT JOIN" } else { "JOIN" };
            let join_idx = upper_rest.find(join_kw).unwrap();
            let left_table = rest[..join_idx].trim().to_string();

            let on_idx = upper_rest
                .find(" ON ")
                .ok_or_else(|| anyhow!("Syntax Error: Missing ON in JOIN clause"))?;

            let right_table = rest[join_idx + join_kw.len()..on_idx].trim().to_string();
            let on_cond = rest[on_idx + 4..].trim();

            let (left_col, right_col) = parse_join_condition(on_cond)?;

            return Ok(Statement::SelectJoin {
                left_table,
                right_table,
                left_col,
                right_col,
                is_left_join,
            });
        }

        // Check for GROUP BY query: SELECT col, COUNT(*) FROM table [WHERE ...] GROUP BY col [HAVING ...]
        if upper_rest.contains("GROUP BY") {
            let gb_idx = upper_rest.find("GROUP BY").unwrap();
            let before_gb = rest[..gb_idx].trim();
            let after_gb = rest[gb_idx + 8..].trim();
            let upper_after_gb = after_gb.to_uppercase();

            let (group_col, having_clause) = if let Some(having_idx) = upper_after_gb.find("HAVING") {
                let gcol = after_gb[..having_idx].trim().to_string();
                let having_cond = after_gb[having_idx + 6..].trim();
                let h_parsed = parse_having_condition(having_cond)?;
                (gcol, Some(h_parsed))
            } else {
                (after_gb.to_string(), None)
            };

            let target_parts: Vec<&str> = target.split(',').map(|s| s.trim()).collect();
            if target_parts.len() != 2 {
                return Err(anyhow!("Syntax Error: GROUP BY query expects 'group_col, FUNC(col)'"));
            }

            let agg_part = target_parts[1].to_uppercase();
            let func_end = agg_part.find('(').ok_or_else(|| anyhow!("Invalid agg func in GROUP BY"))?;
            let func = agg_part[..func_end].to_string();
            let col_end = agg_part.rfind(')').unwrap_or(agg_part.len());
            let agg_col = target_parts[1][func_end + 1..col_end].trim().to_string();

            let upper_before_gb = before_gb.to_uppercase();
            let (table_name, where_clause) = if let Some(w_idx) = upper_before_gb.find("WHERE") {
                let tname = before_gb[..w_idx].trim().to_string();
                let cond = before_gb[w_idx + 5..].trim();
                let wc = parse_where_equality(cond)?;
                (tname, Some(wc))
            } else {
                (before_gb.to_string(), None)
            };

            return Ok(Statement::SelectGroupAggregate {
                group_col,
                func,
                agg_col,
                table_name,
                where_clause,
                having_clause,
            });
        }

        let upper_target = target.to_uppercase();

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

        // Parse trailing ORDER BY and LIMIT / OFFSET
        let (table_and_where, order_by, limit_offset) = parse_order_by_and_limit(rest)?;
        let upper_table_and_where = table_and_where.to_uppercase();

        if let Some(where_idx) = upper_table_and_where.find("WHERE") {
            let table_name = table_and_where[..where_idx].trim().to_string();
            let cond = table_and_where[where_idx + 5..].trim();

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
                order_by,
                limit_offset,
            })
        } else {
            Ok(Statement::Select {
                table_name: table_and_where,
                where_clause: None,
                order_by,
                limit_offset,
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

/// Parses trailing `ORDER BY col [ASC|DESC]` and `LIMIT n [OFFSET m]` clauses.
#[allow(clippy::type_complexity)]
fn parse_order_by_and_limit(rest: &str) -> Result<(String, Option<(String, bool)>, Option<(usize, usize)>)> {
    let upper = rest.to_uppercase();
    let mut main_part = rest;

    let mut limit_offset = None;
    let mut order_by = None;

    let limit_idx = upper.find("LIMIT");
    let order_idx = upper.find("ORDER BY");

    let first_clause_idx = match (order_idx, limit_idx) {
        (Some(o), Some(l)) => Some(o.min(l)),
        (Some(o), None) => Some(o),
        (None, Some(l)) => Some(l),
        (None, None) => None,
    };

    if let Some(idx) = first_clause_idx {
        main_part = rest[..idx].trim();
    }

    if let Some(o_idx) = order_idx {
        let end_idx = limit_idx.filter(|l| *l > o_idx).unwrap_or(rest.len());
        let order_str = rest[o_idx + 8..end_idx].trim();
        let parts: Vec<&str> = order_str.split_whitespace().collect();

        if !parts.is_empty() {
            let col = parts[0].to_string();
            let is_desc = parts.get(1).map(|dir| dir.eq_ignore_ascii_case("DESC")).unwrap_or(false);
            order_by = Some((col, is_desc));
        }
    }

    if let Some(l_idx) = limit_idx {
        let limit_str = rest[l_idx + 5..].trim();
        let upper_limit = limit_str.to_uppercase();

        let (limit_val_str, offset_val_str) = if let Some(off_idx) = upper_limit.find("OFFSET") {
            let l_part = limit_str[..off_idx].trim();
            let o_part = limit_str[off_idx + 6..].trim();
            (l_part, Some(o_part))
        } else {
            (limit_str, None)
        };

        if let Ok(limit_num) = limit_val_str.parse::<usize>() {
            let offset_num = offset_val_str
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            limit_offset = Some((limit_num, offset_num));
        }
    }

    Ok((main_part.to_string(), order_by, limit_offset))
}

fn parse_join_condition(cond: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = cond.split('=').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Syntax Error: Invalid ON condition in JOIN"));
    }
    let left_col = extract_column_name(parts[0].trim());
    let right_col = extract_column_name(parts[1].trim());
    Ok((left_col, right_col))
}

fn extract_column_name(full: &str) -> String {
    if let Some(dot_idx) = full.find('.') {
        full[dot_idx + 1..].trim().to_string()
    } else {
        full.trim().to_string()
    }
}

fn parse_having_condition(cond: &str) -> Result<(String, String)> {
    for op in &[">=", "<=", ">", "<", "="] {
        if let Some(op_idx) = cond.find(op) {
            let val = cond[op_idx + op.len()..].trim().to_string();
            return Ok(((*op).to_string(), val));
        }
    }
    Err(anyhow!("Syntax Error: Invalid HAVING condition '{}'", cond))
}

fn split_values_respecting_quotes(input: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    current.push('\'');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
        } else if ch == '\'' {
            in_quotes = true;
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

fn parse_assignment(expr: &str) -> Result<(String, String)> {
    let eq_idx = expr.find('=')
        .ok_or_else(|| anyhow!("Syntax Error: Expected '=' in expression '{}'", expr))?;

    let col = expr[..eq_idx].trim().to_string();
    let val = expr[eq_idx + 1..].trim().trim_matches('\'').to_string();
    Ok((col, val))
}

fn parse_where_equality(cond: &str) -> Result<(String, String)> {
    parse_assignment(cond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_order_by_limit_offset() {
        let stmt = Parser::parse("SELECT * FROM users ORDER BY age DESC LIMIT 10 OFFSET 5").unwrap();
        assert_eq!(
            stmt,
            Statement::Select {
                table_name: "users".to_string(),
                where_clause: None,
                order_by: Some(("age".to_string(), true)),
                limit_offset: Some((10, 5)),
            }
        );
    }

    #[test]
    fn test_parse_create_schema() {
        let stmt = Parser::parse("CREATE SCHEMA analytics").unwrap();
        assert_eq!(
            stmt,
            Statement::CreateSchema {
                schema_name: "analytics".to_string(),
            }
        );
    }
}
