# Connection Features Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add request-scoped transactions, config-driven prepared statements, and bulk batch operations to the Rivers runtime.

**Architecture:** Three features sharing a common foundation: Connection trait extensions in `rivers-driver-sdk`, PoolGuard behavior change (return-on-drop), driver implementations (postgres, mysql, sqlite, mongodb, neo4j), and ProcessPool host callbacks for JS/WASM handler access. Built in dependency order: pool change → transactions → prepared statements → batch.

**Tech Stack:** Rust, tokio (async), rivers-driver-sdk traits, tokio-postgres/mysql_async/rusqlite/mongodb/neo4rs drivers, V8 host callbacks

**Spec:** `docs/arch/rivers-connection-features-spec.md`

---

## File Map

| Task | File | Action |
|------|------|--------|
| 1 | `crates/rivers-driver-sdk/src/traits.rs` | Modify — add transaction + prepared methods to Connection |
| 2 | `crates/riversd/src/pool.rs` | Modify — PoolGuard returns connections to idle on drop |
| 3 | `crates/rivers-drivers-builtin/src/postgres.rs` | Modify — implement transaction + prepared methods |
| 3 | `crates/rivers-drivers-builtin/src/mysql.rs` | Modify — implement transaction + prepared methods |
| 3 | `crates/rivers-drivers-builtin/src/sqlite.rs` | Modify — implement transaction + prepared methods |
| 4 | `crates/rivers-plugin-mongodb/src/lib.rs` | Modify — implement transaction methods |
| 4 | `crates/rivers-plugin-neo4j/src/lib.rs` | Modify — implement transaction methods |
| 5 | `crates/rivers-runtime/src/dataview.rs` | Modify — add `prepared` field |
| 5 | `crates/rivers-runtime/src/validate_structural.rs` | Modify — add `prepared` to DATAVIEW_FIELDS |
| 6 | `crates/riversd/src/transaction.rs` | Create — TransactionMap for per-request transaction state |
| 7 | `crates/riversd/src/engine_loader/host_callbacks.rs` | Modify — add Rivers.db.begin/commit/rollback/batch callbacks |
| 8 | `crates/rivers-runtime/src/dataview_engine.rs` | Modify — transaction-aware query routing + prepared statement logic |
| 9 | Documentation and task tracking | Modify |

---

### Task 1: Connection Trait — Transaction + Prepared Statement Methods

**Files:**
- Modify: `crates/rivers-driver-sdk/src/traits.rs:476-515`

- [ ] **Step 1: Add transaction methods to Connection trait**

In the `Connection` trait (line 476), add after the existing `ping()` method:

```rust
    /// Begin a transaction on this connection.
    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support transactions",
            self.driver_name()
        )))
    }

    /// Commit the active transaction.
    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support transactions",
            self.driver_name()
        )))
    }

    /// Rollback the active transaction.
    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support transactions",
            self.driver_name()
        )))
    }
```

- [ ] **Step 2: Add prepared statement methods to Connection trait**

```rust
    /// Prepare a query for repeated execution. No-op by default.
    async fn prepare(&mut self, _query: &str) -> Result<(), DriverError> {
        Ok(())
    }

    /// Execute a previously prepared query. Falls through to execute() by default.
    async fn execute_prepared(
        &mut self,
        query: &Query,
    ) -> Result<QueryResult, DriverError> {
        self.execute(query).await
    }

    /// Check if a query has been prepared on this connection.
    fn has_prepared(&self, _query: &str) -> bool {
        false
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rivers-driver-sdk`
Expected: Compiles. Default implementations mean no existing driver code breaks.

- [ ] **Step 4: Verify all drivers still compile**

