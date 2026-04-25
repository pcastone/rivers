//! Per-datasource connection pooling with circuit breaker and health checks.
//!
//! Per `rivers-data-layer-spec.md` §5.
//!
//! Each datasource gets its own `ConnectionPool`. The pool manages idle
//! connections, enforces max lifetime, runs periodic health checks, and
//! integrates a circuit breaker that short-circuits `acquire()` when the
//! datasource is unresponsive.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify, RwLock};
use tracing;

use rivers_runtime::rivers_core::event::Event;
use rivers_runtime::rivers_core::eventbus::{events, EventBus};
use rivers_runtime::rivers_driver_sdk::traits::{Connection, ConnectionParams, DatabaseDriver};

// ── PoolError ──────────────────────────────────────────────────────

/// Errors from the connection pool.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// The circuit breaker is open for the given datasource.
    #[error("circuit breaker is open for datasource '{datasource}'")]
    CircuitOpen {
        /// Datasource that triggered the circuit open.
        datasource: String,
    },

    /// Timed out waiting for an available connection.
    #[error("connection timeout after {timeout_ms}ms for datasource '{datasource}'")]
    Timeout {
        /// Datasource the timeout occurred on.
        datasource: String,
        /// Elapsed timeout in milliseconds.
        timeout_ms: u64,
    },

    /// Pool is draining and rejecting new checkouts.
    #[error("pool is draining, no new checkouts for datasource '{datasource}'")]
    Draining {
        /// Datasource whose pool is draining.
        datasource: String,
    },

    /// Error propagated from the underlying driver.
    #[error("driver error: {0}")]
    Driver(#[from] rivers_runtime::rivers_driver_sdk::error::DriverError),

    /// Invalid pool configuration.
    #[error("pool configuration error: {0}")]
    Config(String),
}

// ── PoolGuard (RAII connection release) ────────────────────────────

/// RAII guard for pool connections.
///
/// Automatically returns the connection to the pool's idle queue when dropped,
/// preserving prepared statement caches across checkouts. If the pool is
/// draining, the connection is discarded instead.
pub struct PoolGuard {
    active_count: Arc<AtomicU64>,
    draining: Arc<AtomicBool>,
    idle_return: Arc<StdMutex<VecDeque<PooledConnection>>>,
    notify: Arc<Notify>,
    /// Held connection — returned to idle on drop.
    conn: Option<Box<dyn Connection>>,
    /// Original creation time — preserved across guard drops so
    /// `max_lifetime_ms` is enforceable. Per code-review P1-1.
    created_at: Instant,
}

impl PoolGuard {
    /// Create a guard for a checked-out connection.
    fn new(
        conn: Box<dyn Connection>,
        active_count: Arc<AtomicU64>,
        draining: Arc<AtomicBool>,
        idle_return: Arc<StdMutex<VecDeque<PooledConnection>>>,
        notify: Arc<Notify>,
        created_at: Instant,
    ) -> Self {
        Self {
            active_count,
            draining,
            idle_return,
            notify,
            conn: Some(conn),
            created_at,
        }
    }

    /// Get a mutable reference to the underlying connection.
    pub fn conn_mut(&mut self) -> &mut Box<dyn Connection> {
        self.conn.as_mut().expect("connection already taken")
    }

    /// Take the connection out of the guard (transfers ownership to the caller).
    ///
    /// The connection is NOT returned to idle — the caller takes full
    /// ownership and is responsible for its lifecycle. `active_count` is
    /// decremented here since Drop will not run.
    pub fn take(mut self) -> Box<dyn Connection> {
        let conn = self.conn.take().expect("connection already taken");
        // Decrement active count before forgetting self so the pool stays consistent.
        self.active_count.fetch_sub(1, Ordering::Relaxed);
        self.notify.notify_one();
        std::mem::forget(self);
        conn
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.active_count.fetch_sub(1, Ordering::Relaxed);

            if self.draining.load(Ordering::Relaxed) {
                // Pool is draining — discard the connection.
                self.notify.notify_waiters();
                return;
            }

            // P1-1: preserve original created_at so max_lifetime_ms triggers.
            // last_used is reset to now so the freshly-returned connection
            // gets its full idle_timeout window before eviction.
            let pooled = PooledConnection {
                conn,
                created_at: self.created_at,
                last_used: Instant::now(),
            };
            if let Ok(mut queue) = self.idle_return.lock() {
                queue.push_back(pooled);
            }
            // Notify any waiters that a connection is available.
            self.notify.notify_one();
        }
    }
}

