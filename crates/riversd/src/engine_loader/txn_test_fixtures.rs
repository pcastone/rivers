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

/// Long-lived multi-threaded tokio runtime used as the rt_handle in
/// `HOST_CONTEXT`. Each `#[tokio::test]` spins up its own runtime that
/// dies at end-of-test, so capturing `Handle::current()` at fixture-init
/// time gives a stale handle by the second test. Phase I8's SQLite
/// driver uses `tokio::task::spawn_blocking` inside its async `connect`
/// — running that on a stale runtime cancels the inner task. The fix:
/// install our own runtime that survives the entire test binary.
fn shared_test_runtime_handle() -> tokio::runtime::Handle {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("rivers-fixture-rt")
            .build()
            .expect("build fixture tokio runtime")
    })
    .handle()
    .clone()
}

/// Idempotent `HOST_CONTEXT` setup. Registers the legacy
/// `mock-txn-driver` (for I3-I5 tests), `dispatch-mock-driver` (for
/// I7 tests), the real built-in `sqlite` driver (for I8 e2e tests),
/// AND the real built-in `postgres` driver (for I-FU2 cluster e2e
/// tests) into the same factory before installing it into the
/// `HOST_CONTEXT` `OnceLock`. Subsequent calls are no-ops because
/// `set_host_context` itself uses `OnceLock::set`.
///
/// SQLite + Postgres registration are co-located here (rather than in
/// separate fixtures) because `HOST_CONTEXT` is a `OnceLock` — only
/// one fixture init wins per test binary, and Phase I8/I-FU2 e2e
/// tests need the mock drivers (kept for I3-I7 tests' commit-fail
/// behavior) plus the real durable drivers wired through the same
/// factory. PostgresDriver is stateless — registering it does not
/// open a connection, so this is safe even when the cluster is
/// unreachable; only the per-test `connect()` calls in `pg_e2e_tests`
/// touch the network, and those are gated on `RIVERS_TEST_CLUSTER=1`.
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
        // I8 — register real SQLite for e2e durability tests. Driver
        // name is "sqlite"; e2e tests construct ConnectionParams whose
        // `database` field is the temp-file path.
        factory.register_database_driver(Arc::new(
            rivers_runtime::rivers_core::drivers::SqliteDriver,
        ));
        // I-FU2 — register real Postgres for cluster-gated e2e tests.
        // Driver name is "postgres"; e2e tests construct ConnectionParams
        // pointing at the test cluster (192.168.2.209). Stateless
        // registration; no network call is issued here.
        factory.register_database_driver(Arc::new(
            rivers_runtime::rivers_core::drivers::PostgresDriver,
        ));
        // Enter the shared fixture runtime BEFORE calling set_host_context
        // so its `Handle::current()` capture binds to the long-lived
        // runtime instead of whichever per-test runtime is on stack.
        let handle = shared_test_runtime_handle();
        let _enter = handle.enter();
        super::host_context::set_host_context(
            Arc::new(tokio::sync::RwLock::new(None)),
            None,
            Some(Arc::new(factory)),
        );
    });
    beh
}

/// I8 e2e helper — build a `DataViewExecutor` wired to the registered
/// `sqlite` driver and a single dataview definition. The executor is
/// returned wrapped in `Arc` so tests can install it into `HOST_CONTEXT`
/// via `host_context::install_dataview_executor_for_test(...)`.
///
/// `dataview_name` becomes the registered name; `query` is the raw SQL
/// (taken verbatim — no parameter coercion done here, callers wire any
/// `$name` placeholders to match SqliteDriver's `DollarNamed` style).
/// `db_path` is the SQLite path passed as the `database` field in
/// `ConnectionParams` — typically a tempfile path so e2e durability
/// assertions can re-open it from outside the dispatch.
pub(crate) fn build_sqlite_executor(
    dataview_name: &str,
    query: &str,
    db_path: &str,
) -> Arc<rivers_runtime::DataViewExecutor> {
    use rivers_runtime::dataview::{DataViewConfig, DataViewParameterConfig};
    use rivers_runtime::dataview_engine::{DataViewExecutor, DataViewRegistry};
    use rivers_runtime::rivers_driver_sdk::ConnectionParams;
    use rivers_runtime::tiered_cache::{DataViewCache, NoopDataViewCache};
    use std::collections::HashMap;

    let factory = super::host_context::HOST_CONTEXT
        .get()
        .expect("HOST_CONTEXT must be set first (call ensure_host_context())")
        .driver_factory
        .clone()
        .expect("driver factory present");

    let mut registry = DataViewRegistry::new();
    registry.register(DataViewConfig {
        name: dataview_name.into(),
        datasource: "sqlite_e2e".into(),
        query: Some(query.to_string()),
        parameters: vec![],
        return_schema: None,
        get_query: Some(query.to_string()),
        post_query: Some(query.to_string()),
        put_query: Some(query.to_string()),
        delete_query: Some(query.to_string()),
        get_schema: None,
        post_schema: None,
        put_schema: None,
        delete_schema: None,
        get_parameters: Vec::<DataViewParameterConfig>::new(),
        post_parameters: Vec::<DataViewParameterConfig>::new(),
        put_parameters: Vec::<DataViewParameterConfig>::new(),
        delete_parameters: Vec::<DataViewParameterConfig>::new(),
        streaming: false,
        circuit_breaker_id: None,
        prepared: false,
        query_params: Default::default(),
        caching: None,
        invalidates: vec![],
        validate_result: false,
        strict_parameters: false,
        max_rows: 1000,
        skip_introspect: false,
    });

    // Connection params for the registered "sqlite_e2e" datasource.
    // The `driver` option steers DataViewExecutor::execute to the
    // "sqlite" driver registered above; without it the executor falls
    // back to using the datasource id as the driver name.
    let mut options = HashMap::new();
    options.insert("driver".to_string(), "sqlite".to_string());
    let params = ConnectionParams {
        host: String::new(),
        port: 0,
        database: db_path.to_string(),
        username: String::new(),
        password: String::new(),
        options,
    };
    let mut params_map = HashMap::new();
    params_map.insert("sqlite_e2e".to_string(), params);

    let cache: Arc<dyn DataViewCache> = Arc::new(NoopDataViewCache);
    Arc::new(DataViewExecutor::new(
        registry,
        factory,
        Arc::new(params_map),
        cache,
    ))
}

