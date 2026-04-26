//! Per-task transaction map for dynamic-engine (cdylib) host callbacks.
//!
//! Mirrors the V8 path's `crate::transaction::TransactionMap` + the
//! `TASK_TRANSACTION` thread-local. V8 uses a per-request `Arc<TransactionMap>`
//! pinned to a worker thread because V8 isolates are 1:1 with task identity.
//! Cdylib host callbacks run on a riversd-side `spawn_blocking` worker, so we
//! key the map explicitly by `(TaskId, datasource_name)` and identify the
//! owning task via a `spawn_blocking`-thread-local set by `TaskGuard::enter`
//! (see `host_context.rs`).
//!
//! Per `docs/superpowers/plans/2026-04-25-phase-i-dyn-transactions.md` and
//! `changedecisionlog.md` TXN-I1.1.

use std::collections::HashMap;
use std::sync::Mutex;

use rivers_runtime::rivers_driver_sdk::{Connection, DriverError};

/// Opaque task identifier issued by `host_context::next_task_id` and bound
/// to the current `spawn_blocking` worker via `TaskGuard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

/// Errors specific to the dyn-engine transaction map.
///
/// These are programmer/handler-state errors rather than driver errors —
/// driver errors are wrapped via the `Driver` variant.
#[derive(Debug, thiserror::Error)]
pub enum DynTxnError {
    /// Tried to begin a transaction on `(task_id, ds)` when one already exists.
    #[error("transaction already active for task {task_id:?} on datasource {ds_name}")]
    AlreadyActive {
        /// Owning task.
        task_id: TaskId,
        /// Datasource name.
        ds_name: String,
    },
    /// Tried to operate on a missing transaction.
    #[error("no active transaction for task {task_id:?} on datasource {ds_name}")]
    NotFound {
        /// Owning task.
        task_id: TaskId,
        /// Datasource name.
        ds_name: String,
    },
    /// Driver error while begin/commit/rollback.
    #[error("driver error: {0}")]
    Driver(#[from] DriverError),
}

/// Process-wide map of in-flight cdylib transactions, keyed by
/// `(TaskId, datasource_name)`. One instance per `riversd` process,
/// stored in `host_context::DYN_TXN_MAP`.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) because every
/// public method either holds the lock for a non-`.await` critical section
/// or — in the case of `with_conn_mut` — takes the entry out, drops the lock,
/// runs the future, then re-acquires the lock to put it back. This avoids
/// holding a sync mutex across an `.await`.
pub struct DynTransactionMap {
    inner: Mutex<HashMap<(TaskId, String), Box<dyn Connection>>>,
}

impl DynTransactionMap {
    /// Create an empty map. Used by the `OnceLock::get_or_init` accessor.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Insert a freshly-begun transaction connection. Errors if
    /// `(task_id, ds_name)` already has an entry — silent overwrite would
    /// drop a `Box<dyn Connection>` and leak its pool slot until that pool
    /// detected the loss.
    ///
    /// Caller is expected to have already invoked `conn.begin_transaction()`.
    pub fn insert(
        &self,
        task_id: TaskId,
        ds_name: &str,
        conn: Box<dyn Connection>,
    ) -> Result<(), DynTxnError> {
        let mut map = self.inner.lock().expect("DynTxnMap mutex poisoned");
        let key = (task_id, ds_name.to_string());
        if map.contains_key(&key) {
            return Err(DynTxnError::AlreadyActive {
                task_id,
                ds_name: ds_name.to_string(),
            });
        }
        map.insert(key, conn);
        Ok(())
    }

    /// Take ownership of a transaction's connection. One-shot — used by
    /// `host_db_commit` / `host_db_rollback` to remove the entry before
    /// running `commit_transaction()` / `rollback_transaction()`.
    pub fn take(&self, task_id: TaskId, ds_name: &str) -> Option<Box<dyn Connection>> {
        let mut map = self.inner.lock().expect("DynTxnMap mutex poisoned");
        map.remove(&(task_id, ds_name.to_string()))
    }

