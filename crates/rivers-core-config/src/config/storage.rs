//! Storage engine and cache configuration types.

use schemars::JsonSchema;
use serde::Deserialize;

// ── [storage_engine] ────────────────────────────────────────────────

/// `[base.storage_engine]` -- internal KV + queue backend config.
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
    /// Per technology-path-spec S16.1: caching config lives on StorageEngine,
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
/// Per technology-path-spec S16.1: caching config lives on StorageEngine,
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
