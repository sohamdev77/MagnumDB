use magnumdb::{Database, Config};

fn main() -> anyhow::Result<()> {
    let config = Config::default().with_path("./my_database");
    let mut db = Database::open(config)?;

    db.put(b"hello", b"world")?;
    let val = db.get(b"hello")?;

    assert_eq!(val.unwrap(), b"world");
    println!("Basic usage complete! Data persisted to ./my_database");
    
    Ok(())
}
