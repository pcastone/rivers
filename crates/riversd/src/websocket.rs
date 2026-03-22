//! WebSocket view layer.
//!
//! Per `rivers-view-layer-spec.md` §6.
//!
//! Provides WebSocket upgrade handling, Broadcast/Direct modes,
//! connection registry, rate limiting, and session revalidation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

// ── WebSocket Mode ──────────────────────────────────────────────

/// WebSocket routing mode.
///
/// Per spec §6.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMode {
    /// All connections share one broadcast channel.
    Broadcast,
    /// Per-connection routing via ConnectionRegistry.
    Direct,
}

impl WebSocketMode {
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            Some(s) if s.eq_ignore_ascii_case("direct") => WebSocketMode::Direct,
            _ => WebSocketMode::Broadcast, // default
        }
    }
}

// ── Connection ──────────────────────────────────────────────────

/// A WebSocket connection identifier.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ConnectionId(pub String);

impl ConnectionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadata about a single WebSocket connection.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub id: ConnectionId,
    pub view_id: String,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub session_id: Option<String>,
    pub path_params: HashMap<String, String>,
}

// ── Connection Registry ─────────────────────────────────────────

/// Per-route connection registry for Direct mode routing.
///
/// Per spec §6.2: maps connection ID → sender for targeted messaging.
pub struct ConnectionRegistry {
    connections: RwLock<HashMap<String, ConnectionEntry>>,
    connection_count: AtomicUsize,
    max_connections: Option<usize>,
}

struct ConnectionEntry {
    info: ConnectionInfo,
    sender: broadcast::Sender<WebSocketMessage>,
}

impl ConnectionRegistry {
    pub fn new(max_connections: Option<usize>) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            connection_count: AtomicUsize::new(0),
            max_connections,
        }
    }

    /// Register a new connection. Returns `Err(ConnectionLimitExceeded)` if at capacity.
    ///
    /// Per spec §6.4: 503 when max_connections exceeded.
    pub async fn register(
        &self,
        info: ConnectionInfo,
    ) -> Result<broadcast::Receiver<WebSocketMessage>, WebSocketError> {
        if let Some(max) = self.max_connections {
            if self.connection_count.load(Ordering::Relaxed) >= max {
                return Err(WebSocketError::ConnectionLimitExceeded(max));
            }
        }

        let (tx, rx) = broadcast::channel(256);
        let id = info.id.0.clone();

        let mut conns = self.connections.write().await;
        conns.insert(
            id,
            ConnectionEntry {
                info,
                sender: tx,
            },
        );
        self.connection_count.fetch_add(1, Ordering::Relaxed);

        Ok(rx)
    }

    /// Unregister a connection on disconnect.
    pub async fn unregister(&self, connection_id: &str) {
        let mut conns = self.connections.write().await;
        if conns.remove(connection_id).is_some() {
            // saturating_sub per spec §6.4
            self.connection_count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    Some(n.saturating_sub(1))
                })
                .ok();
        }
    }

    /// Send a message to a specific connection (Direct mode).
    pub async fn send_to(
        &self,
        connection_id: &str,
        message: WebSocketMessage,
    ) -> Result<(), WebSocketError> {
        let conns = self.connections.read().await;
        match conns.get(connection_id) {
            Some(entry) => {
                entry
                    .sender
                    .send(message)
                    .map_err(|_| WebSocketError::SendFailed("connection receiver dropped".into()))?;
                Ok(())
            }
            None => Err(WebSocketError::ConnectionNotFound(connection_id.to_string())),
        }
    }

    /// Current number of active connections.
    pub fn active_connections(&self) -> usize {
        self.connection_count.load(Ordering::Relaxed)
    }

    /// Maximum allowed connections.
    pub fn max_connections(&self) -> Option<usize> {
        self.max_connections
    }

    /// Get connection info for a specific connection.
    pub async fn get_info(&self, connection_id: &str) -> Option<ConnectionInfo> {
        let conns = self.connections.read().await;
        conns.get(connection_id).map(|e| e.info.clone())
    }

    /// Get all connection IDs (for broadcast in Direct mode).
    pub async fn all_connection_ids(&self) -> Vec<String> {
        let conns = self.connections.read().await;
        conns.keys().cloned().collect()
    }
}

