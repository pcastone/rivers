//! Internal KV storage infrastructure.
//!
//! Per `rivers-storage-engine-spec.md` / SHAPE-18 (pure KV, no queue).
//!
//! StorageEngine is Rivers internal infrastructure — L2 DataView cache backing,
//! session storage, poll state persistence. Application code never accesses it
//! directly. It is not a datasource.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;

// ── Types ───────────────────────────────────────────────────────────

/// A byte buffer for KV values.
pub type Bytes = Vec<u8>;

// ── Errors ──────────────────────────────────────────────────────────

/// Errors returned by [`StorageEngine`] operations.
#[derive(Error, Debug)]
pub enum StorageError {
    /// Requested key does not exist in the given namespace.
    #[error("not found: {namespace}/{key}")]
    NotFound {
        /// Storage namespace.
        namespace: String,
        /// Key within the namespace.
        key: String,
    },

    /// Value could not be serialized or deserialized.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Underlying backend returned an error.
    #[error("backend error: {0}")]
    Backend(String),

    /// Storage capacity limit reached.
    #[error("capacity exceeded: {0}")]
    Capacity(String),

    /// Backend is not reachable or not initialized.
    #[error("storage unavailable: {0}")]
    Unavailable(String),
}

// ── Reserved prefixes ───────────────────────────────────────────────

/// Reserved namespace prefixes that CodeComponents cannot access.
const RESERVED_PREFIXES: &[&str] = &["session:", "csrf:", "poll:", "cache:", "rivers:"];

/// Check if a namespace uses a reserved prefix.
pub fn is_reserved_namespace(namespace: &str) -> bool {
    RESERVED_PREFIXES.iter().any(|p| namespace.starts_with(p))
}

// ── Trait ───────────────────────────────────────────────────────────

/// Internal storage backend trait (SHAPE-18: pure KV, no queue).
///
/// Operations: get/set/delete/list_keys — keyed by (namespace, key).
/// Plus maintenance: flush_expired removes stale entries.
#[async_trait]
pub trait StorageEngine: Send + Sync {
    /// Retrieve a value by namespace and key. Returns `None` if missing or expired.
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, StorageError>;

    /// Store a value with an optional TTL in milliseconds.
    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<(), StorageError>;

    /// Delete a key from the given namespace.
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError>;

    /// List keys in a namespace, optionally filtered by a prefix.
    async fn list_keys(
        &self,
        namespace: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<String>, StorageError>;

    /// Atomically set a key only if it does not already exist.
    ///
    /// Returns `Ok(true)` if the key was set (didn't exist before),
    /// `Ok(false)` if the key already existed (no change made).
    async fn set_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<bool, StorageError>;

    /// Remove expired KV entries.
    /// Returns the number of entries removed.
    async fn flush_expired(&self) -> Result<u64, StorageError>;
}

// ── InMemory backend ────────────────────────────────────────────────

struct KvEntry {
    value: Bytes,
    expires_at: Option<u64>, // unix ms
}

/// In-memory storage backend.
///
/// Data is lost on restart. Suitable for development and testing.
/// TTL is enforced lazily on `get` and eagerly on `flush_expired`.
pub struct InMemoryStorageEngine {
    kv: Arc<Mutex<HashMap<(String, String), KvEntry>>>,
}

impl InMemoryStorageEngine {
    /// Create a new empty in-memory storage backend.
    pub fn new() -> Self {
        Self {
            kv: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryStorageEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[async_trait]
impl StorageEngine for InMemoryStorageEngine {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, StorageError> {
        let mut kv = self.kv.lock().await;
        let compound = (namespace.to_string(), key.to_string());

        match kv.get(&compound) {
            Some(entry) => {
                // Lazy TTL check
                if let Some(expires_at) = entry.expires_at {
                    if now_ms() >= expires_at {
                        kv.remove(&compound);
                        return Ok(None);
                    }
                }
                Ok(Some(entry.value.clone()))
            }
            None => Ok(None),
        }
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<(), StorageError> {
        let expires_at = ttl_ms.map(|ttl| now_ms() + ttl);
        let mut kv = self.kv.lock().await;
        kv.insert(
            (namespace.to_string(), key.to_string()),
            KvEntry { value, expires_at },
        );
        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        let mut kv = self.kv.lock().await;
        kv.remove(&(namespace.to_string(), key.to_string()));
        Ok(())
    }

    async fn list_keys(
        &self,
        namespace: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<String>, StorageError> {
        let mut kv = self.kv.lock().await;
        let now = now_ms();

        // Lazy-delete expired entries (consistent with get() behavior)
        let expired: Vec<(String, String)> = kv
            .iter()
            .filter(|(_, entry)| entry.expires_at.is_some_and(|exp| now >= exp))
            .map(|((ns, key), _)| (ns.clone(), key.clone()))
            .collect();
        for key in &expired {
            kv.remove(key);
        }

        let keys: Vec<String> = kv
            .iter()
            .filter(|((ns, key), _)| {
                ns == namespace
                    && prefix.is_none_or(|p| key.starts_with(p))
            })
            .map(|((_, key), _)| key.clone())
            .collect();
        Ok(keys)
    }

    async fn set_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Bytes,
        ttl_ms: Option<u64>,
    ) -> Result<bool, StorageError> {
        let now = now_ms();
        let expires_at = ttl_ms.map(|ttl| now + ttl);
        let mut kv = self.kv.lock().await;
        let compound = (namespace.to_string(), key.to_string());

        // Check if key exists and is not expired
        if let Some(entry) = kv.get(&compound) {
            if entry.expires_at.is_none_or(|exp| now < exp) {
                // Key exists and is still live — do not overwrite
                return Ok(false);
            }
            // Key exists but is expired — fall through to insert
        }

        kv.insert(compound, KvEntry { value, expires_at });
        Ok(true)
    }

    async fn flush_expired(&self) -> Result<u64, StorageError> {
        let now = now_ms();
        let mut count = 0u64;

        // Sweep expired KV entries
        let mut kv = self.kv.lock().await;
        let before = kv.len();
        kv.retain(|_, entry| entry.expires_at.is_none_or(|exp| now < exp));
        count += (before - kv.len()) as u64;

        Ok(count)
    }
}
