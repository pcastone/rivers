//! Server configuration types.
//!
//! Maps to `riversd.conf` — the top-level TOML config for a riversd instance.
//! Per `rivers-httpd-spec.md` §19.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::event::LogLevel;
use crate::lockbox_config::LockBoxConfig;

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
    /// e.g. `route_prefix = "v1"` → `/<v1>/<bundle>/<app>/<view>`
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

/// `[base]` section — core server settings.
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

/// `[base.backpressure]` — request queuing under load.
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

/// `[base.http2]` — HTTP/2 protocol settings (TLS is configured separately under [base.tls]).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct Http2Config {
    #[serde(default)]
    pub enabled: bool,

    pub initial_window_size: Option<u32>,
    pub max_concurrent_streams: Option<u32>,
}

// ── [base.tls] ──────────────────────────────────────────────────────

/// `[base.tls]` — TLS configuration. Mandatory on the main server.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TlsConfig {
    pub cert: Option<String>,
    pub key: Option<String>,

    #[serde(default = "default_true")]
    pub redirect: bool,

    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,

    #[serde(default)]
    pub x509: TlsX509Config,

    #[serde(default)]
    pub engine: TlsEngineConfig,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            cert: None,
            key: None,
            redirect: true,
            redirect_port: default_redirect_port(),
            x509: TlsX509Config::default(),
            engine: TlsEngineConfig::default(),
        }
    }
}

fn default_redirect_port() -> u16 {
    80
}

/// `[base.tls.x509]` — x509 fields used for auto-gen and riversctl tls gen/request.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TlsX509Config {
    #[serde(default = "default_cn")]
    pub common_name: String,

    #[serde(default)]
    pub organization: Option<String>,

    #[serde(default)]
    pub country: Option<String>,

    #[serde(default)]
    pub state: Option<String>,

    #[serde(default)]
    pub locality: Option<String>,

    #[serde(default = "default_san")]
    pub san: Vec<String>,

    #[serde(default = "default_days")]
    pub days: u32,
}

impl Default for TlsX509Config {
    fn default() -> Self {
        Self {
            common_name: default_cn(),
            organization: None,
            country: None,
            state: None,
            locality: None,
            san: default_san(),
            days: default_days(),
        }
    }
}

fn default_cn() -> String {
    "localhost".to_string()
}

fn default_san() -> Vec<String> {
    vec!["localhost".to_string(), "127.0.0.1".to_string()]
}

fn default_days() -> u32 {
    365
}

/// `[base.tls.engine]` — cipher suites and TLS version.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TlsEngineConfig {
    #[serde(default = "default_min_version")]
    pub min_version: String,

    #[serde(default)]
    pub ciphers: Vec<String>,
}

impl Default for TlsEngineConfig {
    fn default() -> Self {
        Self {
            min_version: default_min_version(),
            ciphers: Vec::new(),
        }
    }
}

fn default_min_version() -> String {
    "tls12".to_string()
}

// ── [base.admin_api] ────────────────────────────────────────────────

/// `[base.admin_api]` — operational admin server on a separate socket.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AdminApiConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_admin_host")]
    pub host: String,

    pub port: Option<u16>,
    /// Ed25519 public key for verifying admin API request signatures (hex-encoded 32-byte seed).
    pub public_key: Option<String>,
    /// Ed25519 private key — used by riversctl for signing requests, NOT used by riversd.
    /// Included in config for tool integration (riversctl reads this when RIVERS_ADMIN_KEY is not set).
    pub private_key: Option<String>,

    /// Skip Ed25519 signature verification (development only).
    ///
    /// Per spec §15.3: `--no-admin-auth` CLI flag maps to this field.
    #[serde(default)]
    pub no_auth: Option<bool>,

    #[serde(default)]
    pub tls: Option<AdminTlsConfig>,

    #[serde(default)]
    pub rbac: Option<RbacConfig>,
}

fn default_admin_host() -> String {
    "127.0.0.1".to_string()
}

/// TLS config for the admin API.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdminTlsConfig {
    pub ca_cert: Option<String>,
    pub server_cert: Option<String>,
    pub server_key: Option<String>,
    #[serde(default)]
    pub require_client_cert: bool,
}

impl Default for AdminTlsConfig {
    fn default() -> Self {
        Self {
            ca_cert: None,
            server_cert: None,
            server_key: None,
            require_client_cert: false,
        }
    }
}