    /// Test-only: check whether `(task_id, ds_name)` has an active entry.
    #[cfg(test)]
    pub fn has(&self, task_id: TaskId, ds_name: &str) -> bool {
        let map = self.inner.lock().expect("DynTxnMap mutex poisoned");
        map.contains_key(&(task_id, ds_name.to_string()))
    }

    /// Apply an async closure to the connection in place, then re-insert.
    /// Used by `host_dataview_execute` to thread the txn connection through
    /// `DataViewExecutor::execute(..., txn_conn = Some(&mut conn))` without
    /// permanently transferring ownership.
    ///
    /// Returns `None` if no transaction exists for `(task_id, ds_name)`.
    /// The closure's output is returned via `Some(R)` — errors from the
    /// closure are passed through as part of `R`; this method does not
    /// transform them.
    ///
    /// The closure receives `&'a mut Box<dyn Connection>` and must return
    /// a `BoxFuture<'a, R>` so the borrow flows naturally into the future.
    /// The HRTB on `F` is what makes the call site usable from `async move`.
    ///
    /// Critically: the sync mutex is **not** held across the `.await`. The
    /// connection is removed under the lock, the future is run with the
    /// lock dropped, and the connection is reinserted under a fresh lock
    /// acquisition. Concurrent host callbacks for other `(task_id, ds_name)`
    /// keys are unaffected.
    pub async fn with_conn_mut<R, F>(
        &self,
        task_id: TaskId,
        ds_name: &str,
        f: F,
    ) -> Option<R>
    where
        for<'a> F: FnOnce(
            &'a mut Box<dyn Connection>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = R> + Send + 'a>,
        >,
    {
        let mut conn = {
            let mut map = self.inner.lock().expect("DynTxnMap mutex poisoned");
            map.remove(&(task_id, ds_name.to_string()))?
        };
        let result = f(&mut conn).await;
        let mut map = self.inner.lock().expect("DynTxnMap mutex poisoned");
        map.insert((task_id, ds_name.to_string()), conn);
        Some(result)
    }

    /// Drain every transaction belonging to a task. Used by the auto-rollback
    /// hook in `TaskGuard::drop`. Returns `(ds_name, Box<dyn Connection>)`
    /// pairs so the caller can run `rollback_transaction()` on each. The
    /// caller is responsible for the rollback — this method only removes
    /// the entries.
    pub fn drain_task(&self, task_id: TaskId) -> Vec<(String, Box<dyn Connection>)> {
        let mut map = self.inner.lock().expect("DynTxnMap mutex poisoned");
        // Collect matching keys first to avoid mutating the map while iterating.
        let keys: Vec<(TaskId, String)> = map
            .keys()
            .filter(|(t, _)| *t == task_id)
            .cloned()
            .collect();
        let mut drained = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(conn) = map.remove(&key) {
                drained.push((key.1, conn));
            }
        }
        drained
    }

    /// Test/diagnostic: number of active transactions across all tasks.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().expect("DynTxnMap mutex poisoned").len()
    }
}

impl Default for DynTransactionMap {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rivers_runtime::rivers_driver_sdk::{Query, QueryResult};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Mock connection that counts how many times `execute` was called —
    /// lets the `with_conn_mut` test verify the closure actually saw the
    /// `&mut Box<dyn Connection>` and could mutate via interior atomics.
    struct MockConnection {
        name: &'static str,
        execute_count: Arc<AtomicU64>,
    }

