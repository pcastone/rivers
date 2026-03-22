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

    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,

    /// LockBox credential reference, e.g. "lockbox://db/myapp-postgres".
    pub credentials_source: Option<String>,

    /// If true, no password/credentials required (e.g. faker driver).
    #[serde(default)]
    pub nopassword: bool,

    /// Build-time type hint for validation tools.
    #[serde(rename = "x-type")]
    pub x_type: Option<String>,

    #[serde(default)]
    pub connection_pool: PoolConfig,

    #[serde(default)]
    pub consumer: Option<ConsumerConfig>,

    #[serde(default)]
    pub event_handlers: Option<DatasourceEventHandlers>,

    /// Driver-specific extra config (e.g. InfluxDB org/language).
    #[serde(default)]
    pub extra: HashMap<String, String>,

    #[serde(default)]
    pub write_batch: Option<WriteBatchConfig>,
}

/// Connection pool configuration.
///
/// Per `rivers-data-layer-spec.md` §12.1.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PoolConfig {
    #[serde(default = "default_pool_max")]
    pub max_size: usize,

    #[serde(default)]
    pub min_idle: usize,

    #[serde(default = "default_conn_timeout")]
    pub connection_timeout_ms: u64,

    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_ms: u64,

    #[serde(default = "default_max_lifetime")]
    pub max_lifetime_ms: u64,

    #[serde(default = "default_health_interval")]
    pub health_check_interval_ms: u64,

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
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,

    #[serde(default = "default_open_timeout")]
    pub open_timeout_ms: u64,

    #[serde(default = "default_half_open_trials")]
    pub half_open_max_trials: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_threshold: default_failure_threshold(),
            open_timeout_ms: default_open_timeout(),
            half_open_max_trials: default_half_open_trials(),
        }
    }
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
    pub group_prefix: Option<String>,
    pub app_id: Option<String>,

    #[serde(default = "default_reconnect_ms")]
    pub reconnect_ms: u64,

    #[serde(default)]
    pub subscriptions: Vec<SubscriptionConfig>,
}

fn default_reconnect_ms() -> u64 {
    5_000
}

/// A single broker subscription.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubscriptionConfig {
    pub topic: String,
    pub event_name: Option<String>,

    #[serde(default = "default_ack_mode")]
    pub ack_mode: String,

    #[serde(default)]
    pub max_retries: u32,

    #[serde(default)]
    pub on_failure: Option<FailurePolicyConfig>,
}

fn default_ack_mode() -> String {
    "auto".to_string()
}

/// Failure policy for broker message processing.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FailurePolicyConfig {
    /// "dead_letter" | "requeue" | "redirect" | "drop"
    pub mode: String,
    pub destination: Option<String>,
}

/// Event handlers attached to a datasource.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct DatasourceEventHandlers {
    #[serde(default)]
    pub on_connection_failed: Vec<EventHandlerRef>,

    #[serde(default)]
    pub on_pool_exhausted: Vec<EventHandlerRef>,
}

/// Reference to a CodeComponent event handler.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EventHandlerRef {
    pub module: String,
    pub entrypoint: String,
}

/// Write batch configuration (e.g. InfluxDB).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteBatchConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_batch_max")]
    pub max_size: usize,

    #[serde(default = "default_flush_interval")]
    pub flush_interval_ms: u64,
}

fn default_batch_max() -> usize {
    1000
}
fn default_flush_interval() -> u64 {
    1000
}
