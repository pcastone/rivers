//! Per-datasource connection pooling with circuit breaker and health checks.
//!
//! Per `rivers-data-layer-spec.md` §5.
//!
//! Each datasource gets its own `ConnectionPool`. The pool manages idle
//! connections, enforces max lifetime, runs periodic health checks, and
//! integrates a circuit breaker that short-circuits `acquire()` when the
//! datasource is unresponsive.
//!
//! ## Internals (post D1)
//!
//! Pool state (`idle` queue + `total` counter) lives behind a single
//! `std::sync::Mutex<PoolState>` so that `PoolGuard::drop` (which is
//! synchronous) and `acquire` (async) share one source of truth.
//! Capacity accounting includes both idle connections and any in-flight
//! create reservations; this prevents over-creation under burst load
//! (D1.2). The mutex is never held across `.await`.
//!
//! `PoolGuard` carries the connection's original `created_at` so
//! `max_lifetime` is enforced across checkouts (D1.1).

use std::collections::{HashMap, VecDeque};
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

    /// No pool registered for the requested datasource id.
    #[error("no pool registered for datasource '{datasource}'")]
    UnknownDatasource {
        /// Datasource id that was requested but not found in the manager.
        datasource: String,
    },

    /// Attempted to register a pool for a datasource id that is already registered.
    #[error("duplicate pool registration for datasource '{datasource}'")]
    DuplicateDatasource {
        /// Datasource id that was duplicated.
        datasource: String,
    },

    /// Error propagated from the underlying driver.
    #[error("driver error: {0}")]
    Driver(#[from] rivers_runtime::rivers_driver_sdk::error::DriverError),

    /// Invalid pool configuration.
    #[error("pool configuration error: {0}")]
    Config(String),
}

// ── PoolState (single source of truth) ─────────────────────────────

/// All mutable bookkeeping for a `ConnectionPool`.
///
/// Protected by a single `std::sync::Mutex` so that synchronous
/// `PoolGuard::drop` and async `acquire` share one accounting view.
/// `total` includes idle connections, checked-out (active) connections,
/// and any in-flight create reservations — see D1.2.
struct PoolState {
    idle: VecDeque<PooledConnection>,
    /// idle.len() + active checkouts + in-flight create reservations.
    /// Always `<= config.max_size`.
    total: usize,
}

impl PoolState {
    fn new() -> Self {
        Self {
            idle: VecDeque::new(),
            total: 0,
        }
    }

    fn active(&self) -> usize {
        self.total.saturating_sub(self.idle.len())
    }
}

// ── PoolGuard (RAII connection release) ────────────────────────────

/// RAII guard for pool connections.
///
/// Automatically returns the connection to the pool's idle queue when dropped,
/// preserving prepared statement caches across checkouts. If the pool is
/// draining or the connection has exceeded `max_lifetime_ms`, it is discarded
/// instead. Carries the connection's original `created_at` so `max_lifetime`
/// is honored across multiple checkouts (D1.1).
pub struct PoolGuard {
    state: Arc<StdMutex<PoolState>>,
    draining: Arc<AtomicBool>,
    notify: Arc<Notify>,
    max_lifetime: Duration,
    /// Original creation instant — preserved across checkouts.
    created_at: Instant,
    /// Held connection — returned to idle on drop.
    conn: Option<Box<dyn Connection>>,
}

impl PoolGuard {
    /// Get a mutable reference to the underlying connection.
    pub fn conn_mut(&mut self) -> &mut Box<dyn Connection> {
        self.conn.as_mut().expect("connection already taken")
    }

    /// Get an immutable reference to the underlying connection.
    pub fn conn(&self) -> &Box<dyn Connection> {
        self.conn.as_ref().expect("connection already taken")
    }