    impl MockConnection {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                execute_count: Arc::new(AtomicU64::new(0)),
            }
        }

        fn counter(&self) -> Arc<AtomicU64> {
            self.execute_count.clone()
        }
    }

    #[async_trait]
    impl Connection for MockConnection {
        async fn execute(&mut self, _query: &Query) -> Result<QueryResult, DriverError> {
            self.execute_count.fetch_add(1, Ordering::Relaxed);
            Ok(QueryResult {
                rows: vec![],
                affected_rows: 0,
                last_insert_id: None,
                column_names: None,
            })
        }
        async fn ping(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn driver_name(&self) -> &str {
            self.name
        }
        async fn begin_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn commit_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    #[test]
    fn insert_then_take_round_trips_conn() {
        let map = DynTransactionMap::new();
        let task = TaskId(1);
        map.insert(task, "pg", Box::new(MockConnection::new("mock")))
            .expect("insert ok");
        assert_eq!(map.len(), 1);
        let conn = map.take(task, "pg").expect("take ok");
        assert_eq!(conn.driver_name(), "mock");
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn insert_duplicate_returns_already_active() {
        let map = DynTransactionMap::new();
        let task = TaskId(1);
        map.insert(task, "pg", Box::new(MockConnection::new("mock")))
            .expect("first insert ok");
        let err = map
            .insert(task, "pg", Box::new(MockConnection::new("mock")))
            .expect_err("second insert must fail");
        match err {
            DynTxnError::AlreadyActive { task_id, ds_name } => {
                assert_eq!(task_id, task);
                assert_eq!(ds_name, "pg");
            }
            other => panic!("expected AlreadyActive, got {other:?}"),
        }
    }

    #[test]
    fn take_unknown_returns_none() {
        let map = DynTransactionMap::new();
        let task = TaskId(99);
        assert!(map.take(task, "pg").is_none());
    }

    #[test]
    fn drain_task_drops_only_that_tasks_entries() {
        let map = DynTransactionMap::new();
        let t1 = TaskId(1);
        let t2 = TaskId(2);
        map.insert(t1, "a", Box::new(MockConnection::new("a1"))).unwrap();
        map.insert(t1, "b", Box::new(MockConnection::new("b1"))).unwrap();
        map.insert(t2, "a", Box::new(MockConnection::new("a2"))).unwrap();
        assert_eq!(map.len(), 3);

        let drained = map.drain_task(t1);
        assert_eq!(drained.len(), 2);
        let mut names: Vec<&str> = drained.iter().map(|(ds, _)| ds.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);

        assert_eq!(map.len(), 1);
        assert!(map.has(t2, "a"));
        assert!(!map.has(t1, "a"));
        assert!(!map.has(t1, "b"));
    }

    #[tokio::test]
    async fn with_conn_mut_observes_modification() {
        let map = DynTransactionMap::new();
        let task = TaskId(7);
        let mock = MockConnection::new("mock");
        let counter = mock.counter();
        map.insert(task, "pg", Box::new(mock)).unwrap();

        let result = map
            .with_conn_mut(task, "pg", |conn| {
                Box::pin(async move {
                    let q = Query::with_operation("GET", "test", "noop");
                    conn.execute(&q).await.map(|r| r.affected_rows)
                })
            })
            .await
            .expect("conn present");
        assert_eq!(result.unwrap(), 0);
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Closure ran a second time — counter should advance, proving the
        // entry was correctly re-inserted after the first invocation.
        let _: Result<QueryResult, DriverError> = map
            .with_conn_mut(task, "pg", |conn| {
                Box::pin(async move {
                    let q = Query::with_operation("GET", "test", "noop");
                    conn.execute(&q).await
                })
            })
            .await
            .expect("conn present second time");
        assert_eq!(counter.load(Ordering::Relaxed), 2);

        // Map should still be populated.
        assert!(map.has(task, "pg"));
        assert_eq!(map.len(), 1);
    }

    #[tokio::test]
    async fn with_conn_mut_returns_none_when_missing() {
        let map = DynTransactionMap::new();
        let task = TaskId(5);
        let result: Option<()> = map
            .with_conn_mut(task, "pg", |_conn| {
                Box::pin(async move {
                    panic!("closure must not run when entry is missing");
                })
            })
            .await;
        assert!(result.is_none());
    }
}
