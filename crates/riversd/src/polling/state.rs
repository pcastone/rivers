//! Poll loop state, registry, error types, and executor trait.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, RwLock};

use super::diff::{DiffStrategy, PollLoopKey};

// ── Poll Loop State ─────────────────────────────────────────────

/// State of a single poll loop instance.
pub struct PollLoopState {
    /// Key identifying this poll loop (dataview + params).
    pub key: PollLoopKey,
    /// Strategy for detecting data changes between ticks.
    pub diff_strategy: DiffStrategy,
    /// Milliseconds between poll ticks.
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
    /// The polled data payload.
    pub data: serde_json::Value,
    /// Whether the data changed since the last tick.
    pub changed: bool,
}

impl PollLoopState {
    /// Create a new poll loop state with the given key, diff strategy, and interval.
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
    /// Create an empty poll loop registry.
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
    /// No clients are subscribed to receive updates.
    #[error("no active clients")]
    NoActiveClients,

    /// The requested poll loop was not found in the registry.
    #[error("poll loop not found: {0}")]
    NotFound(String),

    /// A tick execution failed.
    #[error("tick execution failed: {0}")]
    TickFailed(String),

    /// Storage backend error.
    #[error("storage error: {0}")]
    StorageError(String),

    /// DataView query execution error.
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
    executor: Arc<tokio::sync::RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>>,
}

impl DataViewPollExecutor {
    /// Create a new executor adapter wrapping a shared `DataViewExecutor`.
    pub fn new(executor: Arc<tokio::sync::RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>>) -> Self {
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
        let exec = {
            let guard = self.executor.read().await;
            guard.clone().ok_or_else(|| {
                PollError::DataViewError("DataViewExecutor not initialized".into())
            })?
        };

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