Run: `cargo check --workspace`
Expected: Compiles. All existing Connection impls inherit the defaults.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/traits.rs
git commit -m "feat(connection): add transaction and prepared statement methods to Connection trait"
```

---

### Task 2: PoolGuard — Return Connections to Idle on Drop

**Files:**
- Modify: `crates/riversd/src/pool.rs:97-103`

- [ ] **Step 1: Understand current behavior**

Currently `PoolGuard::drop()` (line 97) just decrements `active_count` and drops the connection. The spec says we need to return it to idle instead, to preserve prepared statement caches.

The challenge: `Drop::drop()` is synchronous, but `ConnectionPool::release()` is async (takes a Mutex lock). We need a way to return the connection without async.

- [ ] **Step 2: Add a pool reference to PoolGuard**

Change `PoolGuard` to hold a reference to the pool's idle queue so it can return the connection synchronously:

```rust
pub struct PoolGuard {
    active_count: Arc<AtomicU64>,
    _conn: Option<Box<dyn Connection>>,
    idle_return: Option<Arc<Mutex<VecDeque<PooledConnection>>>>,
}
```

Update `PoolGuard::new()` to accept the idle queue:

```rust
pub fn new(
    conn: Box<dyn Connection>,
    active_count: Arc<AtomicU64>,
    idle_return: Arc<Mutex<VecDeque<PooledConnection>>>,
) -> Self {
    Self {
        active_count,
        _conn: Some(conn),
        idle_return: Some(idle_return),
    }
}
```

- [ ] **Step 3: Update Drop to return connection to idle**

```rust
impl Drop for PoolGuard {
    fn drop(&mut self) {
        if let Some(conn) = self._conn.take() {
            self.active_count.fetch_sub(1, Ordering::Relaxed);
            if let Some(ref idle) = self.idle_return {
                let pooled = PooledConnection {
                    conn,
                    created_at: Instant::now(),
                    last_used: Instant::now(),
                };
                if let Ok(mut queue) = idle.try_lock() {
                    queue.push_back(pooled);
                }
                // If lock fails (contention), drop the connection — better than blocking in Drop
            }
        }
    }
}
```

Note: Using `try_lock()` instead of `.await` because `Drop` is sync. If the lock is contended (rare), the connection is dropped — acceptable tradeoff.

- [ ] **Step 4: Update all PoolGuard::new() call sites**

Search for `PoolGuard::new(` in `pool.rs` and update to pass the idle queue. The `acquire()` method creates PoolGuards — it has access to `self.idle`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p riversd --lib -- pool`
Expected: All pool tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/riversd/src/pool.rs
git commit -m "feat(pool): PoolGuard returns connections to idle on drop to preserve prepared caches"
```

---

### Task 3: SQL Driver Transaction + Prepared Statement Implementations

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/postgres.rs`
- Modify: `crates/rivers-drivers-builtin/src/mysql.rs`
- Modify: `crates/rivers-drivers-builtin/src/sqlite.rs`

- [ ] **Step 1: PostgreSQL — implement transaction methods**

In `PostgresConnection` impl block, add:

```rust
async fn begin_transaction(&mut self) -> Result<(), DriverError> {
    self.client
        .batch_execute("BEGIN")
        .await
        .map_err(|e| DriverError::Query(format!("postgres BEGIN: {e}")))
}

async fn commit_transaction(&mut self) -> Result<(), DriverError> {
    self.client
        .batch_execute("COMMIT")
        .await
        .map_err(|e| DriverError::Query(format!("postgres COMMIT: {e}")))
}

async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
    self.client
        .batch_execute("ROLLBACK")
        .await
        .map_err(|e| DriverError::Query(format!("postgres ROLLBACK: {e}")))
}
```

- [ ] **Step 2: MySQL — implement transaction methods**

In `MysqlConnection` impl block, add:

```rust
async fn begin_transaction(&mut self) -> Result<(), DriverError> {
    self.conn
        .query_drop("BEGIN")
        .await
        .map_err(|e| DriverError::Query(format!("mysql BEGIN: {e}")))
}

async fn commit_transaction(&mut self) -> Result<(), DriverError> {
    self.conn
        .query_drop("COMMIT")
        .await
        .map_err(|e| DriverError::Query(format!("mysql COMMIT: {e}")))
}

async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
    self.conn
        .query_drop("ROLLBACK")
        .await
        .map_err(|e| DriverError::Query(format!("mysql ROLLBACK: {e}")))
}
```

- [ ] **Step 3: SQLite — implement transaction methods**

SQLite uses `rusqlite` via `spawn_blocking`. In `SqliteConnection` impl block, add:

```rust
async fn begin_transaction(&mut self) -> Result<(), DriverError> {
    let conn = Arc::clone(&self.conn);
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| DriverError::Internal(format!("sqlite mutex: {e}")))?;
        conn.execute_batch("BEGIN").map_err(|e| DriverError::Query(format!("sqlite BEGIN: {e}")))
    })
    .await
    .map_err(|e| DriverError::Internal(format!("spawn_blocking: {e}")))?
}

async fn commit_transaction(&mut self) -> Result<(), DriverError> {
    let conn = Arc::clone(&self.conn);
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| DriverError::Internal(format!("sqlite mutex: {e}")))?;
        conn.execute_batch("COMMIT").map_err(|e| DriverError::Query(format!("sqlite COMMIT: {e}")))
    })
    .await
    .map_err(|e| DriverError::Internal(format!("spawn_blocking: {e}")))?
}

async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
    let conn = Arc::clone(&self.conn);
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| DriverError::Internal(format!("sqlite mutex: {e}")))?;
        conn.execute_batch("ROLLBACK").map_err(|e| DriverError::Query(format!("sqlite ROLLBACK: {e}")))
    })
    .await
    .map_err(|e| DriverError::Internal(format!("spawn_blocking: {e}")))?
}
```

- [ ] **Step 4: Write integration tests for transaction round-trip**

Create or add to the existing live test files. Each driver needs a test:
- Begin → insert → select (sees row) → rollback → select (row gone)

Example for postgres (add to `crates/rivers-core/tests/drivers_tests.rs` or a new test file):

```rust
#[tokio::test]
async fn postgres_transaction_roundtrip() {
    // connect, begin, insert, select (row visible), rollback, select (row gone)
}
```

Follow the existing test patterns for each driver. Tests should SKIP if the database is unreachable.

- [ ] **Step 5: Verify all drivers compile**

Run: `cargo check -p rivers-drivers-builtin`
Expected: Compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-drivers-builtin/src/postgres.rs crates/rivers-drivers-builtin/src/mysql.rs crates/rivers-drivers-builtin/src/sqlite.rs
git commit -m "feat(connection): implement transaction methods for postgres, mysql, sqlite"
```

---

### Task 4: MongoDB + Neo4j Transaction Implementations

**Files:**
- Modify: `crates/rivers-plugin-mongodb/src/lib.rs`
- Modify: `crates/rivers-plugin-neo4j/src/lib.rs`

- [ ] **Step 1: MongoDB — implement transaction methods**

MongoDB transactions require a session. The `MongoConnection` currently holds a `mongodb::Database`. To support transactions, it needs to also hold an optional `ClientSession`.

Add a `session` field to `MongoConnection`:

```rust
pub struct MongoConnection {
    db: mongodb::Database,
    client: mongodb::Client,
    session: Option<mongodb::ClientSession>,
}
```

Implement the transaction methods:

```rust
async fn begin_transaction(&mut self) -> Result<(), DriverError> {
    let mut session = self.client
        .start_session()
        .await
        .map_err(|e| DriverError::Query(format!("mongodb start session: {e}")))?;
    session
        .start_transaction()
        .await
        .map_err(|e| DriverError::Query(format!("mongodb BEGIN: {e}")))?;
    self.session = Some(session);
    Ok(())
}

async fn commit_transaction(&mut self) -> Result<(), DriverError> {
    if let Some(ref mut session) = self.session {
        session.commit_transaction()
            .await
            .map_err(|e| DriverError::Query(format!("mongodb COMMIT: {e}")))?;
    }
    self.session = None;
    Ok(())
}

async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
    if let Some(ref mut session) = self.session {
        session.abort_transaction()
            .await
            .map_err(|e| DriverError::Query(format!("mongodb ROLLBACK: {e}")))?;
    }
    self.session = None;
    Ok(())
}
```

Note: MongoDB's `exec_find`, `exec_insert`, etc. methods will need to use the session when one is active. This requires modifying those methods to check `self.session` and pass it to the MongoDB driver operations. This is a significant change to the MongoDB plugin — the implementer should read the `mongodb` crate docs for session-aware operations.

- [ ] **Step 2: Neo4j — implement transaction methods**

Neo4j's `neo4rs` crate supports transactions via `Graph::start_txn()`. Add a `txn` field to `Neo4jConnection`:

```rust
pub struct Neo4jConnection {
    graph: neo4rs::Graph,
    txn: Option<neo4rs::Txn>,
}
```

Implement:

```rust
async fn begin_transaction(&mut self) -> Result<(), DriverError> {
    let txn = self.graph
        .start_txn()
        .await
        .map_err(|e| DriverError::Query(format!("neo4j BEGIN: {e}")))?;
    self.txn = Some(txn);
    Ok(())
}