// ── PoolConfig ─────────────────────────────────────────────────────

/// Per-datasource pool configuration.
///
/// Per spec §5.1.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of connections in the pool.
    pub max_size: usize,
    /// Minimum number of idle connections to maintain.
    pub min_idle: usize,
    /// Timeout for acquiring a connection (ms).
    pub connection_timeout_ms: u64,
    /// Idle connections older than this are removed (ms).
    pub idle_timeout_ms: u64,
    /// Maximum lifetime of any connection (ms).
    pub max_lifetime_ms: u64,
    /// Health check interval (ms).
    pub health_check_interval_ms: u64,
    /// Circuit breaker configuration.
    pub circuit_breaker: CircuitBreakerConfig,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 10,
            min_idle: 0,
            connection_timeout_ms: 500,
            idle_timeout_ms: 30_000,
            max_lifetime_ms: 300_000,
            health_check_interval_ms: 5_000,
            circuit_breaker: CircuitBreakerConfig::default(),
        }
    }
}

/// Validate a pool configuration.
///
/// Returns a list of validation errors (empty = valid).
pub fn validate_pool_config(config: &PoolConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if config.max_size == 0 {
        errors.push("max_size must be at least 1".into());
    }
    if config.min_idle > config.max_size {
        errors.push(format!(
            "min_idle ({}) must not exceed max_size ({})",
            config.min_idle, config.max_size
        ));
    }
    if config.connection_timeout_ms == 0 {
        errors.push("connection_timeout_ms must be greater than 0".into());
    }
    if config.idle_timeout_ms == 0 {
        errors.push("idle_timeout_ms must be greater than 0".into());
    }
    if config.max_lifetime_ms == 0 {
        errors.push("max_lifetime_ms must be greater than 0".into());
    }
    if config.health_check_interval_ms == 0 {
        errors.push("health_check_interval_ms must be greater than 0".into());
    }
    errors
}

// ── CircuitBreakerConfig ───────────────────────────────────────────

/// Circuit breaker configuration.
///
/// Per SHAPE-1: windowed failure counting (not consecutive).
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Whether the circuit breaker is enabled.
    pub enabled: bool,
    /// Failures within window before opening the circuit.
    pub failure_threshold: u32,
    /// Rolling failure window (ms). Default: 60_000.
    pub window_ms: u64,
    /// Time in OPEN state before attempting HALF_OPEN (ms).
    pub open_timeout_ms: u64,
    /// Maximum trial calls allowed in HALF_OPEN state.
    pub half_open_max_trials: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            failure_threshold: 5,
            window_ms: 60_000,
            open_timeout_ms: 30_000,
            half_open_max_trials: 1,
        }
    }
}

/// Convert datasource config CB to pool CB config.
impl From<&rivers_runtime::datasource::CircuitBreakerConfig> for CircuitBreakerConfig {
    fn from(ds: &rivers_runtime::datasource::CircuitBreakerConfig) -> Self {
        Self {
            enabled: ds.enabled,
            failure_threshold: ds.failure_threshold,
            window_ms: ds.window_ms,
            open_timeout_ms: ds.open_timeout_ms,
            half_open_max_trials: ds.half_open_max_trials,
        }
    }
}

// ── CircuitBreaker ─────────────────────────────────────────────────

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is healthy; all requests pass through.
    Closed,
    /// Circuit has tripped; requests are rejected.
    Open,
    /// Circuit is testing recovery with limited trial requests.
    HalfOpen,
}

