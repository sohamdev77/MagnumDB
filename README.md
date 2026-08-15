# MagnumDB

> Experimental embedded KV and SQL database engine written in Rust.

[![Build Status](https://img.shields.io/github/actions/workflow/status/sohamdev77/MagnumDB/ci.yml?branch=main)](https://github.com/sohamdev77/MagnumDB/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.75%2B-blue.svg)](https://www.rust-lang.org/)
[![Crates.io Version](https://img.shields.io/crates/v/magnumdb.svg)](https://crates.io/crates/magnumdb)
[![Documentation](https://docs.rs/magnumdb/badge.svg)](https://docs.rs/magnumdb)

**Project Status:** Experimental / Alpha. Not recommended for production use.

## What's New in v0.4.7
MagnumDB is an open-source educational database internals project. It provides:
- An embedded key-value storage engine (B+ tree, WAL).
- A basic relational SQL query execution engine (Volcano-style).
- A PostgreSQL wire-protocol (pgwire) TCP server.
- Multi-Version Concurrency Control (MVCC) with Read Committed isolation.

The database engine and core storage components are implemented in Rust, utilizing crates like `tokio` for async networking and `parking_lot` for concurrency locks.

---

## Two Modes of Operation

MagnumDB can be used in two distinct ways:

1. **Embedded Mode**: Compile MagnumDB directly into your Rust application. You get direct access to the `Database` struct for Key-Value access, or the `Executor` for in-memory SQL execution without network overhead.
2. **Server Mode**: Run `magnumdb` as a standalone TCP process. It listens for PostgreSQL Wire Protocol connections, allowing you to connect using standard `psql` clients.

---

## Supported SQL & Protocol Matrix

MagnumDB exposes a basic heuristic, rule-based SQL planner and Volcano-style executor (iterators passing rows up a tree of operators). It is **not** fully PostgreSQL compatible.

| Feature | Supported | Notes |
| :--- | :---: | :--- |
| `CREATE TABLE` | ✅ | Basic types: `INT` (i64), `TEXT` (UTF-8 String), `BOOLEAN` (bool), `FLOAT` (f64). |
| `INSERT` | ✅ | Single and multi-row inserts supported. |
| `SELECT` | ✅ | Column projections and `*` supported. |
| `JOIN` | ✅ | `INNER JOIN` and `LEFT JOIN` using Grace Hash Join. |
| `GROUP BY` | ✅ | Basic aggregations (`COUNT`, `SUM`). |
| `HAVING` | ✅ | Supported on aggregates. |
| `ORDER BY` | ✅ | `ASC` and `DESC` sorting. |
| `LIMIT` / `OFFSET`| ✅ | Supported. |
| Transactions | ✅ | `BEGIN`, `COMMIT`, `ROLLBACK`. |
| Prepared Statements| ✅ | Supported via PostgreSQL Wire Protocol (`Parse`, `Bind`, `Execute`). |
| Authentication | ✅ | MD5 Authentication (implemented for Postgres wire compatibility). |
| `CREATE USER` | ✅ | Provisions users. (Output: `CREATE ROLE`). |
| `UPDATE` | ✅ | Supports single-column update with optional `WHERE` filtering. |
| `DELETE` | ✅ | Supports deletion with optional `WHERE` filtering. |
| `DROP TABLE` | ✅ | Drops table and its associated indexes from storage catalog. |
| `ALTER TABLE` | ✅ | Supports `ADD COLUMN`. |
| **Constraints**| ✅ | `UNIQUE`, `DEFAULT`, and `NOT NULL` are now strictly enforced. (Note: `PRIMARY KEY` and `FOREIGN KEY` are parsed but defer to `UNIQUE` indexing logic). |
| **Advanced SQL** | ✅ | Subqueries (`IN (SELECT...)`), CTEs (`WITH`), Window Functions (`ROW_NUMBER() OVER(...)`), and `UNION` / `UNION ALL`. |

---

## Architecture & Internals

MagnumDB implements a monolithic database architecture:

- **Storage Engine**: Data is stored on disk in 4KB pages. Pages are managed by a custom Disk Pager.
- **B+ Tree**: Table records and Key-Value pairs are stored in a B+ Tree. The tree supports overflow pages for records exceeding the 4KB page size, and recycles freed leaf pages using a free-list.
- **Buffer Pool**: An LRU (Least Recently Used) Buffer Pool manages in-memory pages, pinning them during transactions and evicting when memory limits are reached.
- **Transactions & MVCC**: Transactions are assigned monotonic TxIDs. Row headers contain `xmin` and `xmax` fields for Multi-Version Concurrency Control (MVCC).
- **Concurrency**: `RwLock` is used on the core `Database` struct. While MVCC exists, write operations currently acquire an exclusive lock on the database during the commit phase, meaning writes are serialized. Uncommitted changes are not visible to other transactions.
- **Query Optimizer**: MagnumDB currently uses a heuristic, rule-based planner. (A Cost-Based Optimizer using statistics is planned for the future).

### Durability & Crash Recovery
MagnumDB uses an append-only Write-Ahead Log (WAL) for durability. Writes are buffered and appended to the WAL. A transaction is only considered durable once its `COMMIT` record is appended and the WAL frame (with a CRC32 checksum) is `fsync`'d to disk.
- **Process Crash**: On restart, the engine sequentially replays the WAL from the beginning (No checkpointing mechanism exists yet).
- **Corrupted/Partial WAL Frame**: Checksums detect partial writes; the replay safely stops at the first corrupted frame (assuming it's a truncated tail from a crash). Mid-file corruption will also halt recovery.
- **Uncommitted Transactions**: Transactions without a `COMMIT` record in the WAL are discarded during recovery.

---

## Known Limitations & Security
- **No TLS Support**: The server does not support SSL/TLS encryption. **Do not expose MagnumDB to the public internet.**
- **Authentication**: Uses MD5 for Postgres protocol compatibility. MD5 is cryptographically obsolete and is not a modern security mechanism.
- **Default Credentials**: The server automatically provisions a `postgres` superuser with no password upon database initialization. You must connect as `postgres` and explicitly `CREATE USER ... WITH PASSWORD` to secure the instance.
- **Isolation Level**: Currently only guarantees Read Committed isolation. Each statement receives a snapshot of committed data. Non-repeatable reads and phantom reads are possible.
- **RBAC**: We support user creation, but granular `GRANT`/`REVOKE` table-level permissions do not exist.
- **SQL Error Behavior**: Duplicate rows (if unconstrained) are inserted. Invalid types may cause query panics or execution errors rather than graceful semantic errors.

---

## Benchmarks & Performance
No comparative benchmark results are currently published. Performance metrics (inserts/sec, point reads/sec, concurrent client throughput) will be added in a future release.

---

## Installation & Usage

### 1. Embedded Mode (Rust)

Add to `Cargo.toml`:
```toml
[dependencies]
magnumdb = "0.4.5"
```

```rust
use magnumdb::{Database, Config};
use magnumdb::sql::{Executor, Parser};

fn main() -> anyhow::Result<()> {
    let config = Config::default().with_path("./sql_data");
    let mut db = Database::open(config)?;
    let mut exec = Executor::new(&mut db);

    exec.execute(Parser::parse("CREATE TABLE users(id INT, name TEXT)")?)?;
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

In a separate terminal, connect using `psql` or PostgreSQL compatible clients that use the supported wire-protocol features:
```bash
psql -h 127.0.0.1 -p 5432 -U postgres -d postgres
```

#### Authentication & User Management Flow
```sql
postgres=> CREATE USER admin WITH PASSWORD 'secure123';
CREATE ROLE

-- Reconnect using the new credentials:
-- psql -h 127.0.0.1 -p 5432 -U admin -W
```

#### Transactions
```sql
admin=> BEGIN;
admin=> CREATE TABLE test (id INT);
admin=> INSERT INTO test VALUES (1);
admin=> COMMIT;
```

#### Concurrent Clients Example
Because of the Tokio async server and MVCC locks, multiple clients can connect simultaneously. Uncommitted changes are not visible to other transactions:
```bash
# Terminal 1
psql -h 127.0.0.1 -U admin
admin=> BEGIN;
admin=> INSERT INTO users VALUES (10);
# (Transaction uncommitted)

# Terminal 2
psql -h 127.0.0.1 -U admin
admin=> SELECT * FROM users;
# (Will not see '10' until Terminal 1 commits)
```

---

## Testing & CI

MagnumDB contains over 20 passing unit and integration tests covering storage components, SQL parsing, B+ tree split behavior, and network query roundtripping. 

To run the test suite locally:
```bash
cargo test
cargo test --release
```

*Note: We do not yet employ fuzz testing, which is a planned addition for parser and storage robustness.*

The GitHub CI pipeline currently validates basic `cargo build` and `cargo test` execution on the `main` branch. Rust version `1.75+` is the version used during development, though it may compile on slightly older editions.
