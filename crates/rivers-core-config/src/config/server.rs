//! Server base configuration types.
//!
//! `ServerConfig`, `BaseConfig`, `BackpressureConfig`, `Http2Config`.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::event::LogLevel;
use crate::lockbox_config::LockBoxConfig;

use super::security::SecurityConfig;
use super::storage::StorageEngineConfig;
use super::tls::{AdminApiConfig, ClusterConfig, TlsConfig};
use super::runtime::{
    EnginesConfig, EnvironmentOverride, GraphqlServerConfig, LoggingConfig, PluginsConfig,
    RuntimeConfig, StaticFilesConfig,
};

// ── Top-level ServerConfig ──────────────────────────────────────────

/// Root configuration loaded from `riversd.conf`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ServerConfig {
    /// Core server settings (host, port, workers, TLS, etc.).
    #[serde(default)]
    pub base: BaseConfig,

    /// Path to the bundle directory to auto-load at startup.
    /// Resolved relative to the working directory when riversd starts.
    /// If unset, no bundle is loaded at startup (use `riversctl deploy` instead).
    #[serde(default)]
    pub bundle_path: Option<String>,

    /// Directory for riversd data files (auto-gen TLS certs, etc.).
    /// Defaults to "data" relative to the working directory.
    #[serde(default)]
    pub data_dir: Option<String>,

    /// Application ID for this riversd instance (used in auto-gen cert filenames).
    /// Defaults to "default".
    #[serde(default)]
    pub app_id: Option<String>,

    /// Optional route prefix prepended to all bundle routes.
    /// e.g. `route_prefix = "v1"` -> `/<v1>/<bundle>/<app>/<view>`
    #[serde(default)]
    pub route_prefix: Option<String>,

    /// CORS, rate limiting, sessions, and CSRF protection.
    #[serde(default)]
    pub security: SecurityConfig,

    /// Static file serving and SPA fallback settings.
    #[serde(default)]
    pub static_files: StaticFilesConfig,

    /// Internal KV + queue backend configuration.
    #[serde(default)]
    pub storage_engine: StorageEngineConfig,

    /// Age-encrypted secret store configuration.
    #[serde(default)]
    pub lockbox: Option<LockBoxConfig>,

    /// ProcessPool and CodeComponent runtime settings.
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// GraphQL endpoint configuration.
    #[serde(default)]
    pub graphql: GraphqlServerConfig,

    /// Engine shared library directory.
    #[serde(default)]
    pub engines: EnginesConfig,

    /// Driver plugin shared library directory.
    #[serde(default)]
    pub plugins: PluginsConfig,

    /// Per-environment config overrides (keyed by environment name).
    #[serde(default)]
    pub environment_overrides: std::collections::HashMap<String, EnvironmentOverride>,
}

// ── [base] ──────────────────────────────────────────────────────────

/// `[base]` section -- core server settings.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BaseConfig {
    /// Listen address (default: `"0.0.0.0"`).
    #[serde(default = "default_host")]
    pub host: String,

    /// Listen port (default: `8080`).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Number of HTTP worker threads. `None` = auto-detect from CPU count.
    #[serde(default)]
    pub workers: Option<u32>,

    /// Per-request timeout in seconds (default: `30`).
    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,

    /// Minimum log severity for this instance.
    #[serde(default)]
    pub log_level: LogLevel,

    /// Request queuing under load.
    #[serde(default)]
    pub backpressure: BackpressureConfig,

    /// HTTP/2 protocol settings.
    #[serde(default)]
    pub http2: Http2Config,

    /// Admin API server on a separate socket.
    #[serde(default)]
    pub admin_api: AdminApiConfig,

    /// Clustering and session store settings.
    #[serde(default)]
    pub cluster: ClusterConfig,

    /// Log output format and destination.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// TLS configuration (enables HTTPS when present).
    #[serde(default)]
    pub tls: Option<TlsConfig>,
}

impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            workers: None,
            request_timeout_seconds: default_request_timeout(),
            log_level: LogLevel::default(),
            backpressure: BackpressureConfig::default(),
            http2: Http2Config::default(),
            admin_api: AdminApiConfig::default(),
            cluster: ClusterConfig::default(),
            logging: LoggingConfig::default(),
            tls: None,
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_request_timeout() -> u64 {
    30
}

// ── [base.backpressure] ─────────────────────────────────────────────

/// `[base.backpressure]` -- request queuing under load.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BackpressureConfig {
    /// Whether backpressure queueing is active (default: `true`).
    pub enabled: bool,

    /// Maximum pending requests before rejection (default: `512`).
    pub queue_depth: usize,

    /// How long a request waits in the queue before 503 (default: `100` ms).
    pub queue_timeout_ms: u64,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            queue_depth: 512,
            queue_timeout_ms: 100,
        }
    }
}

// ── [base.http2] ────────────────────────────────────────────────────

/// `[base.http2]` -- HTTP/2 protocol settings (TLS is configured separately under [base.tls]).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct Http2Config {
    /// Enable HTTP/2 (default: `false`).
    #[serde(default)]
    pub enabled: bool,

    /// Initial flow-control window size in bytes.
    pub initial_window_size: Option<u32>,
    /// Maximum concurrent streams per connection.
    pub max_concurrent_streams: Option<u32>,
}
