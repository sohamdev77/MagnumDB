use crate::sql::{Executor, Parser};
use crate::storage::Database;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Server {
    db: Arc<Mutex<Database>>,
    addr: String,
}

impl Server {
    pub fn new(db: Database, addr: String) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            addr,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(&self.addr).await?;
        println!("MagnumDB Server listening on {}", self.addr);

        loop {
            let (mut socket, peer) = listener.accept().await?;
            let db_ref = Arc::clone(&self.db);

            tokio::spawn(async move {
                log::info!("Client connected from {}", peer);
                let mut len_buf = [0u8; 4];

                loop {
                    if socket.read_exact(&mut len_buf).await.is_err() {
                        break; // Connection closed
                    }

                    let query_len = u32::from_le_bytes(len_buf) as usize;
                    let mut query_buf = vec![0u8; query_len];

                    if socket.read_exact(&mut query_buf).await.is_err() {
                        break;
                    }

                    let query = String::from_utf8_lossy(&query_buf);

                    let response = {
                        match db_ref.lock() {
                            Ok(mut guard) => {
                                let mut executor = Executor::new(&mut guard);
                                match Parser::parse(&query) {
                                    Ok(stmt) => match executor.execute(stmt) {
                                        Ok(res) => res,
                                        Err(e) => format!("Error: {}", e),
                                    },
                                    Err(e) => format!("Error: {}", e),
                                }
                            }
                            Err(e) => format!("Error: Internal database lock error: {}", e),
                        }
                    };

                    let resp_bytes = response.as_bytes();
                    let resp_len = (resp_bytes.len() as u32).to_le_bytes();

                    if socket.write_all(&resp_len).await.is_err() {
                        break;
                    }
                    if socket.write_all(resp_bytes).await.is_err() {
                        break;
                    }
                }
            });
        }
    }
}
