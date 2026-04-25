//! In-process EventBus with priority-tiered dispatch.
//!
//! Per `rivers-view-layer-spec.md` §11, `rivers-logging-spec.md` §4.
//!
//! Four priority tiers control handler execution order:
//! - **Expect** — awaited first, must complete before request continues
//! - **Handle** — awaited second, normal blocking handlers
//! - **Emit** — awaited third, for side-effect emission
//! - **Observe** — fire-and-forget, spawned as background tasks (LogHandler lives here)

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::event::Event;

// ── Event type constants ────────────────────────────────────────────

/// Event type constants for all spec-defined events.
/// Per `rivers-logging-spec.md` §4 event-to-level mapping.
pub mod events {
    //! String constants for all spec-defined event types.

    // ── Request lifecycle ───────────────────────────────────────
    /// An HTTP request completed (success or error).
    pub const REQUEST_COMPLETED: &str = "RequestCompleted";

    // ── DataView ────────────────────────────────────────────────
    /// A DataView query was executed.
    pub const DATAVIEW_EXECUTED: &str = "DataViewExecuted";
    /// A cache entry was invalidated.
    pub const CACHE_INVALIDATION: &str = "CacheInvalidation";

    // ── WebSocket ───────────────────────────────────────────────
    /// A WebSocket client connected.
    pub const WEBSOCKET_CONNECTED: &str = "WebSocketConnected";
    /// A WebSocket client disconnected.
    pub const WEBSOCKET_DISCONNECTED: &str = "WebSocketDisconnected";
    /// An inbound WebSocket message was received.
    pub const WEBSOCKET_MESSAGE_IN: &str = "WebSocketMessageIn";
    /// An outbound WebSocket message was sent.
    pub const WEBSOCKET_MESSAGE_OUT: &str = "WebSocketMessageOut";

    // ── SSE ─────────────────────────────────────────────────────
    /// An SSE stream was opened.
    pub const SSE_STREAM_OPENED: &str = "SseStreamOpened";
    /// An SSE stream was closed.
    pub const SSE_STREAM_CLOSED: &str = "SseStreamClosed";
    /// An SSE event was sent to a client.
    pub const SSE_EVENT_SENT: &str = "SseEventSent";

    // ── Driver / Datasource ─────────────────────────────────────
    /// A driver was registered in the DriverFactory.
    pub const DRIVER_REGISTERED: &str = "DriverRegistered";
    /// A datasource connection was established.
    pub const DATASOURCE_CONNECTED: &str = "DatasourceConnected";
    /// A datasource connection was lost.
    pub const DATASOURCE_DISCONNECTED: &str = "DatasourceDisconnected";
    /// A datasource connection attempt failed.
    pub const DATASOURCE_CONNECTION_FAILED: &str = "DatasourceConnectionFailed";
    /// A previously-disconnected datasource reconnected.
    pub const DATASOURCE_RECONNECTED: &str = "DatasourceReconnected";
    /// A circuit breaker tripped open for a datasource.
    pub const DATASOURCE_CIRCUIT_OPENED: &str = "DatasourceCircuitOpened";
    /// A circuit breaker closed (recovered) for a datasource.
    pub const DATASOURCE_CIRCUIT_CLOSED: &str = "DatasourceCircuitClosed";
    /// A datasource health check failed.
    pub const DATASOURCE_HEALTH_CHECK_FAILED: &str = "DatasourceHealthCheckFailed";
    /// All connections in a pool are in use.
    pub const CONNECTION_POOL_EXHAUSTED: &str = "ConnectionPoolExhausted";

    // ── Broker ──────────────────────────────────────────────────
    /// A broker consumer started listening.
    pub const BROKER_CONSUMER_STARTED: &str = "BrokerConsumerStarted";
    /// A broker consumer stopped.
    pub const BROKER_CONSUMER_STOPPED: &str = "BrokerConsumerStopped";
    /// A message was received from a broker.
    pub const BROKER_MESSAGE_RECEIVED: &str = "BrokerMessageReceived";
    /// A message was published to a broker.
    pub const BROKER_MESSAGE_PUBLISHED: &str = "BrokerMessagePublished";
    /// A broker consumer encountered an error.
    pub const BROKER_CONSUMER_ERROR: &str = "BrokerConsumerError";
    /// Consumer lag was detected on a broker topic.
    pub const CONSUMER_LAG_DETECTED: &str = "ConsumerLagDetected";
    /// A Kafka partition rebalance occurred.
    pub const PARTITION_REBALANCED: &str = "PartitionRebalanced";
    /// A message processing attempt failed.
    pub const MESSAGE_FAILED: &str = "MessageFailed";

