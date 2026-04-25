//! Poll loop runner — continuous in-memory poll loops and change detection.

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::rivers_core::storage::StorageEngine;

use super::diff::{compute_data_hash, null_diff, DiffStrategy};
use super::executor::run_poll_tick_and_broadcast;
use super::state::{PollDataViewExecutor, PollError, PollLoopState};

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
                // view_id is qualified as "app_id:view" — extract app_id so the
                // change_detect handler runs in the right per-app namespace.
                let app_id = crate::task_enrichment::app_id_from_qualified_name(view_id);
                dispatch_change_detect(pool, previous_state.as_deref(), &current_data, app_id).await
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
    app_id: &str,
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
    // Polling change_detect runs the user's diff logic for a polling view —
    // semantically just a callback for that view, hence Rest.
    let builder = crate::task_enrichment::enrich(
        builder,
        app_id,
        rivers_runtime::process_pool::TaskKind::Rest,
    );
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
