//! Polling view support.
//!
//! Per `rivers-polling-views-spec.md`.
//!
//! Rivers-managed poll loops for SSE/WS views with diff strategies
//! and client deduplication.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

use rivers_runtime::rivers_core::storage::StorageEngine;

// ── Diff Strategy ───────────────────────────────────────────────

/// Diff strategy for determining whether polled data has changed.
///
/// Per spec: hash, null, or change_detect (CodeComponent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffStrategy {
    /// SHA-256 of canonical JSON — change if hash differs.
    Hash,
    /// Non-empty presence check — change if result is non-null/non-empty.
    Null,
    /// User CodeComponent receives prev + current, decides.
    ChangeDetect,
}

impl DiffStrategy {
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            Some(s) if s.eq_ignore_ascii_case("null") => DiffStrategy::Null,
            Some(s) if s.eq_ignore_ascii_case("change_detect") => DiffStrategy::ChangeDetect,
            _ => DiffStrategy::Hash, // default
        }
    }
}

// ── Poll Loop Key ───────────────────────────────────────────────

/// Key for a poll loop instance: `poll:{view_id}:{param_hash}`.
///
/// Per spec: multiple clients with same parameters share one poll loop.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PollLoopKey {
    pub view_id: String,
    pub param_hash: String,
}

impl PollLoopKey {
    pub fn new(view_id: &str, params: &HashMap<String, String>) -> Self {
        let param_hash = compute_param_hash(params);
        Self {
            view_id: view_id.to_string(),
            param_hash,
        }
    }

    pub fn storage_key(&self) -> String {
        format!("poll:{}:{}", self.view_id, self.param_hash)
    }

    /// Storage key for previous poll result: `poll:{view}:{hash}:prev`.
    pub fn storage_key_prev(&self) -> String {
        format!("poll:{}:{}:prev", self.view_id, self.param_hash)
    }

    /// Storage key for poll loop metadata: `poll:{view}:{hash}:meta`.
    pub fn storage_key_meta(&self) -> String {
        format!("poll:{}:{}:meta", self.view_id, self.param_hash)
    }
}

/// Compute a deterministic hash of parameters for deduplication.
fn compute_param_hash(params: &HashMap<String, String>) -> String {
    use sha2::{Digest, Sha256};

    let mut sorted: Vec<(&String, &String)> = params.iter().collect();
    sorted.sort_by_key(|(k, _)| *k);

    let mut hasher = Sha256::new();
    for (k, v) in sorted {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"&");
    }

    hex::encode(hasher.finalize())
}

// ── Hash Diff ───────────────────────────────────────────────────

