//! Pool Manager tests — config validation, circuit breaker, acquire/release,
//! pool snapshot, health check, drain.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use rivers_runtime::rivers_core::eventbus::EventBus;
use rivers_runtime::rivers_driver_sdk::error::DriverError;
use rivers_runtime::rivers_driver_sdk::traits::{Connection, ConnectionParams, DatabaseDriver};
use rivers_runtime::rivers_driver_sdk::types::{Query, QueryResult};

use riversd::pool::*;

// ── Mock Driver / Connection ──────────────────────────────────────

struct MockConnection {
    healthy: bool,
}

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
        if self.healthy {
            Ok(())
        } else {
            Err(DriverError::Connection("unhealthy".into()))
        }
    }

    fn driver_name(&self) -> &str {
        "mock"
    }
}

struct MockDriver {
    fail_connect: Mutex<bool>,
    /// Total successful `connect` calls — used to assert capacity bounds.
    connect_count: std::sync::atomic::AtomicU64,
    /// Optional artificial delay before returning a connection.
    connect_delay: std::time::Duration,
}

impl MockDriver {
    fn new() -> Self {
        Self {
            fail_connect: Mutex::new(false),
            connect_count: std::sync::atomic::AtomicU64::new(0),
            connect_delay: std::time::Duration::ZERO,
        }
    }

    fn failing() -> Self {
        Self {
            fail_connect: Mutex::new(true),
            connect_count: std::sync::atomic::AtomicU64::new(0),
            connect_delay: std::time::Duration::ZERO,
        }
    }

    fn with_delay(delay: std::time::Duration) -> Self {
        Self {
            fail_connect: Mutex::new(false),
            connect_count: std::sync::atomic::AtomicU64::new(0),
            connect_delay: delay,
        }
    }

    fn connect_count(&self) -> u64 {
        self.connect_count.load(std::sync::atomic::Ordering::Relaxed)
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
        if !self.connect_delay.is_zero() {
            tokio::time::sleep(self.connect_delay).await;
        }
        if *self.fail_connect.lock().await {
            Err(DriverError::Connection("connection refused".into()))
        } else {
            self.connect_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(Box::new(MockConnection { healthy: true }))
        }
    }
}

fn test_params() -> ConnectionParams {
    ConnectionParams {
        host: "localhost".into(),
        port: 5432,
        database: "test".into(),
        username: "user".into(),
        password: "pass".into(),
        options: HashMap::new(),
    }
}

// ── Config Validation ─────────────────────────────────────────────

#[test]
fn validate_default_config() {
    let errors = validate_pool_config(&PoolConfig::default());
    assert!(errors.is_empty(), "default config should be valid: {:?}", errors);
}

#[test]
fn validate_max_size_zero() {
    let config = PoolConfig {
        max_size: 0,
        ..Default::default()
    };
    let errors = validate_pool_config(&config);
    assert!(errors.iter().any(|e| e.contains("max_size")));
}

#[test]
fn validate_min_idle_exceeds_max_size() {
    let config = PoolConfig {
        max_size: 5,
        min_idle: 10,
        ..Default::default()
    };
    let errors = validate_pool_config(&config);
    assert!(errors.iter().any(|e| e.contains("min_idle")));
}

#[test]
fn validate_timeout_zero() {
    let config = PoolConfig {
        connection_timeout_ms: 0,
        ..Default::default()
    };
    let errors = validate_pool_config(&config);
    assert!(errors.iter().any(|e| e.contains("connection_timeout_ms")));
}

#[test]
fn validate_idle_timeout_zero() {
    let config = PoolConfig {
        idle_timeout_ms: 0,
        ..Default::default()
    };
    let errors = validate_pool_config(&config);
    assert!(errors.iter().any(|e| e.contains("idle_timeout_ms")));
}

#[test]
fn validate_max_lifetime_zero() {
    let config = PoolConfig {
        max_lifetime_ms: 0,
        ..Default::default()
    };
    let errors = validate_pool_config(&config);
    assert!(errors.iter().any(|e| e.contains("max_lifetime_ms")));
}

#[test]
fn validate_health_check_interval_zero() {
    let config = PoolConfig {
        health_check_interval_ms: 0,
        ..Default::default()
    };
    let errors = validate_pool_config(&config);
    assert!(errors.iter().any(|e| e.contains("health_check_interval_ms")));
}

// ── Circuit Breaker ───────────────────────────────────────────────

