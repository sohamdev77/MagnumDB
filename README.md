# MagnumDB

> Modern Open Source Embedded Database Engine

[![Build Status](https://img.shields.io/github/actions/workflow/status/example/magnumdb/ci.yml?branch=main)](https://github.com/example/magnumdb/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust Version](https://img.shields.io/badge/rust-1.75%2B-blue.svg)](https://www.rust-lang.org/)
[![Crates.io Version](https://img.shields.io/crates/v/magnumdb.svg)](https://crates.io/crates/magnumdb)
[![GitHub stars](https://img.shields.io/github/stars/example/magnumdb.svg)](https://github.com/example/magnumdb/stargazers)
[![GitHub issues](https://img.shields.io/github/issues/example/magnumdb.svg)](https://github.com/example/magnumdb/issues)

MagnumDB is a production-quality, open-source embedded database engine written in Rust. Designed to be both a high-performance database for real-world applications and a world-class educational resource for teaching database internals.

## Features

- **Embedded & Fast**: Runs directly within your application with zero network overhead.
- **ACID Transactions**: Full compliance with multi-version concurrency control (MVCC).
- **Crash Recovery**: Write-Ahead Log (WAL) ensures durability and consistency.
- **B+ Tree Indexing**: Efficient storage and retrieval mechanism.
- **SQL Support**: Built-in SQL parser (Phase 3).
- **Extensible**: Highly modular architecture.

## Architecture

![Architecture Diagram](https://via.placeholder.com/800x400?text=MagnumDB+Architecture+Diagram)
*(Placeholder for Architecture Diagram)*

MagnumDB is built with a clean, layered architecture separating the storage engine, transaction manager, and query execution. See [ARCHITECTURE.md](ARCHITECTURE.md) for a deep dive.

## Project Goals

- **High Performance**: Target 100K+ reads/sec and 50K+ writes/sec.
- **Educational Excellence**: Exceptionally well-documented codebase with beginner-friendly issues.
- **Memory Efficient**: Predictable resource usage with zero unnecessary allocations.
- **Safe**: Zero `unsafe` Rust code unless absolutely necessary.

## Installation

Add MagnumDB to your `Cargo.toml`:

```toml
[dependencies]
magnumdb = "0.1.0"
```

## Quick Start

```rust
use magnumdb::{Database, Config};

fn main() -> anyhow::Result<()> {
    let config = Config::default().with_path("./my_database");
    let mut db = Database::open(config)?;

    db.put(b"hello", b"world")?;
    let val = db.get(b"hello")?;
    
    assert_eq!(val.unwrap(), b"world");
    Ok(())
}
```

## Usage Examples

Check out the [examples/](examples/) directory for more comprehensive usage scenarios.

## Configuration

MagnumDB can be configured programmatically or via a configuration file (e.g., `magnum.toml`).

```toml
[storage]
path = "./data"
cache_size_mb = 256

[wal]
enabled = true
sync_on_write = false
```

## CLI Commands

MagnumDB comes with a powerful command-line interface:

- `magnum init` - Initialize a new database cluster
- `magnum start` - Start the database server (for network mode)
- `magnum stop` - Stop the server
- `magnum shell` - Open the interactive database shell
- `magnum backup` - Create a consistent backup
- `magnum restore` - Restore from a backup
- `magnum config` - View or validate configuration
- `magnum version` - Display version information
- `magnum benchmark` - Run performance tests

## Database Shell & SQL

Launch the interactive shell using `magnum shell`:

```sql
magnum> CREATE TABLE users(id INT, name TEXT);
Query OK.

magnum> INSERT INTO users VALUES(1, 'Soham');
Query OK, 1 row inserted.

magnum> SELECT * FROM users;
+----+-------+
| id | name  |
+----+-------+
|  1 | Soham |
+----+-------+
```

## Performance Goals

- **Throughput**: > 100K reads/sec, > 50K writes/sec on commodity SSDs.
- **Latency**: Sub-millisecond read/write latency.
- **Concurrency**: Lock-free reads via MVCC.

## Benchmarks

See the [benchmarks/](benchmarks/) directory for reproducible performance test suites.

## Roadmap

See [ROADMAP.md](ROADMAP.md) for our detailed release plan (Phases 1-7).

## Contributing

We welcome contributions of all sizes! MagnumDB is specifically designed to be beginner-friendly. 

- Check out our [CONTRIBUTING.md](CONTRIBUTING.md) guide.
- Look for issues tagged `good first issue` or `beginner`.

## Community

- [Code of Conduct](CODE_OF_CONDUCT.md)
- Join our Discord (Link coming soon)

## FAQ

**Q: Is MagnumDB a drop-in replacement for SQLite?**  
A: MagnumDB aims to provide similar embedded capabilities but focuses heavily on its specific MVCC and networking roadmap. It is not currently a wire-compatible or API-compatible drop-in replacement.

**Q: Why Rust?**  
A: Rust provides the necessary control over memory and hardware to build a high-performance database while eliminating entire classes of memory safety bugs.

## Screenshots

![Shell Screenshot](https://via.placeholder.com/800x400?text=Interactive+Shell+Screenshot)
*(Placeholder for Interactive Shell Screenshot)*

## License

MagnumDB is licensed under the [MIT License](LICENSE).