/// I-FU2 e2e helper — build a `DataViewExecutor` wired to the registered
/// `postgres` driver and a single dataview definition. Mirrors
/// `build_sqlite_executor` but targets the live test cluster.
///
/// Connection params are passed through verbatim (host/port/user/pass/db).
/// The `dataview_name` becomes the registered name; `query` is the raw
/// SQL (callers wire any `$1, $2, ...` placeholders to match Postgres'
/// positional style). The executor is wrapped in `Arc` so tests can
/// install it via `host_context::install_dataview_executor_for_test`.
pub(crate) fn build_postgres_executor(
    dataview_name: &str,
    query: &str,
    params: rivers_runtime::rivers_driver_sdk::ConnectionParams,
    datasource_id: &str,
) -> Arc<rivers_runtime::DataViewExecutor> {
    use rivers_runtime::dataview::{DataViewConfig, DataViewParameterConfig};
    use rivers_runtime::dataview_engine::{DataViewExecutor, DataViewRegistry};
    use rivers_runtime::tiered_cache::{DataViewCache, NoopDataViewCache};
    use std::collections::HashMap;

    let factory = super::host_context::HOST_CONTEXT
        .get()
        .expect("HOST_CONTEXT must be set first (call ensure_host_context())")
        .driver_factory
        .clone()
        .expect("driver factory present");

    let mut registry = DataViewRegistry::new();
    registry.register(DataViewConfig {
        name: dataview_name.into(),
        datasource: datasource_id.into(),
        query: Some(query.to_string()),
        parameters: vec![],
        return_schema: None,
        get_query: Some(query.to_string()),
        post_query: Some(query.to_string()),
        put_query: Some(query.to_string()),
        delete_query: Some(query.to_string()),
        get_schema: None,
        post_schema: None,
        put_schema: None,
        delete_schema: None,
        get_parameters: Vec::<DataViewParameterConfig>::new(),
        post_parameters: Vec::<DataViewParameterConfig>::new(),
        put_parameters: Vec::<DataViewParameterConfig>::new(),
        delete_parameters: Vec::<DataViewParameterConfig>::new(),
        streaming: false,
        circuit_breaker_id: None,
        prepared: false,
        query_params: Default::default(),
        caching: None,
        invalidates: vec![],
        validate_result: false,
        strict_parameters: false,
        max_rows: 1000,
        skip_introspect: false,
    });

    // Force the driver to "postgres" via the connection-params options
    // map. Without it the executor falls back to using the datasource id
    // as the driver name (which would be e.g. "pg_e2e", unregistered).
    let mut params = params;
    params
        .options
        .insert("driver".to_string(), "postgres".to_string());
    let mut params_map = HashMap::new();
    params_map.insert(datasource_id.to_string(), params);

    let cache: Arc<dyn DataViewCache> = Arc::new(NoopDataViewCache);
    Arc::new(DataViewExecutor::new(
        registry,
        factory,
        Arc::new(params_map),
        cache,
    ))
}

/// Process-wide test mutex. Tests share the `SharedConnBehavior` flags
/// and the `CURRENT_TASK_ID` thread-local on `spawn_blocking` workers,
/// so they cannot reliably run in parallel. Acquire this before
/// touching either.
pub(crate) fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
