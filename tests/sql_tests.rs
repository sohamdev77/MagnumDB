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
    let stmt = Parser::parse("CREATE TABLE users(id INT, name TEXT NOT NULL)").unwrap();
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

#[test]
fn test_drop_table() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE dt(id INT PRIMARY KEY)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO dt VALUES(1)").unwrap()).unwrap();
    
    let res = exec.execute(Parser::parse("DROP TABLE dt").unwrap()).unwrap();
    assert!(res.contains("Query OK"));
    
    let err = exec.execute(Parser::parse("SELECT * FROM dt").unwrap());
    assert!(err.is_err());
}

#[test]
fn test_update_and_delete() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE ud(id INT PRIMARY KEY, name TEXT)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO ud VALUES(1, 'Alice'), (2, 'Bob'), (3, 'Charlie')").unwrap()).unwrap();

    let res = exec.execute(Parser::parse("UPDATE ud SET name = 'Alicia' WHERE id = 1").unwrap()).unwrap();
    assert!(res.contains("1 row updated"));

    let select_res = exec.execute(Parser::parse("SELECT * FROM ud WHERE id = 1").unwrap()).unwrap();
    assert!(select_res.contains("Alicia"));

    let res_del = exec.execute(Parser::parse("DELETE FROM ud WHERE id = 2").unwrap()).unwrap();
    assert!(res_del.contains("1 row deleted"));

    let select_res_2 = exec.execute(Parser::parse("SELECT * FROM ud").unwrap()).unwrap();
    assert!(!select_res_2.contains("Bob"));
    
    let res_del_all = exec.execute(Parser::parse("DELETE FROM ud").unwrap()).unwrap();
    assert!(res_del_all.contains("2 rows deleted"));
}

#[test]
fn test_unique_and_default() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE ud(id INT PRIMARY KEY, name TEXT UNIQUE, age INT DEFAULT 18)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO ud VALUES(1, 'Alice')").unwrap()).unwrap();

    // Missing column `age` should be padded with 18
    let res = exec.execute(Parser::parse("SELECT * FROM ud WHERE id = 1").unwrap()).unwrap();
    assert!(res.contains("18"));

    // Duplicate name should fail UNIQUE constraint
    let err = exec.execute(Parser::parse("INSERT INTO ud VALUES(2, 'Alice')").unwrap());
    assert!(err.is_err());
    let err_msg = err.unwrap_err().to_string();
    assert!(err_msg.contains("UNIQUE constraint violation"));
    
    // Duplicate ID should fail PRIMARY KEY unique constraint
    let err2 = exec.execute(Parser::parse("INSERT INTO ud VALUES(1, 'Bob')").unwrap());
    assert!(err2.is_err());
    let err_msg2 = err2.unwrap_err().to_string();
    assert!(err_msg2.contains("UNIQUE constraint violation"));
}

#[test]
fn test_alter_table_add_column() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE at(id INT)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO at VALUES(1)").unwrap()).unwrap();

    let res1 = exec.execute(Parser::parse("SELECT * FROM at").unwrap()).unwrap();
    assert!(res1.contains("1"));

    // Alter table
    exec.execute(Parser::parse("ALTER TABLE at ADD COLUMN name TEXT DEFAULT 'Anonymous'").unwrap()).unwrap();

    // Query old row, should have 'Anonymous'
    let res2 = exec.execute(Parser::parse("SELECT * FROM at").unwrap()).unwrap();
    assert!(res2.contains("Anonymous"));

    // Insert new row
    exec.execute(Parser::parse("INSERT INTO at VALUES(2, 'Bob')").unwrap()).unwrap();
    let res3 = exec.execute(Parser::parse("SELECT * FROM at WHERE id = 2").unwrap()).unwrap();
    assert!(res3.contains("Bob"));
}

