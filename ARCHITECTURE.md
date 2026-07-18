# MagnumDB Architecture

This document describes the high-level architecture of MagnumDB.

## Overview

MagnumDB is an embedded, high-performance database written in Rust. It is designed to be highly modular. The system is split into several distinct layers:

1. **Client / Network Layer (Phase 5+)**
   - Handles TCP connections, authentication, and wire protocols.
   - For embedded use, this layer is bypassed in favor of direct API calls.

2. **SQL Parser & Query Engine (Phase 3)**
   - Parses SQL into Abstract Syntax Trees (AST).
   - Generates and optimizes execution plans.

3. **Transaction Manager (Phase 4)**
   - Manages ACID properties.
   - Implements Multi-Version Concurrency Control (MVCC).
   - Handles row-level and table-level locks.

4. **Storage Engine (Phase 1-2)**
   - **Cache Manager:** Keeps frequently accessed data in RAM.
   - **B+ Tree Index:** Efficient structures for fast lookups.
   - **Key-Value Store:** The foundational storage API.
   - **Write-Ahead Log (WAL):** Ensures durability by logging all mutations sequentially before applying them to the data files.

## Data Flow (Write Operation)

1. Client sends an `INSERT` statement.
2. The Parser converts it into an AST.
3. The Transaction Manager begins a transaction.
4. The Storage Engine writes the change to the WAL for durability.
5. The Storage Engine updates the in-memory cache and B+ Tree structures.
6. The transaction commits.
7. A background flusher eventually syncs the cache to persistent data files.

## Memory Layout

MagnumDB emphasizes a zero-copy, highly efficient memory layout:

- **Buffer Pool:** Fixed-size memory pages (typically 4KB or 8KB).
- **WAL Buffer:** Small ring buffer for fast sequential writes to disk.
- **Data Serialization:** Custom binary formats to avoid unnecessary allocation.

## Code Organization (`src/`)

- `cli/`: Command-line interface definitions and logic.
- `config/`: Configuration file parsing (`toml`) and environment handling.
- `storage/`: The core key-value and page management logic.
- `wal/`: Write-Ahead Log implementation for crash recovery.
- `network/`, `sql/`, `transaction/`, `replication/`: Additional modules scoped for later phases.
