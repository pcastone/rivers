//! Datasource configuration types.
//!
//! Per `rivers-data-layer-spec.md` §12.1, §12.2.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

/// Configuration for a datasource (database or broker).
///
/// Declared in `resources.toml` as `[[datasources]]` and configured
/// in `app.toml` under `[data.datasources.{id}]`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DatasourceConfig {
    /// Unique name within the app.
    pub name: String,

    /// Driver name: "postgres", "mysql", "sqlite", "redis", "http", "faker", etc.
    pub driver: String,

    /// Hostname or IP address of the server.
    pub host: Option<String>,
    /// Port number.
    pub port: Option<u16>,
    /// Database name (or path for SQLite).
    pub database: Option<String>,
    /// Connection username.
    pub username: Option<String>,

    /// LockBox credential reference, e.g. "lockbox://db/myapp-postgres".
    pub credentials_source: Option<String>,

    /// If true, no password/credentials required (e.g. faker driver).
    #[serde(default)]
    pub nopassword: bool,

    /// Build-time type hint for validation tools.
    #[serde(rename = "x-type")]
    pub x_type: Option<String>,

    /// Connection pool settings.
    #[serde(default)]
    pub connection_pool: PoolConfig,

    /// Broker consumer settings (message broker datasources only).
    #[serde(default)]
    pub consumer: Option<ConsumerConfig>,

    /// Lifecycle event handlers (on_connection_failed, on_pool_exhausted).
    #[serde(default)]
    pub event_handlers: Option<DatasourceEventHandlers>,

    /// Driver-specific extra config (e.g. InfluxDB org/language).
    #[serde(default)]
    pub extra: HashMap<String, String>,

    /// Write batching configuration (e.g. InfluxDB bulk writes).
    #[serde(default)]
    pub write_batch: Option<WriteBatchConfig>,

    /// Whether to run schema introspection at startup. Defaults to true.
    #[serde(default = "default_introspect")]
    pub introspect: bool,
}

/// Connection pool configuration.
///
/// Per `rivers-data-layer-spec.md` §12.1.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PoolConfig {
    /// Maximum number of connections in the pool. Default: 10.
    #[serde(default = "default_pool_max")]
    pub max_size: usize,

    /// Minimum idle connections to keep warm. Default: 0.
    #[serde(default)]
    pub min_idle: usize,

    /// Timeout for acquiring a connection in milliseconds. Default: 500.
    #[serde(default = "default_conn_timeout")]
    pub connection_timeout_ms: u64,

    /// Idle connection timeout in milliseconds. Default: 30000 (30s).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_ms: u64,

    /// Maximum connection lifetime in milliseconds. Default: 300000 (5min).
    #[serde(default = "default_max_lifetime")]
    pub max_lifetime_ms: u64,

    /// Health check interval in milliseconds. Default: 5000 (5s).
    #[serde(default = "default_health_interval")]
    pub health_check_interval_ms: u64,

    /// Circuit breaker configuration.
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: default_pool_max(),
            min_idle: 0,
            connection_timeout_ms: default_conn_timeout(),
            idle_timeout_ms: default_idle_timeout(),
            max_lifetime_ms: default_max_lifetime(),
            health_check_interval_ms: default_health_interval(),
            circuit_breaker: CircuitBreakerConfig::default(),
        }
    }
}

fn default_pool_max() -> usize {
    10
}
fn default_conn_timeout() -> u64 {
    500
}
fn default_idle_timeout() -> u64 {
    30_000
}
fn default_max_lifetime() -> u64 {
    300_000
}
fn default_health_interval() -> u64 {
    5_000
}

/// Circuit breaker configuration for a datasource pool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CircuitBreakerConfig {
    /// Whether the circuit breaker is active. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Number of failures before opening the circuit. Default: 5.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,

    /// Rolling window in milliseconds for counting failures. Default: 60000 (60s).
    #[serde(default = "default_window_ms")]
    pub window_ms: u64,

    /// Time to wait in open state before transitioning to half-open. Default: 30000 (30s).
    #[serde(default = "default_open_timeout")]
    pub open_timeout_ms: u64,

    /// Number of probe requests in half-open state before closing. Default: 3.
    #[serde(default = "default_half_open_trials")]
    pub half_open_max_trials: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_threshold: default_failure_threshold(),
            window_ms: default_window_ms(),
            open_timeout_ms: default_open_timeout(),
            half_open_max_trials: default_half_open_trials(),
        }
    }
}

fn default_window_ms() -> u64 {
    60_000
}

fn default_failure_threshold() -> u32 {
    5
}
fn default_open_timeout() -> u64 {
    30_000
}
fn default_half_open_trials() -> u32 {
    3
}

/// Broker consumer configuration.
///
/// Per `rivers-data-layer-spec.md` §12.2.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ConsumerConfig {
    /// Consumer group prefix for load-balanced consumption.
    pub group_prefix: Option<String>,
    /// Application ID used to construct consumer group names.
    pub app_id: Option<String>,

    /// Reconnect delay in milliseconds after broker disconnect. Default: 5000.
    #[serde(default = "default_reconnect_ms")]
    pub reconnect_ms: u64,

    /// Topic/queue subscriptions for this consumer.
    #[serde(default)]
    pub subscriptions: Vec<SubscriptionConfig>,
}

fn default_reconnect_ms() -> u64 {
    5_000
}

/// A single broker subscription.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubscriptionConfig {
    /// Topic or queue name to subscribe to.
    pub topic: String,
    /// Logical event name emitted to the EventBus on message arrival.
    pub event_name: Option<String>,

    /// Acknowledgment mode: "auto" (default) or "manual".
    #[serde(default = "default_ack_mode")]
    pub ack_mode: String,

    /// Maximum retry attempts before applying failure policy. Default: 0.
    #[serde(default)]
    pub max_retries: u32,

    /// What to do when processing fails after max retries.
    #[serde(default)]
    pub on_failure: Option<FailurePolicyConfig>,
}

fn default_ack_mode() -> String {
    "auto".to_string()
}

/// Failure policy for broker message processing.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FailurePolicyConfig {
    /// Policy mode: "dead_letter", "requeue", "redirect", or "drop".
    pub mode: String,
    /// Target destination for "dead_letter" and "redirect" modes.
    pub destination: Option<String>,
}

/// Event handlers attached to a datasource.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct DatasourceEventHandlers {
    /// Handlers invoked when a connection attempt fails.
    #[serde(default)]
    pub on_connection_failed: Vec<EventHandlerRef>,

    /// Handlers invoked when the connection pool is exhausted.
    #[serde(default)]
    pub on_pool_exhausted: Vec<EventHandlerRef>,
}

/// Reference to a CodeComponent event handler.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EventHandlerRef {
    /// CodeComponent module path.
    pub module: String,
    /// Function name within the module.
    pub entrypoint: String,
}

/// Write batch configuration (e.g. InfluxDB).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteBatchConfig {
    /// Whether write batching is active. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Maximum records per batch before flush. Default: 1000.
    #[serde(default = "default_batch_max")]
    pub max_size: usize,

    /// Time-based flush interval in milliseconds. Default: 1000.
    #[serde(default = "default_flush_interval")]
    pub flush_interval_ms: u64,
}

fn default_batch_max() -> usize {
    1000
}
fn default_flush_interval() -> u64 {
    1000
}

fn default_introspect() -> bool {
    true
}