/// RBAC config for the admin API.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RbacConfig {
    #[serde(default)]
    pub roles: std::collections::HashMap<String, Vec<String>>,
    #[serde(default)]
    pub bindings: std::collections::HashMap<String, String>,
}

// ── [base.cluster] ──────────────────────────────────────────────────

/// `[base.cluster]` — clustering and session store settings.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ClusterConfig {
    #[serde(default)]
    pub session_store: SessionStoreConfig,
}

/// `[base.cluster.session_store]` — session persistence.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SessionStoreConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,
}

impl Default for SessionStoreConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cookie_name: default_cookie_name(),
        }
    }
}

fn default_cookie_name() -> String {
    "rivers_session".to_string()
}

// ── [security] ──────────────────────────────────────────────────────

/// `[security]` — CORS, rate limiting, IP allowlists.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SecurityConfig {
    #[serde(default)]
    pub cors_enabled: bool,

    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,

    #[serde(default)]
    pub cors_allowed_methods: Vec<String>,

    #[serde(default)]
    pub cors_allowed_headers: Vec<String>,

    #[serde(default)]
    pub cors_allow_credentials: bool,

    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u32,

    #[serde(default = "default_burst_size")]
    pub rate_limit_burst_size: u32,

    #[serde(default = "default_rate_strategy")]
    pub rate_limit_strategy: String,

    pub rate_limit_custom_header: Option<String>,

    #[serde(default)]
    pub admin_ip_allowlist: Vec<String>,

    #[serde(default)]
    pub session: SessionConfig,

    #[serde(default)]
    pub csrf: CsrfConfig,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            cors_enabled: false,
            cors_allowed_origins: Vec::new(),
            cors_allowed_methods: Vec::new(),
            cors_allowed_headers: Vec::new(),
            cors_allow_credentials: false,
            rate_limit_per_minute: default_rate_limit(),
            rate_limit_burst_size: default_burst_size(),
            rate_limit_strategy: default_rate_strategy(),
            rate_limit_custom_header: None,
            admin_ip_allowlist: Vec::new(),
            session: SessionConfig::default(),
            csrf: CsrfConfig::default(),
        }
    }
}

// ── [security.csrf] ─────────────────────────────────────────────────

/// `[security.csrf]` — CSRF protection configuration.
/// Per `rivers-auth-session-spec.md` §9.5.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CsrfConfig {
    /// Enable CSRF protection (default: true).
    pub enabled: bool,

    /// Minimum seconds between token rotations (default: 300).
    pub csrf_rotation_interval_s: u64,

    /// CSRF cookie name (default: "rivers_csrf").
    pub cookie_name: String,

    /// CSRF header name (default: "X-CSRF-Token").
    pub header_name: String,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            csrf_rotation_interval_s: 300,
            cookie_name: "rivers_csrf".to_string(),
            header_name: "X-CSRF-Token".to_string(),
        }
    }
}

// ── [security.session] ──────────────────────────────────────────────

/// `[security.session]` — session management configuration.
/// Per `rivers-auth-session-spec.md` §4.3, §8.1.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SessionConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Absolute session lifetime in seconds from creation (default: 3600).
    #[serde(default = "default_session_ttl")]
    pub ttl_s: u64,

    /// Inactivity timeout in seconds from last_seen (default: 1800).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_s: u64,

    #[serde(default)]
    pub cookie: SessionCookieConfig,

    /// When true, the session token is included in the JSON response body
    /// of the guard handler's success response. Useful for SPAs that store
    /// tokens in memory rather than relying solely on cookies.
    /// Default: false.
    #[serde(default)]
    pub include_token_in_body: bool,

    /// JSON key name for the session token when `include_token_in_body` is true.
    /// Default: "token".
    #[serde(default = "default_token_body_key")]
    pub token_body_key: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_s: default_session_ttl(),
            idle_timeout_s: default_idle_timeout(),
            cookie: SessionCookieConfig::default(),
            include_token_in_body: false,
            token_body_key: default_token_body_key(),
        }
    }
}

fn default_token_body_key() -> String {
    "token".to_string()
}

fn default_session_ttl() -> u64 {
    3600
}

fn default_idle_timeout() -> u64 {
    1800
}

/// `[security.session.cookie]` — session cookie attributes.
/// Per spec §8.1: http_only=true is enforced and not configurable to false.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SessionCookieConfig {
    pub name: String,

    /// Always true — enforced. Config validation rejects false.
    pub http_only: bool,

    /// HTTPS only. Default true, can be false for local dev (emits warning).
    pub secure: bool,

    /// "Strict" | "Lax" | "None". Default: "Lax".
    pub same_site: String,

    pub path: String,

    /// Not set by default (current domain only).
    pub domain: Option<String>,
}