// ── Broadcast Hub ───────────────────────────────────────────────

/// Shared broadcast hub for Broadcast mode.
///
/// Per spec §6.2: all connections on a route share one channel.
pub struct BroadcastHub {
    sender: broadcast::Sender<WebSocketMessage>,
    connection_count: AtomicUsize,
    max_connections: Option<usize>,
}

impl BroadcastHub {
    pub fn new(max_connections: Option<usize>) -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            sender,
            connection_count: AtomicUsize::new(0),
            max_connections,
        }
    }

    /// Subscribe a new connection. Returns `Err(ConnectionLimitExceeded)` if at capacity.
    pub fn subscribe(&self) -> Result<broadcast::Receiver<WebSocketMessage>, WebSocketError> {
        if let Some(max) = self.max_connections {
            if self.connection_count.load(Ordering::Relaxed) >= max {
                return Err(WebSocketError::ConnectionLimitExceeded(max));
            }
        }
        self.connection_count.fetch_add(1, Ordering::Relaxed);
        Ok(self.sender.subscribe())
    }

    /// Unsubscribe (decrement count on disconnect).
    pub fn unsubscribe(&self) {
        self.connection_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .ok();
    }

    /// Broadcast a message to all subscribers.
    pub fn broadcast(&self, message: WebSocketMessage) -> Result<usize, WebSocketError> {
        self.sender
            .send(message)
            .map_err(|_| WebSocketError::SendFailed("no active receivers".into()))
    }

    /// Current number of active connections.
    pub fn active_connections(&self) -> usize {
        self.connection_count.load(Ordering::Relaxed)
    }
}

// ── WebSocket Message ───────────────────────────────────────────

/// A WebSocket message envelope.
#[derive(Debug, Clone)]
pub struct WebSocketMessage {
    pub payload: String,
    pub connection_id: Option<String>,
}

impl WebSocketMessage {
    pub fn text(payload: String) -> Self {
        Self {
            payload,
            connection_id: None,
        }
    }

    pub fn directed(payload: String, connection_id: String) -> Self {
        Self {
            payload,
            connection_id: Some(connection_id),
        }
    }
}

// ── Rate Limiter (per-connection) ───────────────────────────────

/// Per-connection WebSocket rate limiter using token bucket.
///
/// Per spec §6.5: messages_per_sec with burst.
pub struct WsRateLimiter {
    messages_per_sec: f64,
    burst: u32,
    tokens: std::sync::Mutex<f64>,
    last_refill: std::sync::Mutex<std::time::Instant>,
}

impl WsRateLimiter {
    /// Create a new rate limiter.
    ///
    /// Per spec §6.5 defaults: messages_per_sec=100, burst=20 when enabled but unspecified.
    pub fn new(rate_limit_per_minute: Option<u32>, burst_size: Option<u32>) -> Option<Self> {
        let rpm = rate_limit_per_minute?;
        if rpm == 0 {
            return None;
        }

        let messages_per_sec = rpm as f64 / 60.0;
        let burst = burst_size.unwrap_or(20);

        Some(Self {
            messages_per_sec,
            burst,
            tokens: std::sync::Mutex::new(burst as f64),
            last_refill: std::sync::Mutex::new(std::time::Instant::now()),
        })
    }

