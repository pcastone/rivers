//! MCP subscription registry — per-session SSE channels and URI subscriptions.
//!
//! Per `2026-04-29-cb-p1-1-mcp-subscriptions-design.md` §Layer 2.
//!
//! `SubscriptionRegistry` is owned by `AppContext` (one instance per `riversd`).
//! It tracks which SSE channel belongs to each session and which URIs each
//! session has subscribed to.  Notifications are sent over bounded mpsc channels
//! (capacity 64); slow consumers are dropped with a WARN rather than blocking the
//! notifier.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::response::sse::Event;
use tokio::sync::{mpsc, RwLock};

// ── Error types ───────────────────────────────────────────────────────

/// Errors returned by [`SubscriptionRegistry::subscribe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeError {
    /// Session not found (no SSE channel attached yet).
    SessionNotFound,
    /// Subscription cap reached for this session.
    TooMany,
}

impl std::fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound => write!(f, "session not found"),
            Self::TooMany => write!(f, "too many subscriptions"),
        }
    }
}

// ── Per-session channel ────────────────────────────────────────────────

/// Bounded SSE sender + subscription set for one client session.
pub struct SessionChannel {
    /// Sender half of the bounded mpsc channel that feeds the SSE stream.
    pub sender: mpsc::Sender<Event>,
    /// Set of URIs this session is currently subscribed to.
    pub subscribed_uris: HashSet<String>,
    /// App ID that owns this session.
    pub app_id: String,
}

// ── Registry ──────────────────────────────────────────────────────────

/// Shared registry of active MCP SSE sessions and their subscriptions.
///
/// Constructed once at startup and placed on [`AppContext`].
pub struct SubscriptionRegistry {
    /// session_id → (sse_sender, subscribed URIs, app_id)
    sessions: RwLock<HashMap<String, SessionChannel>>,
}