#[test]
fn circuit_breaker_starts_closed() {
    let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[test]
fn circuit_breaker_allows_when_closed() {
    let mut cb = CircuitBreaker::new(CircuitBreakerConfig::default());
    assert!(cb.allow_request());
}

#[test]
fn circuit_breaker_opens_after_threshold() {
    let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 3,
        open_timeout_ms: 1000,
        half_open_max_trials: 1,
        window_ms: 60_000,
    });

    assert!(!cb.record_failure()); // 1
    assert!(!cb.record_failure()); // 2
    assert!(cb.record_failure());  // 3 → opens
    assert_eq!(cb.state(), CircuitState::Open);
    assert!(!cb.allow_request());
}

#[test]
fn circuit_breaker_success_resets_failures() {
    let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 3,
        open_timeout_ms: 1000,
        half_open_max_trials: 1,
        window_ms: 60_000,
    });

    cb.record_failure();
    cb.record_failure();
    cb.record_success(); // reset in half-open/open, but in closed failures stay in window
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[test]
fn circuit_breaker_half_open_success_closes() {
    let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 1,
        open_timeout_ms: 0, // immediately transition
        half_open_max_trials: 1,
        window_ms: 60_000,
    });

    cb.record_failure(); // → Open
    assert_eq!(cb.state(), CircuitState::Open);

    // With open_timeout_ms=0, allow_request should transition to HalfOpen
    std::thread::sleep(std::time::Duration::from_millis(1));
    assert!(cb.allow_request());
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    cb.record_success(); // → Closed
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[test]
fn circuit_breaker_half_open_failure_reopens() {
    let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 1,
        open_timeout_ms: 0,
        half_open_max_trials: 1,
        window_ms: 60_000,
    });

    cb.record_failure(); // → Open
    std::thread::sleep(std::time::Duration::from_millis(1));
    cb.allow_request(); // → HalfOpen

    assert!(cb.record_failure()); // → Open again
    assert_eq!(cb.state(), CircuitState::Open);
}

#[test]
fn circuit_breaker_disabled_always_allows() {
    let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
        enabled: false,
        failure_threshold: 1,
        open_timeout_ms: 1000,
        half_open_max_trials: 1,
        window_ms: 60_000,
    });

    // Even after many failures, requests are allowed
    for _ in 0..10 {
        cb.record_failure();
    }
    assert!(cb.allow_request());
    assert_eq!(cb.state(), CircuitState::Closed);
}

// ── Pool Acquire/Release ──────────────────────────────────────────

#[tokio::test]
async fn pool_acquire_creates_connection() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("test-ds", PoolConfig::default(), driver, test_params(), event_bus);

    let guard = pool.acquire().await;
    assert!(guard.is_ok(), "should acquire a connection");

    let snap = pool.snapshot().await;
    assert_eq!(snap.active_connections, 1);
    // keep guard alive until after snapshot
    drop(guard);
}

#[tokio::test]
async fn pool_release_returns_connection() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("test-ds", PoolConfig::default(), driver, test_params(), event_bus);

    {
        let _guard = pool.acquire().await.unwrap();
    } // drop returns to idle

    let snap = pool.snapshot().await;
    assert_eq!(snap.active_connections, 0);
    assert_eq!(snap.idle_connections, 1);
}

#[tokio::test]
async fn pool_reuses_idle_connection() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("test-ds", PoolConfig::default(), driver, test_params(), event_bus);

    {
        let _g = pool.acquire().await.unwrap();
    }

    // Second acquire should reuse the idle connection
    let g2 = pool.acquire().await;
    assert!(g2.is_ok());
    drop(g2);

    let snap = pool.snapshot().await;
    assert_eq!(snap.checkout_count, 2);
}

