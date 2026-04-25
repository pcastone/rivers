//! EventBus integration tests.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rivers_core::event::Event;
use rivers_core::eventbus::{events, event_log_level, EventBus, HandlerPriority};
use rivers_core::{EventHandler, LogLevel};

/// Test handler that counts invocations.
struct CountingHandler {
    name: &'static str,
    count: Arc<AtomicU32>,
}

#[async_trait]
impl EventHandler for CountingHandler {
    async fn handle(&self, _event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn name(&self) -> &str {
        self.name
    }
}

/// Test handler that always fails.
struct FailingHandler;

#[async_trait]
impl EventHandler for FailingHandler {
    async fn handle(&self, _event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("intentional failure".into())
    }
    fn name(&self) -> &str {
        "failing-handler"
    }
}

/// Test handler that records order of execution.
struct OrderRecordingHandler {
    name: &'static str,
    order: Arc<tokio::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl EventHandler for OrderRecordingHandler {
    async fn handle(&self, _event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.order.lock().await.push(self.name.to_string());
        Ok(())
    }
    fn name(&self) -> &str {
        self.name
    }
}

fn test_event(event_type: &str) -> Event {
    Event::new(event_type, serde_json::json!({}))
}

#[tokio::test]
async fn publish_to_subscribed_handler() {
    let bus = EventBus::new();
    let count = Arc::new(AtomicU32::new(0));

    bus.subscribe(
        events::REQUEST_COMPLETED,
        Arc::new(CountingHandler { name: "counter", count: count.clone() }),
        HandlerPriority::Handle,
    ).await;

    bus.publish(&test_event(events::REQUEST_COMPLETED)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);

    bus.publish(&test_event(events::REQUEST_COMPLETED)).await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn publish_to_unknown_topic_is_noop() {
    let bus = EventBus::new();
    let errors = bus.publish(&test_event("NonexistentEvent")).await;
    assert!(errors.is_empty());
}

#[tokio::test]
async fn multiple_subscribers_all_invoked() {
    let bus = EventBus::new();
    let c1 = Arc::new(AtomicU32::new(0));
    let c2 = Arc::new(AtomicU32::new(0));

    bus.subscribe(
        events::DATAVIEW_EXECUTED,
        Arc::new(CountingHandler { name: "h1", count: c1.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe(
        events::DATAVIEW_EXECUTED,
        Arc::new(CountingHandler { name: "h2", count: c2.clone() }),
        HandlerPriority::Handle,
    ).await;

    bus.publish(&test_event(events::DATAVIEW_EXECUTED)).await;
    assert_eq!(c1.load(Ordering::SeqCst), 1);
    assert_eq!(c2.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn priority_ordering_expect_before_handle() {
    let bus = EventBus::new();
    let order = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Subscribe Handle first, then Expect — Expect should still run first
    bus.subscribe(
        "test.order",
        Arc::new(OrderRecordingHandler { name: "handle", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe(
        "test.order",
        Arc::new(OrderRecordingHandler { name: "expect", order: order.clone() }),
        HandlerPriority::Expect,
    ).await;

    bus.publish(&test_event("test.order")).await;
    let result = order.lock().await;
    assert_eq!(result[0], "expect");
    assert_eq!(result[1], "handle");
}

#[tokio::test]
async fn observe_handlers_fire_and_forget() {
    let bus = EventBus::new();
    let count = Arc::new(AtomicU32::new(0));

    bus.subscribe(
        "test.observe",
        Arc::new(CountingHandler { name: "observer", count: count.clone() }),
        HandlerPriority::Observe,
    ).await;

    bus.publish(&test_event("test.observe")).await;

    // Give the spawned task time to run
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failing_expect_handler_returns_error() {
    let bus = EventBus::new();

    bus.subscribe(
        "test.fail",
        Arc::new(FailingHandler),
        HandlerPriority::Expect,
    ).await;

    let errors = bus.publish(&test_event("test.fail")).await;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].handler_name, "failing-handler");
    assert!(errors[0].error.contains("intentional failure"));
}

#[tokio::test]
async fn failing_observe_handler_does_not_return_error() {
    let bus = EventBus::new();

    bus.subscribe(
        "test.observe.fail",
        Arc::new(FailingHandler),
        HandlerPriority::Observe,
    ).await;

    let errors = bus.publish(&test_event("test.observe.fail")).await;
    // Observe errors are logged, not returned
    assert!(errors.is_empty());
}

#[tokio::test]
async fn subscriber_count() {
    let bus = EventBus::new();
    assert_eq!(bus.subscriber_count("test.count").await, 0);

    let count = Arc::new(AtomicU32::new(0));
    bus.subscribe(
        "test.count",
        Arc::new(CountingHandler { name: "h", count }),
        HandlerPriority::Handle,
    ).await;
    assert_eq!(bus.subscriber_count("test.count").await, 1);
}

#[tokio::test]
async fn emit_tier_awaited_between_handle_and_observe() {
    let bus = EventBus::new();
    let order = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Subscribe in reverse priority order to confirm sorting works
    bus.subscribe(
        "test.emit",
        Arc::new(OrderRecordingHandler { name: "observe", order: order.clone() }),
        HandlerPriority::Observe,
    ).await;
    bus.subscribe(
        "test.emit",
        Arc::new(OrderRecordingHandler { name: "emit", order: order.clone() }),
        HandlerPriority::Emit,
    ).await;
    bus.subscribe(
        "test.emit",
        Arc::new(OrderRecordingHandler { name: "handle", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;

    bus.publish(&test_event("test.emit")).await;

    // Give Observe (fire-and-forget) time to complete
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let result = order.lock().await;
    // Handle (1) and Emit (2) are awaited sequentially before Observe (3) fires
    assert_eq!(result.len(), 3, "all three handlers should have run");
    assert_eq!(result[0], "handle", "Handle should run first");
    assert_eq!(result[1], "emit", "Emit should run second");
    assert_eq!(result[2], "observe", "Observe should run last");
}

// ── E2: Wildcard + exact subscriber global priority ordering ───────

/// Regression: a wildcard `Expect` subscriber must run BEFORE an exact
/// `Emit` subscriber for the same event.
///
/// Before E2 the dispatch loop concatenated the (already-sorted) exact
/// list with the (already-sorted) wildcard list, so wildcards always
/// dispatched after exact handlers regardless of their priority.
#[tokio::test]
async fn wildcard_expect_runs_before_exact_emit() {
    let bus = EventBus::new();
    let order = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Exact subscriber at Emit (priority 2)
    bus.subscribe(
        "test.global.priority",
        Arc::new(OrderRecordingHandler { name: "exact-emit", order: order.clone() }),
        HandlerPriority::Emit,
    ).await;
    // Wildcard subscriber at Expect (priority 0) — must run first
    bus.subscribe(
        "*",
        Arc::new(OrderRecordingHandler { name: "wildcard-expect", order: order.clone() }),
        HandlerPriority::Expect,
    ).await;

    bus.publish(&test_event("test.global.priority")).await;

    let result = order.lock().await;
    assert_eq!(result.len(), 2, "both handlers should have run");
    assert_eq!(result[0], "wildcard-expect", "wildcard Expect must run before exact Emit");
    assert_eq!(result[1], "exact-emit");
}

/// A wildcard `Observe` subscriber runs AFTER an exact `Handle` subscriber.
#[tokio::test]
async fn wildcard_observe_runs_after_exact_handle() {
    let bus = EventBus::new();
    let order = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    bus.subscribe(
        "*",
        Arc::new(OrderRecordingHandler { name: "wildcard-observe", order: order.clone() }),
        HandlerPriority::Observe,
    ).await;
    bus.subscribe(
        "test.observe.after.handle",
        Arc::new(OrderRecordingHandler { name: "exact-handle", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;

    bus.publish(&test_event("test.observe.after.handle")).await;

    // Allow the spawned Observe handler to complete.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let result = order.lock().await;
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], "exact-handle", "exact Handle must run before wildcard Observe");
    assert_eq!(result[1], "wildcard-observe");
}

/// Same priority, mix of exact + wildcard: tie-break is **insertion order**.
/// Within a single priority bucket the exact subscribers come first
/// (because exact is collected before wildcard at dispatch time), then
/// wildcards. Inside each list, insertion order is preserved by the
/// stable sort.
#[tokio::test]
async fn same_priority_exact_before_wildcard_then_insertion_order() {
    let bus = EventBus::new();
    let order = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Insert in interleaved order to prove insertion order is preserved
    // within each (exact|wildcard) group at the same priority.
    bus.subscribe(
        "test.tiebreak",
        Arc::new(OrderRecordingHandler { name: "exact-1", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe(
        "*",
        Arc::new(OrderRecordingHandler { name: "wild-1", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe(
        "test.tiebreak",
        Arc::new(OrderRecordingHandler { name: "exact-2", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe(
        "*",
        Arc::new(OrderRecordingHandler { name: "wild-2", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;

    bus.publish(&test_event("test.tiebreak")).await;

    let result = order.lock().await;
    assert_eq!(result.len(), 4);
    // Exact handlers first (in insertion order), then wildcards (in insertion order)
    assert_eq!(result[0], "exact-1");
    assert_eq!(result[1], "exact-2");
    assert_eq!(result[2], "wild-1");
    assert_eq!(result[3], "wild-2");
}

#[test]
fn event_log_level_mapping() {
    assert_eq!(event_log_level(events::DATASOURCE_HEALTH_CHECK_FAILED), LogLevel::Error);
    assert_eq!(event_log_level(events::BROKER_CONSUMER_ERROR), LogLevel::Error);
    assert_eq!(event_log_level(events::PLUGIN_LOAD_FAILED), LogLevel::Error);

    assert_eq!(event_log_level(events::CONNECTION_POOL_EXHAUSTED), LogLevel::Warn);
    assert_eq!(event_log_level(events::DATASOURCE_CIRCUIT_OPENED), LogLevel::Warn);
    assert_eq!(event_log_level(events::NODE_HEALTH_CHANGED), LogLevel::Warn);

    assert_eq!(event_log_level(events::EVENTBUS_TOPIC_PUBLISHED), LogLevel::Debug);

    assert_eq!(event_log_level(events::REQUEST_COMPLETED), LogLevel::Info);
    assert_eq!(event_log_level(events::DATAVIEW_EXECUTED), LogLevel::Info);
    assert_eq!(event_log_level(events::WEBSOCKET_CONNECTED), LogLevel::Info);
}
