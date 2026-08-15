use magnumdb::config::Config;
use magnumdb::sql::{Executor, Parser};
use magnumdb::storage::Database;
use tempfile::tempdir;

fn setup_executor(dir: &tempfile::TempDir) -> (Database, Config) {
    let mut config = Config::default();
    config.storage.path = dir.path().to_path_buf();
    config.wal.enabled = false;
    config.storage.sync_interval = 0;
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
    assert!(res.contains("id | name"));
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

#[test]
fn test_insert_with_commas_in_values() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE notes(id INT, content TEXT)").unwrap())
        .unwrap();
    exec.execute(Parser::parse("INSERT INTO notes VALUES(1, 'hello, world')").unwrap())
        .unwrap();

    let res = exec
        .execute(Parser::parse("SELECT * FROM notes").unwrap())
        .unwrap();
    assert!(res.contains("hello, world"));
}

#[test]
fn test_schemas_and_namespaces() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    // 1. Create Schema
    let res = exec.execute(Parser::parse("CREATE SCHEMA analytics").unwrap()).unwrap();
    assert!(res.contains("schema 'analytics' created"));

    // 2. Show Schemas
    let schemas_res = exec.execute(Parser::parse("SHOW SCHEMAS").unwrap()).unwrap();
    assert!(schemas_res.contains("analytics"));
    assert!(schemas_res.contains("public"));

    // 3. Create Table under qualified schema name
    exec.execute(Parser::parse("CREATE TABLE analytics.events(id INT, event_name TEXT)").unwrap())
        .unwrap();

    // 4. Insert into qualified table
    exec.execute(Parser::parse("INSERT INTO analytics.events VALUES(101, 'click')").unwrap())
        .unwrap();

    // 5. Select from qualified table
    let sel_res = exec.execute(Parser::parse("SELECT * FROM analytics.events").unwrap()).unwrap();
    assert!(sel_res.contains("101 | click"));
}

#[test]
fn test_data_type_validation_and_not_null() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    // Create table with INT, TEXT NOT NULL PRIMARY KEY
    exec.execute(Parser::parse("CREATE TABLE products(id INT NOT NULL PRIMARY KEY, name TEXT NOT NULL, price FLOAT)").unwrap())
        .unwrap();

    // Valid insert
    let res = exec.execute(Parser::parse("INSERT INTO products VALUES(1, 'Laptop', 999.99)").unwrap()).unwrap();
    assert!(res.contains("1 row inserted"));

    // Invalid INT type
    let err_type = exec.execute(Parser::parse("INSERT INTO products VALUES('invalid_num', 'Mouse', 19.99)").unwrap());
    assert!(err_type.is_err() || err_type.unwrap_err().to_string().contains("Cannot parse"));

    // NOT NULL violation
    let err_null = exec.execute(Parser::parse("INSERT INTO products VALUES(2, NULL, 49.99)").unwrap());
    assert!(err_null.is_err() || err_null.unwrap_err().to_string().contains("Constraint Violation"));
}

#[test]
fn test_information_schema() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE users(id INT NOT NULL PRIMARY KEY, email TEXT)").unwrap())
        .unwrap();

    let tables_res = exec.execute(Parser::parse("SELECT * FROM information_schema.tables").unwrap()).unwrap();
    assert!(tables_res.contains("public"));
    assert!(tables_res.contains("users"));

    let cols_res = exec.execute(Parser::parse("SELECT * FROM information_schema.columns").unwrap()).unwrap();
    assert!(cols_res.contains("id"));
    assert!(cols_res.contains("email"));
    assert!(cols_res.contains("INTEGER"));
    assert!(cols_res.contains("TEXT"));
}