#[tokio::test]
async fn pool_acquire_fails_when_circuit_open() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::failing());
    let config = PoolConfig {
        circuit_breaker: CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 1,
            open_timeout_ms: 60_000,
            half_open_max_trials: 1,
            window_ms: 60_000,
        },
        ..Default::default()
    };
    let pool = ConnectionPool::new("test-ds", config, driver, test_params(), event_bus);

    // First acquire fails and opens the circuit
    let result = pool.acquire().await;
    assert!(result.is_err());

    // Second acquire should fail with CircuitOpen
    match pool.acquire().await {
        Err(PoolError::CircuitOpen { .. }) => {} // expected
        Err(e) => panic!("expected CircuitOpen, got: {}", e),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[tokio::test]
async fn pool_acquire_fails_when_draining() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("test-ds", PoolConfig::default(), driver, test_params(), event_bus);

    pool.start_drain();

    match pool.acquire().await {
        Err(PoolError::Draining { .. }) => {} // expected
        Err(e) => panic!("expected Draining, got: {}", e),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ── Snapshot ──────────────────────────────────────────────────────

#[tokio::test]
async fn pool_snapshot() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("snap-ds", PoolConfig::default(), driver, test_params(), event_bus);

    let guard = pool.acquire().await.unwrap();
    let snap = pool.snapshot().await;

    assert_eq!(snap.datasource_id, "snap-ds");
    assert_eq!(snap.active_connections, 1);
    assert_eq!(snap.idle_connections, 0);
    assert_eq!(snap.total_connections, 1);
    assert_eq!(snap.checkout_count, 1);
    assert_eq!(snap.max_size, 10);
    assert_eq!(snap.min_idle, 0);

    drop(guard);
    let snap2 = pool.snapshot().await;
    assert_eq!(snap2.active_connections, 0);
    assert_eq!(snap2.idle_connections, 1);
}

// ── Drain ─────────────────────────────────────────────────────────

#[tokio::test]
async fn pool_drain_clears_idle() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("drain-ds", PoolConfig::default(), driver, test_params(), event_bus);

    {
        let _g = pool.acquire().await.unwrap();
    }

    let snap_before = pool.snapshot().await;
    assert_eq!(snap_before.idle_connections, 1);

    pool.drain().await;

    let snap_after = pool.snapshot().await;
    assert_eq!(snap_after.idle_connections, 0);
    assert!(pool.is_drained());
}

// ── Pool Manager ──────────────────────────────────────────────────

#[tokio::test]
async fn pool_manager_add_and_get() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = Arc::new(ConnectionPool::new("mgr-ds", PoolConfig::default(), driver, test_params(), event_bus));

    let manager = PoolManager::new();
    manager.add_pool(pool).await.expect("add_pool should succeed");

    let found = manager.get_pool("mgr-ds").await;
    assert!(found.is_some());
    assert_eq!(found.unwrap().datasource_id(), "mgr-ds");

    let not_found = manager.get_pool("nonexistent").await;
    assert!(not_found.is_none());
}

#[tokio::test]
async fn pool_manager_snapshots() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = Arc::new(ConnectionPool::new("snap-mgr", PoolConfig::default(), driver, test_params(), event_bus));

    let manager = PoolManager::new();
    manager.add_pool(pool).await.expect("add_pool should succeed");

    let snaps = manager.snapshots().await;
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].datasource_id, "snap-mgr");
}

#[tokio::test]
async fn pool_manager_drain_all() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = Arc::new(ConnectionPool::new("drain-mgr", PoolConfig::default(), driver, test_params(), event_bus));

    {
        let _g = pool.acquire().await.unwrap();
    }

    let manager = PoolManager::new();
    manager.add_pool(pool.clone()).await.expect("add_pool should succeed");

    manager.drain_all().await;
    assert!(pool.is_drained());

    let snap = pool.snapshot().await;
    assert_eq!(snap.idle_connections, 0);
}

// ── D1 regression tests ──────────────────────────────────────────

/// D1.1 regression: a connection's `created_at` must persist across
/// checkouts so `max_lifetime_ms` actually expires it.
///
/// The original bug was in `PoolGuard::drop`, which built a new
/// `PooledConnection` with `created_at: Instant::now()` — effectively
/// resetting the lifetime on every release. This test repeatedly checks
/// the same connection out and back in *within* `max_lifetime_ms`. With
/// the bug, the connection would live indefinitely; with the fix, it
/// must be evicted once total wall time since first create exceeds
/// `max_lifetime_ms`.
#[tokio::test]
async fn d1_1_max_lifetime_expires_across_checkouts() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let config = PoolConfig {
        max_size: 1,
        max_lifetime_ms: 200,
        idle_timeout_ms: 60_000, // long enough not to interfere
        connection_timeout_ms: 1_000,
        ..Default::default()
    };
    let pool = ConnectionPool::new("life-ds", config, driver.clone(), test_params(), event_bus);

    // Repeatedly check out and release. Each cycle's idle period is well
    // under both max_lifetime AND idle_timeout, so the only thing that can
    // evict the connection is the original-create lifetime budget.
    let start = std::time::Instant::now();
    let mut last_count = 0;
    while start.elapsed() < std::time::Duration::from_millis(500) {
        let g = pool.acquire().await.unwrap();
        last_count = driver.connect_count();
        drop(g);
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }

    // After ~500ms with 200ms max_lifetime, the original connection MUST
    // have been evicted at least once → at least 2 distinct creates.
    // Under the bug (created_at reset on every Drop), `last_count` would
    // remain 1 indefinitely.
    assert!(
        last_count >= 2,
        "max_lifetime should have evicted the connection at least once \
         across 500ms with max_lifetime=200ms; got connect_count={}",
        last_count
    );
}