#[test]
fn test_advanced_sql() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE t1(id INT, name TEXT)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO t1 VALUES(1, 'A')").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO t1 VALUES(2, 'B')").unwrap()).unwrap();
    
    exec.execute(Parser::parse("CREATE TABLE t2(id INT, name TEXT)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO t2 VALUES(2, 'B')").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO t2 VALUES(3, 'C')").unwrap()).unwrap();

    // UNION
    let res_union = exec.execute(Parser::parse("SELECT * FROM t1 UNION SELECT * FROM t2").unwrap()).unwrap();
    assert!(res_union.contains("3 row(s)")); // A, B, C (no duplicates)
    
    // UNION ALL
    let res_union_all = exec.execute(Parser::parse("SELECT * FROM t1 UNION ALL SELECT * FROM t2").unwrap()).unwrap();
    assert!(res_union_all.contains("4 row(s)")); // A, B, B, C
    
    // SUBQUERY
    let res_subq = exec.execute(Parser::parse("SELECT * FROM t1 WHERE id IN (SELECT id FROM t2)").unwrap()).unwrap();
    assert!(res_subq.contains("1 row(s)")); // Only 2, 'B'
    
    // CTE
    let res_cte = exec.execute(Parser::parse("WITH temp AS (SELECT * FROM t1) SELECT * FROM temp").unwrap()).unwrap();
    assert!(res_cte.contains("2 row(s)")); // 1, 'A' and 2, 'B'
    
    // WINDOW FUNCTION
    exec.execute(Parser::parse("INSERT INTO t1 VALUES(3, 'A')").unwrap()).unwrap();
    let res_win = exec.execute(Parser::parse("SELECT ROW_NUMBER() OVER(PARTITION BY name ORDER BY id) FROM t1").unwrap()).unwrap();
    assert!(res_win.contains("3 row(s)"));
    assert!(res_win.contains("row_number"));
}

#[test]
fn test_advanced_sql_more_subqueries() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE dept(id INT, dname TEXT)").unwrap()).unwrap();
    exec.execute(Parser::parse("CREATE TABLE emp(id INT, name TEXT, dept_id INT)").unwrap()).unwrap();

    exec.execute(Parser::parse("INSERT INTO dept VALUES(1, 'Engineering')").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO dept VALUES(2, 'Sales')").unwrap()).unwrap();

    exec.execute(Parser::parse("INSERT INTO emp VALUES(10, 'Alice', 1)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO emp VALUES(20, 'Bob', 2)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO emp VALUES(30, 'Charlie', 3)").unwrap()).unwrap(); // No such dept

    let res = exec.execute(Parser::parse("SELECT * FROM emp WHERE dept_id IN (SELECT id FROM dept)").unwrap()).unwrap();
    assert!(res.contains("Alice"));
    assert!(res.contains("Bob"));
    assert!(!res.contains("Charlie"));
}

#[test]
fn test_advanced_sql_more_ctes() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE nums(val INT)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO nums VALUES(1)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO nums VALUES(2)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO nums VALUES(3)").unwrap()).unwrap();

    let res = exec.execute(Parser::parse("WITH temp1 AS (SELECT * FROM nums WHERE val = 2) SELECT * FROM temp1").unwrap()).unwrap();
    assert!(res.contains("1 row(s)"));
    assert!(res.contains("2"));
    assert!(!res.contains("1"));
    assert!(!res.contains("3"));
}

#[test]
fn test_advanced_sql_more_window_functions() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE scores(player TEXT, score INT)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO scores VALUES('P1', 100)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO scores VALUES('P1', 200)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO scores VALUES('P2', 150)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO scores VALUES('P2', 50)").unwrap()).unwrap();

    // Partition by player, order by score
    let res = exec.execute(Parser::parse("SELECT ROW_NUMBER() OVER(PARTITION BY player ORDER BY score) FROM scores").unwrap()).unwrap();
    assert!(res.contains("4 row(s)"));
    
    let res_str = res.to_string();
    assert!(res_str.contains("P1 | 100 | 1"));
    assert!(res_str.contains("P1 | 200 | 2"));
    assert!(res_str.contains("P2 | 50 | 1"));
    assert!(res_str.contains("P2 | 150 | 2"));
}

#[test]
fn test_advanced_sql_more_unions() {
    let dir = tempdir().unwrap();
    let (mut db, _) = setup_executor(&dir);
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE A(val INT)").unwrap()).unwrap();
    exec.execute(Parser::parse("CREATE TABLE B(val INT)").unwrap()).unwrap();

    exec.execute(Parser::parse("INSERT INTO A VALUES(10)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO A VALUES(20)").unwrap()).unwrap();

    exec.execute(Parser::parse("INSERT INTO B VALUES(20)").unwrap()).unwrap();
    exec.execute(Parser::parse("INSERT INTO B VALUES(30)").unwrap()).unwrap();

    let res_union = exec.execute(Parser::parse("SELECT * FROM A UNION SELECT * FROM B").unwrap()).unwrap();
    assert!(res_union.contains("3 row(s)")); // 10, 20, 30

    let res_union_all = exec.execute(Parser::parse("SELECT * FROM A UNION ALL SELECT * FROM B").unwrap()).unwrap();
    assert!(res_union_all.contains("4 row(s)")); // 10, 20, 20, 30
}