async fn commit_transaction(&mut self) -> Result<(), DriverError> {
    if let Some(txn) = self.txn.take() {
        txn.commit()
            .await
            .map_err(|e| DriverError::Query(format!("neo4j COMMIT: {e}")))?;
    }
    Ok(())
}

async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
    if let Some(txn) = self.txn.take() {
        txn.rollback()
            .await
            .map_err(|e| DriverError::Query(format!("neo4j ROLLBACK: {e}")))?;
    }
    Ok(())
}
```

Note: Neo4j's execute methods need to use `txn.run()` or `txn.execute()` when a transaction is active instead of `graph.run()`. The implementer should modify the `execute()` method to check `self.txn`.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rivers-plugin-mongodb && cargo check -p rivers-plugin-neo4j`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/rivers-plugin-mongodb/src/lib.rs crates/rivers-plugin-neo4j/src/lib.rs
git commit -m "feat(connection): implement transaction methods for mongodb and neo4j"
```

---

### Task 5: Config — Add `prepared` Field to DataView

**Files:**
- Modify: `crates/rivers-runtime/src/dataview.rs`
- Modify: `crates/rivers-runtime/src/validate_structural.rs`

- [ ] **Step 1: Add `prepared` field to DataViewConfig**

After the `circuit_breaker_id` field, add:

```rust
    /// Enable prepared statement caching for this DataView's queries.
    #[serde(default)]
    pub prepared: bool,
