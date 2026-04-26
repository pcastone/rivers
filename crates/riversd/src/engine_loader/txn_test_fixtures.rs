//! Shared test fixtures for Phase I dyn-engine transaction tests.
//!
//! `HOST_CONTEXT` is a `OnceLock` — only one test in the riversd test
//! binary actually wins the initialization race. Both
//! `engine_loader::host_callbacks::tests` (I3-I5, I6) and
//! `process_pool::dyn_dispatch_tests` (I7) need a `DriverFactory` wired
//! into `HOST_CONTEXT` with mock drivers registered. This module owns the
//! single, shared init so both test modules use the same setup —
//! whichever module's test runs first triggers it; subsequent calls are
//! no-ops.

#![cfg(test)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rivers_runtime::rivers_core::DriverFactory;
use rivers_runtime::rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, Query, QueryResult,
};

/// Behavior knobs for the shared mock connection. Tests flip these to
/// drive specific paths (commit failure, etc.).
#[derive(Default)]
pub(crate) struct SharedConnBehavior {
    pub(crate) commit_fails: AtomicBool,
}

/// Mock connection used across I3-I7 tests. Always returns empty rows
/// for `execute`; commit honors `behavior.commit_fails`.
pub(crate) struct SharedMockConn {
    pub(crate) behavior: Arc<SharedConnBehavior>,
}

#[async_trait]
impl Connection for SharedMockConn {
    async fn execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
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
        "shared-mock"
    }
    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        if self.behavior.commit_fails.load(Ordering::Relaxed) {
            Err(DriverError::Transaction("forced commit failure".into()))
        } else {
            Ok(())
        }
    }
    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

/// Mock driver — every connect returns a fresh `SharedMockConn` bound to
/// the same `SharedConnBehavior` so tests can flip flags globally.
pub(crate) struct SharedMockDriver {
    pub(crate) behavior: Arc<SharedConnBehavior>,
    pub(crate) name: &'static str,
}

#[async_trait]
impl DatabaseDriver for SharedMockDriver {
    fn name(&self) -> &str {
        self.name
    }
    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Ok(Box::new(SharedMockConn {
            behavior: self.behavior.clone(),
        }))
    }
    fn supports_transactions(&self) -> bool {
        true
    }
}

/// Single shared behavior knob. The host_callbacks tests and the
/// dispatch tests both reach for it — the latter via `behavior()`.
static SHARED_BEHAVIOR: OnceLock<Arc<SharedConnBehavior>> = OnceLock::new();
pub(crate) fn behavior() -> Arc<SharedConnBehavior> {
    SHARED_BEHAVIOR
        .get_or_init(|| Arc::new(SharedConnBehavior::default()))
        .clone()
}

/// Idempotent `HOST_CONTEXT` setup. Registers BOTH the legacy
/// `mock-txn-driver` (for I3-I5 tests) and `dispatch-mock-driver` (for
/// I7 tests) into the same factory before installing it into the
/// `HOST_CONTEXT` `OnceLock`. Subsequent calls are no-ops because
/// `set_host_context` itself uses `OnceLock::set`.
pub(crate) fn ensure_host_context() -> Arc<SharedConnBehavior> {
    static SETUP: OnceLock<()> = OnceLock::new();
    let beh = behavior();
    SETUP.get_or_init(|| {
        let mut factory = DriverFactory::new();
        factory.register_database_driver(Arc::new(SharedMockDriver {
            behavior: beh.clone(),
            name: "mock-txn-driver",
        }));
        factory.register_database_driver(Arc::new(SharedMockDriver {
            behavior: beh.clone(),
            name: "dispatch-mock-driver",
        }));
        super::host_context::set_host_context(
            Arc::new(tokio::sync::RwLock::new(None)),
            None,
            Some(Arc::new(factory)),
        );
    });
    beh
}

/// Process-wide test mutex. Tests share the `SharedConnBehavior` flags
/// and the `CURRENT_TASK_ID` thread-local on `spawn_blocking` workers,
/// so they cannot reliably run in parallel. Acquire this before
/// touching either.
pub(crate) fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
