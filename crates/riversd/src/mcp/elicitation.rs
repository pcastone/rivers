//! MCP elicitation — mid-handler user input requests (P2.6).
//!
//! Allows a codecomponent MCP tool handler to pause mid-execution and request
//! structured input from the user via the MCP client. The handler calls
//! `await ctx.elicit(spec)` in TypeScript, which:
//!   1. Suspends the V8 task (via the sync bridge).
//!   2. Sends an `elicitation/create` JSON-RPC notification to the MCP client
//!      over SSE via the session's `SubscriptionRegistry` channel.
//!   3. Resumes when the client sends `elicitation/response`.
//!
//! # Architecture
//!
//! - `ElicitationRegistry` — per-MCP-session store of pending elicitations,
//!   keyed by UUID. Lives on `AppContext` (a single shared instance).
//!
//! - `ElicitationRequest` — the message the V8 host callback sends when
//!   `ctx.elicit(spec)` is called. Carries the spec plus a `oneshot::Sender`
//!   for the response. Posted on a thread-local `mpsc::UnboundedSender`
//!   (`TASK_ELICITATION_TX`) that `dispatch_codecomponent_tool` provides.
//!
//! - `dispatch_codecomponent_tool` creates the channel, stores the `Sender`
//!   in a task-local (via `set_elicitation_tx`), spawns a relay task that
//!   reads each `ElicitationRequest`, sends the `elicitation/create`
//!   notification to the SSE stream, and registers the `oneshot::Sender`
//!   in `ElicitationRegistry`.
//!
//! - `handle_elicitation_response` resolves the pending registry entry when
//!   the MCP client sends `elicitation/response`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

// ── Wire types ────────────────────────────────────────────────────────

/// Spec sent from handler → MCP client via `elicitation/create`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ElicitationSpec {
    pub title: String,
    pub message: String,
    #[serde(rename = "requestedSchema")]
    pub requested_schema: serde_json::Value,
}

/// Response sent from MCP client → handler via `elicitation/response`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ElicitationResponse {
    pub id: String,
    /// One of "accept" | "decline" | "cancel".
    pub action: String,
    pub content: Option<serde_json::Value>,
}

/// Message posted from the V8 `Rivers.__elicit` host callback to the relay task
/// spawned by `dispatch_codecomponent_tool`.
pub struct ElicitationRequest {
    pub id: String,
    pub spec: ElicitationSpec,
    /// The oneshot sender the relay task hands to `ElicitationRegistry::register`.
    pub response_tx: oneshot::Sender<ElicitationResponse>,
}

// ── ElicitationRegistry ───────────────────────────────────────────────

/// Shared registry of pending elicitations across all MCP sessions.
///
/// Keyed by elicitation UUID. The registry lives on `AppContext` (one instance
/// per `riversd` process). Individual `oneshot::Sender`s are placed here by
/// the per-tool-call relay task and removed when the client responds or times
/// out.
#[derive(Default, Clone)]
pub struct ElicitationRegistry {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ElicitationResponse>>>>,
}

impl ElicitationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending elicitation and return the receiver to await.
    ///
    /// The caller (relay task) sends the `oneshot::Sender` here; the V8
    /// worker thread waits on the returned `Receiver`.
    pub fn register(&self, id: String, tx: oneshot::Sender<ElicitationResponse>) {
        self.pending.lock().unwrap().insert(id, tx);
    }

    /// Resolve a pending elicitation with the client's response.
    ///
    /// Returns `true` if the ID was found and the response was delivered,
    /// `false` if the ID was unknown (already timed-out or never registered).
    pub fn resolve(&self, response: ElicitationResponse) -> bool {
        if let Some(tx) = self.pending.lock().unwrap().remove(&response.id) {
            let _ = tx.send(response);
            true
        } else {
            false
        }
    }

    /// Cancel and remove a pending elicitation (called on task timeout / cleanup).
    ///
    /// Sends a synthetic `cancel` response so the waiting V8 worker unblocks.
    pub fn cancel(&self, id: &str) {
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(ElicitationResponse {
                id: id.to_string(),
                action: "cancel".to_string(),
                content: None,
            });
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_resolve_roundtrip() {
        let reg = ElicitationRegistry::new();
        let (tx, rx) = oneshot::channel();
        reg.register("abc".to_string(), tx);
        let resolved = reg.resolve(ElicitationResponse {
            id: "abc".into(),
            action: "accept".into(),
            content: Some(serde_json::json!({"name": "Alice"})),
        });
        assert!(resolved);
        let result = rx.await.unwrap();
        assert_eq!(result.action, "accept");
        assert_eq!(result.content, Some(serde_json::json!({"name": "Alice"})));
    }

    #[tokio::test]
    async fn resolve_unknown_id_returns_false() {
        let reg = ElicitationRegistry::new();
        let resolved = reg.resolve(ElicitationResponse {
            id: "unknown".into(),
            action: "accept".into(),
            content: None,
        });
        assert!(!resolved);
    }

    #[tokio::test]
    async fn cancel_unblocks_waiting_receiver() {
        let reg = ElicitationRegistry::new();
        let (tx, rx) = oneshot::channel();
        reg.register("xyz".to_string(), tx);
        reg.cancel("xyz");
        let result = rx.await.unwrap();
        assert_eq!(result.action, "cancel");
        assert_eq!(result.id, "xyz");
        assert!(result.content.is_none());
    }

    #[tokio::test]
    async fn cancel_unknown_id_is_noop() {
        let reg = ElicitationRegistry::new();
        // Should not panic.
        reg.cancel("nope");
    }

    #[tokio::test]
    async fn registry_is_clonable_and_shares_state() {
        let reg = ElicitationRegistry::new();
        let reg2 = reg.clone();

        let (tx, rx) = oneshot::channel();
        reg.register("shared".to_string(), tx);

        // Resolve through the clone — same underlying map.
        let resolved = reg2.resolve(ElicitationResponse {
            id: "shared".into(),
            action: "decline".into(),
            content: None,
        });
        assert!(resolved);
        let result = rx.await.unwrap();
        assert_eq!(result.action, "decline");
    }
}
