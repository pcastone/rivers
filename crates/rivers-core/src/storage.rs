//! Internal KV storage infrastructure — re-exported from rivers-core-config.
//!
//! The StorageEngine trait, InMemoryStorageEngine, and types live in
//! rivers-core-config. This module re-exports them and adds:
//! - `create_storage_engine()` factory
//! - Sentinel key management
//! - Background sweep task

use std::sync::Arc;
use std::time::Duration;

// Re-export everything from rivers-core-config::storage
pub use rivers_core_config::storage::*;

use crate::config::StorageEngineConfig;

// ── Factory ─────────────────────────────────────────────────────────

/// Returns the names of config fields that are parsed but not yet enforced at runtime.
///
/// Callers should emit a `tracing::warn!` when this returns a non-empty list, so
/// operators learn that their config has no effect rather than silently ignoring it.
pub fn unenforced_storage_config_fields(config: &StorageEngineConfig) -> Vec<&'static str> {
    let defaults = StorageEngineConfig::default();
    let mut out = Vec::new();
    if config.retention_ms != defaults.retention_ms {
        out.push("retention_ms");
    }
    if config.max_events != defaults.max_events {
        out.push("max_events");
    }
    if !config.cache.datasources.is_empty() {
        out.push("cache.datasources");
    }
    if !config.cache.dataviews.is_empty() {
        out.push("cache.dataviews");
    }
    out
}

/// Create a storage engine from configuration.
///
/// Supported backends: `memory`, `sqlite`, `redis`.
pub fn create_storage_engine(
    config: &StorageEngineConfig,
) -> Result<Box<dyn StorageEngine>, StorageError> {
    match config.backend.as_str() {
        "memory" => Ok(Box::new(InMemoryStorageEngine::new())),
        #[cfg(feature = "storage-backends")]
        "sqlite" => {
            let path = config.path.as_deref().ok_or_else(|| {
                StorageError::Backend("sqlite backend requires `path` config".into())
            })?;
            let engine = rivers_storage_backends::SqliteStorageEngine::new(path)?;
            Ok(Box::new(engine))
        }
        #[cfg(feature = "storage-backends")]
        "redis" => {
            let url = config.url.as_deref().ok_or_else(|| {
                StorageError::Backend("redis backend requires `url` config".into())
            })?;
            let prefix = config.key_prefix.as_deref().unwrap_or("rivers:");
            let engine = rivers_storage_backends::RedisStorageEngine::with_prefix(url, prefix)?;
            Ok(Box::new(engine))
        }
        other => Err(StorageError::Backend(format!(
            "unknown storage backend: {}",
            other
        ))),
    }
}

// ── Sentinel Key (SHAPE-8) ──────────────────────────────────────────

/// Sentinel key namespace for single-node enforcement.
const SENTINEL_NAMESPACE: &str = "rivers:node";

/// Sentinel heartbeat TTL in milliseconds (30 seconds).
const SENTINEL_TTL_MS: u64 = 30_000;

