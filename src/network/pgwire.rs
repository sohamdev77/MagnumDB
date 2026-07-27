use crate::sql::{Executor, Parser};
use crate::storage::Database;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;

pub struct PgWireHandler {
    db: Arc<RwLock<Database>>,
}

impl PgWireHandler {
    pub fn new(db: Arc<RwLock<Database>>) -> Self {
        Self { db }
    }

    pub async fn handle_connection(&self, mut socket: TcpStream) -> Result<()> {
        let mut len_buf = [0u8; 4];

        // Read Startup Header
        if socket.read_exact(&mut len_buf).await.is_err() {
            return Ok(());
        }

        let msg_len = i32::from_be_bytes(len_buf) as usize;
        if msg_len < 8 {
            return Ok(());
        }

        let mut body = vec![0u8; msg_len - 4];
        socket.read_exact(&mut body).await?;

        let code = i32::from_be_bytes(body[0..4].try_into().unwrap_or([0; 4]));

        if code == 80877103 {
            // SSL Request -> Respond 'N' (SSL not supported, fallback to plain TCP)
            socket.write_all(b"N").await?;

            // Read actual StartupMessage
            if socket.read_exact(&mut len_buf).await.is_err() {
                return Ok(());
            }
            let startup_len = i32::from_be_bytes(len_buf) as usize;
            let mut startup_body = vec![0u8; startup_len - 4];
            socket.read_exact(&mut startup_body).await?;
        }

        // Send AuthenticationOk: 'R' [0,0,0,8] [0,0,0,0]
        let auth_ok = vec![b'R', 0, 0, 0, 8, 0, 0, 0, 0];
        socket.write_all(&auth_ok).await?;

        // Send ParameterStatus ('S') for server_version
        let param_msg = Self::make_parameter_status("server_version", "15.0");
        socket.write_all(&param_msg).await?;

        // Send ReadyForQuery ('Z') 'I'
        let ready_msg = vec![b'Z', 0, 0, 0, 5, b'I'];
        socket.write_all(&ready_msg).await?;

        let mut prepared_statements: HashMap<String, String> = HashMap::new();
        let mut bound_sql = String::new();

        // Main Message Loop
        loop {
            let mut tag_buf = [0u8; 1];
            if socket.read_exact(&mut tag_buf).await.is_err() {
                break;
            }

            let tag = tag_buf[0];
            if socket.read_exact(&mut len_buf).await.is_err() {
                break;
            }

            let qlen = i32::from_be_bytes(len_buf) as usize;
            let mut qbody = vec![0u8; qlen - 4];
            if socket.read_exact(&mut qbody).await.is_err() {
                break;
            }

            match tag {
                b'Q' => {
                    // Simple Query ('Q')
                    let query_str = String::from_utf8_lossy(&qbody[..qbody.len().saturating_sub(1)]);
                    let sql = query_str.trim();

                    self.execute_and_respond(&mut socket, sql).await?;
                    socket.write_all(&ready_msg).await?;
                }
                b'P' => {
                    // Parse ('P'): Prepared Statement Registration
                    if let Ok((stmt_name, sql_template)) = parse_p_message(&qbody) {
                        prepared_statements.insert(stmt_name, sql_template);
                    }
                    // Response ParseComplete ('1')
                    socket.write_all(&[b'1', 0, 0, 0, 4]).await?;
                }
                b'B' => {
                    // Bind ('B'): Parameter Value Binding
                    if let Ok((stmt_name, params)) = parse_b_message(&qbody) {
                        if let Some(template) = prepared_statements.get(&stmt_name) {
                            bound_sql = substitute_params(template, &params);
                        } else if let Some(template) = prepared_statements.get("") {
                            bound_sql = substitute_params(template, &params);
                        }
                    }
                    // Response BindComplete ('2')
                    socket.write_all(&[b'2', 0, 0, 0, 4]).await?;
                }
                b'E' => {
                    // Execute ('E'): Execute bound statement
                    if !bound_sql.is_empty() {
                        let sql = bound_sql.clone();
                        self.execute_and_respond(&mut socket, &sql).await?;
                    } else {
                        Self::send_query_response(&mut socket, "Query OK.").await?;
                    }
                    socket.write_all(&ready_msg).await?;
                }
                b'S' => {
                    // Sync ('S')
                    socket.write_all(&ready_msg).await?;
                }
                b'X' => {
                    // Terminate ('X')
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn execute_and_respond(&self, socket: &mut TcpStream, sql: &str) -> Result<()> {
        let exec_res = {
            let mut guard = self.db.write().await;
            let mut executor = Executor::new(&mut guard);
            match Parser::parse(sql) {
                Ok(stmt) => executor.execute(stmt),
                Err(e) => Err(e),
            }
        };

        match exec_res {
            Ok(out) => {
                Self::send_query_response(socket, &out).await?;
            }
            Err(e) => {
                Self::send_error_response(socket, &e.to_string()).await?;
            }
        }

        Ok(())
    }

    fn make_parameter_status(name: &str, val: &str) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(val.as_bytes());
        body.push(0);

        let mut msg = vec![b'S'];
        let len = (body.len() + 4) as i32;
        msg.extend_from_slice(&len.to_be_bytes());
        msg.extend_from_slice(&body);
        msg
    }

    async fn send_query_response(socket: &mut TcpStream, out: &str) -> Result<()> {
        let lines: Vec<&str> = out.lines().collect();

        if lines.len() >= 4 && lines[0].starts_with("+--") {
            let header_line = lines[1].trim_matches('|').trim();
            let col_names: Vec<&str> = header_line.split('|').map(|s| s.trim()).collect();

            // Send RowDescription ('T')
            let mut desc_body = Vec::new();
            desc_body.extend_from_slice(&(col_names.len() as i16).to_be_bytes());

            for col in &col_names {
                desc_body.extend_from_slice(col.as_bytes());
                desc_body.push(0);
                desc_body.extend_from_slice(&0i32.to_be_bytes());
                desc_body.extend_from_slice(&0i16.to_be_bytes());
                desc_body.extend_from_slice(&25i32.to_be_bytes());
                desc_body.extend_from_slice(&(-1i16).to_be_bytes());
                desc_body.extend_from_slice(&(-1i32).to_be_bytes());
                desc_body.extend_from_slice(&0i16.to_be_bytes());
            }

            let mut desc_msg = vec![b'T'];
            let desc_len = (desc_body.len() + 4) as i32;
            desc_msg.extend_from_slice(&desc_len.to_be_bytes());
            desc_msg.extend_from_slice(&desc_body);
            socket.write_all(&desc_msg).await?;

            // Send DataRow ('D') for each row
            for line in &lines[3..lines.len() - 2] {
                if line.starts_with('|') {
                    let row_vals: Vec<&str> = line.trim_matches('|').split('|').map(|s| s.trim()).collect();
                    let mut row_body = Vec::new();
                    row_body.extend_from_slice(&(row_vals.len() as i16).to_be_bytes());

                    for v in row_vals {
                        let bytes = v.as_bytes();
                        row_body.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                        row_body.extend_from_slice(bytes);
                    }

                    let mut row_msg = vec![b'D'];
                    let rlen = (row_body.len() + 4) as i32;
                    row_msg.extend_from_slice(&rlen.to_be_bytes());
                    row_msg.extend_from_slice(&row_body);
                    socket.write_all(&row_msg).await?;
                }
            }
        }

        // Send CommandComplete ('C')
        let tag = b"SELECT 1\0";
        let mut msg = vec![b'C'];
        let len = (tag.len() + 4) as i32;
        msg.extend_from_slice(&len.to_be_bytes());
        msg.extend_from_slice(tag);
        socket.write_all(&msg).await?;

        Ok(())
    }

    async fn send_error_response(socket: &mut TcpStream, err_msg: &str) -> Result<()> {
        let mut body = vec![b'S', b'E', 0, b'M'];
        body.extend_from_slice(err_msg.as_bytes());
        body.push(0);
        body.push(0);

        let mut msg = vec![b'E'];
        let len = (body.len() + 4) as i32;
        msg.extend_from_slice(&len.to_be_bytes());
        msg.extend_from_slice(&body);
        socket.write_all(&msg).await?;
        Ok(())
    }
}

/// Substitutes `$1`, `$2`, ... placeholders in a SQL template with bound values.
pub fn substitute_params(sql: &str, params: &[String]) -> String {
    let mut result = sql.to_string();
    for (i, param) in params.iter().enumerate() {
        let placeholder = format!("${}", i + 1);
        result = result.replace(&placeholder, param);
    }
    result
}

fn parse_p_message(body: &[u8]) -> Result<(String, String)> {
    let mut parts = body.split(|&b| b == 0);
    let stmt_name = String::from_utf8_lossy(parts.next().unwrap_or(&[])).to_string();
    let sql_template = String::from_utf8_lossy(parts.next().unwrap_or(&[])).to_string();
    Ok((stmt_name, sql_template))
}

fn parse_b_message(body: &[u8]) -> Result<(String, Vec<String>)> {
    let mut offset = 0;

    // Portal name (null terminated)
    while offset < body.len() && body[offset] != 0 {
        offset += 1;
    }
    offset += 1; // skip null

    // Statement name (null terminated)
    let stmt_start = offset;
    while offset < body.len() && body[offset] != 0 {
        offset += 1;
    }
    let stmt_name = String::from_utf8_lossy(&body[stmt_start..offset]).to_string();
    offset += 1; // skip null

    let mut params = Vec::new();
    if offset + 2 <= body.len() {
        let num_formats = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
        offset += 2 + num_formats * 2;

        if offset + 2 <= body.len() {
            let num_params = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
            offset += 2;

            for _ in 0..num_params {
                if offset + 4 <= body.len() {
                    let param_len = i32::from_be_bytes([
                        body[offset],
                        body[offset + 1],
                        body[offset + 2],
                        body[offset + 3],
                    ]);
                    offset += 4;

                    if param_len > 0 && offset + (param_len as usize) <= body.len() {
                        let p_val = String::from_utf8_lossy(&body[offset..offset + (param_len as usize)]).to_string();
                        params.push(p_val);
                        offset += param_len as usize;
                    }
                }
            }
        }
    }

    Ok((stmt_name, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_params() {
        let sql = "SELECT * FROM users WHERE id = $1 AND name = $2";
        let params = vec!["1".to_string(), "'Alice'".to_string()];
        let substituted = substitute_params(sql, &params);
        assert_eq!(substituted, "SELECT * FROM users WHERE id = 1 AND name = 'Alice'");
    }
}
