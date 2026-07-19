use magnumdb::config::Config;
use magnumdb::sql::{Executor, Parser};
use magnumdb::storage::Database;
use tempfile::tempdir;

fn setup_executor(dir: &tempfile::TempDir) -> (Database, Config) {
    let mut config = Config::default();
    config.storage.path = dir.path().to_path_buf();
    config.wal.enabled = false;
    let db = Database::open(config.clone()).unwrap();
    (db, config)
}

#[test]
fn test_create_table_and_insert() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    // 1. Create table
    let stmt = Parser::parse("CREATE TABLE users(id INT, name TEXT)").unwrap();
    assert!(exec.execute(stmt).is_ok());

    // 2. Insert into non-existent table fails
    let stmt = Parser::parse("INSERT INTO fake VALUES(1, 'John')").unwrap();
    assert!(exec.execute(stmt).is_err());

    // 3. Insert valid row
    let stmt = Parser::parse("INSERT INTO users VALUES(1, 'John')").unwrap();
    assert!(exec.execute(stmt).is_ok());

    // 4. Insert wrong column count fails
    let stmt = Parser::parse("INSERT INTO users VALUES(2)").unwrap();
    assert!(exec.execute(stmt).is_err());

    // 5. Select round-trips correctly
    let stmt = Parser::parse("SELECT * FROM users").unwrap();
    let res = exec.execute(stmt).unwrap();
    assert!(res.contains("id INT | name TEXT"));
    assert!(res.contains("1 | John"));
}

#[test]
fn test_transactions() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    let stmt = Parser::parse("CREATE TABLE users(id INT, name TEXT)").unwrap();
    exec.execute(stmt).unwrap();

    // 1. Rollback test
    exec.execute(Parser::parse("BEGIN").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO users VALUES(1, 'Alice')").unwrap())
        .unwrap();

    // Verify it exists in current transaction context
    let res = exec
        .execute(Parser::parse("SELECT * FROM users").unwrap())
        .unwrap();
    assert!(res.contains("Alice"));

    // Rollback
    exec.execute(Parser::parse("ROLLBACK").unwrap()).unwrap();

    // Verify it's gone
    let res = exec
        .execute(Parser::parse("SELECT * FROM users").unwrap())
        .unwrap();
    assert!(!res.contains("Alice"));

    // 2. Commit test
    exec.execute(Parser::parse("BEGIN").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO users VALUES(2, 'Bob')").unwrap())
        .unwrap();
    exec.execute(Parser::parse("COMMIT").unwrap()).unwrap();

    let res = exec
        .execute(Parser::parse("SELECT * FROM users").unwrap())
        .unwrap();
    assert!(res.contains("Bob"));
}
