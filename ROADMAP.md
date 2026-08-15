# MagnumDB Roadmap

This document outlines the roadmap and current completion state for MagnumDB.

## Phase 1: Core Storage Engine
- [x] Key-Value Store
- [x] Persistent Storage
- [x] Page Free-List Space Recycling
- [x] Write-Ahead Log (WAL with LSN & CRC32 Checksums)
- [x] Crash Recovery (Checksum-Verified Replay)
- [x] CLI (`init`, `start`, `backup`, `restore`, `shell`, `benchmark`)
- [x] Config File (`toml` parsing & environment handling)
- [x] Structured Logging (`env_logger`)

## Phase 2: Indexing & Storage Engine
- [x] B+ Tree Index (Fixed 4KB Page Serialization)
- [x] Prefix Range Scanning (`scan_prefix`)
- [x] Overflow Page Chaining for Large Records (> 4KB)
- [ ] Secondary Indexing (`CREATE INDEX`)
- [x] LRU Buffer Pool Page Manager

## Phase 3: SQL Interface & Query Engine
- [x] SQL Parser (DDL, DML, Range Queries, Aggregates)
- [x] Volcano Streaming Execution Iterator Engine (`SeqScanExec`, `FilterExec`, `AggregateExec`)
- [x] Supported Commands: `CREATE TABLE`, `INSERT`, `SELECT`, `SHOW TABLES`
- [ ] Unsupported Commands (Planned): `UPDATE`, `DELETE`, `DROP TABLE`, `CREATE INDEX`
- [x] Aggregate Functions: `COUNT(*)`, `SUM(col)`, `AVG(col)`
- [ ] Cost-Based Query Optimizer (CBO strategy selection)

## Phase 4: Concurrency & Transactions
- [x] Write-Ahead Log Transaction Framing (`BEGIN`, `COMMIT`, `ROLLBACK`)
- [x] Single-Writer WAL Transaction Durability
- [x] Multi-Version Concurrency Control (MVCC tuple headers)
- [x] Fine-Grained Page Latching (`RwLock<Page>`)

## Phase 5: Client-Server Architecture
- [x] Async Multi-Client TCP Server (Tokio runtime)
- [x] PostgreSQL Wire Protocol (`pgwire` handler for `StartupMessage`, `Query`, `ReadyForQuery`)
- [x] User Authentication & Password Hashing
- [x] Role-Based Access Control (RBAC)

## Phase 6: Distributed Mode
- [ ] Replication Engine
- [ ] Leader Election
- [ ] Raft Consensus Integration
- [ ] Consistent Hash Sharding & Cluster Mode

## Phase 7: Operational Tooling & Observability
- [x] Hot Backup & Physical Snapshot Recovery
- [x] High-Throughput Engine Benchmarking CLI
- [ ] Prometheus Metrics Exporter
- [ ] gRPC API Interface

## Current Architecture Highlights
- **Zero Space Leak Paging**: Deleted pages are returned to Page 0's Free-List chain and reused on subsequent allocations.
- **WAL Data Protection**: Every WAL frame is checksummed with CRC32. Corrupted log bytes or partial writes at crash time are automatically caught and discarded safely.
- **Large Record Overflow**: Records larger than a single page are chained across dedicated overflow pages and freed upon record deletion.
