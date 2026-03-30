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

    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub static_files: StaticFilesConfig,

    #[serde(default)]
    pub storage_engine: StorageEngineConfig,

    #[serde(default)]
    pub lockbox: Option<LockBoxConfig>,

    #[serde(default)]
    pub runtime: RuntimeConfig,

    #[serde(default)]
    pub graphql: GraphqlServerConfig,

    #[serde(default)]
    pub engines: EnginesConfig,

    #[serde(default)]
    pub plugins: PluginsConfig,

    #[serde(default)]
    pub environment_overrides: std::collections::HashMap<String, EnvironmentOverride>,
}

// ── [base] ──────────────────────────────────────────────────────────

/// `[base]` section -- core server settings.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BaseConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub workers: Option<u32>,

    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,

    #[serde(default)]
    pub log_level: LogLevel,

    #[serde(default)]
    pub backpressure: BackpressureConfig,

    #[serde(default)]
    pub http2: Http2Config,

    #[serde(default)]
    pub admin_api: AdminApiConfig,

    #[serde(default)]
    pub cluster: ClusterConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

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
    pub enabled: bool,

    pub queue_depth: usize,

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
    #[serde(default)]
    pub enabled: bool,

    pub initial_window_size: Option<u32>,
    pub max_concurrent_streams: Option<u32>,
}
