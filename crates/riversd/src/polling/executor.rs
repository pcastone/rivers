//! Poll tick execution and storage persistence.

use std::collections::HashMap;

use rivers_runtime::rivers_core::storage::StorageEngine;

use super::diff::{hash_diff, null_diff, DiffStrategy};
use super::state::{PollDataViewExecutor, PollError, PollLoopState, PollUpdate};

// ── Poll State Persistence (B3.5) ──────────────────────────────

/// StorageEngine namespace for poll state persistence.
const POLL_STATE_NAMESPACE: &str = "poll_state";

/// Save a poll result to StorageEngine for future diff computation.
///
/// Key format: `poll:{view_id}:{params_hash}`
/// When `ttl_s` is `Some(n)` with n > 0, the state expires after n seconds.
pub async fn save_poll_state(
    storage: &dyn StorageEngine,
    key: &super::diff::PollLoopKey,
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
    key: &super::diff::PollLoopKey,
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
    key: &super::diff::PollLoopKey,
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