/// Compute SHA-256 hash of canonical JSON for hash diff strategy.
///
/// Per spec: canonical = serde_json::to_string (deterministic for same structure).
pub fn compute_data_hash(data: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let canonical = serde_json::to_string(data).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// Check if data has changed using hash diff strategy.
pub fn hash_diff(prev_hash: Option<&str>, current: &serde_json::Value) -> (bool, String) {
    let current_hash = compute_data_hash(current);
    let changed = match prev_hash {
        Some(prev) => prev != current_hash,
        None => true, // first poll always "changed"
    };
    (changed, current_hash)
}

/// Check if data has changed using null diff strategy.
///
/// Per spec: change if result is non-null and non-empty.
pub fn null_diff(current: &serde_json::Value) -> bool {
    match current {
        serde_json::Value::Null => false,
        serde_json::Value::Array(arr) => !arr.is_empty(),
        serde_json::Value::Object(obj) => !obj.is_empty(),
        serde_json::Value::String(s) => !s.is_empty(),
        _ => true, // numbers, bools are non-null presence
    }
}

// ── Change Detect Diff Strategy (D15) ───────────────────────

/// Result of a JSON diff comparison.
#[derive(Debug, Clone)]
pub struct DiffResult {
    /// Whether any changes were detected.
    pub changed: bool,
    /// Number of added keys/elements.
    pub added_count: usize,
    /// Number of removed keys/elements.
    pub removed_count: usize,
    /// Number of modified values.
    pub modified_count: usize,
}

/// Diff strategy for polling views.
///
/// Per SHAPE-20: emits diagnostic events on diff operations.
/// Compares two JSON values and reports changes at the top level.
pub fn compute_diff(
    prev: &serde_json::Value,
    current: &serde_json::Value,
) -> DiffResult {
    if prev == current {
        return DiffResult {
            changed: false,
            added_count: 0,
            removed_count: 0,
            modified_count: 0,
        };
    }

    match (prev, current) {
        (serde_json::Value::Object(prev_obj), serde_json::Value::Object(curr_obj)) => {
            let mut added = 0usize;
            let mut removed = 0usize;
            let mut modified = 0usize;

            // Check for added and modified keys
            for (key, curr_val) in curr_obj {
                match prev_obj.get(key) {
                    Some(prev_val) => {
                        if prev_val != curr_val {
                            modified += 1;
                        }
                    }
                    None => {
                        added += 1;
                    }
                }
            }

            // Check for removed keys
            for key in prev_obj.keys() {
                if !curr_obj.contains_key(key) {
                    removed += 1;
                }
            }

            DiffResult {
                changed: added > 0 || removed > 0 || modified > 0,
                added_count: added,
                removed_count: removed,
                modified_count: modified,
            }
        }
        (serde_json::Value::Array(prev_arr), serde_json::Value::Array(curr_arr)) => {
            let prev_len = prev_arr.len();
            let curr_len = curr_arr.len();

            let mut modified = 0usize;
            let common_len = prev_len.min(curr_len);

            for i in 0..common_len {
                if prev_arr[i] != curr_arr[i] {
                    modified += 1;
                }
            }

            let added = if curr_len > prev_len {
                curr_len - prev_len
            } else {
                0
            };
            let removed = if prev_len > curr_len {
                prev_len - curr_len
            } else {
                0
            };

            DiffResult {
                changed: added > 0 || removed > 0 || modified > 0,
                added_count: added,
                removed_count: removed,
                modified_count: modified,
            }
        }
        _ => {
            // Different types or scalar change
            DiffResult {
                changed: true,
                added_count: 0,
                removed_count: 0,
                modified_count: 1,
            }
        }
    }
}

// ── Poll Loop State ─────────────────────────────────────────────

/// State of a single poll loop instance.
pub struct PollLoopState {
    pub key: PollLoopKey,
    pub diff_strategy: DiffStrategy,
    pub tick_interval_ms: u64,
    /// Previous data hash (for Hash strategy).
    pub prev_hash: RwLock<Option<String>>,
    /// Broadcast channel for connected clients.
    sender: broadcast::Sender<PollUpdate>,
    /// Number of connected clients.
    client_count: AtomicUsize,
}

/// An update pushed to poll loop clients.
#[derive(Debug, Clone)]
pub struct PollUpdate {
    pub data: serde_json::Value,
    pub changed: bool,
}

impl PollLoopState {
    pub fn new(
        key: PollLoopKey,
        diff_strategy: DiffStrategy,
        tick_interval_ms: u64,
    ) -> Self {
        let (sender, _) = broadcast::channel(64);
        Self {
            key,
            diff_strategy,
            tick_interval_ms,
            prev_hash: RwLock::new(None),
            sender,
            client_count: AtomicUsize::new(0),
        }
    }

    /// Subscribe a new client.
    pub fn subscribe(&self) -> broadcast::Receiver<PollUpdate> {
        self.client_count.fetch_add(1, Ordering::Relaxed);
        self.sender.subscribe()
    }

    /// Unsubscribe a client.
    pub fn unsubscribe(&self) {
        self.client_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .ok();
    }

    /// Current number of connected clients.
    pub fn client_count(&self) -> usize {
        self.client_count.load(Ordering::Relaxed)
    }

    /// Push an update to all connected clients.
    pub fn push_update(&self, update: PollUpdate) -> Result<usize, PollError> {
        self.sender
            .send(update)
            .map_err(|_| PollError::NoActiveClients)
    }
}

// ── Poll Loop Registry ──────────────────────────────────────────

/// Registry of active poll loops.
///
/// Per spec: create on first client, stop on last disconnect.
pub struct PollLoopRegistry {
    loops: RwLock<HashMap<String, Arc<PollLoopState>>>,
}

impl PollLoopRegistry {
    pub fn new() -> Self {
        Self {
            loops: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a poll loop for the given key.
    ///
    /// Per spec: shared poll loop for clients with same parameters.
    pub async fn get_or_create(
        &self,
        key: PollLoopKey,
        diff_strategy: DiffStrategy,
        tick_interval_ms: u64,
    ) -> Arc<PollLoopState> {
        let storage_key = key.storage_key();

        // Fast path: check read lock
        {
            let loops = self.loops.read().await;
            if let Some(state) = loops.get(&storage_key) {
                return state.clone();
            }
        }

        // Slow path: create new
        let mut loops = self.loops.write().await;
        // Double-check after acquiring write lock
        if let Some(state) = loops.get(&storage_key) {
            return state.clone();
        }

        let state = Arc::new(PollLoopState::new(
            key,
            diff_strategy,
            tick_interval_ms,
        ));
        loops.insert(storage_key, state.clone());
        state
    }

    /// Remove a poll loop when last client disconnects.
    pub async fn remove(&self, key: &str) -> Option<Arc<PollLoopState>> {
        self.loops.write().await.remove(key)
    }

    /// Number of active poll loops.
    pub async fn active_loops(&self) -> usize {
        self.loops.read().await.len()
    }

    /// Get a poll loop by storage key.
    pub async fn get(&self, key: &str) -> Option<Arc<PollLoopState>> {
        self.loops.read().await.get(key).cloned()
    }
}

impl Default for PollLoopRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Error Types ─────────────────────────────────────────────────

/// Polling errors.
#[derive(Debug, thiserror::Error)]
pub enum PollError {
    #[error("no active clients")]
    NoActiveClients,

    #[error("poll loop not found: {0}")]
    NotFound(String),

    #[error("tick execution failed: {0}")]
    TickFailed(String),

    #[error("storage error: {0}")]
    StorageError(String),

    #[error("dataview execution error: {0}")]
    DataViewError(String),
}

// ── DataView Executor Trait ─────────────────────────────────────

/// Trait for executing DataView queries from the poll loop.
///
/// Abstracted so the poll loop does not depend directly on the full
/// DataViewEngine, making it testable with mock executors.
#[async_trait]
pub trait PollDataViewExecutor: Send + Sync {
    /// Execute a DataView by name with the given parameters.
    /// Returns the result as a JSON value.
    async fn execute(
        &self,
        dataview_name: &str,
        params: &HashMap<String, String>,
    ) -> Result<serde_json::Value, PollError>;
}

/// Adapter that bridges `DataViewExecutor` to the `PollDataViewExecutor` trait.
///
/// Allows poll loops to execute real DataView queries without depending
/// directly on the full DataViewExecutor type.
pub struct DataViewPollExecutor {
    executor: Arc<tokio::sync::RwLock<Option<rivers_runtime::DataViewExecutor>>>,
}

impl DataViewPollExecutor {
    pub fn new(executor: Arc<tokio::sync::RwLock<Option<rivers_runtime::DataViewExecutor>>>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl PollDataViewExecutor for DataViewPollExecutor {
    async fn execute(
        &self,
        dataview_name: &str,
        params: &HashMap<String, String>,
    ) -> Result<serde_json::Value, PollError> {
        let guard = self.executor.read().await;
        let exec = guard.as_ref().ok_or_else(|| {
            PollError::DataViewError("DataViewExecutor not initialized".into())
        })?;

        // Convert HashMap<String, String> → HashMap<String, QueryValue>
        let query_params: HashMap<String, rivers_runtime::rivers_driver_sdk::types::QueryValue> = params
            .iter()
            .map(|(k, v)| (k.clone(), rivers_runtime::rivers_driver_sdk::types::QueryValue::String(v.clone())))
            .collect();

        let response = exec
            .execute(dataview_name, query_params, "GET", "poll")
            .await
            .map_err(|e| PollError::DataViewError(e.to_string()))?;

        serde_json::to_value(&response.query_result.rows)
            .map_err(|e| PollError::DataViewError(format!("serialize: {}", e)))
    }
}

// ── Poll State Persistence (B3.5) ──────────────────────────────

/// StorageEngine namespace for poll state persistence.
const POLL_STATE_NAMESPACE: &str = "poll_state";

/// Save a poll result to StorageEngine for future diff computation.
///
/// Key format: `poll:{view_id}:{params_hash}`
/// When `ttl_s` is `Some(n)` with n > 0, the state expires after n seconds.
pub async fn save_poll_state(
    storage: &dyn StorageEngine,
    key: &PollLoopKey,
    data: &serde_json::Value,
    ttl_s: Option<u64>,
) -> Result<(), PollError> {
    let storage_key = key.storage_key();
    let bytes = serde_json::to_vec(data)
        .map_err(|e| PollError::StorageError(format!("serialize poll state: {}", e)))?;
    let ttl_ms = ttl_s
        .filter(|&s| s > 0)
        .map(|s| s.saturating_mul(1000));
    storage
        .set(POLL_STATE_NAMESPACE, &storage_key, bytes, ttl_ms)
        .await
        .map_err(|e| PollError::StorageError(e.to_string()))
}

/// Load the previous poll result from StorageEngine.
///
/// Returns `None` if no previous state exists (first poll).
pub async fn load_poll_state(
    storage: &dyn StorageEngine,
    key: &PollLoopKey,
) -> Result<Option<serde_json::Value>, PollError> {
    let storage_key = key.storage_key();
    match storage.get(POLL_STATE_NAMESPACE, &storage_key).await {
        Ok(Some(bytes)) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| PollError::StorageError(format!("deserialize poll state: {}", e)))?;
            Ok(Some(value))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(PollError::StorageError(e.to_string())),
    }
}

/// Delete poll state from StorageEngine (cleanup on loop removal).
pub async fn delete_poll_state(
    storage: &dyn StorageEngine,
    key: &PollLoopKey,
) -> Result<(), PollError> {
    let storage_key = key.storage_key();
    storage
        .delete(POLL_STATE_NAMESPACE, &storage_key)
        .await
        .map_err(|e| PollError::StorageError(e.to_string()))
}

// ── Poll Tick Execution (B3.4) ──────────────────────────────────

/// Result of a single poll tick.
#[derive(Debug, Clone)]
pub struct PollTickResult {
    /// The fresh data from the DataView execution.
    pub current_data: serde_json::Value,
    /// Whether the data changed compared to the previous poll.
    pub changed: bool,
    /// The new hash (for Hash strategy), if applicable.
    pub new_hash: Option<String>,
}

/// Execute a single poll tick for a poll loop.
///
/// Sequence:
/// 1. Execute the DataView query via the executor.
/// 2. Load previous result from StorageEngine.
/// 3. Compute diff using the loop's diff strategy.
/// 4. Save current result to StorageEngine for next tick.
/// 5. Return the tick result indicating whether data changed.
pub async fn execute_poll_tick(
    executor: &dyn PollDataViewExecutor,
    storage: &dyn StorageEngine,
    loop_state: &PollLoopState,
    dataview_name: &str,
    params: &HashMap<String, String>,
) -> Result<PollTickResult, PollError> {
    // Step 1: Execute DataView query
    let current_data = executor.execute(dataview_name, params).await?;

    // Step 2: Load previous result from storage
    let _previous_data = load_poll_state(storage, &loop_state.key).await?;

    // Step 3: Compute diff based on strategy
    let (changed, new_hash) = match loop_state.diff_strategy {
        DiffStrategy::Hash => {
            let prev_hash_guard = loop_state.prev_hash.read().await;
            let prev = prev_hash_guard.as_deref();
            let (changed, hash) = hash_diff(prev, &current_data);
            (changed, Some(hash))
        }
        DiffStrategy::Null => {
            let changed = null_diff(&current_data);
            (changed, None)
        }
        DiffStrategy::ChangeDetect => {
            // ChangeDetect requires CodeComponent — for now, fall back to hash
            let prev_hash_guard = loop_state.prev_hash.read().await;
            let prev = prev_hash_guard.as_deref();
            let (changed, hash) = hash_diff(prev, &current_data);
            (changed, Some(hash))
        }
    };

    // Step 4: Save current result to storage for next tick
    save_poll_state(storage, &loop_state.key, &current_data, None).await?;

    // Update the in-memory hash if using Hash strategy
    if let Some(ref hash) = new_hash {
        let mut prev_hash = loop_state.prev_hash.write().await;
        *prev_hash = Some(hash.clone());
    }

    Ok(PollTickResult {
        current_data,
        changed,
        new_hash,
    })
}

/// Run a poll tick and broadcast results to connected clients.
///
/// Combines `execute_poll_tick` with `push_update` on the loop state.
/// Only broadcasts when data has changed (or on first tick).
pub async fn run_poll_tick_and_broadcast(
    executor: &dyn PollDataViewExecutor,
    storage: &dyn StorageEngine,
    loop_state: &PollLoopState,
    dataview_name: &str,
    params: &HashMap<String, String>,
) -> Result<PollTickResult, PollError> {
    let tick_result = execute_poll_tick(executor, storage, loop_state, dataview_name, params).await?;

    if tick_result.changed {
        // Ignore NoActiveClients error — clients may have disconnected between tick start and broadcast
        let _ = loop_state.push_update(PollUpdate {
            data: tick_result.current_data.clone(),
            changed: true,
        });
    }

    Ok(tick_result)
}

// ── Poll Loop Runner (N4.6–N4.8) ────────────────────────────────

use crate::process_pool::ProcessPoolManager;

// ── Change Detect Timeout (§10.4 / SHAPE-20) ────────────────

/// Timeout threshold for change_detect CodeComponent execution (ms).
const CHANGE_DETECT_TIMEOUT_MS: u64 = 5000;

/// Check if a change_detect execution exceeded the timeout threshold.
///
/// Per spec SHAPE-20: emit PollChangeDetectTimeout diagnostic event
/// when change_detect CodeComponent execution exceeds the threshold.
/// Returns true if the duration exceeded the threshold.
pub fn check_change_detect_timeout(duration_ms: u64) -> bool {
    if duration_ms > CHANGE_DETECT_TIMEOUT_MS {
        tracing::warn!(
            target: "rivers.polling",
            duration_ms = duration_ms,
            threshold_ms = CHANGE_DETECT_TIMEOUT_MS,
            "PollChangeDetectTimeout: change_detect exceeded threshold"
        );
        true
    } else {
        false
    }
}

/// Execute one poll tick using in-memory diff: query → diff → broadcast.
///
/// Per technology-path-spec §12.5: previous state held in-memory.
/// This is a simpler variant of `execute_poll_tick` that operates
/// purely in-memory without StorageEngine, suitable for lightweight
/// poll loops.
///
/// When `executor` is `Some`, real DataView queries are executed.
/// When `None`, stub data is returned (for testing/development).
///
/// When `pool` is `Some` and strategy is `ChangeDetect`, the
/// CodeComponent is dispatched for custom diff logic.
pub async fn execute_poll_tick_inmemory(
    view_id: &str,
    _param_hash: &str,
    previous_state: &mut Option<String>,
    diff_strategy: &DiffStrategy,
    pool: Option<&ProcessPoolManager>,
    executor: Option<&dyn PollDataViewExecutor>,
) -> Option<serde_json::Value> {
    // Step 1: Execute the DataView to get current data
    let current_data = if let Some(exec) = executor {
        match exec.execute(view_id, &HashMap::new()).await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(
                    view_id = %view_id,
                    error = %e,
                    "poll tick DataView execution failed, using empty data"
                );
                serde_json::Value::Null
            }
        }
    } else {
        // Stub data when no executor is available
        serde_json::json!({ "_poll_stub": true, "view_id": view_id })
    };

    let current_hash = compute_data_hash(&current_data);

    // Step 2: Compute diff based on strategy
    let changed = match diff_strategy {
        DiffStrategy::Hash => previous_state
            .as_ref()
            .map_or(true, |prev| prev != &current_hash),
        DiffStrategy::Null => null_diff(&current_data),
        DiffStrategy::ChangeDetect => {
            // Dispatch to CodeComponent for custom diff when pool is available
            if let Some(pool) = pool {
                dispatch_change_detect(pool, previous_state.as_deref(), &current_data).await
            } else {
                // Fallback to hash diff without pool
                previous_state
                    .as_ref()
                    .map_or(true, |prev| prev != &current_hash)
            }
        }
    };

    if changed {
        *previous_state = Some(current_hash);
        Some(current_data)
    } else {
        None
    }
}

/// Dispatch ChangeDetect strategy to a CodeComponent via ProcessPool.
///
/// Per spec: the CodeComponent receives `{ prev, current }` and returns
/// `{ changed: bool }`. Falls back to hash diff on dispatch failure.
async fn dispatch_change_detect(
    pool: &ProcessPoolManager,
    prev_hash: Option<&str>,
    current_data: &serde_json::Value,
) -> bool {
    use crate::process_pool::{Entrypoint, TaskContextBuilder};

    let start = std::time::Instant::now();

    let entrypoint = Entrypoint {
        module: "change_detect".to_string(),
        function: "detect".to_string(),
        language: "javascript".to_string(),
    };

    let args = serde_json::json!({
        "prev_hash": prev_hash,
        "current": current_data,
    });

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id("change_detect".to_string());
    let builder = crate::task_enrichment::enrich(builder, "");
    let task_ctx = match builder.build() {
        Ok(ctx) => ctx,
        Err(_) => {
            // Build failed — fall back to hash diff
            return true;
        }
    };

    match pool.dispatch("default", task_ctx).await {
        Ok(result) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            check_change_detect_timeout(duration_ms);

            result
                .value
                .get("changed")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        }
        Err(_) => {
            // Dispatch failed — fall back to reporting changed
            true
        }
    }
}

/// Run a continuous in-memory poll loop, broadcasting updates on change.
///
/// Combines `execute_poll_tick_inmemory` with a tick interval and
/// broadcasts to the `PollLoopState` when data changes.
pub async fn run_poll_loop_inmemory(
    loop_state: Arc<PollLoopState>,
    executor: &dyn PollDataViewExecutor,
    storage: &dyn StorageEngine,
    dataview_name: &str,
    params: &HashMap<String, String>,
) {
    let mut tick = tokio::time::interval(tokio::time::Duration::from_millis(
        loop_state.tick_interval_ms,
    ));

    loop {
        tick.tick().await;

        // Skip if no clients are connected
        if loop_state.client_count() == 0 {
            continue;
        }

        match run_poll_tick_and_broadcast(executor, storage, &loop_state, dataview_name, params)
            .await
        {
            Ok(_) => {}
            Err(PollError::DataViewError(e)) => {
                tracing::warn!(
                    view_id = %loop_state.key.view_id,
                    error = %e,
                    "poll tick DataView execution failed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    view_id = %loop_state.key.view_id,
                    error = %e,
                    "poll tick failed"
                );
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;

    /// Mock DataView executor for testing.
    struct MockExecutor {
        /// Data to return on each call (cycled).
        responses: tokio::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl MockExecutor {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl PollDataViewExecutor for MockExecutor {
        async fn execute(
            &self,
            _dataview_name: &str,
            _params: &HashMap<String, String>,
        ) -> Result<serde_json::Value, PollError> {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    fn make_key() -> PollLoopKey {
        PollLoopKey {
            view_id: "test_view".into(),
            param_hash: "abc123".into(),
        }
    }

    #[tokio::test]
    async fn test_save_and_load_poll_state() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let data = serde_json::json!({"count": 42});

        // Save
        save_poll_state(&storage, &key, &data, None).await.unwrap();

        // Load
        let loaded = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(loaded, Some(data));
    }

    #[tokio::test]
    async fn test_load_poll_state_returns_none_for_missing() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();

        let loaded = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_delete_poll_state() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let data = serde_json::json!({"x": 1});

        save_poll_state(&storage, &key, &data, None).await.unwrap();
        delete_poll_state(&storage, &key).await.unwrap();

        let loaded = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(loaded, None);
    }

    #[tokio::test]
    async fn test_poll_tick_first_tick_always_changed() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000);

        let data = serde_json::json!({"items": [1, 2, 3]});
        let executor = MockExecutor::new(vec![data.clone()]);

        let result = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &HashMap::new(),
        )
        .await
        .unwrap();

        assert!(result.changed);
        assert_eq!(result.current_data, data);
        assert!(result.new_hash.is_some());

        // State should now be persisted
        let persisted = load_poll_state(&storage, &key).await.unwrap();
        assert_eq!(persisted, Some(data));
    }

    #[tokio::test]
    async fn test_poll_tick_no_change_on_same_data() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000);

        let data = serde_json::json!({"stable": true});
        let executor = MockExecutor::new(vec![data.clone(), data.clone()]);

        // First tick — changed
        let r1 = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert!(r1.changed);

        // Second tick — same data, not changed
        let r2 = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert!(!r2.changed);
    }

    #[tokio::test]
    async fn test_poll_tick_detects_change() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000);

        let data1 = serde_json::json!({"version": 1});
        let data2 = serde_json::json!({"version": 2});
        let executor = MockExecutor::new(vec![data1, data2.clone()]);

        // First tick
        execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &HashMap::new(),
        )
        .await
        .unwrap();

        // Second tick — data changed
        let r2 = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "test_dv",
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert!(r2.changed);
        assert_eq!(r2.current_data, data2);
    }

    #[tokio::test]
    async fn test_poll_tick_null_strategy() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = PollLoopState::new(key.clone(), DiffStrategy::Null, 1000);

        // Non-empty data — changed
        let executor = MockExecutor::new(vec![serde_json::json!({"x": 1})]);
        let r = execute_poll_tick(
            &executor,
            &storage,
            &loop_state,
            "dv",
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert!(r.changed);
        assert!(r.new_hash.is_none());

        // Null data — not changed
        let executor2 = MockExecutor::new(vec![serde_json::Value::Null]);
        let r2 = execute_poll_tick(
            &executor2,
            &storage,
            &loop_state,
            "dv",
            &HashMap::new(),
        )
        .await
        .unwrap();
        assert!(!r2.changed);
    }

    #[tokio::test]
    async fn test_broadcast_only_on_change() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = Arc::new(PollLoopState::new(key.clone(), DiffStrategy::Hash, 1000));

        // Subscribe a client
        let mut rx = loop_state.subscribe();

        let data = serde_json::json!({"tick": 1});
        let executor = MockExecutor::new(vec![data.clone(), data.clone()]);

        // First tick — broadcasts
        run_poll_tick_and_broadcast(
            &executor,
            &storage,
            &loop_state,
            "dv",
            &HashMap::new(),
        )
        .await
        .unwrap();

        let update = rx.try_recv().unwrap();
        assert!(update.changed);
        assert_eq!(update.data, data);

        // Second tick — same data, no broadcast
        run_poll_tick_and_broadcast(
            &executor,
            &storage,
            &loop_state,
            "dv",
            &HashMap::new(),
        )
        .await
        .unwrap();

        // Should not have a new message
        assert!(rx.try_recv().is_err());
    }

    // ── N4.6–N4.8: In-memory poll tick tests ──────────────

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_first_tick_changed() {
        let mut prev = None;
        let result =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Hash, None, None)
                .await;
        assert!(result.is_some());
        assert!(prev.is_some());
    }

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_second_tick_unchanged() {
        let mut prev = None;
        // First tick — changed
        let _ =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Hash, None, None)
                .await;

        // Second tick — same stub data, should not change
        let result =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Hash, None, None)
                .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_null_strategy() {
        let mut prev = None;
        // Null strategy: non-null data is always "changed"
        let result =
            execute_poll_tick_inmemory("test_view", "hash1", &mut prev, &DiffStrategy::Null, None, None)
                .await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_execute_poll_tick_inmemory_change_detect_fallback_to_hash() {
        let mut prev = None;
        // First call — no previous state, always changed
        let result = execute_poll_tick_inmemory(
            "test_view",
            "hash1",
            &mut prev,
            &DiffStrategy::ChangeDetect,
            None,
            None,
        )
        .await;
        assert!(result.is_some());

        // Second call — without pool, ChangeDetect falls back to hash diff;
        // identical stub data produces the same hash → no change
        let result2 = execute_poll_tick_inmemory(
            "test_view",
            "hash1",
            &mut prev,
            &DiffStrategy::ChangeDetect,
            None,
            None,
        )
        .await;
        assert!(result2.is_none());
    }

    #[tokio::test]
    async fn test_run_poll_loop_inmemory_broadcasts() {
        let storage = InMemoryStorageEngine::new();
        let key = make_key();
        let loop_state = Arc::new(PollLoopState::new(key, DiffStrategy::Hash, 50));

        // Subscribe a client
        let mut rx = loop_state.subscribe();

        let data = serde_json::json!({"tick": 1});
        let executor = MockExecutor::new(vec![data.clone()]);

        let ls = loop_state.clone();
        let handle = tokio::spawn(async move {
            run_poll_loop_inmemory(ls, &executor, &storage, "test_dv", &HashMap::new()).await;
        });

        // Wait for at least one broadcast
        let update = tokio::time::timeout(
            tokio::time::Duration::from_millis(500),
            rx.recv(),
        )
        .await;
        assert!(update.is_ok());
        let update = update.unwrap().unwrap();
        assert!(update.changed);

        handle.abort();
    }

    #[tokio::test]
    async fn test_poll_state_key_format() {
        let key = PollLoopKey {
            view_id: "my_view".into(),
            param_hash: "deadbeef".into(),
        };
        assert_eq!(key.storage_key(), "poll:my_view:deadbeef");
    }

    // ── D15: compute_diff tests ─────────────────────────────

    #[test]
    fn test_compute_diff_identical_objects() {
        let a = serde_json::json!({"x": 1, "y": 2});
        let b = serde_json::json!({"x": 1, "y": 2});
        let result = compute_diff(&a, &b);
        assert!(!result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_added_key() {
        let a = serde_json::json!({"x": 1});
        let b = serde_json::json!({"x": 1, "y": 2});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 1);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_removed_key() {
        let a = serde_json::json!({"x": 1, "y": 2});
        let b = serde_json::json!({"x": 1});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_modified_value() {
        let a = serde_json::json!({"x": 1});
        let b = serde_json::json!({"x": 2});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 1);
    }

    #[test]
    fn test_compute_diff_arrays_added() {
        let a = serde_json::json!([1, 2]);
        let b = serde_json::json!([1, 2, 3]);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 1);
        assert_eq!(result.removed_count, 0);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_arrays_removed() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([1, 2]);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 0);
        assert_eq!(result.removed_count, 1);
        assert_eq!(result.modified_count, 0);
    }

    #[test]
    fn test_compute_diff_arrays_modified() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([1, 99, 3]);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.modified_count, 1);
    }

    #[test]
    fn test_compute_diff_scalar_change() {
        let a = serde_json::json!(42);
        let b = serde_json::json!(43);
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.modified_count, 1);
    }

    #[test]
    fn test_compute_diff_type_change() {
        let a = serde_json::json!(42);
        let b = serde_json::json!("hello");
        let result = compute_diff(&a, &b);
        assert!(result.changed);
    }

    #[test]
    fn test_compute_diff_identical_arrays() {
        let a = serde_json::json!([1, 2, 3]);
        let b = serde_json::json!([1, 2, 3]);
        let result = compute_diff(&a, &b);
        assert!(!result.changed);
    }

    #[test]
    fn test_compute_diff_complex_object() {
        let a = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let b = serde_json::json!({"a": 1, "b": 99, "d": 4});
        let result = compute_diff(&a, &b);
        assert!(result.changed);
        assert_eq!(result.added_count, 1);   // d added
        assert_eq!(result.removed_count, 1); // c removed
        assert_eq!(result.modified_count, 1); // b changed
    }

    // ── U8: PollChangeDetectTimeout tests ──────────────────

    #[test]
    fn change_detect_timeout_detected() {
        assert!(check_change_detect_timeout(6000));
        assert!(!check_change_detect_timeout(3000));
    }

    #[test]
    fn change_detect_timeout_boundary() {
        // Exactly at threshold should not trigger
        assert!(!check_change_detect_timeout(5000));
        // Just above threshold should trigger
        assert!(check_change_detect_timeout(5001));
    }
}
