//! Two-tier DataView cache — L1 in-process LRU + L2 StorageEngine-backed.
//!
//! Per `rivers-data-layer-spec.md` §7, `rivers-storage-engine-spec.md` §5.
//!
//! L1 is fast, per-node, bounded by entry count with LRU eviction and lazy TTL.
//! L2 is shared via StorageEngine, using KV get/set with JSON serialization.
//! Cache keys are stable across nodes: `SHA-256(view_name:sorted_params_json)`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Sha256, Digest};

use async_trait::async_trait;
use tokio::sync::Mutex;

use rivers_core::storage::StorageEngine;
use rivers_driver_sdk::types::{QueryResult, QueryValue};

use crate::dataview_engine::DataViewError;

// ── DataViewCachingPolicy ─────────────────────────────────────────

/// Per-view caching policy.
///
/// Per spec §7.2 / §5.6.
#[derive(Debug, Clone)]
pub struct DataViewCachingPolicy {
    pub ttl_seconds: u64,
    /// L1 in-process LRU cache enabled. Default: true.
    pub l1_enabled: bool,
    /// Max L1 memory in bytes. Default: 157,286,400 (150 MB).
    pub l1_max_bytes: usize,
    /// Hard cap on L1 entry count. Default: 100,000 (safety valve).
    pub l1_max_entries: usize,
    /// L2 StorageEngine cache enabled. Default: false.
    pub l2_enabled: bool,
    /// Results larger than this (serialized bytes) skip L2. Default: 131072 (128 KB).
    pub l2_max_value_bytes: usize,
}

/// 150 MB default L1 cache size.
pub const DEFAULT_L1_MAX_BYTES: usize = 150 * 1024 * 1024;

impl Default for DataViewCachingPolicy {
    fn default() -> Self {
        Self {
            ttl_seconds: 60,
            l1_enabled: true,
            l1_max_bytes: DEFAULT_L1_MAX_BYTES,
            l1_max_entries: 100_000,
            l2_enabled: false,
            l2_max_value_bytes: 131_072,
        }
    }
}

// ── Cache Key ─────────────────────────────────────────────────────

/// Generate a stable cache key from view name and parameters.
///
/// Per spec §5.4 / SHAPE-3: `SHA-256(view_name + ":" + sorted_params_json)`.
/// Uses BTreeMap for stable parameter ordering, SHA-256 for cross-node stability.
pub fn cache_key(view_name: &str, parameters: &HashMap<String, QueryValue>) -> String {
    // Sort params into BTreeMap for deterministic ordering
    let sorted: BTreeMap<&String, &QueryValue> = parameters.iter().collect();
    let params_json = serde_json::to_string(&sorted).unwrap_or_default();
    let input = format!("{}:{}", view_name, params_json);

    // SHA-256 for cross-node deterministic keys
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let hash = hex::encode(hasher.finalize());
    format!("cache:views:{}:{}", view_name, hash)
}

// ── Cache Trait ───────────────────────────────────────────────────

/// DataView cache trait.
///
/// Per spec §7.3. Returns `Arc<QueryResult>` to avoid deep clones on cache hits.
#[async_trait]
pub trait DataViewCache: Send + Sync {
    /// Look up a cached result. Returns Arc to avoid cloning large result sets.
    async fn get(
        &self,
        view_name: &str,
        parameters: &HashMap<String, QueryValue>,
    ) -> Result<Option<Arc<QueryResult>>, DataViewError>;

    /// Store a result in the cache.
    ///
    /// `ttl_override` allows per-view TTL from `DataViewCachingConfig.ttl_seconds`.
    /// When `None`, the cache uses its default policy TTL.
    async fn set(
        &self,
        view_name: &str,
        parameters: &HashMap<String, QueryValue>,
        result: &QueryResult,
        ttl_override: Option<u64>,
    ) -> Result<(), DataViewError>;

    /// Invalidate cache entries for a specific view, or all entries if None.
    async fn invalidate(&self, view_name: Option<&str>);
}

