use magnumdb::config::Config;
use magnumdb::network::Server;
use magnumdb::storage::Database;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn test_tcp_server_query_roundtrip() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let mut config = Config::default();
    config.storage.path = dir.path().to_path_buf();
    config.wal.enabled = false;

    let db = Database::open(config)?;
    let addr = "127.0.0.1:17432".to_string();

    let server = Server::new(db, addr.clone());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });

    // Give server time to bind
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect client
    let mut client = TcpStream::connect(&addr).await?;

    // Send CREATE TABLE
    let query = "CREATE TABLE users(id INT, name TEXT)";
    let len_bytes = (query.len() as u32).to_le_bytes();
    client.write_all(&len_bytes).await?;
    client.write_all(query.as_bytes()).await?;

    // Read response
    let mut resp_len_buf = [0u8; 4];
    client.read_exact(&mut resp_len_buf).await?;
    let resp_len = u32::from_le_bytes(resp_len_buf) as usize;

    let mut resp_buf = vec![0u8; resp_len];
    client.read_exact(&mut resp_buf).await?;

    let response = String::from_utf8(resp_buf)?;
    assert!(response.contains("Query OK, table 'users' created."));

    Ok(())
}