impl SubscriptionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Attach an SSE channel for `session_id`.
    ///
    /// Creates a bounded mpsc channel (capacity 64), stores the sender, and
    /// returns the receiver for the caller to wrap in an `axum::response::sse::Sse`.
    ///
    /// If a channel already exists for this session it is replaced (previous
    /// receiver is dropped, closing the old stream).
    pub async fn attach_sse(
        &self,
        session_id: &str,
        app_id: &str,
    ) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel::<Event>(64);
        let mut sessions = self.sessions.write().await;
        sessions.insert(
            session_id.to_string(),
            SessionChannel {
                sender: tx,
                subscribed_uris: HashSet::new(),
                app_id: app_id.to_string(),
            },
        );
        rx
    }

    /// Remove a session and all its subscriptions (call on client disconnect).
    pub async fn detach(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
    }

    /// Add `uri` to the subscription set for `session_id`.
    ///
    /// Returns `SubscribeError::SessionNotFound` when no SSE channel is
    /// registered for the session.
    ///
    /// Returns `SubscribeError::TooMany` when the session already holds
    /// `max_subscriptions` subscriptions.
    pub async fn subscribe(
        &self,
        session_id: &str,
        uri: &str,
        max_subscriptions: u64,
    ) -> Result<(), SubscribeError> {
        let mut sessions = self.sessions.write().await;
        let channel = sessions
            .get_mut(session_id)
            .ok_or(SubscribeError::SessionNotFound)?;

        // Already subscribed — idempotent.
        if channel.subscribed_uris.contains(uri) {
            return Ok(());
        }

        if channel.subscribed_uris.len() as u64 >= max_subscriptions {
            return Err(SubscribeError::TooMany);
        }

        channel.subscribed_uris.insert(uri.to_string());
        Ok(())
    }

    /// Remove `uri` from the subscription set for `session_id`.
    ///
    /// No-op when the session or URI is not found.
    pub async fn unsubscribe(&self, session_id: &str, uri: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(channel) = sessions.get_mut(session_id) {
            channel.subscribed_uris.remove(uri);
        }
    }

    /// Send a `notifications/resources/updated` event to every session that is
    /// subscribed to `uri`.
    ///
    /// Each qualifying session receives at most one notification (dedupe: the
    /// `HashSet` already prevents duplicate subscriptions per session, so one
    /// notification per session is guaranteed).
    ///
    /// If a session's mpsc channel is full, the notification is dropped and a
    /// `WARN` is emitted — the slow consumer is not disconnected.
    pub async fn notify_changed(&self, uri: &str) {
        let notification_data = format!(
            r#"{{"jsonrpc":"2.0","method":"notifications/resources/updated","params":{{"uri":"{}"}}}}"#,
            uri
        );
        let sessions = self.sessions.read().await;
        for (session_id, channel) in sessions.iter() {
            if !channel.subscribed_uris.contains(uri) {
                continue;
            }
            let event = Event::default().data(notification_data.clone());
            match channel.sender.try_send(event) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        session_id = %session_id,
                        uri = %uri,
                        "MCP subscription notification dropped — client channel full (slow consumer)"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    // Channel closed — session will be cleaned up on disconnect.
                    tracing::debug!(
                        session_id = %session_id,
                        "MCP subscription channel closed for session"
                    );
                }
            }
        }
    }

    /// Return a snapshot of all active `(session_id, uri)` subscription pairs.
    ///
    /// Used by the poller to know which `(app_id, uri)` combinations need
    /// active polling tasks.
    pub async fn snapshot_subscriptions(&self) -> Vec<(String, String)> {
        let sessions = self.sessions.read().await;
        let mut pairs = Vec::new();
        for (session_id, channel) in sessions.iter() {
            for uri in &channel.subscribed_uris {
                pairs.push((session_id.clone(), uri.clone()));
            }
        }
        pairs
    }

    /// Return the app_id for a session, or `None` if not found.
    pub async fn session_app_id(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|c| c.app_id.clone())
    }

    /// Return `true` if any session is subscribed to `uri`.
    pub async fn has_subscribers(&self, uri: &str) -> bool {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .any(|c| c.subscribed_uris.contains(uri))
    }

    /// Send a raw SSE notification to a specific session by `session_id`.
    ///
    /// P2.6: Used by the elicitation relay task to deliver `elicitation/create`
    /// notifications. Returns `true` when the session exists and the message was
    /// queued, `false` when the session has no open SSE stream or the channel is
    /// full/closed.
    pub async fn send_to_session(&self, session_id: &str, data: String) -> bool {
        let sessions = self.sessions.read().await;
        let Some(channel) = sessions.get(session_id) else {
            return false;
        };
        let event = Event::default().data(data);
        match channel.sender.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    session_id = %session_id,
                    "send_to_session: channel full — notification dropped"
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

impl Default for SubscriptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a registry with one attached session.
    async fn make_registry_with_session(
        session_id: &str,
        app_id: &str,
    ) -> (Arc<SubscriptionRegistry>, mpsc::Receiver<Event>) {
        let registry = Arc::new(SubscriptionRegistry::new());
        let rx = registry.attach_sse(session_id, app_id).await;
        (registry, rx)
    }

    // ── subscribe / unsubscribe round-trip ───────────────────────────

    #[tokio::test]
    async fn subscribe_and_unsubscribe_round_trip() {
        let (registry, _rx) = make_registry_with_session("s1", "app-1").await;

        registry.subscribe("s1", "rivers://app-1/items", 100).await.unwrap();

        let snap = registry.snapshot_subscriptions().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0], ("s1".to_string(), "rivers://app-1/items".to_string()));

        registry.unsubscribe("s1", "rivers://app-1/items").await;

        let snap = registry.snapshot_subscriptions().await;
        assert!(snap.is_empty());
    }

    // ── max-subscriptions enforcement ────────────────────────────────

    #[tokio::test]
    async fn max_subscriptions_enforced() {
        let (registry, _rx) = make_registry_with_session("s1", "app-1").await;

        registry.subscribe("s1", "rivers://app-1/a", 2).await.unwrap();
        registry.subscribe("s1", "rivers://app-1/b", 2).await.unwrap();

        let err = registry
            .subscribe("s1", "rivers://app-1/c", 2)
            .await
            .unwrap_err();
        assert_eq!(err, SubscribeError::TooMany);

        // Existing subscriptions are intact.
        let snap = registry.snapshot_subscriptions().await;
        assert_eq!(snap.len(), 2);
    }

    // ── notification delivery ─────────────────────────────────────────

    #[tokio::test]
    async fn notification_delivered_to_subscriber() {
        let (registry, mut rx) = make_registry_with_session("s1", "app-1").await;

        registry
            .subscribe("s1", "rivers://app-1/items", 100)
            .await
            .unwrap();

        registry.notify_changed("rivers://app-1/items").await;

        let event = rx.recv().await.expect("should receive notification");
        // Event::default().data(...) produces a data field — verify it's present
        // by checking the debug representation contains the URI.
        let debug = format!("{:?}", event);
        assert!(debug.contains("rivers://app-1/items"), "event: {}", debug);
    }

    // ── notification not delivered to non-subscriber ──────────────────

    #[tokio::test]
    async fn notification_not_delivered_when_not_subscribed() {
        let (registry, mut rx) = make_registry_with_session("s1", "app-1").await;

        // Subscribed to "a" but notify about "b".
        registry.subscribe("s1", "rivers://app-1/a", 100).await.unwrap();
        registry.notify_changed("rivers://app-1/b").await;

        // Channel should be empty.
        assert!(rx.try_recv().is_err(), "should not have received notification for unsubscribed URI");
    }

    // ── slow consumer: channel full → WARN and drop ───────────────────

    #[tokio::test]
    async fn slow_consumer_notifications_dropped_when_channel_full() {
        let (registry, _rx) = make_registry_with_session("s1", "app-1").await;
        // _rx is intentionally NOT read — this fills the channel.

        registry
            .subscribe("s1", "rivers://app-1/items", 100)
            .await
            .unwrap();

        // Fill the channel (capacity 64) + 10 more to confirm drops don't panic.
        for _ in 0..74 {
            registry.notify_changed("rivers://app-1/items").await;
        }
        // Test passes if no panic occurred. The WARN log lines are side-effects.
    }

    // ── dedupe: same URI subscribed only once per session ─────────────

    #[tokio::test]
    async fn dedupe_same_uri_subscribed_only_once() {
        let (registry, _rx) = make_registry_with_session("s1", "app-1").await;

        registry.subscribe("s1", "rivers://app-1/items", 100).await.unwrap();
        // Second subscribe to same URI is idempotent.
        registry.subscribe("s1", "rivers://app-1/items", 100).await.unwrap();

        let snap = registry.snapshot_subscriptions().await;
        assert_eq!(snap.len(), 1, "should have exactly one subscription, not two");
    }

    // ── detach clears all subscriptions ──────────────────────────────

    #[tokio::test]
    async fn detach_removes_all_subscriptions() {
        let (registry, _rx) = make_registry_with_session("s1", "app-1").await;

        registry.subscribe("s1", "rivers://app-1/a", 100).await.unwrap();
        registry.subscribe("s1", "rivers://app-1/b", 100).await.unwrap();

        registry.detach("s1").await;

        let snap = registry.snapshot_subscriptions().await;
        assert!(snap.is_empty());
        assert!(!registry.has_subscribers("rivers://app-1/a").await);
    }

    // ── session not found ─────────────────────────────────────────────

    #[tokio::test]
    async fn subscribe_returns_session_not_found_for_unknown_session() {
        let registry = SubscriptionRegistry::new();
        let err = registry
            .subscribe("nonexistent", "rivers://app-1/items", 100)
            .await
            .unwrap_err();
        assert_eq!(err, SubscribeError::SessionNotFound);
    }

    // ── multiple sessions, fan-out ────────────────────────────────────

    #[tokio::test]
    async fn notify_fans_out_to_multiple_sessions() {
        let registry = Arc::new(SubscriptionRegistry::new());
        let mut rx1 = registry.attach_sse("s1", "app-1").await;
        let mut rx2 = registry.attach_sse("s2", "app-1").await;

        registry.subscribe("s1", "rivers://app-1/items", 100).await.unwrap();
        registry.subscribe("s2", "rivers://app-1/items", 100).await.unwrap();

        registry.notify_changed("rivers://app-1/items").await;

        assert!(rx1.recv().await.is_some(), "s1 should receive notification");
        assert!(rx2.recv().await.is_some(), "s2 should receive notification");
    }
}
