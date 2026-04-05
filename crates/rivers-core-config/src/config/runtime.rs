//! Runtime, process pool, environment override, logging, engines, plugins,
//! GraphQL, and static files configuration types.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::event::LogLevel;

use super::server::ServerConfig;

// ── [metrics] ───────────────────────────────────────────────────────

/// `[metrics]` -- Prometheus metrics configuration.
#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
pub struct MetricsConfig {
    /// Whether metrics collection is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Port for the Prometheus HTTP scrape endpoint (default: 9091).
    pub port: Option<u16>,
}

// ── [runtime] ───────────────────────────────────────────────────────

/// `[runtime]` -- ProcessPool runtime configuration.
/// Per `rivers-processpool-runtime-spec-v2.md` S8-9.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RuntimeConfig {
    /// Named process pools. Key is pool name (e.g. "default", "wasm").
    #[serde(default)]
    pub process_pools: std::collections::HashMap<String, ProcessPoolConfig>,
}

/// `[runtime.process_pools.<name>]` -- per-pool configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProcessPoolConfig {
    /// Engine type: "v8" or "wasmtime".
    #[serde(default = "default_engine")]
    pub engine: String,

    /// Number of worker threads.
    #[serde(default = "default_workers")]
    pub workers: usize,

    /// Max heap size per worker in megabytes (V8 only).
    #[serde(default = "default_max_heap_mb")]
    pub max_heap_mb: usize,

    /// Wall clock timeout per task in milliseconds.
    #[serde(default = "default_task_timeout_ms")]
    pub task_timeout_ms: u64,

    /// Max queued tasks (0 = workers * 4).
    #[serde(default)]
    pub max_queue_depth: usize,

    /// Epoch tick frequency for WASM preemption (ms).
    #[serde(default = "default_epoch_interval_ms")]
    pub epoch_interval_ms: u64,

    /// Heap threshold for V8 isolate recycling (0.0-1.0).
    #[serde(default = "default_heap_recycle_threshold")]
    pub heap_recycle_threshold: f64,

    /// Recycle worker isolate after this many tasks (0 = never).
    #[serde(default)]
    pub recycle_after_tasks: Option<u64>,
}

impl Default for ProcessPoolConfig {
    fn default() -> Self {
        Self {
            engine: default_engine(),
            workers: default_workers(),
            max_heap_mb: default_max_heap_mb(),
            task_timeout_ms: default_task_timeout_ms(),
            max_queue_depth: 0,
            epoch_interval_ms: default_epoch_interval_ms(),
            heap_recycle_threshold: default_heap_recycle_threshold(),
            recycle_after_tasks: None,
        }
    }
}

fn default_engine() -> String {
    "v8".to_string()
}

fn default_workers() -> usize {
    4
}

fn default_max_heap_mb() -> usize {
    128 // 128 MiB
}

fn default_task_timeout_ms() -> u64 {
    5000
}

fn default_epoch_interval_ms() -> u64 {
    10
}

fn default_heap_recycle_threshold() -> f64 {
    0.8
}

// ── [environment_overrides] ─────────────────────────────────────────

/// `[environment_overrides.{env}]` -- per-environment config overrides.
/// Fields mirror `BaseConfig` and `SecurityConfig` but are all optional.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EnvironmentOverride {
    /// Partial base config overrides.
    pub base: Option<BaseOverride>,
    /// Partial security config overrides.
    pub security: Option<SecurityOverride>,
    /// Partial storage engine config overrides.
    pub storage_engine: Option<StorageEngineOverride>,
}

