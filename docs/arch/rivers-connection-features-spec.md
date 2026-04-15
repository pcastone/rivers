# Rivers Connection Features Specification

**Document Type:** Implementation Specification
**Scope:** Request-scoped transactions, config-driven prepared statements, bulk batch operations
**Status:** Approved
**Version:** 1.0

---

## Table of Contents

1. [Overview](#1-overview)
2. [Connection Trait Changes](#2-connection-trait-changes)
3. [Transactions — Request-Scoped, Handler-Driven](#3-transactions--request-scoped-handler-driven)
4. [Prepared Statements — Config-Driven](#4-prepared-statements--config-driven)
5. [Batch Operations](#5-batch-operations)
6. [ProcessPool Host Callbacks](#6-processpool-host-callbacks)
7. [Testing Strategy](#7-testing-strategy)

---

## 1. Overview

This specification adds three connection-level features to the Rivers runtime:

- **Transactions** — request-scoped, handler-driven BEGIN/COMMIT/ROLLBACK with auto-rollback on timeout or panic.
- **Prepared Statements** — config-driven (`prepared = true` on DataView), transparent to handlers, cached per-connection.
- **Batch Operations** — single-DataView bulk execution with multiple parameter sets in one round-trip.

### Design Decisions

- **Implicit per-datasource transactions** — `Rivers.db.begin("datasource-name")` starts a transaction. All subsequent queries on that datasource use the active transaction automatically. No tokens or handles.
- **Connection acquired on begin** — calling `begin()` immediately acquires a connection from the pool. Predictable, no lazy surprises.
- **Queries go through DataView engine** — transactional queries still resolve named DataViews with parameter mapping and schema validation. No raw SQL from handlers.
- **Prepared statements are config, not API** — `prepared = true` in `app.toml`. Handlers don't manage preparation.
- **Batch inherits transaction state** — if a transaction is active, batch uses it. If not, batch executes without one. Handler controls atomicity.
- **Pool returns connections on drop** — to preserve prepared statement caches, `PoolGuard::drop()` returns connections to the idle queue instead of dropping them.

### Shared Dependency

All three features require changes to the PoolGuard connection lifecycle. They must be implemented as one coordinated effort:
1. Pool behavior change (return on drop) — prerequisite for all three
2. Transactions — builds on pool change
3. Prepared statements — builds on pool change
4. Batch operations — builds on transactions (optional integration)

---

## 2. Connection Trait Changes

### Transactions

Three new methods on the `Connection` trait in `rivers-driver-sdk`:

```rust
async fn begin_transaction(&mut self) -> Result<(), DriverError>;
async fn commit_transaction(&mut self) -> Result<(), DriverError>;
async fn rollback_transaction(&mut self) -> Result<(), DriverError>;
```

Default implementations return `DriverError::Unsupported`. Non-transactional drivers (faker, memcached, http, exec, cassandra, kafka, rabbitmq, nats, couchdb, elasticsearch, influxdb, ldap, redis) inherit the defaults and need no changes.

Postgres, MySQL, SQLite, MongoDB, and Neo4j implement these. SQL drivers execute `BEGIN`, `COMMIT`, `ROLLBACK`. MongoDB uses its session transaction API. Neo4j uses Cypher transaction API.

### Prepared Statements

Three new methods on the `Connection` trait:

```rust
async fn prepare(&mut self, query: &str) -> Result<(), DriverError>;
async fn execute_prepared(&mut self, query: &str, params: &[QueryValue]) -> Result<QueryResult, DriverError>;
fn has_prepared(&self, query: &str) -> bool;
```

Default implementations: `prepare()` is a no-op, `execute_prepared()` falls through to regular `execute()`, `has_prepared()` returns false. Drivers that support preparation (postgres, mysql, sqlite) override these with real implementations. MongoDB and Neo4j do not support prepared statements in the traditional sense — they use the defaults.

The prepared statement cache is per-connection, stored as a `HashMap<String, PreparedStatement>` (or driver-specific handle type) keyed by query string.

### Batch Operations

No new `Connection` trait methods. Batch is implemented at the runtime level by calling `execute()` in a loop on one connection. The optimization is connection reuse, not a database-level batch API.

### Constraints

| ID | Rule |
|---|---|
| CONN-1 | Default trait implementations MUST allow non-transactional drivers to compile without changes. |
| CONN-2 | `begin_transaction()` MUST execute the database's BEGIN statement (or equivalent). |
| CONN-3 | `prepare()` MUST be idempotent — preparing the same query twice is a no-op. |
| CONN-4 | `has_prepared()` MUST be synchronous (no async, no I/O). |

---

## 3. Transactions — Request-Scoped, Handler-Driven

### Connection Lifecycle

1. Handler calls `Rivers.db.begin("postgres-main")` → host callback fires.
2. Host callback acquires a connection from the pool for that datasource.
3. Calls `connection.begin_transaction()` — executes `BEGIN`.
4. Stores the connection in a per-request `TransactionMap`: `HashMap<String, Box<dyn Connection>>` keyed by datasource name.
5. All subsequent `Rivers.db.query("my-dataview", params)` calls check the `TransactionMap` — if the DataView's datasource has an active transaction, use that connection instead of acquiring a new one from the pool.
6. On `Rivers.db.commit("postgres-main")` → calls `commit_transaction()`, returns connection to pool, removes from map.
7. On `Rivers.db.rollback("postgres-main")` → calls `rollback_transaction()`, drops connection (don't reuse after rollback), removes from map.

### Auto-Rollback

When the request completes (handler returns, timeout fires, or handler panics), any remaining entries in the `TransactionMap` are auto-rolled back and dropped. A warning is logged:

```
WARN: auto-rollback on datasource 'postgres-main' — handler did not commit or rollback
```

### TransactionMap Location

The `TransactionMap` lives in the per-handler execution context — accessible from both the DataView executor (for query routing) and the host callbacks (for begin/commit/rollback). This is the `TaskContext` or equivalent structure passed through the ProcessPool dispatch.

### Constraints

| ID | Rule |
|---|---|
| TXN-1 | `begin()` MUST immediately acquire a connection from the pool. |
| TXN-2 | `begin()` on a datasource with an active transaction MUST return an error. Only one transaction per datasource per request. |
| TXN-3 | `commit()` MUST return the connection to the pool. |
| TXN-4 | `rollback()` MUST drop the connection (do not return to pool after rollback). |
| TXN-5 | Auto-rollback MUST fire on request completion for any uncommitted transactions. |
| TXN-6 | Auto-rollback MUST log a warning identifying the datasource. |
| TXN-7 | Transactional queries MUST go through the DataView engine (parameter mapping, schema validation). Only the connection source changes. |
| TXN-8 | Each datasource has its own independent transaction. Multi-datasource atomicity is the handler's responsibility. |

---

## 4. Prepared Statements — Config-Driven

### DataView Config

A new optional attribute on DataView config in `app.toml`:

```toml
[data.dataviews.search_orders]
name       = "search_orders"
datasource = "postgres-main"
query      = "SELECT * FROM orders WHERE status = $1"
prepared   = true
```

### Runtime Behavior

- When `prepared = true`, the first time this DataView executes on a given connection, the runtime calls `connection.prepare(query)` and marks it as prepared.
- Subsequent executions on the same connection call `connection.execute_prepared(query, params)` — skipping parse/plan overhead.
- When `prepared = false` (default), queries execute normally via `connection.execute()`.
- The runtime checks `connection.has_prepared(query)` before each execution to decide which path to take. This handles the case where a fresh connection from the pool doesn't have the statement cached yet.

### Pool Behavior Change

To preserve prepared statement caches, connections must be reused rather than dropped:

- `PoolGuard::drop()` calls `pool.release(connection)` — returning the connection to the idle queue instead of discarding it.
- Connections are still subject to `max_lifetime_ms` and `idle_timeout_ms` — stale connections (and their prepared caches) are evicted normally.
- The existing `active_count` tracking continues to work — `release()` decrements active count and adds to idle queue.

### No Handler API

Prepared statements are transparent to handlers. No `Rivers.db.prepare()` callback exists. The DataView config controls everything.

### Constraints

| ID | Rule |
|---|---|
| PREP-1 | `prepared = true` MUST cause the runtime to call `prepare()` on first execution per connection. |
| PREP-2 | Subsequent executions on the same connection MUST use `execute_prepared()`. |
| PREP-3 | `prepared` defaults to `false`. Existing DataViews are unaffected. |
| PREP-4 | `PoolGuard::drop()` MUST return the connection to the idle queue, not discard it. |
| PREP-5 | Connection eviction via `max_lifetime_ms` and `idle_timeout_ms` MUST still function normally. |
| PREP-6 | The `prepared` field MUST be added to `DATAVIEW_FIELDS` in structural validation. |

---

## 5. Batch Operations

### Handler API

```javascript
let results = Rivers.db.batch("insert-order", [
    { customerId: "C001", amount: 100 },
    { customerId: "C002", amount: 250 },
    { customerId: "C003", amount: 75 }
]);
// results: array of QueryResult, one per parameter set
```

### Runtime Behavior

1. Host callback receives the DataView name and array of parameter sets.
2. Acquires a single connection — or uses the transaction connection if one is active for that datasource.
3. Resolves the DataView — gets query template, datasource, schema.
4. For each parameter set: apply parameter mapping, validate against schema, execute on the same connection.
5. Collects results into `Vec<QueryResult>`, returns to handler.
6. If no transaction is active, connection is returned to pool after all executions.
7. If a transaction is active, connection stays in the `TransactionMap`.

### Error Behavior

- **No transaction active:** If one row fails, return results so far plus the error. Already-executed rows are committed individually (no implicit rollback). The handler gets partial results.
- **Transaction active:** If one row fails, the error is returned but the transaction stays open. The handler decides whether to rollback the whole batch or continue. This matches the "batch inherits transaction state" design.

### Constraints

| ID | Rule |
|---|---|
| BATCH-1 | Batch MUST execute all parameter sets on a single connection. |
| BATCH-2 | Batch MUST inherit active transaction state. If a transaction is active on the datasource, batch uses that connection. |
| BATCH-3 | Batch without a transaction MUST return the connection to pool after completion. |
| BATCH-4 | Batch MUST require at least one parameter set. Empty array returns an error. |
| BATCH-5 | Each parameter set MUST be validated against the DataView schema before execution. |
| BATCH-6 | Partial results MUST be returned on error when no transaction is active. |

---

## 6. ProcessPool Host Callbacks

### New Callbacks

```javascript
// Transactions
Rivers.db.begin("datasource-name")      // start transaction, acquire connection
Rivers.db.commit("datasource-name")     // commit and release connection to pool
Rivers.db.rollback("datasource-name")   // rollback and drop connection

// Batch
Rivers.db.batch("dataview-name", [      // bulk execute with multiple parameter sets
    { param1: value1 },
    { param2: value2 }
])
```

No `Rivers.db.prepare()` — prepared statements are config-driven.

### Error Cases

| Call | Condition | Error Message |
|------|-----------|---------------|
| `begin()` | Unknown datasource | `"datasource 'xxx' not found"` |
| `begin()` | Non-transactional driver | `"datasource 'xxx' (faker) does not support transactions"` |
| `begin()` | Transaction already active | `"transaction already active on datasource 'xxx'"` |
| `commit()` | No active transaction | `"no active transaction on datasource 'xxx'"` |
| `rollback()` | No active transaction | `"no active transaction on datasource 'xxx'"` |
| `batch()` | Unknown DataView | `"DataView 'xxx' not found"` |
| `batch()` | Empty parameter array | `"batch requires at least one parameter set"` |

### Implementation Location

Host callbacks are registered in `crates/riversd/src/engine_loader/host_callbacks.rs`, following the existing pattern for `Rivers.log()`, `Rivers.keystore.*`, `Rivers.crypto.*`.

### Constraints

| ID | Rule |
|---|---|
| HOST-1 | All callbacks MUST throw on error (not return null/undefined). |
| HOST-2 | `begin()` MUST validate that the datasource exists and supports transactions before acquiring a connection. |
| HOST-3 | `batch()` MUST resolve the DataView name using the same namespace rules as regular `Rivers.db.query()`. |

---

## 7. Testing Strategy

### Unit Tests — Connection Trait

- `begin_transaction()` / `commit_transaction()` / `rollback_transaction()` on postgres, mysql, sqlite, mongodb, neo4j drivers.
- Non-transactional drivers (faker, memcached, http, exec, cassandra, kafka, etc.) return `DriverError::Unsupported` from default implementations.
- `prepare()` / `execute_prepared()` on postgres, mysql, sqlite drivers.
- Default trait implementations pass through correctly (prepare is no-op, execute_prepared falls through to execute).

### Unit Tests — Runtime

- `TransactionMap` lifecycle: begin stores connection, commit removes it, rollback removes it.
- Auto-rollback: request ends with active transaction → rollback + warning logged.
- Double `begin()` on same datasource → error.
- `commit()` / `rollback()` without `begin()` → error.
- Query routing: with active transaction, DataView query uses transaction connection; without, acquires from pool.
- Batch: executes N parameter sets on one connection, returns N results.
- Batch error without transaction: partial results returned.
- Batch error with transaction: error returned, transaction stays open.
- Prepared statements: `prepared = true` causes `prepare()` call on first execution, `execute_prepared()` on subsequent.

### Unit Tests — Pool

- `PoolGuard::drop()` returns connection to idle queue instead of dropping.
- Returned connection is reusable (prepared cache preserved).
- `max_lifetime_ms` still evicts old connections with their prepared caches.
- Pool `min_idle` / `max_size` limits still enforced after behavior change.

### Integration Tests (Against Test Infrastructure)

- Postgres: begin → insert → select (sees row) → rollback → select (row gone).
- MySQL: same pattern.
- SQLite: same pattern.
- MongoDB: begin → insert document → find (sees document) → rollback → find (document gone).
- Neo4j: begin → create node → match (sees node) → rollback → match (node gone).
- Batch insert 100 rows in one call, verify all present.
- Transaction + batch: begin → batch insert → rollback → verify none present.
- Prepared statement: execute same DataView 100 times, verify no performance regression vs unprepared.

### Canary Tests

- Handler that exercises `Rivers.db.begin()` / `Rivers.db.commit()` / `Rivers.db.rollback()`.
- Handler that exercises `Rivers.db.batch()`.
- Verify auto-rollback by intentionally not committing in a handler.