    // ── EventBus internal ───────────────────────────────────────
    /// An event was published to a topic.
    pub const EVENTBUS_TOPIC_PUBLISHED: &str = "EventBusTopicPublished";
    /// A handler subscribed to a topic.
    pub const EVENTBUS_TOPIC_SUBSCRIBED: &str = "EventBusTopicSubscribed";
    /// A handler unsubscribed from a topic.
    pub const EVENTBUS_TOPIC_UNSUBSCRIBED: &str = "EventBusTopicUnsubscribed";

    // ── Deployment / Config ─────────────────────────────────────
    /// A deployment status changed (deploying, deployed, failed).
    pub const DEPLOYMENT_STATUS_CHANGED: &str = "DeploymentStatusChanged";
    /// A config file was modified on disk.
    pub const CONFIG_FILE_CHANGED: &str = "ConfigFileChanged";

    // ── Cluster / Security ──────────────────────────────────────
    /// A cluster node's health status changed.
    pub const NODE_HEALTH_CHANGED: &str = "NodeHealthChanged";

    // ── Plugin ──────────────────────────────────────────────────
    /// A plugin failed to load.
    pub const PLUGIN_LOAD_FAILED: &str = "PluginLoadFailed";

    // ── Polling ─────────────────────────────────────────────────
    /// A poll tick handler failed.
    pub const POLL_TICK_FAILED: &str = "PollTickFailed";
    /// An onChange handler failed.
    pub const ON_CHANGE_FAILED: &str = "OnChangeFailed";
    /// A poll change detection timed out.
    pub const POLL_CHANGE_DETECT_TIMEOUT: &str = "PollChangeDetectTimeout";

    // ── Internal ────────────────────────────────────────────────
    /// An EventBus handler execution failed.
    pub const HANDLER_EXECUTION_FAILED: &str = "HandlerExecutionFailed";
}

// ── LogLevel mapping ────────────────────────────────────────────────

use crate::event::LogLevel;

/// Map an event type to its default log level.
/// Per `rivers-logging-spec.md` §4.
pub fn event_log_level(event_type: &str) -> LogLevel {
    match event_type {
        // Error
        events::DATASOURCE_HEALTH_CHECK_FAILED
        | events::BROKER_CONSUMER_ERROR
        | events::PLUGIN_LOAD_FAILED => LogLevel::Error,

        // Warn
        events::CONNECTION_POOL_EXHAUSTED
        | events::DATASOURCE_CIRCUIT_OPENED
        | events::DATASOURCE_DISCONNECTED
        | events::NODE_HEALTH_CHANGED => LogLevel::Warn,

        // Debug
        events::EVENTBUS_TOPIC_PUBLISHED => LogLevel::Debug,

        // Info — everything else
        _ => LogLevel::Info,
    }
}

// ── Priority ────────────────────────────────────────────────────────

/// Handler execution priority tier.
///
/// Dispatch order: Expect → Handle → Emit → Observe.
/// Expect, Handle, and Emit handlers are awaited sequentially.
/// Observe handlers are spawned fire-and-forget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandlerPriority {
    /// Awaited first — must complete before the request continues.
    Expect = 0,
    /// Awaited second — normal blocking handlers.
    Handle = 1,
    /// Awaited third — side-effect emission.
    Emit = 2,
    /// Fire-and-forget — spawned as background tasks.
    Observe = 3,
}

// ── EventHandler trait ──────────────────────────────────────────────

/// Trait for EventBus subscribers.
///
/// Implementations receive events and process them asynchronously.
/// Errors from Observe-tier handlers are logged but never propagated.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Handle an event. Return Ok(()) on success, Err on failure.
    async fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Human-readable name for logging/debugging.
    fn name(&self) -> &str;
}

// ── Subscription ────────────────────────────────────────────────────

/// Monotonically-increasing identifier for live subscriptions.
///
/// Used by [`SubscriptionHandle`] to locate and remove its entry in the
/// topic list when dropped.
type SubscriptionId = u64;

struct Subscription {
    id: SubscriptionId,
    handler: Arc<dyn EventHandler>,
    priority: HandlerPriority,
}

