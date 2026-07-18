use crate::sql::parser::Statement;
use crate::storage::Database;
use anyhow::Result;

pub struct Executor<'a> {
    db: &'a mut Database,
}

impl<'a> Executor<'a> {
    pub fn new(db: &'a mut Database) -> Self {
        Self { db }
    }

    pub fn execute(&mut self, stmt: Statement) -> Result<String> {
        match stmt {
            Statement::CreateTable { table_name, .. } => {
                // In a full implementation, we'd save schema metadata.
                // For Phase 3, we just pretend it succeeds.
                Ok(format!("Query OK, table '{}' created.", table_name))
            }
            Statement::Insert { table_name, values } => {
                // Encode table_name + primary key as internal DB key.
                // We assume values[0] is the primary key (id).
                if values.is_empty() {
                    return Err(anyhow::anyhow!("No values provided for insertion"));
                }
                
                let pk = &values[0];
                let internal_key = format!("{}:{}", table_name, pk);
                let internal_val = values.join(",");

                self.db.put(internal_key.as_bytes(), internal_val.as_bytes())?;
                
                Ok("Query OK, 1 row inserted.".to_string())
            }
            Statement::Select { table_name } => {
                let records = self.db.scan()?;
                let prefix = format!("{}:", table_name);
                
                let mut output = format!("+-----------------+\n| {} (Data) |\n+-----------------+\n", table_name);
                let mut count = 0;
                
                for (key, val) in records {
                    let k_str = String::from_utf8_lossy(&key);
                    if k_str.starts_with(&prefix) {
                        let v_str = String::from_utf8_lossy(&val);
                        output.push_str(&format!("| {} |\n", v_str));
                        count += 1;
                    }
                }
                
                output.push_str("+-----------------+\n");
                output.push_str(&format!("{} row(s) in set.", count));
                
                Ok(output)
            }
        }
    }
}