// ── Noop Cache ────────────────────────────────────────────────────

/// No-op cache that always misses.
///
/// Per spec §7.3 — the default when no cache is configured.
pub struct NoopDataViewCache;

#[async_trait]
impl DataViewCache for NoopDataViewCache {
    async fn get(
        &self,
        _view_name: &str,
        _parameters: &HashMap<String, QueryValue>,
    ) -> Result<Option<Arc<QueryResult>>, DataViewError> {
        Ok(None)
    }

    async fn set(
        &self,
        _view_name: &str,
        _parameters: &HashMap<String, QueryValue>,
        _result: &QueryResult,
        _ttl_override: Option<u64>,
    ) -> Result<(), DataViewError> {
        Ok(())
    }

    async fn invalidate(&self, _view_name: Option<&str>) {}
}

// ── L1 — LRU Cache ───────────────────────────────────────────────

/// A cached result with expiry tracking.
struct CachedEntry {
    result: Arc<QueryResult>,
    expires_at: Instant,
    /// Estimated heap size in bytes (for memory-bounded eviction).
    size_bytes: usize,
}

/// L1 in-process LRU cache with memory-bounded eviction.
///
/// Per spec §5.2. Uses HashMap for O(1) key lookup + VecDeque for LRU order.
/// Evicts LRU entries when total memory exceeds `max_bytes` or count exceeds `max_entries`.
pub struct LruDataViewCache {
    /// O(1) key → entry lookup.
    map: Mutex<HashMap<String, CachedEntry>>,
    /// LRU order — least recently used at the front, most recent at back.
    order: Mutex<VecDeque<String>>,
    /// Memory limit in bytes.
    max_bytes: usize,
    /// Hard cap on entry count (safety valve).
    max_entries: usize,
    /// Current total estimated bytes.
    total_bytes: Mutex<usize>,
    ttl: Duration,
}

impl LruDataViewCache {
    /// Create a new memory-bounded L1 cache.
    pub fn new(max_bytes: usize, max_entries: usize, ttl_seconds: u64) -> Self {
        let initial_cap = max_entries.min(4096);
        Self {
            map: Mutex::new(HashMap::with_capacity(initial_cap)),
            order: Mutex::new(VecDeque::with_capacity(initial_cap)),
            max_bytes,
            max_entries,
            total_bytes: Mutex::new(0),
            ttl: Duration::from_secs(ttl_seconds),
        }
    }

    /// Get a cached result by key. Returns None on miss or expiry.
    /// Returns Arc clone (cheap pointer bump, no deep copy).
    pub async fn get(&self, key: &str) -> Option<Arc<QueryResult>> {
        let mut map = self.map.lock().await;
        let now = Instant::now();

        match map.get(key) {
            Some(entry) if now >= entry.expires_at => {
                // Expired — remove from both structures
                let removed = map.remove(key).unwrap();
                *self.total_bytes.lock().await -= removed.size_bytes;
                let mut order = self.order.lock().await;
                if let Some(pos) = order.iter().position(|k| k == key) {
                    order.remove(pos);
                }
                None
            }
            Some(entry) => {
                let result = Arc::clone(&entry.result);
                // Move to back (most recently used)
                let mut order = self.order.lock().await;
                if let Some(pos) = order.iter().position(|k| k == key) {
                    order.remove(pos);
                }
                order.push_back(key.to_string());
                Some(result)
            }
            None => None,
        }
    }