/// RAII handle for an EventBus subscription.
///
/// Per G_R2.1 (P2-2): subscriptions registered through [`EventBus::subscribe`]
/// are removed automatically when the returned handle is dropped, preventing
/// the leak of stale handlers when their owning component goes away.
///
/// For permanent subscriptions registered at startup (LogHandler, broker
/// bridges, datasource event handlers), use [`EventBus::subscribe_static`]
/// instead — it does not return a handle and the registration lasts for the
/// lifetime of the bus.
#[must_use = "dropping the SubscriptionHandle immediately unregisters the subscription"]
pub struct SubscriptionHandle {
    topic: String,
    id: SubscriptionId,
    bus: std::sync::Weak<EventBusInner>,
}

impl SubscriptionHandle {
    /// Detach the handle without removing the subscription.
    ///
    /// Equivalent to `subscribe_static` after the fact: the subscription
    /// lives for the rest of the bus's lifetime. Use this only when you
    /// are certain the subscription is permanent.
    pub fn forget(self) {
        std::mem::forget(self);
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        let Some(inner) = self.bus.upgrade() else {
            return; // Bus already dropped; nothing to remove.
        };
        let topic = std::mem::take(&mut self.topic);
        let id = self.id;
        // Spawn a task to remove the subscription. We are in Drop so we
        // cannot await; use try_write fast path then fall back to spawn.
        if let Ok(mut topics) = inner.topics.try_write() {
            remove_subscription_locked(&mut topics, &topic, id);
            #[cfg(feature = "metrics")]
            update_subscriber_metrics_locked(&topics);
            return;
        }
        // Fall back: spawn a task on the current runtime.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut topics = inner.topics.write().await;
                remove_subscription_locked(&mut topics, &topic, id);
                #[cfg(feature = "metrics")]
                update_subscriber_metrics_locked(&topics);
            });
        }
    }
}

fn remove_subscription_locked(
    topics: &mut HashMap<String, Vec<Subscription>>,
    topic: &str,
    id: SubscriptionId,
) {
    if let Some(subs) = topics.get_mut(topic) {
        subs.retain(|s| s.id != id);
        if subs.is_empty() {
            topics.remove(topic);
        }
    }
}

#[cfg(feature = "metrics")]
fn update_subscriber_metrics_locked(topics: &HashMap<String, Vec<Subscription>>) {
    let total: usize = topics.values().map(|v| v.len()).sum();
    metrics::gauge!("rivers_eventbus_subscribers", "kind" => "total").set(total as f64);
}

// ── EventBus ────────────────────────────────────────────────────────

/// Inner shared state for the EventBus, held behind an [`Arc`] so
/// [`SubscriptionHandle`] can keep a `Weak` reference for drop-time cleanup.
struct EventBusInner {
    topics: RwLock<HashMap<String, Vec<Subscription>>>,
    next_id: std::sync::atomic::AtomicU64,
    /// Per G_R2.2: cap on the number of receivers attached to a forwarder
    /// created by [`EventBus::subscribe_broadcast`]. Exceeding this triggers
    /// a warning at publish time so leaks surface in logs.
    max_broadcast_subscribers: usize,
}

/// In-process pub/sub EventBus with priority-tiered dispatch.
///
/// - Topics are created on first subscribe (no pre-registration needed).
/// - `publish()` dispatches to all subscribers of the event's `event_type`.
/// - Handlers are invoked in priority order:
///   1. All Expect handlers — awaited sequentially
///   2. All Handle handlers — awaited sequentially
///   3. All Emit handlers — awaited sequentially
///   4. All Observe handlers — spawned as tokio tasks (fire-and-forget)
///
/// ### Subscription lifecycle (G_R2)
///
/// - [`subscribe`](Self::subscribe) returns a [`SubscriptionHandle`] that
///   removes the subscription on drop. Callers MUST keep the handle alive
///   for as long as the subscription is wanted.
/// - [`subscribe_static`](Self::subscribe_static) is for permanent
///   subscriptions installed at startup (e.g. logging, broker bridges).
///   It does not return a handle and never removes the subscription.
pub struct EventBus {
    inner: Arc<EventBusInner>,
}

/// Default cap on broadcast forwarder receivers (per G_R2.2).
pub const DEFAULT_MAX_BROADCAST_SUBSCRIBERS: usize = 1000;

impl EventBus {
    /// Create a new empty EventBus.
    pub fn new() -> Self {
        Self::with_max_broadcast_subscribers(DEFAULT_MAX_BROADCAST_SUBSCRIBERS)
    }

