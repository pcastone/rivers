//! Pool Manager tests — config validation, circuit breaker, acquire/release,
//! pool snapshot, health check, drain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    connect_count: AtomicU64,
}

impl MockDriver {
    fn new() -> Self {
        Self {
            fail_connect: Mutex::new(false),
            connect_count: AtomicU64::new(0),
        }
    }

    fn failing() -> Self {
        Self {
            fail_connect: Mutex::new(true),
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
        if *self.fail_connect.lock().await {
            Err(DriverError::Connection("connection refused".into()))
        } else {
            self.connect_count.fetch_add(1, Ordering::Relaxed);
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

    let conn = pool.acquire().await;
    assert!(conn.is_ok(), "should acquire a connection");

    let snap = pool.snapshot().await;
    assert_eq!(snap.active_connections, 1);
}

#[tokio::test]
async fn pool_release_returns_connection() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("test-ds", PoolConfig::default(), driver, test_params(), event_bus);

    let conn = pool.acquire().await.unwrap();
    pool.release(conn, None).await;

    let snap = pool.snapshot().await;
    assert_eq!(snap.active_connections, 0);
    assert_eq!(snap.idle_connections, 1);
}

#[tokio::test]
async fn pool_reuses_idle_connection() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = ConnectionPool::new("test-ds", PoolConfig::default(), driver, test_params(), event_bus);

    let conn = pool.acquire().await.unwrap();
    pool.release(conn, None).await;

    // Second acquire should reuse the idle connection
    let conn2 = pool.acquire().await;
    assert!(conn2.is_ok());

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

    let conn = pool.acquire().await.unwrap();
    let snap = pool.snapshot().await;

    assert_eq!(snap.datasource_id, "snap-ds");
    assert_eq!(snap.active_connections, 1);
    assert_eq!(snap.idle_connections, 0);
    assert_eq!(snap.total_connections, 1);
    assert_eq!(snap.checkout_count, 1);
    assert_eq!(snap.max_size, 10);
    assert_eq!(snap.min_idle, 0);

    pool.release(conn, None).await;
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

    let conn = pool.acquire().await.unwrap();
    pool.release(conn, None).await;

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
    manager.add_pool(pool).await;

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
    manager.add_pool(pool).await;

    let snaps = manager.snapshots().await;
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].datasource_id, "snap-mgr");
}

#[tokio::test]
async fn pool_manager_drain_all() {
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let pool = Arc::new(ConnectionPool::new("drain-mgr", PoolConfig::default(), driver, test_params(), event_bus));

    let conn = pool.acquire().await.unwrap();
    pool.release(conn, None).await;

    let manager = PoolManager::new();
    manager.add_pool(pool.clone()).await;

    manager.drain_all().await;
    assert!(pool.is_drained());

    let snap = pool.snapshot().await;
    assert_eq!(snap.idle_connections, 0);
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

// ── PoolGuard Lifetime Accounting (CR-P1-1) ───────────────────────

#[tokio::test]
async fn pool_guard_preserves_created_at_across_drop() {
    use std::time::{Duration, Instant};
    let event_bus = Arc::new(EventBus::new());
    let driver = Arc::new(MockDriver::new());
    let mut cfg = PoolConfig::default();
    cfg.max_lifetime_ms = 50;
    cfg.idle_timeout_ms = 60_000;
    let pool = Arc::new(ConnectionPool::new(
        "lifetime-ds",
        cfg,
        driver.clone(),
        test_params(),
        event_bus,
    ));

    // First acquire creates the connection.
    let conn = pool.acquire().await.unwrap();
    let guard = pool.guard(conn, Instant::now());

    // Hold the guard long enough that the original connection has already
    // exceeded max_lifetime_ms by the time it's released back to idle.
    tokio::time::sleep(Duration::from_millis(60)).await;
    drop(guard); // returns to idle via PoolGuard::Drop

    // Tiny sleep so the next acquire is well within the (reset) drop time
    // window — the bug, if present, would let the connection look "fresh"
    // and be reused. With the fix, original created_at is preserved and
    // try_get_idle evicts it.
    tokio::time::sleep(Duration::from_millis(5)).await;

    // Next acquire should NOT reuse the aged-out connection — it should
    // create a new one. MockDriver counts connect() calls.
    let _conn2 = pool.acquire().await.unwrap();
    assert_eq!(
        driver.connect_count(),
        2,
        "max_lifetime should have evicted the dropped guard's connection"
    );
}