    /// Check if a message is allowed. Returns true if allowed.
    pub fn check(&self) -> bool {
        let mut tokens = self.tokens.lock().unwrap();
        let mut last = self.last_refill.lock().unwrap();

        let now = std::time::Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64();

        // Refill tokens
        *tokens = (*tokens + elapsed * self.messages_per_sec).min(self.burst as f64);
        *last = now;

        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn messages_per_sec(&self) -> f64 {
        self.messages_per_sec
    }

    pub fn burst(&self) -> u32 {
        self.burst
    }
}

// ── Binary Frame Tracker (SHAPE-13) ─────────────────────────────

/// Per-connection binary frame counter for rate-limited logging.
///
/// Per SHAPE-13: first binary frame logs WARN; subsequent frames increment
/// a counter; every 60 seconds a summary is emitted if count > 0.
pub struct BinaryFrameTracker {
    count: std::sync::atomic::AtomicU64,
    first_logged: std::sync::atomic::AtomicBool,
}

impl BinaryFrameTracker {
    pub fn new() -> Self {
        Self {
            count: std::sync::atomic::AtomicU64::new(0),
            first_logged: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Record a binary frame. Returns `true` if this is the first frame
    /// (caller should emit a WARN log).
    pub fn record_binary_frame(&self) -> bool {
        self.count.fetch_add(1, Ordering::Relaxed);
        // CAS: only the first call returns true
        self.first_logged
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Drain the counter for periodic summary logging.
    /// Returns the count since last drain.
    pub fn drain_count(&self) -> u64 {
        self.count.swap(0, Ordering::Relaxed)
    }

    /// Total binary frames received (without draining).
    pub fn total(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

impl Default for BinaryFrameTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Session Revalidation ────────────────────────────────────────

/// Session revalidation message sent before closing expired WS connections.
///
/// Per spec §6.7.
pub fn session_expired_message() -> WebSocketMessage {
    WebSocketMessage::text(
        serde_json::json!({"rivers_session_expired": true}).to_string(),
    )
}

// ── WebSocket Route Registry ────────────────────────────────────

/// Manages all WebSocket routes and their associated hubs/registries.
pub struct WebSocketRouteManager {
    /// Broadcast hubs keyed by view_id.
    broadcast_hubs: RwLock<HashMap<String, Arc<BroadcastHub>>>,
    /// Connection registries keyed by view_id (Direct mode).
    direct_registries: RwLock<HashMap<String, Arc<ConnectionRegistry>>>,
}

impl WebSocketRouteManager {
    pub fn new() -> Self {
        Self {
            broadcast_hubs: RwLock::new(HashMap::new()),
            direct_registries: RwLock::new(HashMap::new()),
        }
    }

    /// Register a Broadcast mode WebSocket route.
    pub async fn register_broadcast(
        &self,
        view_id: String,
        max_connections: Option<usize>,
    ) -> Arc<BroadcastHub> {
        let hub = Arc::new(BroadcastHub::new(max_connections));
        self.broadcast_hubs
            .write()
            .await
            .insert(view_id, hub.clone());
        hub
    }

    /// Register a Direct mode WebSocket route.
    pub async fn register_direct(
        &self,
        view_id: String,
        max_connections: Option<usize>,
    ) -> Arc<ConnectionRegistry> {
        let registry = Arc::new(ConnectionRegistry::new(max_connections));
        self.direct_registries
            .write()
            .await
            .insert(view_id, registry.clone());
        registry
    }

    /// Get a broadcast hub by view ID.
    pub async fn get_broadcast(&self, view_id: &str) -> Option<Arc<BroadcastHub>> {
        self.broadcast_hubs.read().await.get(view_id).cloned()
    }

    /// Get a connection registry by view ID.
    pub async fn get_direct(&self, view_id: &str) -> Option<Arc<ConnectionRegistry>> {
        self.direct_registries.read().await.get(view_id).cloned()
    }
}

impl Default for WebSocketRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── WS on_stream Handler (D7) ───────────────────────────────────

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError};

/// Execute the on_stream CodeComponent handler for a WebSocket message.
///
/// Per spec §6.3: the handler receives the message payload and connection
/// info, and may return an optional reply message to send back.
pub async fn execute_ws_on_stream(
    pool: &ProcessPoolManager,
    entrypoint: &Entrypoint,
    message: &serde_json::Value,
    connection_id: &ConnectionId,
    trace_id: &str,
) -> Result<Option<serde_json::Value>, TaskError> {
    let args = serde_json::json!({
        "message": message,
        "connection_id": connection_id.0,
    });

    let ctx = TaskContextBuilder::new()
        .entrypoint(entrypoint.clone())
        .args(args)
        .trace_id(trace_id.to_string())
        .build()?;

    let result = pool.dispatch("default", ctx).await?;

    // If the handler returns a value with "reply", send it back
    let reply = result.value.get("reply").cloned();
    Ok(reply)
}

// ── WebSocket Lifecycle Dispatch (N4.4–N4.5) ────────────────────

/// Dispatch a WebSocket lifecycle event to the ProcessPool.
///
/// Per technology-path-spec §14.2: three hooks (onConnect, onMessage, onDisconnect),
/// all short-lived ProcessPool invocations.
pub async fn dispatch_ws_lifecycle(
    pool: &ProcessPoolManager,
    hook_module: &str,
    hook_entrypoint: &str,
    connection_id: &str,
    message: Option<&serde_json::Value>,
    session: Option<&serde_json::Value>,
    trace_id: &str,
) -> Result<serde_json::Value, TaskError> {
    let entrypoint = Entrypoint {
        module: hook_module.to_string(),
        function: hook_entrypoint.to_string(),
        language: "javascript".to_string(),
    };

    let args = serde_json::json!({
        "ws": {
            "connection_id": connection_id,
            "message": message,
        },
        "session": session,
        "trace_id": trace_id,
    });

    let task_ctx = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(trace_id.to_string())
        .build()?;

    let result = pool.dispatch("default", task_ctx).await?;
    Ok(result.value)
}

// ── WebSocket Lag Detection (D8) ────────────────────────────────

/// WebSocket lag detector — tracks send queue depth.
///
/// Used to detect when a client is falling behind on message consumption.
/// Per spec §6.5: lag detection triggers warning logs and may disconnect.
pub struct LagDetector {
    /// Maximum allowed queue depth before critical lag.
    pub max_queue_depth: usize,
    /// Current pending message count.
    pub current_depth: AtomicUsize,
    /// Total number of lag events detected.
    pub lag_events: AtomicUsize,
}

/// Lag status indicating send queue health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LagStatus {
    /// Queue depth is within normal range.
    Normal,
    /// Queue depth is > 50% — client may be slow.
    Warning,
    /// Queue depth is > 90% — may disconnect client.
    Critical,
}

impl LagDetector {
    /// Create a new lag detector with the given max queue depth.
    pub fn new(max_queue_depth: usize) -> Self {
        Self {
            max_queue_depth,
            current_depth: AtomicUsize::new(0),
            lag_events: AtomicUsize::new(0),
        }
    }

    /// Check the current lag status.
    pub fn check_lag(&self) -> LagStatus {
        let depth = self.current_depth.load(Ordering::Relaxed);
        let threshold_warning = self.max_queue_depth / 2;
        let threshold_critical = self.max_queue_depth * 9 / 10;

        if depth > threshold_critical {
            self.lag_events.fetch_add(1, Ordering::Relaxed);
            LagStatus::Critical
        } else if depth > threshold_warning {
            LagStatus::Warning
        } else {
            LagStatus::Normal
        }
    }

    /// Record that a message was queued for sending.
    pub fn on_send(&self) {
        self.current_depth.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a message was acknowledged/consumed by the client.
    pub fn on_ack(&self) {
        self.current_depth
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .ok();
    }

    /// Current queue depth.
    pub fn depth(&self) -> usize {
        self.current_depth.load(Ordering::Relaxed)
    }

    /// Total lag events recorded.
    pub fn total_lag_events(&self) -> usize {
        self.lag_events.load(Ordering::Relaxed)
    }
}

// ── Error Types ─────────────────────────────────────────────────

/// WebSocket errors.
#[derive(Debug, thiserror::Error)]
pub enum WebSocketError {
    #[error("connection limit exceeded: max {0}")]
    ConnectionLimitExceeded(usize),

    #[error("connection not found: {0}")]
    ConnectionNotFound(String),

    #[error("send failed: {0}")]
    SendFailed(String),

    #[error("upgrade failed: {0}")]
    UpgradeFailed(String),

    #[error("handler requires CodeComponent (not yet available)")]
    CodeComponentRequired,

    #[error("rate limited")]
    RateLimited,

    #[error("session expired")]
    SessionExpired,
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── D7: WS on_stream tests ──────────────────────────────

    #[tokio::test]
    async fn test_ws_on_stream_engine_unavailable() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "ws_handler.js".into(),
            function: "on_stream".into(),
            language: "javascript".into(),
        };
        let message = serde_json::json!({"text": "hello"});
        let conn_id = ConnectionId("conn-1".to_string());

        let result =
            execute_ws_on_stream(&pool, &entrypoint, &message, &conn_id, "trace-1").await;
        // Should fail with EngineUnavailable
        assert!(result.is_err());
    }

    // ── N4.4–N4.5: WebSocket lifecycle dispatch tests ──────

    #[tokio::test]
    async fn test_dispatch_ws_lifecycle_on_connect() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let result = dispatch_ws_lifecycle(
            &pool,
            "ws_handler.js",
            "onConnect",
            "conn-123",
            None,
            Some(&serde_json::json!({"user_id": "u-1"})),
            "trace-ws-1",
        )
        .await;
        // Engine unavailable — expected in stub mode
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dispatch_ws_lifecycle_on_message() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let msg = serde_json::json!({"text": "hello"});
        let result = dispatch_ws_lifecycle(
            &pool,
            "ws_handler.js",
            "onMessage",
            "conn-123",
            Some(&msg),
            None,
            "trace-ws-2",
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dispatch_ws_lifecycle_on_disconnect() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let result = dispatch_ws_lifecycle(
            &pool,
            "ws_handler.js",
            "onDisconnect",
            "conn-123",
            None,
            None,
            "trace-ws-3",
        )
        .await;
        assert!(result.is_err());
    }

    // ── D8: Lag detection tests ─────────────────────────────

    #[test]
    fn test_lag_detector_normal() {
        let detector = LagDetector::new(100);
        assert_eq!(detector.check_lag(), LagStatus::Normal);
        assert_eq!(detector.depth(), 0);
    }

    #[test]
    fn test_lag_detector_warning() {
        let detector = LagDetector::new(100);
        // Push depth above 50%
        for _ in 0..55 {
            detector.on_send();
        }
        assert_eq!(detector.check_lag(), LagStatus::Warning);
    }

    #[test]
    fn test_lag_detector_critical() {
        let detector = LagDetector::new(100);
        // Push depth above 90%
        for _ in 0..95 {
            detector.on_send();
        }
        assert_eq!(detector.check_lag(), LagStatus::Critical);
        assert_eq!(detector.total_lag_events(), 1);
    }

    #[test]
    fn test_lag_detector_on_ack() {
        let detector = LagDetector::new(100);
        detector.on_send();
        detector.on_send();
        assert_eq!(detector.depth(), 2);
        detector.on_ack();
        assert_eq!(detector.depth(), 1);
        detector.on_ack();
        assert_eq!(detector.depth(), 0);
        // Saturating sub — shouldn't underflow
        detector.on_ack();
        assert_eq!(detector.depth(), 0);
    }

    #[test]
    fn test_lag_detector_events_counted() {
        let detector = LagDetector::new(10);
        for _ in 0..10 {
            detector.on_send();
        }
        detector.check_lag(); // Critical
        detector.check_lag(); // Critical again
        assert_eq!(detector.total_lag_events(), 2);
    }
}