    /// Create a new EventBus with a custom broadcast subscriber cap.
    pub fn with_max_broadcast_subscribers(max: usize) -> Self {
        Self {
            inner: Arc::new(EventBusInner {
                topics: RwLock::new(HashMap::new()),
                next_id: std::sync::atomic::AtomicU64::new(1),
                max_broadcast_subscribers: max,
            }),
        }
    }

    /// Subscribe a handler that lives for as long as the returned
    /// [`SubscriptionHandle`].
    ///
    /// Drop the handle to remove the subscription. Use
    /// [`subscribe_static`](Self::subscribe_static) for permanent
    /// startup-time wiring (it never unregisters).
    pub async fn subscribe(
        &self,
        topic: impl Into<String>,
        handler: Arc<dyn EventHandler>,
        priority: HandlerPriority,
    ) -> SubscriptionHandle {
        let topic_str = topic.into();
        let id = self.insert_subscription(&topic_str, handler, priority).await;
        SubscriptionHandle {
            topic: topic_str,
            id,
            bus: Arc::downgrade(&self.inner),
        }
    }

    /// Permanent subscription — does NOT return a handle, never unregisters.
    ///
    /// Per G_R2.1: use this for startup-time wiring (logging, broker
    /// bridges, datasource event subscribers) where the subscription should
    /// last for the lifetime of the EventBus.
    pub async fn subscribe_static(
        &self,
        topic: impl Into<String>,
        handler: Arc<dyn EventHandler>,
        priority: HandlerPriority,
    ) {
        let topic_str = topic.into();
        let _ = self.insert_subscription(&topic_str, handler, priority).await;
    }

    async fn insert_subscription(
        &self,
        topic: &str,
        handler: Arc<dyn EventHandler>,
        priority: HandlerPriority,
    ) -> SubscriptionId {
        let id = self
            .inner
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut topics = self.inner.topics.write().await;
        let subs = topics.entry(topic.to_string()).or_default();
        subs.push(Subscription {
            id,
            handler,
            priority,
        });
        // Keep sorted by priority so dispatch order is deterministic
        subs.sort_by_key(|s| s.priority);
        #[cfg(feature = "metrics")]
        update_subscriber_metrics_locked(&topics);
        id
    }