impl EnvironmentOverride {
    /// Apply this override to a ServerConfig, overwriting only the fields that are set.
    pub fn apply_to(&self, config: &mut ServerConfig) {
        if let Some(ref base) = self.base {
            if let Some(ref host) = base.host { config.base.host = host.clone(); }
            if let Some(port) = base.port { config.base.port = port; }
            if let Some(workers) = base.workers { config.base.workers = Some(workers); }
            if let Some(timeout) = base.request_timeout_seconds { config.base.request_timeout_seconds = timeout; }
            if let Some(ref level) = base.log_level { config.base.log_level = level.clone(); }
            if let Some(ref bp) = base.backpressure {
                if let Some(enabled) = bp.enabled { config.base.backpressure.enabled = enabled; }
                if let Some(depth) = bp.queue_depth { config.base.backpressure.queue_depth = depth; }
                if let Some(timeout) = bp.queue_timeout_ms { config.base.backpressure.queue_timeout_ms = timeout; }
            }
        }
        if let Some(ref sec) = self.security {
            if let Some(cors) = sec.cors_enabled { config.security.cors_enabled = cors; }
            if let Some(ref origins) = sec.cors_allowed_origins { config.security.cors_allowed_origins = origins.clone(); }
            if let Some(rpm) = sec.rate_limit_per_minute { config.security.rate_limit_per_minute = rpm; }
            if let Some(burst) = sec.rate_limit_burst_size { config.security.rate_limit_burst_size = burst; }
        }
        if let Some(ref se) = self.storage_engine {
            if let Some(ref backend) = se.backend { config.storage_engine.backend = backend.clone(); }
            if let Some(ref url) = se.url { config.storage_engine.url = Some(url.clone()); }
            if let Some(ref cred) = se.credentials_source { config.storage_engine.credentials_source = Some(cred.clone()); }
            if let Some(ref prefix) = se.key_prefix { config.storage_engine.key_prefix = Some(prefix.clone()); }
            if let Some(pool) = se.pool_size { config.storage_engine.pool_size = Some(pool); }
        }
    }
}

/// Partial `[base]` override for an environment.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct BaseOverride {
    /// Override listen address.
    pub host: Option<String>,
    /// Override listen port.
    pub port: Option<u16>,
    /// Override worker thread count.
    pub workers: Option<u32>,
    /// Override request timeout in seconds.
    pub request_timeout_seconds: Option<u64>,
    /// Override log level.
    pub log_level: Option<LogLevel>,
    /// Override backpressure settings.
    pub backpressure: Option<BackpressureOverride>,
}

/// Partial `[base.backpressure]` override.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct BackpressureOverride {
    /// Override backpressure enabled state.
    pub enabled: Option<bool>,
    /// Override queue depth.
    pub queue_depth: Option<usize>,
    /// Override queue timeout in milliseconds.
    pub queue_timeout_ms: Option<u64>,
}

/// Partial `[security]` override.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SecurityOverride {
    /// Override CORS enabled state.
    pub cors_enabled: Option<bool>,
    /// Override allowed CORS origins.
    pub cors_allowed_origins: Option<Vec<String>>,
    /// Override rate limit per minute.
    pub rate_limit_per_minute: Option<u32>,
    /// Override rate limit burst size.
    pub rate_limit_burst_size: Option<u32>,
}

/// Partial `[storage_engine]` override.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct StorageEngineOverride {
    /// Override storage backend type.
    pub backend: Option<String>,
    /// Override storage backend URL.
    pub url: Option<String>,
    /// Override credentials source.
    pub credentials_source: Option<String>,
    /// Override key prefix.
    pub key_prefix: Option<String>,
    /// Override connection pool size.
    pub pool_size: Option<usize>,
}

// ── [base.logging] ──────────────────────────────────────────────────

/// `[base.logging]` -- log output configuration.
/// Per `rivers-logging-spec.md` S9.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LoggingConfig {
    /// Log severity filter (default: `Info`).
    #[serde(default)]
    pub level: LogLevel,

    /// Output format: `"json"` or `"text"` (default: `"json"`).
    #[serde(default = "default_log_format")]
    pub format: String,

    /// Optional path for a local log file (in addition to stdout).
    pub local_file_path: Option<String>,

    /// Directory for per-application log files. Each loaded app gets `<app_name>.log`.
    /// If not set, app logs go to the main `local_file_path`.
    #[serde(default)]
    pub app_log_dir: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: default_log_format(),
            local_file_path: None,
            app_log_dir: None,
        }
    }
}