/// Circuit breaker state machine with rolling window failure counting.
///
/// Per SHAPE-1:
/// - CLOSED → (failure_threshold failures within window_ms) → OPEN
/// - OPEN → (open_timeout_ms elapsed) → HALF_OPEN
/// - HALF_OPEN → (trial succeeds) → CLOSED
/// - HALF_OPEN → (trial fails) → OPEN
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: CircuitState,
    /// Rolling window of failure timestamps.
    failure_times: VecDeque<Instant>,
    last_failure_time: Option<Instant>,
    half_open_trials: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker in the closed state.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: CircuitState::Closed,
            failure_times: VecDeque::new(),
            last_failure_time: None,
            half_open_trials: 0,
        }
    }

    /// Evict failure timestamps outside the rolling window.
    fn evict_expired(&mut self) {
        let window = Duration::from_millis(self.config.window_ms);
        let now = Instant::now();
        while let Some(&front) = self.failure_times.front() {
            if now.duration_since(front) >= window {
                self.failure_times.pop_front();
            } else {
                break;
            }
        }
    }

    /// Check if a request is allowed through the circuit breaker.
    pub fn allow_request(&mut self) -> bool {
        if !self.config.enabled {
            return true;
        }

        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = self.last_failure_time {
                    if last_failure.elapsed()
                        >= Duration::from_millis(self.config.open_timeout_ms)
                    {
                        self.state = CircuitState::HalfOpen;
                        self.half_open_trials = 0;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                self.half_open_trials < self.config.half_open_max_trials
            }
        }
    }

    /// Record a successful operation.
    pub fn record_success(&mut self) {
        if !self.config.enabled {
            return;
        }

        match self.state {
            CircuitState::HalfOpen => {
                self.state = CircuitState::Closed;
                self.failure_times.clear();
                self.half_open_trials = 0;
            }
            CircuitState::Closed => {
                // Success doesn't clear window — only expiry does
            }
            CircuitState::Open => {
                self.failure_times.clear();
            }
        }
    }

    /// Record a failed operation. Returns `true` if the circuit just opened.
    pub fn record_failure(&mut self) -> bool {
        if !self.config.enabled {
            return false;
        }

        let now = Instant::now();
        self.last_failure_time = Some(now);

        match self.state {
            CircuitState::Closed => {
                self.failure_times.push_back(now);
                self.evict_expired();
                if self.failure_times.len() >= self.config.failure_threshold as usize {
                    self.state = CircuitState::Open;
                    return true;
                }
                false
            }
            CircuitState::HalfOpen => {
                self.half_open_trials += 1;
                self.state = CircuitState::Open;
                true
            }
            CircuitState::Open => false,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Get the number of failures within the current window.
    pub fn failures_in_window(&mut self) -> u32 {
        self.evict_expired();
        self.failure_times.len() as u32
    }
}

// ── PoolSnapshot ───────────────────────────────────────────────────

/// Health snapshot for a connection pool.
///
/// Per spec §5.4 — accessible via admin `/status` endpoint.
#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    /// Identifier of the datasource this snapshot belongs to.
    pub datasource_id: String,
    /// Number of connections currently checked out.
    pub active_connections: usize,
    /// Number of connections sitting idle in the pool.
    pub idle_connections: usize,
    /// Sum of active and idle connections.
    pub total_connections: usize,
    /// Cumulative number of successful checkouts.
    pub checkout_count: u64,
    /// Average wait time per checkout in milliseconds.
    pub avg_wait_ms: u64,
    /// Configured maximum pool size.
    pub max_size: usize,
    /// Configured minimum idle connections.
    pub min_idle: usize,
}

// ── PooledConnection ───────────────────────────────────────────────

/// A connection with metadata for pool management.
struct PooledConnection {
    conn: Box<dyn Connection>,
    created_at: Instant,
    last_used: Instant,
}

// ── ConnectionPool ─────────────────────────────────────────────────