/// Attempt to claim the sentinel key for this node.
pub async fn claim_sentinel(
    engine: &dyn StorageEngine,
    node_id: &str,
) -> Result<(), StorageError> {
    let key = node_id.to_string();

    // Check if any other node has an active sentinel first
    let existing = engine.list_keys(SENTINEL_NAMESPACE, None).await?;
    for existing_key in &existing {
        if *existing_key != key {
            return Err(StorageError::Backend(format!(
                "another node already active: {}",
                existing_key
            )));
        }
    }

    let claimed = engine
        .set_if_absent(SENTINEL_NAMESPACE, &key, node_id.as_bytes().to_vec(), Some(SENTINEL_TTL_MS))
        .await?;

    if !claimed {
        if let Some(existing) = engine.get(SENTINEL_NAMESPACE, &key).await? {
            if existing == node_id.as_bytes() {
                return engine
                    .set(SENTINEL_NAMESPACE, &key, node_id.as_bytes().to_vec(), Some(SENTINEL_TTL_MS))
                    .await;
            }
        }
        Err(StorageError::Backend(
            "sentinel claim failed: another node claimed concurrently".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Refresh the sentinel heartbeat for this node.
pub async fn refresh_sentinel(
    engine: &dyn StorageEngine,
    node_id: &str,
) -> Result<(), StorageError> {
    let key = node_id.to_string();
    engine
        .set(SENTINEL_NAMESPACE, &key, node_id.as_bytes().to_vec(), Some(SENTINEL_TTL_MS))
        .await
}

/// Release the sentinel key for this node.
pub async fn release_sentinel(
    engine: &dyn StorageEngine,
    node_id: &str,
) -> Result<(), StorageError> {
    let key = node_id.to_string();
    engine.delete(SENTINEL_NAMESPACE, &key).await
}

// ── Sweep task ──────────────────────────────────────────────────────

/// Spawn a background task that calls `flush_expired()` at regular intervals.
pub fn spawn_sweep_task(
    engine: Arc<dyn StorageEngine>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            match engine.flush_expired().await {
                Ok(count) if count > 0 => {
                    tracing::debug!(removed = count, "storage sweep completed");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "storage sweep failed");
                }
                _ => {}
            }
        }
    })
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_memory_backend() {
        let config = StorageEngineConfig::default(); // backend = "memory"
        let engine = create_storage_engine(&config);
        assert!(engine.is_ok());
    }

    #[cfg(feature = "storage-backends")]
    #[test]
    fn create_sqlite_backend() {
        let mut config = StorageEngineConfig::default();
        config.backend = "sqlite".into();
        config.path = Some(":memory:".into());
        let engine = create_storage_engine(&config);
        assert!(engine.is_ok());
    }

    #[test]
    fn create_unknown_backend_fails() {
        let mut config = StorageEngineConfig::default();
        config.backend = "unknown".into();
        let engine = create_storage_engine(&config);
        assert!(engine.is_err());
    }

    #[cfg(feature = "storage-backends")]
    #[test]
    fn sqlite_backend_requires_path() {
        let mut config = StorageEngineConfig::default();
        config.backend = "sqlite".into();
        let engine = create_storage_engine(&config);
        assert!(engine.is_err());
    }

    #[cfg(feature = "storage-backends")]
    #[test]
    fn redis_backend_requires_url() {
        let mut config = StorageEngineConfig::default();
        config.backend = "redis".into();
        let engine = create_storage_engine(&config);
        assert!(engine.is_err());
    }

    #[test]
    fn default_backend_is_memory() {
        let config = StorageEngineConfig::default();
        assert_eq!(config.backend, "memory");
    }

    #[test]
    fn reserved_namespace_check() {
        assert!(is_reserved_namespace("session:abc"));
        assert!(is_reserved_namespace("csrf:token"));
        assert!(is_reserved_namespace("poll:xyz"));
        assert!(is_reserved_namespace("raft:state"));
        assert!(is_reserved_namespace("cache:dataview"));
        assert!(is_reserved_namespace("rivers:node"));
        assert!(!is_reserved_namespace("user:data"));
    }

    /// G_R3: the canonical reserved-prefix list lives in
    /// `rivers-core-config::storage::RESERVED_PREFIXES`. Asserting from this
    /// crate's perspective — `is_reserved_namespace` is the re-exported public
    /// API — both `poll:` and `raft:` MUST resolve to true. Historically these
    /// two crates each tracked their own list and drifted.
    #[test]
    fn reserved_prefix_list_includes_poll_and_raft() {
        assert!(is_reserved_namespace("poll:foo"), "core sees poll: as reserved");
        assert!(is_reserved_namespace("raft:foo"), "core sees raft: as reserved");
    }

    // ── RW3.3.b: unenforced storage config field detection ──────────────

    #[test]
    fn unenforced_fields_empty_for_defaults() {
        let config = StorageEngineConfig::default();
        assert!(unenforced_storage_config_fields(&config).is_empty());
    }

    #[test]
    fn unenforced_fields_reports_nondefault_retention_ms() {
        let mut config = StorageEngineConfig::default();
        config.retention_ms = 3_600_000; // 1h vs default 24h
        let fields = unenforced_storage_config_fields(&config);
        assert!(fields.contains(&"retention_ms"), "expected retention_ms in {fields:?}");
    }

    #[test]
    fn unenforced_fields_reports_nondefault_max_events() {
        let mut config = StorageEngineConfig::default();
        config.max_events = 500;
        let fields = unenforced_storage_config_fields(&config);
        assert!(fields.contains(&"max_events"), "expected max_events in {fields:?}");
    }

    #[test]
    fn unenforced_fields_reports_nonempty_cache_datasources() {
        use crate::config::DatasourceCacheConfig;
        let mut config = StorageEngineConfig::default();
        config.cache.datasources.insert("ds1".into(), DatasourceCacheConfig {
            enabled: true,
            ttl_seconds: 60,
            invalidation_strategy: "dataview".into(),
        });
        let fields = unenforced_storage_config_fields(&config);
        assert!(fields.contains(&"cache.datasources"), "expected cache.datasources in {fields:?}");
    }

    #[test]
    fn unenforced_fields_reports_nonempty_cache_dataviews() {
        use crate::config::DataViewCacheOverride;
        let mut config = StorageEngineConfig::default();
        config.cache.dataviews.insert("dv1".into(), DataViewCacheOverride { ttl_seconds: Some(30) });
        let fields = unenforced_storage_config_fields(&config);
        assert!(fields.contains(&"cache.dataviews"), "expected cache.dataviews in {fields:?}");
    }

    #[test]
    fn unenforced_fields_reports_all_set_at_once() {
        use crate::config::{DatasourceCacheConfig, DataViewCacheOverride};
        let mut config = StorageEngineConfig::default();
        config.retention_ms = 1_000;
        config.max_events = 10;
        config.cache.datasources.insert("ds1".into(), DatasourceCacheConfig {
            enabled: true,
            ttl_seconds: 60,
            invalidation_strategy: "dataview".into(),
        });
        config.cache.dataviews.insert("dv1".into(), DataViewCacheOverride { ttl_seconds: Some(30) });
        let fields = unenforced_storage_config_fields(&config);
        assert_eq!(fields.len(), 4, "expected 4 unenforced fields, got {fields:?}");
    }
}