impl SessionCookieConfig {
    /// Validate session cookie security invariants.
    ///
    /// Per spec §8.1: http_only=true is mandatory. Setting it to false is a
    /// configuration error — session cookies must never be readable by JavaScript.
    pub fn validate(&self) -> Result<(), String> {
        if !self.http_only {
            return Err(
                "session cookie http_only must be true — setting http_only=false is a security violation".into(),
            );
        }
        Ok(())
    }
}

impl Default for SessionCookieConfig {
    fn default() -> Self {
        Self {
            name: default_cookie_name(),
            http_only: true,
            secure: true,
            same_site: "Lax".to_string(),
            path: "/".to_string(),
            domain: None,
        }
    }
}

fn default_rate_limit() -> u32 {
    120
}

fn default_burst_size() -> u32 {
    60
}

fn default_rate_strategy() -> String {
    "ip".to_string()
}

// ── [static_files] ──────────────────────────────────────────────────

/// `[static_files]` — static file serving and SPA fallback.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StaticFilesConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub root_path: Option<String>,

    #[serde(default = "default_index_file")]
    pub index_file: String,

    #[serde(default)]
    pub spa_fallback: bool,

    #[serde(default)]
    pub max_age: Option<u64>,

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

// ── [storage_engine] ────────────────────────────────────────────────

/// `[base.storage_engine]` — internal KV + queue backend config.
/// Per `rivers-storage-engine-spec.md`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StorageEngineConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: String,

    pub path: Option<String>,
    pub url: Option<String>,
    pub credentials_source: Option<String>,
    pub key_prefix: Option<String>,

    #[serde(default)]
    pub pool_size: Option<usize>,

    #[serde(default = "default_retention_ms")]
    pub retention_ms: u64,

    #[serde(default = "default_max_events")]
    pub max_events: u64,

    #[serde(default = "default_sweep_interval")]
    pub sweep_interval_s: u64,

    /// Cache policy configuration.
    ///
    /// Per technology-path-spec §16.1: caching config lives on StorageEngine,
    /// not on DataViews.
    #[serde(default)]
    pub cache: CacheConfig,
}

impl Default for StorageEngineConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            path: None,
            url: None,
            credentials_source: None,
            key_prefix: None,
            pool_size: None,
            retention_ms: default_retention_ms(),
            max_events: default_max_events(),
            sweep_interval_s: default_sweep_interval(),
            cache: CacheConfig::default(),
        }
    }
}

/// Cache policy configuration on the StorageEngine.
///
/// Per technology-path-spec §16.1: caching config lives on StorageEngine,
/// not on DataViews.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CacheConfig {
    /// Per-datasource cache defaults.
    #[serde(default)]
    pub datasources: std::collections::HashMap<String, DatasourceCacheConfig>,

    /// Per-DataView cache overrides.
    #[serde(default)]
    pub dataviews: std::collections::HashMap<String, DataViewCacheOverride>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DatasourceCacheConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_cache_ttl")]
    pub ttl_seconds: u64,

    /// "dataview" (default) or "datasource"
    #[serde(default = "default_invalidation_strategy")]
    pub invalidation_strategy: String,
}

fn default_cache_ttl() -> u64 { 120 }
fn default_invalidation_strategy() -> String { "dataview".to_string() }

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataViewCacheOverride {
    pub ttl_seconds: Option<u64>,
}

fn default_storage_backend() -> String {
    "memory".to_string()
}

fn default_retention_ms() -> u64 {
    86_400_000 // 24h
}

fn default_max_events() -> u64 {
    100_000
}

fn default_sweep_interval() -> u64 {
    60
}

// ── [runtime] ───────────────────────────────────────────────────────

/// `[runtime]` — ProcessPool runtime configuration.
/// Per `rivers-processpool-runtime-spec-v2.md` §8-9.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RuntimeConfig {
    /// Named process pools. Key is pool name (e.g. "default", "wasm").
    #[serde(default)]
    pub process_pools: std::collections::HashMap<String, ProcessPoolConfig>,
}

/// `[runtime.process_pools.<name>]` — per-pool configuration.
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

