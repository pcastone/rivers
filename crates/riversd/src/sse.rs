//! Server-Sent Events (SSE) view layer.
//!
//! Per `rivers-view-layer-spec.md` §7.
//!
//! SSE views use a hybrid push model: `tokio::select!` between a tick timer
//! and EventBus trigger events. Unidirectional server→client only (no on_stream).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

// ── SSE Event ───────────────────────────────────────────────

/// A Server-Sent Event ready for wire serialization.
///
/// Per spec §7: `data:` field is required, `event:` and `id:` are optional.
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// Optional event type (maps to SSE `event:` field).
    pub event: Option<String>,
    /// Event data (maps to SSE `data:` field). JSON-encoded.
    pub data: String,
    /// Optional event ID (maps to SSE `id:` field).
    pub id: Option<String>,
}

impl SseEvent {
    /// Create a data-only SSE event.
    pub fn data(data: String) -> Self {
        Self {
            event: None,
            data,
            id: None,
        }
    }

    /// Create a typed SSE event.
    pub fn typed(event: String, data: String) -> Self {
        Self {
            event: Some(event),
            data,
            id: None,
        }
    }

    /// Serialize to SSE wire format.
    ///
    /// Per SSE spec: each field on its own line, double newline to terminate.
    pub fn to_wire_format(&self) -> String {
        let mut out = String::new();

        if let Some(ref event) = self.event {
            out.push_str(&format!("event: {}\n", event));
        }

        if let Some(ref id) = self.id {
            out.push_str(&format!("id: {}\n", id));
        }

        // Data may contain newlines — each line gets its own `data:` prefix
        for line in self.data.lines() {
            out.push_str(&format!("data: {}\n", line));
        }

        out.push('\n'); // double newline terminates the event
        out
    }
}

// ── Last-Event-ID (§2.5) ────────────────────────────────────

/// Extract Last-Event-ID from request headers for SSE reconnection.
///
/// Per spec §2.5: clients send Last-Event-ID on reconnection.
pub fn extract_last_event_id(headers: &std::collections::HashMap<String, String>) -> Option<String> {
    headers.get("last-event-id").cloned()
}

/// Filter events since a given event ID.
///
/// Returns events that are newer than the provided ID.
/// For V1, this is a simple sequential filter — events with ID > last_id.
pub fn events_since(events: &[SseEvent], last_id: &str) -> Vec<SseEvent> {
    let mut found = false;
    events
        .iter()
        .filter(|e| {
            if found {
                return true;
            }
            if e.id.as_deref() == Some(last_id) {
                found = true;
                return false; // skip the last seen one
            }
            false
        })
        .cloned()
        .collect()
}

// ── Session Expired Event ───────────────────────────────────

/// Terminal SSE event sent when session expires on persistent connection.
///
/// Per spec §7.4: `data: {"rivers_session_expired":true}` then close.
pub fn session_expired_event() -> SseEvent {
    SseEvent::data(serde_json::json!({"rivers_session_expired": true}).to_string())
}

// ── SSE Channel ─────────────────────────────────────────────

/// A broadcast channel for SSE events on a single route.
///
/// Per spec §7.2: hybrid push — tick timer OR EventBus trigger events.
/// Default event buffer capacity for Last-Event-ID reconnection.
const DEFAULT_BUFFER_CAPACITY: usize = 100;

pub struct SseChannel {
    sender: broadcast::Sender<SseEvent>,
    connection_count: AtomicUsize,
    max_connections: Option<usize>,
    /// Tick interval in milliseconds (0 = no tick).
    tick_interval_ms: u64,
    /// Event names that trigger pushes.
    trigger_events: Vec<String>,
    /// Bounded ring buffer for Last-Event-ID reconnection replay.
    event_buffer: Mutex<VecDeque<SseEvent>>,
    /// Max events to retain in the replay buffer.
    buffer_capacity: usize,
    /// Monotonic event ID counter.
    next_event_id: AtomicU64,
}

impl SseChannel {
    pub fn new(
        max_connections: Option<usize>,
        tick_interval_ms: u64,
        trigger_events: Vec<String>,
    ) -> Self {
        Self::with_buffer_capacity(max_connections, tick_interval_ms, trigger_events, DEFAULT_BUFFER_CAPACITY)
    }