    /// Set a cached result. Evicts LRU entries when memory or count limit is exceeded.
    ///
    /// `ttl_override` allows per-view TTL; falls back to the default `self.ttl`.
    pub async fn set(&self, key: String, result: Arc<QueryResult>, ttl_override: Option<Duration>) {
        let effective_ttl = ttl_override.unwrap_or(self.ttl);

        // TTL=0 means "no caching" — don't store the entry
        if effective_ttl.is_zero() {
            return;
        }

        let entry_bytes = result.estimated_bytes();

        let mut map = self.map.lock().await;
        let mut order = self.order.lock().await;
        let mut total = self.total_bytes.lock().await;
        let now = Instant::now();

        // Remove existing entry with same key
        if let Some(old) = map.remove(&key) {
            *total -= old.size_bytes;
            if let Some(pos) = order.iter().position(|k| k == &key) {
                order.remove(pos);
            }
        }

        // Evict LRU entries until under memory and count limits
        while (*total + entry_bytes > self.max_bytes || map.len() >= self.max_entries)
            && !order.is_empty()
        {
            if let Some(evicted_key) = order.pop_front() {
                if let Some(evicted) = map.remove(&evicted_key) {
                    *total -= evicted.size_bytes;
                }
            }
        }

        *total += entry_bytes;
        map.insert(
            key.clone(),
            CachedEntry {
                result,
                expires_at: now + effective_ttl,
                size_bytes: entry_bytes,
            },
        );
        order.push_back(key);
    }

    /// Clear entries matching a view name prefix, or all entries.
    pub async fn invalidate(&self, view_name: Option<&str>) {
        let mut map = self.map.lock().await;
        let mut order = self.order.lock().await;
        let mut total = self.total_bytes.lock().await;
        match view_name {
            Some(name) => {
                let prefix = format!("cache:views:{}:", name);
                map.retain(|k, v| {
                    if k.starts_with(&prefix) {
                        *total -= v.size_bytes;
                        false
                    } else {
                        true
                    }
                });
                order.retain(|k| !k.starts_with(&prefix));
            }
            None => {
                map.clear();
                order.clear();
                *total = 0;
            }
        }
    }

    /// Return the number of entries (for testing).
    pub async fn len(&self) -> usize {
        self.map.lock().await.len()
    }

    /// Check if empty.
    pub async fn is_empty(&self) -> bool {
        self.map.lock().await.is_empty()
    }

    /// Return current estimated memory usage in bytes.
    pub async fn total_bytes(&self) -> usize {
        *self.total_bytes.lock().await
    }
}

// ── Tiered Cache ──────────────────────────────────────────────────

/// Two-tier cache combining L1 (LRU) and L2 (StorageEngine).
///
/// Per spec §7.1 / §5.1:
/// - L1 hit → return immediately
/// - L1 miss + L2 hit → warm L1, return
/// - Full miss → caller executes driver, then calls set() to populate
pub struct TieredDataViewCache {
    l1: LruDataViewCache,
    l2: Option<Arc<dyn StorageEngine>>,
    policy: DataViewCachingPolicy,
}

/// L2 StorageEngine namespace for cache entries.
const L2_NAMESPACE: &str = "cache";

impl TieredDataViewCache {
    /// Create a tiered cache with the given policy.
    pub fn new(policy: DataViewCachingPolicy) -> Self {
        let l1 = LruDataViewCache::new(policy.l1_max_bytes, policy.l1_max_entries, policy.ttl_seconds);
        Self {
            l1,
            l2: None,
            policy,
        }
    }

    /// Enable L2 with a StorageEngine backend.
    pub fn with_storage(mut self, storage: Arc<dyn StorageEngine>) -> Self {
        self.l2 = Some(storage);
        self
    }

    /// Get the L1 cache entry count (for testing).
    pub async fn l1_len(&self) -> usize {
        self.l1.len().await
    }
}

