use crate::sql::{Executor, Parser};
use crate::storage::Database;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct PgWireHandler {
    db: Arc<Mutex<Database>>,
}

impl PgWireHandler {
    pub fn new(db: Arc<Mutex<Database>>) -> Self {
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

        // Main Query Loop
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

            if tag == b'Q' {
                // Query
                let query_str = String::from_utf8_lossy(&qbody[..qbody.len() - 1]);
                let sql = query_str.trim();

                let exec_res = match self.db.lock() {
                    Ok(mut guard) => {
                        let mut executor = Executor::new(&mut guard);
                        match Parser::parse(sql) {
                            Ok(stmt) => executor.execute(stmt),
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Database lock error: {}", e)),
                };

                match exec_res {
                    Ok(out) => {
                        Self::send_query_response(&mut socket, &out).await?;
                    }
                    Err(e) => {
                        Self::send_error_response(&mut socket, &e.to_string()).await?;
                    }
                }

                socket.write_all(&ready_msg).await?;
            } else if tag == b'X' {
                // Terminate
                break;
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
        // Send CommandComplete ('C')
        let tag = b"SELECT 1\0";
        let mut msg = vec![b'C'];
        let len = (tag.len() + 4) as i32;
        msg.extend_from_slice(&len.to_be_bytes());
        msg.extend_from_slice(tag);
        socket.write_all(&msg).await?;
        let _ = out;
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
