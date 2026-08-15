# MagnumDB

> Open-source embedded SQL and key-value database engine written in Rust.

[![Build Status](https://img.shields.io/github/actions/workflow/status/sohamdev77/MagnumDB/ci.yml?branch=main)](https://github.com/sohamdev77/MagnumDB/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.75%2B-blue.svg)](https://www.rust-lang.org/)
[![Crates.io Version](https://img.shields.io/crates/v/magnumdb.svg)](https://crates.io/crates/magnumdb)
[![Documentation](https://docs.rs/magnumdb/badge.svg)](https://docs.rs/magnumdb)
[![GitHub stars](https://img.shields.io/github/stars/sohamdev77/MagnumDB.svg)](https://github.com/sohamdev77/MagnumDB/stargazers)

**Project Status:** Experimental / Alpha (Not recommended for production use yet).

MagnumDB is an open-source database project built to explore database internals. The database engine and core storage components (B+ Tree, Buffer Pool, WAL, SQL Executor) are implemented in Rust.

## Features & Supported SQL Matrix

MagnumDB exposes a Volcano-style query engine. 

| Feature | Supported | Notes |
| :--- | :---: | :--- |
| `CREATE TABLE` | ✅ | Basic types: `INT`, `TEXT`, `BOOLEAN`, `FLOAT`. No constraints yet except `NOT NULL`. |
| `CREATE INDEX` | ❌ | B+Tree indexing exists for PKs, but secondary indexes are not yet supported via SQL. |
| `INSERT` | ✅ | Single and multi-row inserts supported. |
| `SELECT` | ✅ | Column projections and `*` supported. |
| `UPDATE` | ❌ | Planned for Phase 6. |
| `DELETE` | ❌ | Planned for Phase 6. |
| `DROP TABLE` | ❌ | Planned for Phase 6. |
| `JOIN` | ✅ | `INNER JOIN` and `LEFT JOIN` using Grace Hash Join. |
| `GROUP BY` | ✅ | Basic aggregations (`COUNT`, `SUM`). |
| `HAVING` | ✅ | Supported on aggregates. |
| `ORDER BY` | ✅ | `ASC` and `DESC` sorting. |
| `LIMIT` / `OFFSET`| ✅ | Supported. |
| Transactions | ✅ | `BEGIN`, `COMMIT`, `ROLLBACK`. Read-Committed isolation (No Snapshot Isolation yet). |
| Prepared Statements| ✅ | Supported via PostgreSQL Wire Protocol (`Parse`, `Bind`, `Execute`). |
| Authentication | ✅ | MD5 Authentication (implemented for Postgres compatibility, not modern security). |
| `CREATE USER` | ✅ | Provisions users. (Note: Full RBAC `GRANT`/`REVOKE` is not yet implemented). |

## Two Modes of Operation

MagnumDB can be used in two distinct ways:

1. **Embedded Mode**: Compile MagnumDB directly into your Rust application. You get direct access to the `Database` struct for Key-Value access, or the `Executor` for in-memory SQL execution without network overhead.
2. **Server Mode**: Run `magnumdb` as a standalone TCP process. It listens for PostgreSQL Wire Protocol connections, meaning you can connect to it using `psql`, `pg8000`, or standard ORMs.

## What's New in v0.4.4
- 🔒 **Authentication Handshake**: Native PostgreSQL wire-protocol MD5 password authentication. (Note: MD5 is cryptographically obsolete, but required for legacy Postgres client handshakes).
- 👤 **`CREATE USER` DDL**: Provision users directly via SQL (`CREATE USER admin WITH PASSWORD 'pass';`).
- 🧵 **Thread-Safe MVCC**: Thread-safe transaction execution using `parking_lot::RwLock`.

## Architecture & Storage Format

MagnumDB implements a standard monolithic RDBMS architecture:

- **Storage**: Data is stored on disk in 4KB pages. Pages are managed by a custom Disk Pager.
- **B+ Tree**: Table records are stored in a B+ Tree. The tree supports overflow pages for records exceeding page size, and recycles freed leaf pages using a free-list.
- **Buffer Pool**: An LRU (Least Recently Used) Buffer Pool manages in-memory pages, pinning them during transactions and evicting when memory limits are reached.
- **Transactions & MVCC**: Transactions are assigned monotonic TxIDs. Row headers contain `xmin` and `xmax` fields for Multi-Version Concurrency Control (MVCC).
- **Optimizer**: We currently use a heuristic, rule-based planner. (A Cost-Based Optimizer / CBO using statistics is planned for the future).
- **Concurrency**: `RwLock` is used on the core `Database` struct. While reads can be concurrent, writes currently take exclusive locks during commit phases.

### Durability & Crash Recovery
MagnumDB uses a Write-Ahead Log (WAL) for durability. Writes are buffered and appended to the WAL. A committed transaction is only considered durable once its WAL frame (with a CRC32 checksum) is `fsync`'d to disk.
- **Process Crash**: On restart, the engine replays the WAL from the last checkpoint.
- **Partial/Corrupted WAL Frame**: Checksums detect partial writes; the replay stops at the first corrupted frame.
- **Uncommitted Transactions**: Transactions without a `COMMIT` record in the WAL are discarded during recovery.

## Known Limitations & Security
- **No TLS Support**: The server does not support SSL/TLS encryption. **Do not expose MagnumDB to the public internet.**
- **Authentication limits**: Uses MD5 for Postgres compatibility. No strong cryptographic auth exists.
- **Isolation Level**: Currently only guarantees Read Committed. Phantom reads are possible.
- **RBAC**: We support user creation, but granular `GRANT`/`REVOKE` table-level permissions do not exist yet.

## Benchmarks & Comparisons
MagnumDB is an educational / research project. We do not yet claim to outperform production systems like SQLite, RocksDB, or redb. 
- *Benchmark results will be published here in v0.5.0.*

## Installation & Usage

### 1. Embedded Mode (Rust)

Add to `Cargo.toml`:
```toml
[dependencies]
magnumdb = "0.4.4"
```

```rust
use magnumdb::{Database, Config};
use magnumdb::sql::{Executor, Parser};

fn main() -> anyhow::Result<()> {
    let config = Config::default().with_path("./sql_data");
    let mut db = Database::open(config)?;
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE users(id INT, name TEXT)")?)?;
    
    // Transactions
    exec.execute(Parser::parse("BEGIN;")?)?;
    exec.execute(Parser::parse("INSERT INTO users VALUES(1, 'Alice')")?)?;
    exec.execute(Parser::parse("COMMIT;")?)?;

    Ok(())
}
```

### 2. Server Mode (PostgreSQL Protocol)

Clone and build from source:
```bash
git clone https://github.com/sohamdev77/MagnumDB.git
cd MagnumDB
cargo build --release

# Start the TCP server on 127.0.0.1:5432
cargo run --bin magnumdb --release
```

In a separate terminal, connect using `psql` (Authentication requires the default `postgres` user):
```bash
psql -h 127.0.0.1 -p 5432 -U postgres -d postgres
```
```sql
postgres=> CREATE USER admin WITH PASSWORD 'secure123';
Query OK, user 'admin' created.

postgres=> BEGIN;
postgres=> CREATE TABLE test (id INT);
postgres=> INSERT INTO test VALUES (1);
postgres=> COMMIT;
```

### 3. Concurrent Clients Example
Because of the Tokio async server and MVCC locks, multiple clients can connect simultaneously:
```bash
# Terminal 1
psql -h 127.0.0.1 -U postgres
postgres=> BEGIN;
postgres=> INSERT INTO users VALUES (10);
# (Transaction uncommitted)

# Terminal 2
psql -h 127.0.0.1 -U postgres
postgres=> SELECT * FROM users;
# (Will not see '10' until Terminal 1 commits due to Read Committed isolation)
```

## Testing

To run the test suite locally:
```bash
cargo test
cargo test --release
```
We include unit tests for storage components, SQL parsing, and integration tests for crash-recovery and network concurrency.
