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

    bus.subscribe_static(
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

    bus.subscribe_static(
        events::DATAVIEW_EXECUTED,
        Arc::new(CountingHandler { name: "h1", count: c1.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe_static(
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
    bus.subscribe_static(
        "test.order",
        Arc::new(OrderRecordingHandler { name: "handle", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe_static(
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

    bus.subscribe_static(
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

    bus.subscribe_static(
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

    bus.subscribe_static(
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
    bus.subscribe_static(
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
    bus.subscribe_static(
        "test.emit",
        Arc::new(OrderRecordingHandler { name: "observe", order: order.clone() }),
        HandlerPriority::Observe,
    ).await;
    bus.subscribe_static(
        "test.emit",
        Arc::new(OrderRecordingHandler { name: "emit", order: order.clone() }),
        HandlerPriority::Emit,
    ).await;
    bus.subscribe_static(
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
    bus.subscribe_static(
        "test.global.priority",
        Arc::new(OrderRecordingHandler { name: "exact-emit", order: order.clone() }),
        HandlerPriority::Emit,
    ).await;
    // Wildcard subscriber at Expect (priority 0) — must run first
    bus.subscribe_static(
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

    bus.subscribe_static(
        "*",
        Arc::new(OrderRecordingHandler { name: "wildcard-observe", order: order.clone() }),
        HandlerPriority::Observe,
    ).await;
    bus.subscribe_static(
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
    bus.subscribe_static(
        "test.tiebreak",
        Arc::new(OrderRecordingHandler { name: "exact-1", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe_static(
        "*",
        Arc::new(OrderRecordingHandler { name: "wild-1", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe_static(
        "test.tiebreak",
        Arc::new(OrderRecordingHandler { name: "exact-2", order: order.clone() }),
        HandlerPriority::Handle,
    ).await;
    bus.subscribe_static(
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

// ── G_R2: SubscriptionHandle lifecycle ─────────────────────────────

#[tokio::test]
async fn dropping_subscription_handle_removes_subscription() {
    let bus = EventBus::new();
    let count = Arc::new(AtomicU32::new(0));

    {
        let handle = bus
            .subscribe(
                "test.handle.drop",
                Arc::new(CountingHandler { name: "h", count: count.clone() }),
                HandlerPriority::Handle,
            )
            .await;
        bus.publish(&test_event("test.handle.drop")).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(bus.subscriber_count("test.handle.drop").await, 1);
        drop(handle);
        // Drop schedules removal — give the runtime a tick.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(bus.subscriber_count("test.handle.drop").await, 0);
    bus.publish(&test_event("test.handle.drop")).await;
    assert_eq!(count.load(Ordering::SeqCst), 1, "handler must not fire after drop");
}

#[tokio::test]
async fn forget_keeps_subscription_alive() {
    let bus = EventBus::new();
    let count = Arc::new(AtomicU32::new(0));

    let handle = bus
        .subscribe(
            "test.forget",
            Arc::new(CountingHandler { name: "h", count: count.clone() }),
            HandlerPriority::Handle,
        )
        .await;
    handle.forget();

    bus.publish(&test_event("test.forget")).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(bus.subscriber_count("test.forget").await, 1);
}

#[tokio::test]
async fn subscribe_static_never_unregisters() {
    let bus = EventBus::new();
    let count = Arc::new(AtomicU32::new(0));

    bus.subscribe_static(
        "test.static",
        Arc::new(CountingHandler { name: "h", count: count.clone() }),
        HandlerPriority::Handle,
    )
    .await;

    assert_eq!(bus.subscriber_count("test.static").await, 1);
    bus.publish(&test_event("test.static")).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn broadcast_forwarder_warns_when_exceeding_max_subscribers() {
    let bus = EventBus::with_max_broadcast_subscribers(2);
    let sender = bus.subscribe_broadcast("test.broadcast.cap", 16).await;

    // Attach 3 receivers — exceeds the cap of 2.
    let _r1 = sender.subscribe();
    let _r2 = sender.subscribe();
    let _r3 = sender.subscribe();

    // Publish should not fail; warning is logged.
    let errors = bus.publish(&test_event("test.broadcast.cap")).await;
    assert!(errors.is_empty());
    assert_eq!(sender.receiver_count(), 3);
}

// ── H11/T2-1: Observe-tier dispatch concurrency cap ──────────────────

/// Slow Observe handler: bumps `running` while in flight, releases on
/// completion. Used to exercise the per-bus Observe semaphore.
struct SlowObserveHandler {
    running: Arc<AtomicU32>,
    peak: Arc<AtomicU32>,
    /// Channel that gates handler completion; tests drop the sender to
    /// release all in-flight handlers at once.
    gate: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl EventHandler for SlowObserveHandler {
    async fn handle(&self, _event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        // Update high-water mark.
        let mut current_peak = self.peak.load(Ordering::SeqCst);
        while now > current_peak {
            match self.peak.compare_exchange(
                current_peak,
                now,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => current_peak = observed,
            }
        }
        // Park until released.
        self.gate.notified().await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
    fn name(&self) -> &str {
        "slow-observe"
    }
}

#[tokio::test]
async fn observe_dispatch_is_capped_by_semaphore() {
    // Cap of 4: regardless of how many events we publish, at most 4
    // SlowObserveHandler tasks should be in flight at once.
    let bus = EventBus::with_caps(rivers_core::eventbus::DEFAULT_MAX_BROADCAST_SUBSCRIBERS, 4);
    let running = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let gate = Arc::new(tokio::sync::Notify::new());

    bus.subscribe_static(
        "test.observe.cap",
        Arc::new(SlowObserveHandler {
            running: running.clone(),
            peak: peak.clone(),
            gate: gate.clone(),
        }),
        HandlerPriority::Observe,
    )
    .await;

    // Publish 200 events. With cap=4, only 4 spawns can fit; the rest
    // must be dropped (NEVER block the dispatch loop).
    for _ in 0..200 {
        bus.publish(&test_event("test.observe.cap")).await;
    }

    // Give spawned tasks a moment to start and hit the gate.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let in_flight = running.load(Ordering::SeqCst);
    let high_water = peak.load(Ordering::SeqCst);
    let dropped = bus.observe_dropped();

    assert!(
        in_flight <= 4,
        "in_flight={in_flight} exceeded cap of 4"
    );
    assert!(
        high_water <= 4,
        "high-water={high_water} exceeded cap of 4"
    );
    assert!(
        dropped > 0,
        "expected some dropped Observe dispatches, got 0 (in_flight={in_flight})"
    );
    // 200 publishes - up to 4 spawned = at least 196 dropped.
    assert!(
        dropped >= 196,
        "expected >=196 dropped, got {dropped} (in_flight={in_flight})"
    );

    // Release all handlers so the test exits cleanly.
    gate.notify_waiters();
}

#[tokio::test]
async fn observe_dispatch_does_not_block_publish_when_saturated() {
    // Even with the semaphore fully saturated, publish() must return
    // promptly — the contract is "drop, never block."
    let bus = EventBus::with_caps(rivers_core::eventbus::DEFAULT_MAX_BROADCAST_SUBSCRIBERS, 1);
    let running = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let gate = Arc::new(tokio::sync::Notify::new());

    bus.subscribe_static(
        "test.observe.nonblock",
        Arc::new(SlowObserveHandler {
            running: running.clone(),
            peak: peak.clone(),
            gate: gate.clone(),
        }),
        HandlerPriority::Observe,
    )
    .await;

    // Saturate the semaphore.
    bus.publish(&test_event("test.observe.nonblock")).await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // 50 more publishes while semaphore is full — must complete in well
    // under a second total. (If it blocked we'd be stuck on the gate.)
    let start = std::time::Instant::now();
    for _ in 0..50 {
        bus.publish(&test_event("test.observe.nonblock")).await;
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "publish loop took {elapsed:?} — Observe dispatch appears to be blocking"
    );
    assert!(bus.observe_dropped() >= 50);

    gate.notify_waiters();
}
