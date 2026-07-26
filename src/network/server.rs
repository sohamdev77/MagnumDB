use crate::sql::{Executor, Parser};
use crate::storage::Database;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, Semaphore};

pub struct Server {
    db: Arc<RwLock<Database>>,
    addr: String,
    max_connections: usize,
    idle_timeout: Duration,
}

impl Server {
    pub fn new(db: Database, addr: String) -> Self {
        Self {
            db: Arc::new(RwLock::new(db)),
            addr,
            max_connections: 128,
            idle_timeout: Duration::from_secs(30),
        }
    }

    /// Creates a server with custom connection limits.
    pub fn with_limits(mut self, max_connections: usize, idle_timeout_secs: u64) -> Self {
        self.max_connections = max_connections;
        self.idle_timeout = Duration::from_secs(idle_timeout_secs);
        self
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(&self.addr).await?;
        println!("MagnumDB Server listening on {}", self.addr);

        let semaphore = Arc::new(Semaphore::new(self.max_connections));
        let idle_timeout = self.idle_timeout;

        loop {
            let (mut socket, peer) = listener.accept().await?;
            let db_ref = Arc::clone(&self.db);
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    log::warn!("Connection limit reached, rejecting {}", peer);
                    let msg = b"Error: Too many connections\n";
                    let len = (msg.len() as u32).to_le_bytes();
                    let _ = socket.write_all(&len).await;
                    let _ = socket.write_all(msg).await;
                    continue;
                }
            };

            tokio::spawn(async move {
                let _permit = permit; // held until task ends
                log::info!("Client connected from {}", peer);
                let mut len_buf = [0u8; 4];

                // Persistent executor per connection — transaction state survives across queries
                loop {
                    let read_result = if idle_timeout.as_secs() > 0 {
                        tokio::time::timeout(idle_timeout, socket.read_exact(&mut len_buf)).await
                    } else {
                        Ok(socket.read_exact(&mut len_buf).await)
                    };

                    let read_result = match read_result {
                        Ok(r) => r,
                        Err(_) => {
                            log::info!("Client {} timed out after idle", peer);
                            break;
                        }
                    };

                    if read_result.is_err() {
                        break; // Connection closed
                    }

                    let query_len = u32::from_le_bytes(len_buf) as usize;

                    // Sanity check: reject absurdly large queries
                    if query_len > 16 * 1024 * 1024 {
                        log::warn!("Client {} sent oversized query ({}B), dropping", peer, query_len);
                        break;
                    }

                    let mut query_buf = vec![0u8; query_len];

                    if socket.read_exact(&mut query_buf).await.is_err() {
                        break;
                    }

                    let query = String::from_utf8_lossy(&query_buf);

                    let response = {
                        // Acquire write lock for mutations, allowing future read-lock optimization
                        let mut guard = db_ref.write().await;
                        let mut executor = Executor::new(&mut guard);
                        match Parser::parse(&query) {
                            Ok(stmt) => match executor.execute(stmt) {
                                Ok(res) => res,
                                Err(e) => format!("Error: {}", e),
                            },
                            Err(e) => format!("Error: {}", e),
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
