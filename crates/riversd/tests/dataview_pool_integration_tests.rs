//! End-to-end tests: DataView execution must go through the connection pool.
//!
//! Per `docs/code_review.md` P0-3 / P1-1.
//!
//! These tests prove three properties:
//! 1. Sequential DataView calls reuse pooled connections (50 calls →
//!    very few driver `connect()` invocations).
//! 2. Connections that exceed `max_lifetime_ms` are evicted on the next
//!    acquire (PoolGuard preserves the original `created_at`).
//! 3. DataViews on datasources without a registered pool fall through to
//!    `factory.connect()` — the same code path used by broker `produce`
//!    dispatch today.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use rivers_runtime::pool_handle::SharedPoolHandle;
use rivers_runtime::rivers_core::{DriverFactory, EventBus};
use rivers_runtime::rivers_driver_sdk::error::DriverError;
use rivers_runtime::rivers_driver_sdk::traits::{
    Connection, ConnectionParams, DatabaseDriver,
};
use rivers_runtime::rivers_driver_sdk::types::{Query, QueryResult};
use rivers_runtime::tiered_cache::NoopDataViewCache;
use rivers_runtime::{DataViewConfig, DataViewExecutor, DataViewRegistry};
use riversd::pool::{PoolConfig, PoolManager};

// ─── MockDriver (mirrors crates/riversd/tests/pool_tests.rs) ───────
//
// Integration test files cannot import siblings, so we copy the driver
// shape from `pool_tests.rs`. Keep this in sync with that file.

struct MockConnection;

#[async_trait]
impl Connection for MockConnection {
    async fn execute(&mut self, _query: &Query) -> Result<QueryResult, DriverError> {
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
        "mock"
    }
}

struct MockDriver {
    connect_count: AtomicU64,
}

impl MockDriver {
    fn new() -> Self {
        Self {
            connect_count: AtomicU64::new(0),
        }
    }

    fn connect_count(&self) -> u64 {
        self.connect_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl DatabaseDriver for MockDriver {
    fn name(&self) -> &str {
        "mock"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        self.connect_count.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(MockConnection))
    }
}

// ─── Helpers ───────────────────────────────────────────────────────

fn ds_name() -> &'static str {
    "mock-ds"
}

fn ping_dv() -> DataViewConfig {
    DataViewConfig {
        name: "ping".into(),
        datasource: ds_name().into(),
        query: Some("SELECT 1".into()),
        parameters: vec![],
        return_schema: None,
        invalidates: vec![],
        validate_result: false,
        strict_parameters: false,
        caching: None,
        get_query: None,
        post_query: None,
        put_query: None,
        delete_query: None,
        get_schema: None,
        post_schema: None,
        put_schema: None,
        delete_schema: None,
        get_parameters: Vec::new(),
        post_parameters: Vec::new(),
        put_parameters: Vec::new(),
        delete_parameters: Vec::new(),
        streaming: false,
        circuit_breaker_id: None,
        prepared: false,
        query_params: HashMap::new(),
        max_rows: 1000,
    }
}

fn mock_params() -> ConnectionParams {
    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "mock".to_string());
    ConnectionParams {
        host: String::new(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options: opts,
    }
}

async fn build_executor_and_pool(
    cfg: PoolConfig,
) -> (DataViewExecutor, Arc<PoolManager>, Arc<MockDriver>) {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let driver_dyn: Arc<dyn DatabaseDriver> = driver.clone();

    let manager = Arc::new(PoolManager::new());
    manager
        .ensure_pool(
            ds_name(),
            cfg,
            driver_dyn.clone(),
            mock_params(),
            event_bus,
        )
        .await;

    let mut factory = DriverFactory::new();
    factory.register_database_driver(driver_dyn);

    let mut params_map = HashMap::new();
    params_map.insert(ds_name().to_string(), mock_params());

    let mut registry = DataViewRegistry::new();
    registry.register(ping_dv());

    let mut executor = DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(params_map),
        Arc::new(NoopDataViewCache),
    );
    let handle: SharedPoolHandle = manager.clone();
    executor.set_pool_manager(handle);

    (executor, manager, driver)
}