```

- [ ] **Step 2: Add to DATAVIEW_FIELDS**

In `validate_structural.rs`, add `"prepared"` to the `DATAVIEW_FIELDS` array.

- [ ] **Step 3: Add `prepared: false` to all DataViewConfig struct initializers**

Search for existing exhaustive initializers (same files that needed `circuit_breaker_id: None`) and add `prepared: false`.

- [ ] **Step 4: Add deserialization test**

```rust
#[test]
fn dataview_config_parses_prepared() {
    let toml_str = r#"
        name = "test"
        datasource = "ds"
        prepared = true
    "#;
    let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.prepared);
}

#[test]
fn dataview_config_prepared_defaults_false() {
    let toml_str = r#"
        name = "test"
        datasource = "ds"
    "#;
    let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
    assert!(!cfg.prepared);
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rivers-runtime -- dataview_config`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-runtime/src/dataview.rs crates/rivers-runtime/src/validate_structural.rs
git commit -m "feat(connection): add prepared field to DataViewConfig"
```

---

### Task 6: TransactionMap — Per-Request Transaction State

**Files:**
- Create: `crates/riversd/src/transaction.rs`

- [ ] **Step 1: Create TransactionMap**

```rust
//! Per-request transaction state management.

use std::collections::HashMap;
use rivers_driver_sdk::{Connection, DriverError};
use tokio::sync::Mutex;
use std::sync::Arc;

/// Holds active transaction connections for a single request.
/// Keyed by datasource name.
pub struct TransactionMap {
    connections: Mutex<HashMap<String, Box<dyn Connection>>>,
}

impl TransactionMap {
    /// Create an empty transaction map.
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    /// Begin a transaction on a datasource. Stores the connection.
    pub async fn begin(
        &self,
        datasource: &str,
        mut conn: Box<dyn Connection>,
    ) -> Result<(), DriverError> {
        let mut map = self.connections.lock().await;
        if map.contains_key(datasource) {
            return Err(DriverError::Query(format!(
                "transaction already active on datasource '{}'",
                datasource
            )));
        }
        conn.begin_transaction().await?;
        map.insert(datasource.to_string(), conn);
        Ok(())
    }

    /// Get a mutable reference to the transaction connection for a datasource.
    /// Returns None if no transaction is active on that datasource.
    pub async fn get_connection(
        &self,
        datasource: &str,
    ) -> Option<Box<dyn Connection>> {
        // We can't return a reference through the Mutex, so we temporarily take it
        // Caller must return it via return_connection()
        let mut map = self.connections.lock().await;
        map.remove(datasource)
    }

    /// Return a connection to the transaction map after use.
    pub async fn return_connection(
        &self,
        datasource: &str,
        conn: Box<dyn Connection>,
    ) {
        let mut map = self.connections.lock().await;
        map.insert(datasource.to_string(), conn);
    }

    /// Check if a transaction is active on a datasource.
    pub async fn has_transaction(&self, datasource: &str) -> bool {
        let map = self.connections.lock().await;
        map.contains_key(datasource)
    }

    /// Commit the transaction on a datasource. Returns the connection for pool release.
    pub async fn commit(
        &self,
        datasource: &str,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let mut map = self.connections.lock().await;
        match map.remove(datasource) {
            Some(mut conn) => {
                conn.commit_transaction().await?;
                Ok(conn)
            }
            None => Err(DriverError::Query(format!(
                "no active transaction on datasource '{}'",
                datasource
            ))),
        }
    }

    /// Rollback the transaction on a datasource. Connection is dropped (not returned to pool).
    pub async fn rollback(
        &self,
        datasource: &str,
    ) -> Result<(), DriverError> {
        let mut map = self.connections.lock().await;
        match map.remove(datasource) {
            Some(mut conn) => {
                conn.rollback_transaction().await?;
                // Connection dropped — don't return to pool after rollback
                Ok(())
            }
            None => Err(DriverError::Query(format!(
                "no active transaction on datasource '{}'",
                datasource
            ))),
        }
    }

    /// Auto-rollback all remaining transactions. Called at request end.
    /// Logs a warning for each uncommitted transaction.
    pub async fn auto_rollback_all(&self) {
        let mut map = self.connections.lock().await;
        for (datasource, mut conn) in map.drain() {
            tracing::warn!(
                datasource = %datasource,
                "auto-rollback — handler did not commit or rollback"
            );
            if let Err(e) = conn.rollback_transaction().await {
                tracing::error!(
                    datasource = %datasource,
                    error = %e,
                    "auto-rollback failed"
                );
            }
            // Connection dropped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Query, QueryResult, QueryValue};
    use async_trait::async_trait;

    struct MockConnection {
        began: bool,
        committed: bool,
        rolled_back: bool,
    }

    impl MockConnection {
        fn new() -> Self {
            Self { began: false, committed: false, rolled_back: false }
        }
    }

    #[async_trait]
    impl Connection for MockConnection {
        async fn execute(&mut self, _query: &Query) -> Result<QueryResult, DriverError> {
            Ok(QueryResult { rows: vec![], affected_rows: 0, last_insert_id: None })
        }
        async fn ping(&mut self) -> Result<(), DriverError> { Ok(()) }
        fn driver_name(&self) -> &str { "mock" }
        async fn begin_transaction(&mut self) -> Result<(), DriverError> {
            self.began = true;
            Ok(())
        }
        async fn commit_transaction(&mut self) -> Result<(), DriverError> {
            self.committed = true;
            Ok(())
        }
        async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
            self.rolled_back = true;
            Ok(())
        }
    }

    #[tokio::test]
    async fn begin_stores_connection() {
        let map = TransactionMap::new();
        let conn = Box::new(MockConnection::new());
        map.begin("pg", conn).await.unwrap();
        assert!(map.has_transaction("pg").await);
    }

    #[tokio::test]
    async fn double_begin_fails() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new())).await.unwrap();
        let err = map.begin("pg", Box::new(MockConnection::new())).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn commit_removes_connection() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new())).await.unwrap();
        let conn = map.commit("pg").await.unwrap();
        assert!(!map.has_transaction("pg").await);
        // Connection returned for pool release
        drop(conn);
    }

    #[tokio::test]
    async fn rollback_removes_connection() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new())).await.unwrap();
        map.rollback("pg").await.unwrap();
        assert!(!map.has_transaction("pg").await);
    }

    #[tokio::test]
    async fn commit_without_begin_fails() {
        let map = TransactionMap::new();
        assert!(map.commit("pg").await.is_err());
    }

    #[tokio::test]
    async fn rollback_without_begin_fails() {
        let map = TransactionMap::new();
        assert!(map.rollback("pg").await.is_err());
    }

    #[tokio::test]
    async fn auto_rollback_clears_all() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new())).await.unwrap();
        map.begin("mysql", Box::new(MockConnection::new())).await.unwrap();
        map.auto_rollback_all().await;
        assert!(!map.has_transaction("pg").await);
        assert!(!map.has_transaction("mysql").await);
    }
}
```

- [ ] **Step 2: Register module**

In `crates/riversd/src/lib.rs`, add:

```rust
/// Per-request transaction state management.
pub mod transaction;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p riversd --lib -- transaction`
Expected: All 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/riversd/src/transaction.rs crates/riversd/src/lib.rs
git commit -m "feat(connection): add TransactionMap for per-request transaction state"
```

---

### Task 7: Host Callbacks — Rivers.db.begin/commit/rollback/batch

**Files:**
- Modify: `crates/riversd/src/engine_loader/host_callbacks.rs`

This task adds 4 new host callbacks accessible from JS/WASM handlers. The implementation follows the existing `host_dataview_execute` pattern: extern "C" FFI function, read input JSON, access HOST_CONTEXT, spawn async work on the runtime handle, write output.

- [ ] **Step 1: Understand the existing callback registration**

Read how callbacks are registered in the host context. Look for where `host_dataview_execute` is assigned to the HostCallbacks struct. The pattern is:

```rust
callbacks.dataview_execute = Some(host_dataview_execute);
```

New callbacks need:
- `callbacks.db_begin = Some(host_db_begin);`
- `callbacks.db_commit = Some(host_db_commit);`
- `callbacks.db_rollback = Some(host_db_rollback);`
- `callbacks.db_batch = Some(host_db_batch);`

Note: The `HostCallbacks` struct may need new fields in `rivers-engine-sdk`. Check if it already has slots for these, or if they need to be added.

- [ ] **Step 2: Add callback function stubs**

Each callback follows the same FFI pattern. The implementer should:
1. Read the `rivers-engine-sdk` `HostCallbacks` struct to understand available slots
2. Add new slots if needed (this requires modifying `crates/rivers-engine-sdk`)
3. Implement each callback following the `host_dataview_execute` pattern
4. Wire the TransactionMap into the HOST_CONTEXT or per-task context

This is the most complex integration task. The implementer needs to trace how `host_dataview_execute` accesses the DataViewExecutor and connection pool, then adapt that pattern for transaction management.

Key considerations:
- The TransactionMap must be per-request (per-handler-invocation), not global
- The HOST_CONTEXT is a thread-local — each V8 worker has its own
- The batch callback reuses the existing DataView execution path but loops over parameter sets

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p riversd`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/riversd/src/engine_loader/host_callbacks.rs crates/rivers-engine-sdk/src/lib.rs
git commit -m "feat(connection): add Rivers.db.begin/commit/rollback/batch host callbacks"
```

---

### Task 8: DataView Engine — Transaction-Aware Query Routing + Prepared Statements

**Files:**
- Modify: `crates/rivers-runtime/src/dataview_engine.rs`

- [ ] **Step 1: Add transaction connection parameter to execute()**

The `DataViewExecutor::execute()` method needs an optional transaction connection parameter. When provided, queries run on that connection instead of acquiring from the pool:

```rust
pub async fn execute(
    &self,
    name: &str,
    params: HashMap<String, QueryValue>,
    method: &str,
    trace_id: &str,
    txn_conn: Option<&mut Box<dyn Connection>>,
) -> Result<DataViewResult, DataViewError> {
```

If `txn_conn` is `Some`, use it directly. If `None`, acquire from pool as before.

Note: This changes the function signature — all existing call sites need updating. Search for `.execute(` on `DataViewExecutor` and add `None` as the last argument to maintain current behavior.

- [ ] **Step 2: Add prepared statement logic**

In the execute path, after resolving the DataView config, check `dv_config.prepared`:

```rust
if dv_config.prepared {
    if !conn.has_prepared(&query_statement) {
        conn.prepare(&query_statement).await?;
    }
    conn.execute_prepared(&query).await?
} else {
    conn.execute(&query).await?
}
```

- [ ] **Step 3: Update all execute() call sites**

Search for all calls to `executor.execute(` across the codebase and add `None` as the txn_conn parameter.

- [ ] **Step 4: Verify compilation**

Run: `cargo check --workspace`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-runtime/src/dataview_engine.rs
git commit -m "feat(connection): transaction-aware query routing and prepared statement support in DataView engine"
```

---

### Task 9: Documentation and Task Tracking

**Files:**
- Create: `docs/guide/tutorials/tutorial-transactions.md`
- Modify: `todo/ProgramReviewTasks.md`

- [ ] **Step 1: Write transactions tutorial**

Cover:
- What transactions do in Rivers
- How to use `Rivers.db.begin()`, `Rivers.db.commit()`, `Rivers.db.rollback()` in JS handlers
- Auto-rollback behavior
- Batch operations with `Rivers.db.batch()`
- Prepared statements via `prepared = true` config
- Which drivers support transactions

- [ ] **Step 2: Mark tasks complete in ProgramReviewTasks.md**

- [ ] **Step 3: Commit**

```bash
git add docs/guide/tutorials/tutorial-transactions.md todo/ProgramReviewTasks.md
git commit -m "docs: add transactions tutorial and update task tracking"
```

---

### Task 10: Final Validation

- [ ] **Step 1: Full workspace compile**

Run: `cargo check --workspace`
Expected: Compiles clean.

- [ ] **Step 2: Run all riversd lib tests**

Run: `cargo test -p riversd --lib`
Expected: All tests pass (including new transaction and pool tests).

- [ ] **Step 3: Run all rivers-runtime tests**

Run: `cargo test -p rivers-runtime --lib`
Expected: All tests pass.

- [ ] **Step 4: Run driver-sdk tests**

Run: `cargo test -p rivers-driver-sdk`
Expected: All tests pass.

- [ ] **Step 5: Validate address-book-bundle**

Run: `cargo run -p riverpackage -- validate address-book-bundle`
Expected: 0 errors (address-book-bundle doesn't use transactions/prepared).

- [ ] **Step 6: Run live integration tests (if infra available)**

Run: `cargo test -p rivers-plugin-neo4j --test neo4j_live_test -- --nocapture`
Expected: 2 tests pass.

Run postgres/mysql/sqlite transaction round-trip tests if they were created in Task 3.