/// D1.2 regression: under burst load the pool must never create more than
/// `max_connections` total connections, even when many concurrent acquires
/// race on capacity.
#[tokio::test]
async fn d1_2_burst_load_respects_max_connections() {
    let event_bus = Arc::new(EventBus::new());
    // Add a small connect delay so concurrent acquires actually race on the
    // create path (otherwise the first creates may finish before the second
    // even checks capacity).
    let driver = Arc::new(MockDriver::with_delay(std::time::Duration::from_millis(20)));
    let config = PoolConfig {
        max_size: 3,
        connection_timeout_ms: 2_000,
        ..Default::default()
    };
    let pool = Arc::new(ConnectionPool::new(
        "burst-ds",
        config,
        driver.clone(),
        test_params(),
        event_bus,
    ));

    // 10 concurrent acquires — only 3 can be active at once.
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let p = pool.clone();
        tasks.push(tokio::spawn(async move {
            let g = p.acquire().await.expect("acquire should succeed");
            // Hold briefly so concurrent acquires really do contend.
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            drop(g);
        }));
    }

    for t in tasks {
        t.await.unwrap();
    }

    assert!(
        driver.connect_count() <= 3,
        "should never create more than max_size=3 connections, got {}",
        driver.connect_count()
    );

    let snap = pool.snapshot().await;
    assert!(snap.total_connections <= 3, "total snapshot must respect max_size");
}

/// D1.3 regression: registering two pools with the same datasource id must
/// fail with a clear `DuplicateDatasource` error.
#[tokio::test]
async fn d1_3_duplicate_datasource_id_rejected() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());

    let pool1 = Arc::new(ConnectionPool::new(
        "dupe-ds",
        PoolConfig::default(),
        driver.clone(),
        test_params(),
        event_bus.clone(),
    ));
    let pool2 = Arc::new(ConnectionPool::new(
        "dupe-ds",
        PoolConfig::default(),
        driver,
        test_params(),
        event_bus,
    ));

    let manager = PoolManager::new();
    manager.add_pool(pool1).await.expect("first registration should succeed");

    match manager.add_pool(pool2).await {
        Err(PoolError::DuplicateDatasource { datasource }) => {
            assert_eq!(datasource, "dupe-ds");
        }
        Err(e) => panic!("expected DuplicateDatasource, got: {}", e),
        Ok(()) => panic!("expected duplicate registration to fail"),
    }
}

/// D1.3 supplementary: `PoolManager::acquire` returns `UnknownDatasource`
/// for an unregistered id (smoke test for the D2 hook).
#[tokio::test]
async fn d1_3_acquire_unknown_datasource() {
    let manager = PoolManager::new();
    match manager.acquire("nope").await {
        Err(PoolError::UnknownDatasource { datasource }) => {
            assert_eq!(datasource, "nope");
        }
        Err(e) => panic!("expected UnknownDatasource, got: {}", e),
        Ok(_) => panic!("expected error for unregistered datasource"),
    }
}

// ── Config Defaults ───────────────────────────────────────────────

#[test]
fn pool_config_defaults() {
    let config = PoolConfig::default();
    assert_eq!(config.max_size, 10);
    assert_eq!(config.min_idle, 0);
    assert_eq!(config.connection_timeout_ms, 500);
    assert_eq!(config.idle_timeout_ms, 30_000);
    assert_eq!(config.max_lifetime_ms, 300_000);
    assert_eq!(config.health_check_interval_ms, 5_000);
}

#[test]
fn circuit_breaker_config_defaults() {
    let config = CircuitBreakerConfig::default();
    assert!(config.enabled);
    assert_eq!(config.failure_threshold, 5);
    assert_eq!(config.open_timeout_ms, 30_000);
    assert_eq!(config.half_open_max_trials, 1);
}