/// `[environment_overrides.{env}]` — per-environment config overrides.
/// Fields mirror `BaseConfig` and `SecurityConfig` but are all optional.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EnvironmentOverride {
    pub base: Option<BaseOverride>,
    pub security: Option<SecurityOverride>,
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
    pub host: Option<String>,
    pub port: Option<u16>,
    pub workers: Option<u32>,
    pub request_timeout_seconds: Option<u64>,
    pub log_level: Option<LogLevel>,
    pub backpressure: Option<BackpressureOverride>,
}

/// Partial `[base.backpressure]` override.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct BackpressureOverride {
    pub enabled: Option<bool>,
    pub queue_depth: Option<usize>,
    pub queue_timeout_ms: Option<u64>,
}

/// Partial `[security]` override.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SecurityOverride {
    pub cors_enabled: Option<bool>,
    pub cors_allowed_origins: Option<Vec<String>>,
    pub rate_limit_per_minute: Option<u32>,
    pub rate_limit_burst_size: Option<u32>,
}

/// Partial `[storage_engine]` override.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct StorageEngineOverride {
    pub backend: Option<String>,
    pub url: Option<String>,
    pub credentials_source: Option<String>,
    pub key_prefix: Option<String>,
    pub pool_size: Option<usize>,
}

// ── [base.logging] ──────────────────────────────────────────────────

/// `[base.logging]` — log output configuration.
/// Per `rivers-logging-spec.md` §9.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LoggingConfig {
    #[serde(default)]
    pub level: LogLevel,

    #[serde(default = "default_log_format")]
    pub format: String,

    pub local_file_path: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: default_log_format(),
            local_file_path: None,
        }
    }
}

fn default_log_format() -> String {
    "json".to_string()
}

// ── [engines] ────────────────────────────────────────────────────────

/// `[engines]` — directory containing engine shared libraries.
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

/// `[plugins]` — directory containing driver plugin shared libraries.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PluginsConfig {
    /// Path to plugin shared library directory (default: "plugins").
    #[serde(default = "default_plugins_dir")]
    pub dir: String,
    /// Driver names to ignore — these plugins will not be loaded.
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

/// `[graphql]` — GraphQL endpoint configuration at the server level.
///
/// Per `rivers-view-layer-spec.md` §9.
/// The full schema-building config lives in `riversd::graphql::GraphqlConfig`;
/// this is the minimal enable/path config for ServerConfig.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GraphqlServerConfig {
    #[serde(default)]
    pub enabled: bool,

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

// ── Helpers ─────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

// ── Schema Generation ────────────────────────────────────────────

/// Generate JSON Schema for `ServerConfig` (the `riversd.conf` format).
pub fn server_config_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ServerConfig)).unwrap_or_default()
}

#[cfg(test)]
mod tls_config_tests {
    use super::*;

    #[test]
    fn tls_config_parses_with_cert_paths() {
        let toml = r#"
            [base.tls]
            cert = "/etc/rivers/server.crt"
            key = "/etc/rivers/server.key"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let tls = cfg.base.tls.unwrap();
        assert_eq!(tls.cert.unwrap(), "/etc/rivers/server.crt");
        assert_eq!(tls.key.unwrap(), "/etc/rivers/server.key");
    }

    #[test]
    fn tls_config_parses_without_cert_paths() {
        let toml = r#"
            [base.tls]
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let tls = cfg.base.tls.unwrap();
        assert!(tls.cert.is_none());
        assert!(tls.key.is_none());
        assert_eq!(tls.redirect_port, 80);
        assert!(tls.redirect);
    }

    #[test]
    fn tls_config_x509_defaults() {
        let toml = r#"
            [base.tls]
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let tls = cfg.base.tls.unwrap();
        assert_eq!(tls.x509.san, vec!["localhost", "127.0.0.1"]);
        assert_eq!(tls.x509.days, 365);
    }

    #[test]
    fn admin_tls_config_optional_fields() {
        let toml = r#"
            [base.admin_api.tls]
            require_client_cert = false
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let admin_tls = cfg.base.admin_api.tls.unwrap();
        assert!(admin_tls.server_cert.is_none());
        assert!(admin_tls.server_key.is_none());
        assert!(admin_tls.ca_cert.is_none());
        assert!(!admin_tls.require_client_cert);
    }

    #[test]
    fn http2_config_has_no_tls_fields() {
        // Exhaustive destructuring — compile error if any new field (e.g. tls_cert) is added
        let Http2Config { enabled: _, initial_window_size: _, max_concurrent_streams: _ } = Http2Config::default();
    }
}