/// Per-datasource connection pool.
///
/// Per spec §5. Manages idle connections, enforces max lifetime and idle
/// timeout, integrates circuit breaker, and supports health checks.
pub struct ConnectionPool {
    datasource_id: String,
    config: PoolConfig,
    driver: Arc<dyn DatabaseDriver>,
    params: ConnectionParams,
    idle: Mutex<VecDeque<PooledConnection>>,
    /// Sync queue for connections returned via `PoolGuard::drop()`.
    /// Drained into `idle` on the next `try_get_idle_with_meta()` call.
    idle_return: Arc<StdMutex<VecDeque<PooledConnection>>>,
    active_count: Arc<AtomicU64>,
    checkout_count: AtomicU64,
    total_wait_ms: AtomicU64,
    circuit_breaker: Mutex<CircuitBreaker>,
    event_bus: Arc<EventBus>,
    draining: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ConnectionPool {
    /// Create a new connection pool.
    pub fn new(
        datasource_id: impl Into<String>,
        config: PoolConfig,
        driver: Arc<dyn DatabaseDriver>,
        params: ConnectionParams,
        event_bus: Arc<EventBus>,
    ) -> Self {
        let cb_config = config.circuit_breaker.clone();
        Self {
            datasource_id: datasource_id.into(),
            config,
            driver,
            params,
            idle: Mutex::new(VecDeque::new()),
            idle_return: Arc::new(StdMutex::new(VecDeque::new())),
            active_count: Arc::new(AtomicU64::new(0)),
            checkout_count: AtomicU64::new(0),
            total_wait_ms: AtomicU64::new(0),
            circuit_breaker: Mutex::new(CircuitBreaker::new(cb_config)),
            event_bus,
            draining: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Acquire a connection from the pool.
    ///
    /// 1. Check circuit breaker
    /// 2. Try to get an idle connection (evicting expired ones)
    /// 3. If none available and under max_size, create a new one
    /// 4. If at max_size, wait up to connection_timeout_ms
    ///
    /// Thin wrapper over `acquire_with_meta` that drops the timestamp.
    /// Prefer `acquire_with_meta` when wrapping the connection in a
    /// `PoolGuard`, so the original creation time is preserved across
    /// guard drops (per code-review P1-1).
    pub async fn acquire(&self) -> Result<Box<dyn Connection>, PoolError> {
        self.acquire_with_meta().await.map(|(c, _)| c)
    }

    /// Acquire a connection along with its original `created_at`.
    ///
    /// The timestamp must be threaded into a `PoolGuard` (see `guard()`)
    /// so `max_lifetime_ms` is enforceable across release/reacquire
    /// cycles. Per code-review CR-P1-1.
    pub async fn acquire_with_meta(
        &self,
    ) -> Result<(Box<dyn Connection>, Instant), PoolError> {
        if self.draining.load(Ordering::Relaxed) {
            return Err(PoolError::Draining {
                datasource: self.datasource_id.clone(),
            });
        }

        // Check circuit breaker
        {
            let mut cb = self.circuit_breaker.lock().await;
            if !cb.allow_request() {
                return Err(PoolError::CircuitOpen {
                    datasource: self.datasource_id.clone(),
                });
            }
        }

        let start = Instant::now();
        let deadline = start + Duration::from_millis(self.config.connection_timeout_ms);

        loop {
            // Try to get an idle connection (with its preserved created_at)
            if let Some((conn, created_at)) = self.try_get_idle_with_meta().await {
                let wait_ms = start.elapsed().as_millis() as u64;
                self.checkout_count.fetch_add(1, Ordering::Relaxed);
                self.total_wait_ms.fetch_add(wait_ms, Ordering::Relaxed);
                self.active_count.fetch_add(1, Ordering::Relaxed);

                let mut cb = self.circuit_breaker.lock().await;
                cb.record_success();
                return Ok((conn, created_at));
            }

            // Try to create a new connection if under max_size
            let total = self.active_count.load(Ordering::Relaxed) as usize
                + self.idle.lock().await.len();
            if total < self.config.max_size {
                match self.create_connection().await {
                    Ok(conn) => {
                        let wait_ms = start.elapsed().as_millis() as u64;
                        self.checkout_count.fetch_add(1, Ordering::Relaxed);
                        self.total_wait_ms.fetch_add(wait_ms, Ordering::Relaxed);
                        self.active_count.fetch_add(1, Ordering::Relaxed);

                        let mut cb = self.circuit_breaker.lock().await;
                        cb.record_success();
                        return Ok((conn, Instant::now()));
                    }
                    Err(e) => {
                        let mut cb = self.circuit_breaker.lock().await;
                        let opened = cb.record_failure();
                        if opened {
                            let event = Event::new(
                                events::DATASOURCE_CIRCUIT_OPENED,
                                serde_json::json!({
                                    "datasource": self.datasource_id,
                                }),
                            );
                            let _ = self.event_bus.publish(&event).await;
                        }
                        return Err(PoolError::Driver(e));
                    }
                }
            }

            // At max_size — wait for a connection to be released
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PoolError::Timeout {
                    datasource: self.datasource_id.clone(),
                    timeout_ms: self.config.connection_timeout_ms,
                });
            }

            // Wait for notification or timeout
            tokio::select! {
                _ = self.notify.notified() => {
                    // A connection was released — loop back to try again
                }
                _ = tokio::time::sleep(remaining) => {
                    return Err(PoolError::Timeout {
                        datasource: self.datasource_id.clone(),
                        timeout_ms: self.config.connection_timeout_ms,
                    });
                }
            }
        }
    }

    /// Release a connection back to the pool.
    ///
    /// If the connection has exceeded max_lifetime, it is dropped instead.
    pub async fn release(&self, conn: Box<dyn Connection>, created_at: Option<Instant>) {
        self.active_count.fetch_sub(1, Ordering::Relaxed);

        if self.draining.load(Ordering::Relaxed) {
            // Drop the connection during drain
            self.notify.notify_waiters();
            return;
        }

        let pooled = PooledConnection {
            conn,
            created_at: created_at.unwrap_or_else(Instant::now),
            last_used: Instant::now(),
        };

        let mut idle = self.idle.lock().await;
        idle.push_back(pooled);

        // Notify waiters that a connection is available
        drop(idle);
        self.notify.notify_one();
    }

    /// Try to get a valid idle connection, evicting expired ones.
    ///
    /// Drains any connections returned via `PoolGuard::drop()` (the sync
    /// `idle_return` queue) into the main idle queue first. Returns the
    /// connection alongside its original `created_at` so callers can
    /// thread it into a `PoolGuard` (see `acquire_with_meta`).
    async fn try_get_idle_with_meta(&self) -> Option<(Box<dyn Connection>, Instant)> {
        let mut idle = self.idle.lock().await;

        // Drain connections returned synchronously by PoolGuard::drop().
        if let Ok(mut returned) = self.idle_return.try_lock() {
            while let Some(conn) = returned.pop_front() {
                idle.push_back(conn);
            }
        }

        let now = Instant::now();

        while let Some(pooled) = idle.pop_front() {
            // Check max lifetime
            if now.duration_since(pooled.created_at)
                >= Duration::from_millis(self.config.max_lifetime_ms)
            {
                continue; // drop expired connection
            }
            // Check idle timeout
            if now.duration_since(pooled.last_used)
                >= Duration::from_millis(self.config.idle_timeout_ms)
            {
                continue; // drop idle-timed-out connection
            }
            return Some((pooled.conn, pooled.created_at));
        }
        None
    }

    /// Create a new connection via the driver.
    async fn create_connection(
        &self,
    ) -> Result<Box<dyn Connection>, rivers_runtime::rivers_driver_sdk::error::DriverError> {
        self.driver.connect(&self.params).await
    }

    /// Get a snapshot of pool health.
    pub async fn snapshot(&self) -> PoolSnapshot {
        let idle_count = self.idle.lock().await.len();
        let active = self.active_count.load(Ordering::Relaxed) as usize;
        let checkouts = self.checkout_count.load(Ordering::Relaxed);
        let total_wait = self.total_wait_ms.load(Ordering::Relaxed);

        PoolSnapshot {
            datasource_id: self.datasource_id.clone(),
            active_connections: active,
            idle_connections: idle_count,
            total_connections: active + idle_count,
            checkout_count: checkouts,
            avg_wait_ms: if checkouts > 0 {
                total_wait / checkouts
            } else {
                0
            },
            max_size: self.config.max_size,
            min_idle: self.config.min_idle,
        }
    }

    /// Run health checks on idle connections.
    ///
    /// Pings each idle connection; removes those that fail.
    /// Emits DatasourceHealthCheckFailed if all idle connections fail.
    pub async fn health_check(&self) {
        let mut idle = self.idle.lock().await;
        let mut healthy = VecDeque::new();
        let mut failures = 0usize;
        let total = idle.len();

        while let Some(mut pooled) = idle.pop_front() {
            match pooled.conn.ping().await {
                Ok(()) => {
                    pooled.last_used = Instant::now();
                    healthy.push_back(pooled);
                }
                Err(e) => {
                    failures += 1;
                    tracing::warn!(
                        datasource = %self.datasource_id,
                        error = %e,
                        "health check failed, removing connection"
                    );
                }
            }
        }

        *idle = healthy;

        if failures > 0 && total > 0 && failures == total {
            drop(idle);
            let event = Event::new(
                events::DATASOURCE_HEALTH_CHECK_FAILED,
                serde_json::json!({
                    "datasource": self.datasource_id,
                    "failed": failures,
                }),
            );
            let _ = self.event_bus.publish(&event).await;
        }
    }

    /// Start draining the pool — no new checkouts, active connections
    /// complete their current operations.
    pub fn start_drain(&self) {
        self.draining.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    /// Check if the pool is fully drained (no active connections).
    pub fn is_drained(&self) -> bool {
        self.draining.load(Ordering::Relaxed)
            && self.active_count.load(Ordering::Relaxed) == 0
    }

    /// Drain the pool, dropping all idle connections.
    pub async fn drain(&self) {
        self.start_drain();
        let mut idle = self.idle.lock().await;
        idle.clear();
    }

    /// Wrap a checked-out connection in a `PoolGuard`.
    ///
    /// `created_at` should be the connection's original creation time — for
    /// fresh connections from `create_connection`, pass `Instant::now()`; for
    /// connections retrieved from idle, pass the stored `created_at` (use
    /// `acquire_with_meta` to obtain it). Per code-review P1-1, preserving
    /// this lets `max_lifetime_ms` actually fire after a guard drop.
    ///
    /// The guard will return the connection to the idle queue on drop,
    /// preserving prepared statement caches.
    pub fn guard(&self, conn: Box<dyn Connection>, created_at: Instant) -> PoolGuard {
        PoolGuard::new(
            conn,
            Arc::clone(&self.active_count),
            Arc::clone(&self.draining),
            Arc::clone(&self.idle_return),
            Arc::clone(&self.notify),
            created_at,
        )
    }

    /// Get the datasource ID.
    pub fn datasource_id(&self) -> &str {
        &self.datasource_id
    }

    /// Get the pool config.
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }
}

// ── Health check task ──────────────────────────────────────────────

/// Spawn a background task that runs health checks at the configured interval.
///
/// Returns a `JoinHandle` that can be aborted on shutdown.
pub fn spawn_health_check_task(
    pool: Arc<ConnectionPool>,
) -> tokio::task::JoinHandle<()> {
    let interval_ms = pool.config().health_check_interval_ms;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            interval.tick().await;
            if pool.draining.load(Ordering::Relaxed) {
                return;
            }
            pool.health_check().await;
        }
    })
}