fn default_log_format() -> String {
    "json".to_string()
}

// ── [engines] ────────────────────────────────────────────────────────

/// `[engines]` -- directory containing engine shared libraries.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EnginesConfig {
    /// Path to engine shared library directory (default: "lib").
    #[serde(default = "default_engines_dir")]
    pub dir: String,
}

impl Default for EnginesConfig {
    fn default() -> Self {
        Self { dir: default_engines_dir() }
    }
}

fn default_engines_dir() -> String {
    "lib".to_string()
}

// ── [plugins] ────────────────────────────────────────────────────────

/// `[plugins]` -- directory containing driver plugin shared libraries.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PluginsConfig {
    /// Path to plugin shared library directory (default: "plugins").
    #[serde(default = "default_plugins_dir")]
    pub dir: String,
    /// Driver names to ignore -- these plugins will not be loaded.
    /// If a bundle references an ignored driver, bundle validation fails.
    #[serde(default)]
    pub ignore: Vec<String>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self { dir: default_plugins_dir(), ignore: Vec::new() }
    }
}

fn default_plugins_dir() -> String {
    "plugins".to_string()
}

// ── [graphql] ────────────────────────────────────────────────────────

/// `[graphql]` -- GraphQL endpoint configuration at the server level.
///
/// Per `rivers-view-layer-spec.md` S9.
/// The full schema-building config lives in `riversd::graphql::GraphqlConfig`;
/// this is the minimal enable/path config for ServerConfig.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GraphqlServerConfig {
    /// Enable the GraphQL endpoint (default: `false`).
    #[serde(default)]
    pub enabled: bool,

    /// URL path for the GraphQL endpoint (default: `"/graphql"`).
    #[serde(default = "default_graphql_path")]
    pub path: String,

    /// Allow introspection queries (default: true).
    #[serde(default = "default_true")]
    pub introspection: bool,

    /// Max query depth (default: 10).
    #[serde(default = "default_graphql_depth")]
    pub max_depth: usize,

    /// Max query complexity (default: 1000).
    #[serde(default = "default_graphql_complexity")]
    pub max_complexity: usize,
}

impl Default for GraphqlServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_graphql_path(),
            introspection: true,
            max_depth: 10,
            max_complexity: 1000,
        }
    }
}

fn default_graphql_path() -> String {
    "/graphql".to_string()
}

fn default_graphql_depth() -> usize {
    10
}

fn default_graphql_complexity() -> usize {
    1000
}

// ── [static_files] ──────────────────────────────────────────────────

/// `[static_files]` -- static file serving and SPA fallback.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StaticFilesConfig {
    /// Enable static file serving (default: `false`).
    #[serde(default)]
    pub enabled: bool,

    /// Root directory for static files.
    #[serde(default)]
    pub root_path: Option<String>,

    /// Default index file (default: `"index.html"`).
    #[serde(default = "default_index_file")]
    pub index_file: String,

    /// Serve `index_file` for unmatched routes (SPA mode, default: `false`).
    #[serde(default)]
    pub spa_fallback: bool,

    /// `Cache-Control: max-age` in seconds for static assets.
    #[serde(default)]
    pub max_age: Option<u64>,

    /// URL path prefixes excluded from static file serving.
    #[serde(default)]
    pub exclude_paths: Vec<String>,
}

impl Default for StaticFilesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            root_path: None,
            index_file: default_index_file(),
            spa_fallback: false,
            max_age: None,
            exclude_paths: Vec::new(),
        }
    }
}

fn default_index_file() -> String {
    "index.html".to_string()
}

// ── Helpers ─────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

// ── Schema Generation ────────────────────────────────────────────

/// Generate JSON Schema for `ServerConfig` (the `riversd.conf` format).
pub fn server_config_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ServerConfig)).unwrap_or_default()
}