// ─── Tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn dataview_reuses_pool_connection_across_calls() {
    let (executor, manager, driver) =
        build_executor_and_pool(PoolConfig::default()).await;

    for _ in 0..50 {
        executor
            .execute("ping", HashMap::new(), "GET", "trace-1", None)
            .await
            .expect("dataview execute");
        // The pool's release path runs in a tokio::spawn (because the
        // sync ReleaseToken trait can't await). Yield so the spawned
        // release lands back in `idle` before the next acquire — without
        // this yield, sequential calls would create up to `max_size`
        // connections before any reuse could happen.
        tokio::task::yield_now().await;
    }

    let snaps = manager.snapshots().await;
    let ds = snaps
        .iter()
        .find(|s| s.datasource_id == ds_name())
        .expect("snapshot for mock-ds");

    // With sequential calls and a yield between each, the pool should
    // reuse aggressively — far fewer than 50 distinct connections.
    // Without pool routing, connect_count would be 50.
    assert!(
        driver.connect_count() <= 5,
        "pool created {} connections in 50 sequential calls; expected <= 5",
        driver.connect_count()
    );
    assert!(
        ds.total_connections <= ds.max_size,
        "total_connections ({}) must not exceed max_size ({})",
        ds.total_connections,
        ds.max_size,
    );
    assert_eq!(ds.checkout_count, 50);
}

#[tokio::test]
async fn dataview_evicts_connection_past_max_lifetime() {
    let cfg = PoolConfig {
        max_lifetime_ms: 50,
        idle_timeout_ms: 60_000,
        ..Default::default()
    };
    let (executor, _manager, driver) = build_executor_and_pool(cfg).await;

    executor
        .execute("ping", HashMap::new(), "GET", "t1", None)
        .await
        .expect("first execute");
    let connects_after_first = driver.connect_count();
    assert_eq!(connects_after_first, 1);

    // Wait past max_lifetime so the just-released connection ages out.
    // Margin is >2x max_lifetime_ms to avoid flakiness.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    executor
        .execute("ping", HashMap::new(), "GET", "t2", None)
        .await
        .expect("second execute");
    assert_eq!(
        driver.connect_count(),
        2,
        "second execute should have created a fresh connection after max_lifetime expired"
    );
}

#[tokio::test]
async fn dataview_unpooled_datasource_falls_through_to_factory() {
    // Build an executor whose pool manager has NO pool registered for the
    // configured datasource. acquire() returns Ok(None); execute() must
    // fall back to factory.connect — semantically the same path used today
    // by broker datasources for produce dispatch.
    let driver = Arc::new(MockDriver::new());
    let driver_dyn: Arc<dyn DatabaseDriver> = driver.clone();

    // Pool manager exists but is empty — no ensure_pool call.
    let manager: Arc<PoolManager> = Arc::new(PoolManager::new());

    let mut factory = DriverFactory::new();
    factory.register_database_driver(driver_dyn);

    let mut params_map = HashMap::new();
    params_map.insert(ds_name().to_string(), mock_params());

    let mut registry = DataViewRegistry::new();
    registry.register(ping_dv());

    let mut executor = DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(params_map),
        Arc::new(NoopDataViewCache),
    );
    let handle: SharedPoolHandle = manager.clone();
    executor.set_pool_manager(handle);

    executor
        .execute("ping", HashMap::new(), "GET", "trace-fallthrough", None)
        .await
        .expect("dataview execute via fallthrough");

    // Each call without a pool produces a fresh factory.connect since no
    // reuse path exists. Single call → exactly one connect.
    assert_eq!(driver.connect_count(), 1);

    // Pool manager still has zero pools.
    assert!(manager.snapshots().await.is_empty());
}