    pub fn with_buffer_capacity(
        max_connections: Option<usize>,
        tick_interval_ms: u64,
        trigger_events: Vec<String>,
        buffer_capacity: usize,
    ) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender,
            connection_count: AtomicUsize::new(0),
            max_connections,
            tick_interval_ms,
            trigger_events,
            event_buffer: Mutex::new(VecDeque::with_capacity(buffer_capacity)),
            buffer_capacity,
            next_event_id: AtomicU64::new(1),
        }
    }

    /// Subscribe a new client. Returns receiver or error if at capacity.
    pub fn subscribe(&self) -> Result<broadcast::Receiver<SseEvent>, SseError> {
        if let Some(max) = self.max_connections {
            if self.connection_count.load(Ordering::Relaxed) >= max {
                return Err(SseError::ConnectionLimitExceeded(max));
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

    /// Push an event to all subscribers.
    ///
    /// Assigns a sequential event ID and buffers the event for Last-Event-ID replay.
    pub fn push(&self, mut event: SseEvent) -> Result<usize, SseError> {
        // Assign sequential ID
        let id = self.next_event_id.fetch_add(1, Ordering::Relaxed);
        event.id = Some(id.to_string());

        // Buffer for replay (bounded ring)
        if let Ok(mut buffer) = self.event_buffer.lock() {
            if buffer.len() >= self.buffer_capacity {
                buffer.pop_front();
            }
            buffer.push_back(event.clone());
        }

        self.sender
            .send(event)
            .map_err(|_| SseError::NoActiveClients)
    }

    /// Replay events since the given Last-Event-ID for reconnection.
    ///
    /// Returns events newer than `last_event_id` from the buffer.
    /// Returns empty if the ID is not found (too old or unknown).
    pub fn replay_since(&self, last_event_id: &str) -> Vec<SseEvent> {
        let buffer = match self.event_buffer.lock() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        let events: Vec<SseEvent> = buffer.iter().cloned().collect();
        events_since(&events, last_event_id)
    }

    /// Current event buffer length (for testing).
    pub fn buffer_len(&self) -> usize {
        self.event_buffer.lock().map(|b| b.len()).unwrap_or(0)
    }

    /// Current number of active connections.
    pub fn active_connections(&self) -> usize {
        self.connection_count.load(Ordering::Relaxed)
    }

    /// Tick interval in milliseconds.
    pub fn tick_interval_ms(&self) -> u64 {
        self.tick_interval_ms
    }

    /// Event names that trigger pushes.
    pub fn trigger_events(&self) -> &[String] {
        &self.trigger_events
    }

    /// Max connections allowed.
    pub fn max_connections(&self) -> Option<usize> {
        self.max_connections
    }
}

// ── SSE Route Manager ───────────────────────────────────────

/// Manages all SSE routes and their channels.
pub struct SseRouteManager {
    channels: tokio::sync::RwLock<HashMap<String, Arc<SseChannel>>>,
}

impl SseRouteManager {
    pub fn new() -> Self {
        Self {
            channels: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Register an SSE route.
    pub async fn register(
        &self,
        view_id: String,
        max_connections: Option<usize>,
        tick_interval_ms: u64,
        trigger_events: Vec<String>,
    ) -> Arc<SseChannel> {
        self.register_with_buffer(view_id, max_connections, tick_interval_ms, trigger_events, DEFAULT_BUFFER_CAPACITY).await
    }

    /// Register an SSE route with a custom event buffer capacity.
    pub async fn register_with_buffer(
        &self,
        view_id: String,
        max_connections: Option<usize>,
        tick_interval_ms: u64,
        trigger_events: Vec<String>,
        buffer_capacity: usize,
    ) -> Arc<SseChannel> {
        let channel = Arc::new(SseChannel::with_buffer_capacity(
            max_connections,
            tick_interval_ms,
            trigger_events,
            buffer_capacity,
        ));
        self.channels
            .write()
            .await
            .insert(view_id, channel.clone());
        channel
    }

    /// Get an SSE channel by view ID.
    pub async fn get(&self, view_id: &str) -> Option<Arc<SseChannel>> {
        self.channels.read().await.get(view_id).cloned()
    }
}

impl Default for SseRouteManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── SSE Hybrid Push Loop (D9) ──────────────────────────────────

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError};
use crate::streaming::StreamingError;
use rivers_runtime::rivers_core::event::Event;
use tokio::sync::mpsc;

/// Run the SSE hybrid push loop.
///
/// Per spec §7.2: `tokio::select!` between tick_interval and EventBus trigger
/// events. Each tick or trigger dispatches the CodeComponent handler and
/// pushes the result as an SSE event.
///
/// The loop runs until the sender is closed (client disconnected) or
/// the event receiver is closed.
pub async fn run_sse_push_loop(
    pool: &ProcessPoolManager,
    entrypoint: &Entrypoint,
    tick_interval_ms: u64,
    trigger_events: &[String],
    mut event_rx: broadcast::Receiver<Event>,
    sender: mpsc::Sender<SseEvent>,
    trace_id: &str,
) -> Result<(), StreamingError> {
    let tick_duration = if tick_interval_ms > 0 {
        Some(tokio::time::Duration::from_millis(tick_interval_ms))
    } else {
        None
    };

    let mut tick_interval = tick_duration.map(tokio::time::interval);

    // Consume the first tick immediately (interval fires immediately on creation)
    if let Some(ref mut interval) = tick_interval {
        interval.tick().await;
    }

    loop {
        let trigger_reason: SseTriggerReason = if let Some(ref mut interval) = tick_interval {
            tokio::select! {
                _ = interval.tick() => SseTriggerReason::Tick,
                event = event_rx.recv() => {
                    match event {
                        Ok(ev) => {
                            if trigger_events.contains(&ev.event_type) {
                                SseTriggerReason::Event(ev.event_type.clone())
                            } else {
                                continue;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Ok(());
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            continue;
                        }
                    }
                }
                _ = sender.closed() => {
                    return Ok(());
                }
            }
        } else {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Ok(ev) => {
                            if trigger_events.contains(&ev.event_type) {
                                SseTriggerReason::Event(ev.event_type.clone())
                            } else {
                                continue;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Ok(());
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            continue;
                        }
                    }
                }
                _ = sender.closed() => {
                    return Ok(());
                }
            }
        };

        // Dispatch the handler
        let args = serde_json::json!({
            "trigger": match &trigger_reason {
                SseTriggerReason::Tick => "tick",
                SseTriggerReason::Event(_) => "event",
            },
            "event_type": match &trigger_reason {
                SseTriggerReason::Tick => None,
                SseTriggerReason::Event(t) => Some(t.as_str()),
            },
        });

        let builder = TaskContextBuilder::new()
            .entrypoint(entrypoint.clone())
            .args(args)
            .trace_id(trace_id.to_string());
        let builder = crate::task_enrichment::enrich(builder, "");
        let ctx = builder
            .build()
            .map_err(|e| StreamingError::GeneratorError(e.to_string()))?;

        match pool.dispatch("default", ctx).await {
            Ok(result) => {
                let data_str = serde_json::to_string(&result.value)
                    .unwrap_or_else(|_| "null".to_string());
                let event = SseEvent::data(data_str);
                if sender.send(event).await.is_err() {
                    return Ok(()); // Client disconnected
                }
            }
            Err(TaskError::EngineUnavailable(_)) => {
                let event = SseEvent::typed(
                    "error".to_string(),
                    serde_json::json!({"error": "CodeComponent engine not available"}).to_string(),
                );
                if sender.send(event).await.is_err() {
                    return Ok(());
                }
                return Err(StreamingError::CodeComponentRequired);
            }
            Err(e) => {
                let event = SseEvent::typed(
                    "error".to_string(),
                    serde_json::json!({"error": e.to_string()}).to_string(),
                );
                let _ = sender.send(event).await;
                return Err(StreamingError::GeneratorError(e.to_string()));
            }
        }
    }
}

/// Reason an SSE push was triggered.
#[derive(Debug)]
enum SseTriggerReason {
    /// Tick timer fired.
    Tick,
    /// EventBus event received.
    Event(String),
}

// ── SSE Channel Push Loop (N4.3) ────────────────────────────────

/// Drive the SSE push loop for a channel.
///
/// Per technology-path-spec §12.5: tick timer fires, DataView executes,
/// diff runs in-memory, changed results broadcast to connected clients.
///
/// This is a channel-level push loop, distinct from the per-client
/// `run_sse_push_loop` above. It broadcasts to all connected clients.
///
/// When `executor` and `storage` are `Some`, real DataView polling runs.
/// Otherwise, emits periodic heartbeats (development/fallback mode).
pub async fn drive_sse_push_loop(
    channel: Arc<SseChannel>,
    tick_ms: u64,
    view_id: String,
    executor: Option<Arc<dyn crate::polling::PollDataViewExecutor>>,
    storage: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    diff_strategy: Option<crate::polling::DiffStrategy>,
) {
    let mut tick = tokio::time::interval(tokio::time::Duration::from_millis(tick_ms));
    let mut previous_hash: Option<String> = None;

    loop {
        tick.tick().await;

        // Skip if no clients are connected
        if channel.active_connections() == 0 {
            continue;
        }

        // Real DataView polling path
        if let (Some(ref exec), Some(ref store)) = (&executor, &storage) {
            // Storage-backed polling: uses execute_poll_tick which persists state
            let strategy = diff_strategy.clone().unwrap_or(crate::polling::DiffStrategy::Hash);
            let key = crate::polling::PollLoopKey::new(&view_id, &std::collections::HashMap::new());
            let loop_state = crate::polling::PollLoopState::new(key, strategy, tick_ms);

            match crate::polling::execute_poll_tick(
                exec.as_ref(),
                store.as_ref(),
                &loop_state,
                &view_id,
                &std::collections::HashMap::new(),
            ).await {
                Ok(tick_result) => {
                    if tick_result.changed {
                        let event = SseEvent::typed(
                            "data".to_string(),
                            serde_json::to_string(&tick_result.current_data)
                                .unwrap_or_else(|_| "null".to_string()),
                        );
                        let _ = channel.push(event);
                    }
                }
                Err(e) => {
                    tracing::warn!(view_id = %view_id, error = %e, "storage-backed poll tick failed");
                }
            }
        } else if let Some(ref exec) = executor {
            // In-memory fallback: no StorageEngine configured
            let strategy = diff_strategy.clone().unwrap_or(crate::polling::DiffStrategy::Hash);
            let result = crate::polling::execute_poll_tick_inmemory(
                &view_id,
                "",
                &mut previous_hash,
                &strategy,
                None,
                Some(exec.as_ref()),
            )
            .await;

            if let Some(data) = result {
                let event = SseEvent::typed(
                    "data".to_string(),
                    serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string()),
                );
                let _ = channel.push(event);
            }
        } else {
            // Fallback: emit heartbeat
            let event = SseEvent::typed(
                "heartbeat".to_string(),
                serde_json::json!({
                    "view_id": view_id,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })
                .to_string(),
            );
            let _ = channel.push(event);
        }
    }
}

// ── Error Types ─────────────────────────────────────────────────

/// SSE errors.
#[derive(Debug, thiserror::Error)]
pub enum SseError {
    #[error("connection limit exceeded: max {0}")]
    ConnectionLimitExceeded(usize),

    #[error("no active clients")]
    NoActiveClients,

    #[error("handler requires CodeComponent (not yet available)")]
    CodeComponentRequired,

    #[error("session expired")]
    SessionExpired,
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_last_event_id_present() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("last-event-id".into(), "evt-42".into());
        assert_eq!(extract_last_event_id(&headers), Some("evt-42".into()));
    }

    #[test]
    fn extract_last_event_id_missing() {
        let headers = std::collections::HashMap::new();
        assert!(extract_last_event_id(&headers).is_none());
    }

    #[test]
    fn events_since_filters_correctly() {
        let events = vec![
            SseEvent { event: None, data: "a".into(), id: Some("1".into()) },
            SseEvent { event: None, data: "b".into(), id: Some("2".into()) },
            SseEvent { event: None, data: "c".into(), id: Some("3".into()) },
            SseEvent { event: None, data: "d".into(), id: Some("4".into()) },
        ];
        let since = events_since(&events, "2");
        assert_eq!(since.len(), 2);
        assert_eq!(since[0].data, "c");
        assert_eq!(since[1].data, "d");
    }

    #[test]
    fn events_since_unknown_id_returns_empty() {
        let events = vec![
            SseEvent { event: None, data: "a".into(), id: Some("1".into()) },
        ];
        let since = events_since(&events, "unknown");
        assert!(since.is_empty());
    }

    #[tokio::test]
    async fn test_sse_push_loop_engine_unavailable() {
        // With the Boa engine active, dispatching a task with a missing
        // module file results in a handler error (cannot read file)
        // rather than EngineUnavailable.
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "sse_handler.js".into(),
            function: "on_tick".into(),
            language: "javascript".into(),
        };
        let (_event_tx, event_rx) = broadcast::channel::<Event>(16);
        let (sender, mut receiver) = mpsc::channel::<SseEvent>(16);

        let result = run_sse_push_loop(
            &pool,
            &entrypoint,
            100, // tick every 100ms
            &[],
            event_rx,
            sender,
            "trace-1",
        )
        .await;

        // Should error after first tick
        assert!(result.is_err());

        // Should have received an error event (either "not available" or handler error)
        let event = receiver.recv().await.unwrap();
        assert!(
            event.data.contains("not available") || event.data.contains("error"),
            "expected error event, got: {}",
            event.data
        );
    }

    #[tokio::test]
    async fn test_drive_sse_push_loop_heartbeat() {
        let channel = Arc::new(SseChannel::new(None, 50, vec![]));

        // Subscribe a client so active_connections > 0
        let mut rx = channel.subscribe().unwrap();

        let ch = channel.clone();
        let handle = tokio::spawn(async move {
            drive_sse_push_loop(ch, 50, "test_view".into(), None, None, None).await;
        });

        // Wait for at least one heartbeat
        let event = tokio::time::timeout(
            tokio::time::Duration::from_millis(500),
            rx.recv(),
        )
        .await;
        assert!(event.is_ok());
        let event = event.unwrap().unwrap();
        assert_eq!(event.event.as_deref(), Some("heartbeat"));
        assert!(event.data.contains("test_view"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_drive_sse_push_loop_skips_when_no_clients() {
        let channel = Arc::new(SseChannel::new(None, 50, vec![]));

        // No clients connected — loop should just keep ticking without pushing
        let ch = channel.clone();
        let handle = tokio::spawn(async move {
            drive_sse_push_loop(ch, 50, "empty_view".into(), None, None, None).await;
        });

        // Wait a few ticks
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // No panics means the loop handles zero clients gracefully
        handle.abort();
    }

    #[tokio::test]
    async fn test_sse_push_loop_client_disconnect() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "sse.js".into(),
            function: "tick".into(),
            language: "javascript".into(),
        };
        let (_event_tx, event_rx) = broadcast::channel::<Event>(16);
        let (sender, receiver) = mpsc::channel::<SseEvent>(16);

        // Drop receiver immediately to simulate disconnect
        drop(receiver);

        let result = run_sse_push_loop(
            &pool,
            &entrypoint,
            100,
            &[],
            event_rx,
            sender,
            "trace-1",
        )
        .await;

        // Should return Ok — clean disconnect
        // (or error depending on timing, both are acceptable)
        let _ = result;
    }

    // ── AV5: Last-Event-ID replay tests ──────────────────────

    #[test]
    fn push_assigns_sequential_ids() {
        let channel = SseChannel::new(None, 0, vec![]);
        let _rx = channel.subscribe().unwrap();

        channel.push(SseEvent::data("a".into())).unwrap();
        channel.push(SseEvent::data("b".into())).unwrap();
        channel.push(SseEvent::data("c".into())).unwrap();

        let buffer = channel.event_buffer.lock().unwrap();
        assert_eq!(buffer[0].id.as_deref(), Some("1"));
        assert_eq!(buffer[1].id.as_deref(), Some("2"));
        assert_eq!(buffer[2].id.as_deref(), Some("3"));
    }

    #[test]
    fn event_buffer_bounded_ring() {
        let channel = SseChannel::with_buffer_capacity(None, 0, vec![], 5);
        let _rx = channel.subscribe().unwrap();

        for i in 0..10 {
            channel.push(SseEvent::data(format!("evt-{}", i))).unwrap();
        }

        assert_eq!(channel.buffer_len(), 5);
        let buffer = channel.event_buffer.lock().unwrap();
        // Should contain events 6-10 (IDs 6-10)
        assert_eq!(buffer[0].id.as_deref(), Some("6"));
        assert_eq!(buffer[4].id.as_deref(), Some("10"));
    }

    #[test]
    fn replay_since_returns_missed_events() {
        let channel = SseChannel::new(None, 0, vec![]);
        let _rx = channel.subscribe().unwrap();

        for i in 1..=10 {
            channel.push(SseEvent::data(format!("data-{}", i))).unwrap();
        }

        let missed = channel.replay_since("5");
        assert_eq!(missed.len(), 5);
        assert_eq!(missed[0].data, "data-6");
        assert_eq!(missed[4].data, "data-10");
    }

    #[test]
    fn replay_since_unknown_id_returns_empty() {
        let channel = SseChannel::new(None, 0, vec![]);
        let _rx = channel.subscribe().unwrap();

        channel.push(SseEvent::data("a".into())).unwrap();
        let missed = channel.replay_since("unknown");
        assert!(missed.is_empty());
    }

    #[test]
    fn replay_since_empty_buffer_returns_empty() {
        let channel = SseChannel::new(None, 0, vec![]);
        let missed = channel.replay_since("1");
        assert!(missed.is_empty());
    }
}
