# MagnumDB Roadmap

This document outlines the planned feature phases for MagnumDB. We aim to build incrementally, ensuring each phase is robust and heavily tested before proceeding.

## Phase 1: Core Storage Engine
- [x] Key-Value Store
- [x] Persistent Storage
- [x] WAL (Write Ahead Log)
- [x] Crash Recovery
- [x] CLI
- [x] Config File
- [x] Logging

## Phase 2: Indexing & Caching
- [ ] B+ Tree Index
- [ ] Storage Engine enhancements
- [ ] Cache Manager
- [ ] Compression

## Phase 3: SQL Interface
- [ ] SQL Parser
- [ ] Query Execution Engine
- [ ] Supported commands: `CREATE TABLE`, `INSERT`, `SELECT`, `DELETE`, `UPDATE`, `DROP TABLE`

## Phase 4: Concurrency & Transactions
- [ ] ACID Transactions
- [ ] MVCC (Multi-Version Concurrency Control)
- [ ] Locks and isolation levels

## Phase 5: Client-Server Architecture
- [ ] TCP Server
- [ ] Multiple Clients support
- [ ] Authentication
- [ ] User Management

## Phase 6: Distributed Mode
- [ ] Replication
- [ ] Leader Election
- [ ] Raft Consensus integration
- [ ] Cluster Mode

## Phase 7: APIs and Observability
- [ ] REST API
- [ ] gRPC API
- [ ] Metrics Dashboard

## Known Limitations
- B-Tree deletions currently do not rebalance or merge underflowing pages. Pages are left under-filled, which may result in a larger file size than necessary. This is a known limitation that will be addressed in future phases.