// ── Pool Manager ───────────────────────────────────────────────────

/// Manages all connection pools for all datasources.
///
/// Per spec §5.3 — each datasource has its own pool.
pub struct PoolManager {
    pools: RwLock<Vec<Arc<ConnectionPool>>>,
}

impl PoolManager {
    /// Create a new empty pool manager.
    pub fn new() -> Self {
        Self {
            pools: RwLock::new(Vec::new()),
        }
    }

    /// Add a pool to the manager.
    pub async fn add_pool(&self, pool: Arc<ConnectionPool>) {
        let mut pools = self.pools.write().await;
        pools.push(pool);
    }

    /// Get or create a pool for a datasource.
    ///
    /// If a pool with `datasource_id` already exists, returns the existing
    /// pool unchanged (config/driver args are ignored). Otherwise creates,
    /// registers, and returns a fresh pool. Idempotent — safe to call on
    /// every bundle load / hot reload.
    pub async fn ensure_pool(
        &self,
        datasource_id: &str,
        config: PoolConfig,
        driver: Arc<dyn DatabaseDriver>,
        params: ConnectionParams,
        event_bus: Arc<EventBus>,
    ) -> Arc<ConnectionPool> {
        {
            let pools = self.pools.read().await;
            if let Some(p) = pools.iter().find(|p| p.datasource_id() == datasource_id) {
                return Arc::clone(p);
            }
        }
        let mut pools = self.pools.write().await;
        // Re-check after acquiring write lock to avoid race.
        if let Some(p) = pools.iter().find(|p| p.datasource_id() == datasource_id) {
            return Arc::clone(p);
        }
        let pool = Arc::new(ConnectionPool::new(
            datasource_id.to_string(),
            config,
            driver,
            params,
            event_bus,
        ));
        pools.push(Arc::clone(&pool));
        pool
    }

    /// Get a pool by datasource ID.
    pub async fn get_pool(&self, datasource_id: &str) -> Option<Arc<ConnectionPool>> {
        let pools = self.pools.read().await;
        pools
            .iter()
            .find(|p| p.datasource_id() == datasource_id)
            .cloned()
    }

    /// Get snapshots of all pools.
    pub async fn snapshots(&self) -> Vec<PoolSnapshot> {
        let pools = self.pools.read().await;
        let mut snapshots = Vec::with_capacity(pools.len());
        for pool in pools.iter() {
            snapshots.push(pool.snapshot().await);
        }
        snapshots
    }

    /// Drain all pools on shutdown.
    pub async fn drain_all(&self) {
        let pools = self.pools.read().await;
        for pool in pools.iter() {
            pool.drain().await;
        }
    }
}

impl Default for PoolManager {
    fn default() -> Self {
        Self::new()
    }
}