    /// Publish an event to all subscribers of its `event_type`.
    ///
    /// Dispatch order:
    /// 1. Expect handlers — awaited sequentially
    /// 2. Handle handlers — awaited sequentially
    /// 3. Emit handlers — awaited sequentially
    /// 4. Observe handlers — spawned fire-and-forget
    ///
    /// Errors from Expect/Handle/Emit handlers are collected and returned.
    /// Errors from Observe handlers are logged via tracing.
    pub async fn publish(&self, event: &Event) -> Vec<EventBusError> {
        #[cfg(feature = "metrics")]
        let _dispatch_start = std::time::Instant::now();

        // Collect subscribers under read lock, then drop lock before dispatching.
        // This prevents slow Observe handlers (file I/O) from blocking all publishes.
        let handlers: Vec<(Arc<dyn EventHandler>, HandlerPriority)> = {
            let topics = self.inner.topics.read().await;
            let mut collected: Vec<(Arc<dyn EventHandler>, HandlerPriority)> = Vec::new();

            // Exact topic subscribers
            if let Some(subs) = topics.get(&event.event_type) {
                for sub in subs {
                    collected.push((Arc::clone(&sub.handler), sub.priority));
                }
            }

            // Wildcard subscribers (receive all events)
            if event.event_type != "*" {
                if let Some(wildcard_subs) = topics.get("*") {
                    for sub in wildcard_subs {
                        collected.push((Arc::clone(&sub.handler), sub.priority));
                    }
                }
            }

            // E2: merge exact + wildcard into a single global priority order.
            //
            // Each individual list (exact, wildcard) is already sorted by priority
            // (maintained by `subscribe`), but a wildcard subscriber at e.g. Expect
            // must still dispatch before an exact subscriber at Emit. A stable sort
            // by priority gives the correct global ordering and preserves insertion
            // order within a priority bucket (and within that bucket, exact handlers
            // come before wildcards because they were collected first).
            collected.sort_by_key(|(_, p)| *p);
            collected
        }; // read lock dropped here

        let mut errors = Vec::new();

        for (handler, priority) in &handlers {
            match priority {
                HandlerPriority::Expect | HandlerPriority::Handle | HandlerPriority::Emit => {
                    if let Err(e) = handler.handle(event).await {
                        tracing::error!(
                            handler = handler.name(),
                            event_type = %event.event_type,
                            error = %e,
                            "EventBus handler failed"
                        );
                        errors.push(EventBusError {
                            handler_name: handler.name().to_string(),
                            event_type: event.event_type.clone(),
                            error: e.to_string(),
                        });
                    }
                }
                HandlerPriority::Observe => {
                    let handler = Arc::clone(handler);
                    let event_clone = event.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handler.handle(&event_clone).await {
                            tracing::warn!(
                                handler = handler.name(),
                                event_type = %event_clone.event_type,
                                error = %e,
                                "Observe-tier handler failed (non-fatal)"
                            );
                        }
                    });
                }
            }
        }

        #[cfg(feature = "metrics")]
        {
            let elapsed = _dispatch_start.elapsed().as_secs_f64();
            metrics::histogram!(
                "rivers_eventbus_dispatch_seconds",
                "event" => event.event_type.clone()
            )
            .record(elapsed);
        }

        errors
    }

    /// Create a broadcast channel bridged to an EventBus topic.
    ///
    /// Returns a `broadcast::Sender<Event>` — callers use `sender.subscribe()`
    /// to get a `Receiver` that yields events from this topic as a stream.
    /// Used by GraphQL subscriptions to bridge EventBus → async stream.
    ///
    /// G_R2.2: the forwarder is registered as a permanent (`subscribe_static`)
    /// subscription and warns when the receiver count exceeds
    /// `max_broadcast_subscribers` (default
    /// [`DEFAULT_MAX_BROADCAST_SUBSCRIBERS`]).
    pub async fn subscribe_broadcast(
        &self,
        topic: impl Into<String>,
        capacity: usize,
    ) -> tokio::sync::broadcast::Sender<crate::event::Event> {
        let topic_str = topic.into();
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        let forwarder = Arc::new(BroadcastForwarder {
            sender: sender.clone(),
            topic: topic_str.clone(),
            max_subscribers: self.inner.max_broadcast_subscribers,
        });
        // Broadcast forwarders are permanent — no handle returned.
        self.subscribe_static(topic_str, forwarder, HandlerPriority::Handle)
            .await;
        sender
    }

    /// Return the number of subscribers for a given topic.
    pub async fn subscriber_count(&self, topic: &str) -> usize {
        let topics = self.inner.topics.read().await;
        topics.get(topic).map_or(0, |s| s.len())
    }

    /// Return all registered topic names.
    pub async fn topics(&self) -> Vec<String> {
        let topics = self.inner.topics.read().await;
        topics.keys().cloned().collect()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// An error from an Expect, Handle, or Emit handler during publish.
#[derive(Debug)]
pub struct EventBusError {
    /// Name of the handler that failed.
    pub handler_name: String,
    /// Event type that triggered the failure.
    pub event_type: String,
    /// Error message.
    pub error: String,
}

// ── Broadcast Forwarder ─────────────────────────────────────────────

/// Internal handler that forwards EventBus events to a broadcast channel.
///
/// Created by `subscribe_broadcast()` for GraphQL subscription bridging.
struct BroadcastForwarder {
    sender: tokio::sync::broadcast::Sender<crate::event::Event>,
    topic: String,
    max_subscribers: usize,
}

#[async_trait]
impl EventHandler for BroadcastForwarder {
    async fn handle(&self, event: &crate::event::Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // G_R2.2: warn (and emit metric) when the receiver count exceeds the
        // configured cap. We do not drop receivers — that would surprise
        // existing GraphQL subscribers — but a leak should be visible.
        let receivers = self.sender.receiver_count();
        if receivers > self.max_subscribers {
            tracing::warn!(
                target: "rivers.eventbus",
                topic = %self.topic,
                receivers,
                max = self.max_subscribers,
                "broadcast forwarder exceeds max_broadcast_subscribers — possible subscription leak"
            );
            #[cfg(feature = "metrics")]
            metrics::gauge!(
                "rivers_eventbus_subscribers",
                "kind" => "broadcast_overflow"
            )
            .set(receivers as f64);
        }
        let _ = self.sender.send(event.clone());
        Ok(())
    }
    fn name(&self) -> &str {
        "BroadcastForwarder"
    }
}

// ── Cross-node gossip (V2.12 multi-node foundations) ────────────────

/// Configuration for cross-node event gossip.
///
/// Per technology-path-spec §12: V2 will support multiple named EventBus
/// pools. Cross-node gossip forwards events to peer nodes.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GossipConfig {
    /// Enable cross-node gossip.
    #[serde(default)]
    pub enabled: bool,

    /// Peer node addresses for gossip protocol.
    #[serde(default)]
    pub peers: Vec<String>,

    /// Gossip interval in milliseconds.
    #[serde(default = "default_gossip_interval")]
    pub interval_ms: u64,
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            peers: Vec::new(),
            interval_ms: default_gossip_interval(),
        }
    }
}