#[async_trait]
impl DataViewCache for TieredDataViewCache {
    async fn get(
        &self,
        view_name: &str,
        parameters: &HashMap<String, QueryValue>,
    ) -> Result<Option<Arc<QueryResult>>, DataViewError> {
        let key = cache_key(view_name, parameters);

        // L1 check
        if self.policy.l1_enabled {
            if let Some(result) = self.l1.get(&key).await {
                return Ok(Some(result));
            }
        }

        // L2 check
        if self.policy.l2_enabled {
            if let Some(ref storage) = self.l2 {
                match storage.get(L2_NAMESPACE, &key).await {
                    Ok(Some(bytes)) => {
                        // Deserialize
                        match serde_json::from_slice::<SerializableQueryResult>(&bytes) {
                            Ok(cached) => {
                                let result = Arc::new(cached.into_query_result());
                                // Warm L1 (use default policy TTL for L2→L1 warming)
                                if self.policy.l1_enabled {
                                    self.l1.set(key, Arc::clone(&result), None).await;
                                }
                                return Ok(Some(result));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    key = %key,
                                    error = %e,
                                    "L2 cache deserialization failed, treating as miss"
                                );
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            key = %key,
                            error = %e,
                            "L2 cache read failed, treating as miss"
                        );
                    }
                }
            }
        }

        Ok(None)
    }

    async fn set(
        &self,
        view_name: &str,
        parameters: &HashMap<String, QueryValue>,
        result: &QueryResult,
        ttl_override: Option<u64>,
    ) -> Result<(), DataViewError> {
        let effective_ttl = ttl_override.unwrap_or(self.policy.ttl_seconds);

        // TTL=0 means "no caching"
        if effective_ttl == 0 {
            return Ok(());
        }

        let key = cache_key(view_name, parameters);

        // L2 first (so L1 is always at least as warm as L2)
        if self.policy.l2_enabled {
            if let Some(ref storage) = self.l2 {
                let serializable = SerializableQueryResult::from_query_result(result);
                match serde_json::to_vec(&serializable) {
                    Ok(bytes) => {
                        // Size gate: skip L2 if too large
                        if bytes.len() <= self.policy.l2_max_value_bytes {
                            let ttl_ms = Some(effective_ttl * 1000);
                            if let Err(e) = storage.set(L2_NAMESPACE, &key, bytes, ttl_ms).await {
                                tracing::warn!(
                                    key = %key,
                                    error = %e,
                                    "L2 cache write failed"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            key = %key,
                            error = %e,
                            "L2 cache serialization failed"
                        );
                    }
                }
            }
        }

        // L1 (pass per-view TTL override as Duration)
        if self.policy.l1_enabled {
            let ttl_dur = ttl_override.map(Duration::from_secs);
            self.l1.set(key, Arc::new(result.clone()), ttl_dur).await;
        }

        Ok(())
    }

    async fn invalidate(&self, view_name: Option<&str>) {
        // L1 invalidation (synchronous within lock)
        if self.policy.l1_enabled {
            self.l1.invalidate(view_name).await;
        }

        // L2 invalidation via list_keys + delete
        if self.policy.l2_enabled {
            if let Some(ref storage) = self.l2 {
                let prefix = view_name.map(|name| format!("cache:views:{}:", name));
                match storage
                    .list_keys(L2_NAMESPACE, prefix.as_deref())
                    .await
                {
                    Ok(keys) => {
                        for key in keys {
                            if let Err(e) = storage.delete(L2_NAMESPACE, &key).await {
                                tracing::warn!(
                                    key = %key,
                                    error = %e,
                                    "L2 cache invalidation delete failed"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "L2 cache invalidation list_keys failed"
                        );
                    }
                }
            }
        }
    }
}

// ── Serializable QueryResult ──────────────────────────────────────

/// A serializable wrapper for QueryResult (for L2 storage).
///
/// QueryResult itself doesn't derive Serialize/Deserialize, so we
/// use a thin wrapper.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerializableQueryResult {
    rows: Vec<HashMap<String, QueryValue>>,
    affected_rows: u64,
    last_insert_id: Option<String>,
}

impl SerializableQueryResult {
    fn from_query_result(qr: &QueryResult) -> Self {
        Self {
            rows: qr.rows.clone(),
            affected_rows: qr.affected_rows,
            last_insert_id: qr.last_insert_id.clone(),
        }
    }

    fn into_query_result(self) -> QueryResult {
        QueryResult {
            rows: self.rows,
            affected_rows: self.affected_rows,
            last_insert_id: self.last_insert_id,
        }
    }
}
