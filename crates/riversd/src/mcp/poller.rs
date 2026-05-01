//! MCP resource change poller — drives `notifications/resources/updated` pushes.
//!
//! Per `2026-04-29-cb-p1-1-mcp-subscriptions-design.md` §Layer 3.
//!
//! One `ChangePoller` is held on `AppContext`. When `resources/subscribe` is
//! called it invokes `ensure_running(app_id, uri, ...)` which, if no poller
//! is already running for that `(app_id, uri)`, spawns a background task.
//!
//! The task polls the bound DataView, hashes the result rows with SHA-256, and
//! calls `registry.notify_changed(uri)` on hash change.  It exits automatically
//! when `registry.snapshot_subscriptions()` returns zero subscribers for its URI.

use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use rivers_runtime::view::McpResourceConfig;
use rivers_runtime::DataViewExecutor;
use rivers_runtime::rivers_driver_sdk::QueryValue;

use super::subscriptions::SubscriptionRegistry;
use super::dispatch::extract_uri_template_vars_pub;

// ── ChangePoller ────────────────────────────────────────────────────

/// Active polling tasks keyed by `(app_id, uri)`.
pub struct ChangePoller {
    handles: Mutex<HashMap<(String, String), JoinHandle<()>>>,
}

impl ChangePoller {
    /// Create an empty `ChangePoller` with no active tasks.
    pub fn new() -> Self {
        Self {
            handles: Mutex::new(HashMap::new()),
        }
    }

    /// Ensure a poller is running for `(app_id, uri)`.
    ///
    /// If a task already exists for this key, this is a no-op.
    /// Otherwise, spawns a new polling task.
    pub async fn ensure_running(
        &self,
        app_id: String,
        uri: String,
        dv_namespace: String,
        resources: HashMap<String, McpResourceConfig>,
        executor: Arc<DataViewExecutor>,
        registry: Arc<SubscriptionRegistry>,
        poll_secs: u64,
        min_poll_secs: u64,
    ) {
        let key = (app_id.clone(), uri.clone());
        let mut handles = self.handles.lock().await;

        // Already running — no-op.
        if handles.contains_key(&key) {
            return;
        }

        let interval = poll_secs.max(min_poll_secs);
        let handle = tokio::spawn(poll_loop(
            app_id, uri, dv_namespace, resources, executor, registry, interval,
        ));

        handles.insert(key, handle);
    }

    /// Remove finished or aborted poller handles from the map.
    pub async fn gc(&self) {
        let mut handles = self.handles.lock().await;
        handles.retain(|_, h| !h.is_finished());
    }

    /// Active poller count (for tests / metrics).
    pub async fn active_count(&self) -> usize {
        let handles = self.handles.lock().await;
        handles.values().filter(|h| !h.is_finished()).count()
    }
}

// ── Poll loop ────────────────────────────────────────────────────────

async fn poll_loop(
    app_id: String,
    uri: String,
    dv_namespace: String,
    resources: HashMap<String, McpResourceConfig>,
    executor: Arc<DataViewExecutor>,
    registry: Arc<SubscriptionRegistry>,
    poll_secs: u64,
) {
    let mut last_hash: Option<[u8; 32]> = None;

    loop {
        // P1.1.3.c — exit when no subscribers remain for this (app_id, uri).
        let subs = registry.snapshot_subscriptions().await;
        let has_subs = subs.iter().any(|(_, sub_uri)| sub_uri == &uri);
        if !has_subs {
            debug!(app_id = %app_id, uri = %uri, "MCP poller: no subscribers — exiting");
            return;
        }

        // Execute the DataView for this URI.
        if let Some(rows_hash) = execute_and_hash(&uri, &dv_namespace, &resources, &executor, &app_id).await {
            if last_hash.map(|h| h != rows_hash).unwrap_or(true) {
                if last_hash.is_some() {
                    // Data changed — notify all subscribers.
                    registry.notify_changed(&uri).await;
                    debug!(app_id = %app_id, uri = %uri, "MCP poller: change detected, notification sent");
                }
                last_hash = Some(rows_hash);
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
    }
}

/// Execute the DataView bound to `uri` and return a SHA-256 hash of the rows.
///
/// Returns `None` when the URI cannot be matched or the DataView fails.
async fn execute_and_hash(
    uri: &str,
    dv_namespace: &str,
    resources: &HashMap<String, McpResourceConfig>,
    executor: &Arc<DataViewExecutor>,
    app_id: &str,
) -> Option<[u8; 32]> {
    // Match URI against resource templates (same logic as handle_resources_read).
    let matched = resources.iter().find_map(|(name, config)| {
        let template = config.uri_template.as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| format!("rivers://{}/{}", app_id, name));

        extract_uri_template_vars_pub(&template, uri).map(|path_vars| {
            (config.clone(), path_vars)
        })
    });

    let (config, path_vars) = matched?;

    let mut params: HashMap<String, QueryValue> = HashMap::new();
    for (k, v) in path_vars.iter() {
        if let Some(s) = v.as_str() {
            params.insert(k.clone(), QueryValue::String(s.to_string()));
        }
    }

    let namespaced = format!("{}:{}", dv_namespace, config.dataview);
    let trace_id = uuid::Uuid::new_v4().to_string();

    match executor.execute(&namespaced, params, "GET", &trace_id, None).await {
        Ok(response) => {
            let serialized = serde_json::to_vec(&response.query_result.rows).ok()?;
            let mut hasher = Sha256::new();
            hasher.update(&serialized);
            Some(hasher.finalize().into())
        }
        Err(e) => {
            warn!(uri = %uri, error = %e, "MCP poller: DataView execute failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_poller_has_zero_active() {
        let p = ChangePoller::new();
        assert_eq!(p.active_count().await, 0);
    }
}