fn default_gossip_interval() -> u64 {
    1000
}

/// A gossip message carrying an event to a peer node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GossipMessage {
    /// The event type to forward.
    pub event_type: String,
    /// The event payload.
    pub payload: serde_json::Value,
    /// Optional trace ID from the original event.
    pub trace_id: Option<String>,
    /// Source node ID.
    pub source_node: String,
    /// Timestamp of original event.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl EventBus {
    /// Forward an event to peer nodes via gossip.
    ///
    /// V2 placeholder — logs the intent but does not send.
    /// Real implementation will use HTTP POST to peer /gossip/receive endpoints.
    pub async fn gossip_forward(
        &self,
        event: &crate::event::Event,
        peers: &[String],
        source_node: &str,
    ) {
        if peers.is_empty() {
            return;
        }
        let msg = GossipMessage {
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            trace_id: event.trace_id.clone(),
            source_node: source_node.to_string(),
            timestamp: event.timestamp,
        };
        tracing::debug!(
            target: "rivers.gossip",
            "would forward event '{}' to {} peers (gossip not yet wired)",
            msg.event_type,
            peers.len()
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gossip_forward_no_peers_noop() {
        let bus = EventBus::new();
        let event = crate::event::Event::new("test.event", serde_json::json!({}));
        bus.gossip_forward(&event, &[], "node-1").await;
        // Should not panic
    }

    #[tokio::test]
    async fn gossip_forward_with_peers_logs() {
        let bus = EventBus::new();
        let event =
            crate::event::Event::new("test.event", serde_json::json!({"key": "value"}));
        bus.gossip_forward(&event, &["http://peer1:8080".into()], "node-1")
            .await;
        // Should log but not actually send (placeholder)
    }

    #[test]
    fn gossip_message_serializes() {
        let msg = GossipMessage {
            event_type: "test".into(),
            payload: serde_json::json!({"x": 1}),
            trace_id: Some("t-1".into()),
            source_node: "node-1".into(),
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("test"));
    }

    #[test]
    fn gossip_message_deserializes() {
        let json = r#"{"event_type":"test","payload":{"x":1},"trace_id":"t-1","source_node":"node-1","timestamp":"2025-01-01T00:00:00Z"}"#;
        let msg: GossipMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.event_type, "test");
        assert_eq!(msg.source_node, "node-1");
    }

    #[test]
    fn gossip_config_defaults() {
        let config = GossipConfig::default();
        assert!(!config.enabled);
        assert!(config.peers.is_empty());
        assert_eq!(config.interval_ms, 1000); // matches default_gossip_interval()
    }

    #[test]
    fn gossip_config_deserializes_with_defaults() {
        let toml_str = r#"enabled = true"#;
        let config: GossipConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert!(config.peers.is_empty());
        assert_eq!(config.interval_ms, 1000); // from default_gossip_interval
    }

    // ── BA2: Broadcast subscription tests ────────────────────

    #[tokio::test]
    async fn subscribe_broadcast_receives_events() {
        let bus = EventBus::new();
        let sender = bus.subscribe_broadcast("test.topic", 16).await;
        let mut rx = sender.subscribe();

        let event = crate::event::Event::new("test.topic", serde_json::json!({"value": 42}));
        bus.publish(&event).await;

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, "test.topic");
        assert_eq!(received.payload["value"], 42);
    }

    #[tokio::test]
    async fn subscribe_broadcast_multiple_receivers() {
        let bus = EventBus::new();
        let sender = bus.subscribe_broadcast("multi", 16).await;
        let mut rx1 = sender.subscribe();
        let mut rx2 = sender.subscribe();

        let event = crate::event::Event::new("multi", serde_json::json!({"msg": "hello"}));
        bus.publish(&event).await;

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert_eq!(r1.payload["msg"], "hello");
        assert_eq!(r2.payload["msg"], "hello");
    }
}