    /// When this guard's underlying connection was originally created.
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Take the connection out of the guard (transfers ownership to the caller).
    ///
    /// The connection is NOT returned to idle — the caller takes full
    /// ownership and is responsible for its lifecycle. The pool's `total`
    /// counter is decremented here since `Drop` will not run.
    pub fn take(mut self) -> Box<dyn Connection> {
        let conn = self.conn.take().expect("connection already taken");
        if let Ok(mut state) = self.state.lock() {
            state.total = state.total.saturating_sub(1);
        }
        self.notify.notify_one();
        std::mem::forget(self);
        conn
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else {
            return;
        };

        // Discard the connection if the pool is draining OR if the
        // connection has outlived its max lifetime.
        let now = Instant::now();
        let expired = now.duration_since(self.created_at) >= self.max_lifetime;
        let draining = self.draining.load(Ordering::Relaxed);

        if let Ok(mut state) = self.state.lock() {
            if draining || expired {
                state.total = state.total.saturating_sub(1);
                drop(state);
                drop(conn);
                if draining {
                    self.notify.notify_waiters();
                } else {
                    self.notify.notify_one();
                }
                return;
            }

            state.idle.push_back(PooledConnection {
                conn,
                created_at: self.created_at,
                last_used: now,
            });
        }
        self.notify.notify_one();
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
    /// Circuit breaker state at the time of snapshot.
    pub circuit_state: CircuitState,
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
    /// Single source of truth for idle queue + capacity accounting (D1.2).
    state: Arc<StdMutex<PoolState>>,
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
            state: Arc::new(StdMutex::new(PoolState::new())),
            checkout_count: AtomicU64::new(0),
            total_wait_ms: AtomicU64::new(0),
            circuit_breaker: Mutex::new(CircuitBreaker::new(cb_config)),
            event_bus,
            draining: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Acquire a connection from the pool, returning a `PoolGuard`.
    ///
    /// 1. Check circuit breaker
    /// 2. Try to get an idle connection (evicting expired ones)
    /// 3. If none available and under `max_size`, reserve a slot and create
    ///    a new connection
    /// 4. Otherwise wait up to `connection_timeout_ms`
    pub async fn acquire(&self) -> Result<PoolGuard, PoolError> {
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
            // Step 1+2: try idle, or reserve a slot to create new — under the
            // single state lock so that idle pop and total++ are atomic w.r.t.
            // capacity accounting (D1.2).
            enum Decision {
                Reuse(Box<dyn Connection>, Instant),
                CreateReserved,
                AtCapacity,
            }

            let decision: Decision = {
                let mut state = self.state.lock().expect("pool state poisoned");

                // Evict expired/idle-timed-out connections from the front.
                let now = Instant::now();
                let max_lifetime = Duration::from_millis(self.config.max_lifetime_ms);
                let max_idle = Duration::from_millis(self.config.idle_timeout_ms);
                while let Some(front) = state.idle.front() {
                    if now.duration_since(front.created_at) >= max_lifetime
                        || now.duration_since(front.last_used) >= max_idle
                    {
                        // Drop expired connection — total stays elevated until
                        // we pop it because `total` counts idle. So decrement.
                        state.idle.pop_front();
                        state.total = state.total.saturating_sub(1);
                    } else {
                        break;
                    }
                }

                if let Some(pooled) = state.idle.pop_front() {
                    // Reuse: total stays the same (idle→active).
                    Decision::Reuse(pooled.conn, pooled.created_at)
                } else if state.total < self.config.max_size {
                    // Reserve a slot for the new connection BEFORE the await.
                    state.total += 1;
                    Decision::CreateReserved
                } else {
                    Decision::AtCapacity
                }
            };

            match decision {
                Decision::Reuse(conn, created_at) => {
                    let wait_ms = start.elapsed().as_millis() as u64;
                    self.checkout_count.fetch_add(1, Ordering::Relaxed);
                    self.total_wait_ms.fetch_add(wait_ms, Ordering::Relaxed);

                    let mut cb = self.circuit_breaker.lock().await;
                    cb.record_success();

                    return Ok(self.guard_with_created_at(conn, created_at));
                }
                Decision::CreateReserved => {
                    match self.create_connection().await {
                        Ok(conn) => {
                            let created_at = Instant::now();
                            let wait_ms = start.elapsed().as_millis() as u64;
                            self.checkout_count.fetch_add(1, Ordering::Relaxed);
                            self.total_wait_ms.fetch_add(wait_ms, Ordering::Relaxed);

                            let mut cb = self.circuit_breaker.lock().await;
                            cb.record_success();
                            return Ok(self.guard_with_created_at(conn, created_at));
                        }
                        Err(e) => {
                            // Release the reserved slot.
                            if let Ok(mut state) = self.state.lock() {
                                state.total = state.total.saturating_sub(1);
                            }
                            self.notify.notify_one();

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
                Decision::AtCapacity => {
                    // Wait for a release.
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(PoolError::Timeout {
                            datasource: self.datasource_id.clone(),
                            timeout_ms: self.config.connection_timeout_ms,
                        });
                    }

                    tokio::select! {
                        _ = self.notify.notified() => {
                            // A connection was released — loop back.
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
        }
    }

    /// Create a new connection via the driver.
    async fn create_connection(
        &self,
    ) -> Result<Box<dyn Connection>, rivers_runtime::rivers_driver_sdk::error::DriverError> {
        self.driver.connect(&self.params).await
    }

    /// Get a snapshot of pool health.
    pub async fn snapshot(&self) -> PoolSnapshot {
        let (idle_count, active) = {
            let state = self.state.lock().expect("pool state poisoned");
            (state.idle.len(), state.active())
        };
        let checkouts = self.checkout_count.load(Ordering::Relaxed);
        let total_wait = self.total_wait_ms.load(Ordering::Relaxed);
        let circuit_state = {
            let cb = self.circuit_breaker.lock().await;
            cb.state()
        };

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
            circuit_state,
        }
    }

    /// Run health checks on idle connections.
    ///
    /// Pings each idle connection; removes those that fail.
    /// Emits DatasourceHealthCheckFailed if all idle connections fail.
    pub async fn health_check(&self) {
        // Take ownership of the idle queue under the lock; ping outside the
        // lock to keep the critical section short and avoid blocking acquire.
        let mut to_check: VecDeque<PooledConnection> = {
            let mut state = self.state.lock().expect("pool state poisoned");
            std::mem::take(&mut state.idle)
        };

        let total = to_check.len();
        let mut healthy = VecDeque::with_capacity(total);
        let mut failures = 0usize;

        while let Some(mut pooled) = to_check.pop_front() {
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

        // Re-insert healthy connections; total drops by `failures` since the
        // unhealthy connections are gone.
        {
            let mut state = self.state.lock().expect("pool state poisoned");
            // Push the still-healthy to the front so FIFO order is preserved
            // for connections that arrived after the snapshot.
            for pooled in healthy.into_iter().rev() {
                state.idle.push_front(pooled);
            }
            state.total = state.total.saturating_sub(failures);
        }

        if failures > 0 && total > 0 && failures == total {
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
        if !self.draining.load(Ordering::Relaxed) {
            return false;
        }
        let state = self.state.lock().expect("pool state poisoned");
        state.active() == 0
    }

    /// Drain the pool, dropping all idle connections.
    pub async fn drain(&self) {
        self.start_drain();
        let mut state = self.state.lock().expect("pool state poisoned");
        let dropped = state.idle.len();
        state.idle.clear();
        state.total = state.total.saturating_sub(dropped);
    }

    /// Construct a `PoolGuard` for an already-checked-out connection.
    fn guard_with_created_at(
        &self,
        conn: Box<dyn Connection>,
        created_at: Instant,
    ) -> PoolGuard {
        PoolGuard {
            state: Arc::clone(&self.state),
            draining: Arc::clone(&self.draining),
            notify: Arc::clone(&self.notify),
            max_lifetime: Duration::from_millis(self.config.max_lifetime_ms),
            created_at,
            conn: Some(conn),
        }
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
///
/// Pools are keyed by datasource id (`HashMap`), giving O(1) lookup and
/// rejecting duplicate registrations (D1.3).
pub struct PoolManager {
    pools: RwLock<HashMap<String, Arc<ConnectionPool>>>,
}

impl PoolManager {
    /// Create a new empty pool manager.
    pub fn new() -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
        }
    }

    /// Register a pool with the manager.
    ///
    /// Returns `Err(PoolError::DuplicateDatasource)` if a pool is already
    /// registered for the same datasource id.
    pub async fn add_pool(&self, pool: Arc<ConnectionPool>) -> Result<(), PoolError> {
        let id = pool.datasource_id().to_string();
        let mut pools = self.pools.write().await;
        if pools.contains_key(&id) {
            return Err(PoolError::DuplicateDatasource { datasource: id });
        }
        pools.insert(id, pool);
        Ok(())
    }

    /// Get a pool by datasource ID.
    pub async fn get_pool(&self, datasource_id: &str) -> Option<Arc<ConnectionPool>> {
        let pools = self.pools.read().await;
        pools.get(datasource_id).cloned()
    }

    /// Acquire a connection from the named datasource's pool.
    ///
    /// Returns `Err(PoolError::UnknownDatasource)` if no pool is registered
    /// for `datasource_id`.
    pub async fn acquire(&self, datasource_id: &str) -> Result<PoolGuard, PoolError> {
        let pool = self.get_pool(datasource_id).await.ok_or_else(|| {
            PoolError::UnknownDatasource {
                datasource: datasource_id.to_string(),
            }
        })?;
        pool.acquire().await
    }

    /// Get snapshots of all pools.
    pub async fn snapshots(&self) -> Vec<PoolSnapshot> {
        let pools = self.pools.read().await;
        let mut snapshots = Vec::with_capacity(pools.len());
        for pool in pools.values() {
            snapshots.push(pool.snapshot().await);
        }
        snapshots
    }

    /// Drain all pools on shutdown.
    pub async fn drain_all(&self) {
        let pools = self.pools.read().await;
        for pool in pools.values() {
            pool.drain().await;
        }
    }
}

impl Default for PoolManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── ConnectionAcquirer impl (D2) ───────────────────────────────────

/// Adapter so `PoolGuard` satisfies the runtime-crate-local
/// `rivers_runtime::PooledConnection` trait. The wrapped guard releases
/// the connection back to its pool when dropped.
struct PoolGuardAdapter(PoolGuard);

impl rivers_runtime::PooledConnection for PoolGuardAdapter {
    fn conn_mut(&mut self) -> &mut Box<dyn Connection> {
        self.0.conn_mut()
    }
}

/// Map `PoolError` → `rivers_runtime::AcquireError`. The runtime crate has
/// its own thin error enum so it doesn't need to depend on `riversd::pool`.
fn map_pool_error(err: PoolError) -> rivers_runtime::AcquireError {
    use rivers_runtime::AcquireError;
    match err {
        PoolError::CircuitOpen { datasource } => AcquireError::CircuitOpen(datasource),
        PoolError::Timeout {
            datasource,
            timeout_ms,
        } => AcquireError::Timeout {
            datasource,
            timeout_ms,
        },
        PoolError::Draining { datasource } => AcquireError::Draining(datasource),
        PoolError::UnknownDatasource { datasource } => AcquireError::UnknownDatasource(datasource),
        PoolError::DuplicateDatasource { datasource } => {
            AcquireError::Other(format!("duplicate datasource '{datasource}'"))
        }
        PoolError::Driver(e) => AcquireError::Driver(e.to_string()),
        PoolError::Config(s) => AcquireError::Other(format!("config: {s}")),
    }
}

#[async_trait::async_trait]
impl rivers_runtime::ConnectionAcquirer for PoolManager {
    async fn acquire(
        &self,
        datasource_id: &str,
    ) -> Result<Box<dyn rivers_runtime::PooledConnection>, rivers_runtime::AcquireError> {
        let guard = PoolManager::acquire(self, datasource_id)
            .await
            .map_err(map_pool_error)?;
        Ok(Box::new(PoolGuardAdapter(guard)))
    }

    async fn has_pool(&self, datasource_id: &str) -> bool {
        self.get_pool(datasource_id).await.is_some()
    }
}
