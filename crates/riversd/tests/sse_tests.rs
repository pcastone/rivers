use std::sync::Arc;
use riversd::sse::{SseChannel, SseError, SseEvent, SseRouteManager};

// ── SseEvent ────────────────────────────────────────────────────

#[test]
fn event_data_wire_format() {
    let event = SseEvent::data(r#"{"count":42}"#.to_string());
    let wire = event.to_wire_format();
    assert_eq!(wire, "data: {\"count\":42}\n\n");
}

#[test]
fn event_typed_wire_format() {
    let event = SseEvent::typed("update".to_string(), r#"{"v":1}"#.to_string());
    let wire = event.to_wire_format();
    assert!(wire.contains("event: update\n"));
    assert!(wire.contains("data: {\"v\":1}\n"));
    assert!(wire.ends_with("\n\n"));
}

#[test]
fn event_with_id() {
    let mut event = SseEvent::data("hello".to_string());
    event.id = Some("42".to_string());
    let wire = event.to_wire_format();
    assert!(wire.contains("id: 42\n"));
    assert!(wire.contains("data: hello\n"));
}

#[test]
fn event_multiline_data() {
    let event = SseEvent::data("line1\nline2\nline3".to_string());
    let wire = event.to_wire_format();
    assert!(wire.contains("data: line1\n"));
    assert!(wire.contains("data: line2\n"));
    assert!(wire.contains("data: line3\n"));
}

// ── Session Expired Event ───────────────────────────────────────

#[test]
fn session_expired_event_format() {
    let event = riversd::sse::session_expired_event();
    let parsed: serde_json::Value = serde_json::from_str(&event.data).unwrap();
    assert_eq!(parsed["rivers_session_expired"], true);
}

// ── SseChannel ──────────────────────────────────────────────────

#[test]
fn channel_subscribe_increments_count() {
    let channel = SseChannel::new(None, 1000, vec![]);
    let _rx = channel.subscribe().unwrap();
    assert_eq!(channel.active_connections(), 1);
}

#[test]
fn channel_unsubscribe_decrements_count() {
    let channel = SseChannel::new(None, 1000, vec![]);
    let _rx = channel.subscribe().unwrap();
    channel.unsubscribe();
    assert_eq!(channel.active_connections(), 0);
}

#[test]
fn channel_enforces_max_connections() {
    let channel = SseChannel::new(Some(1), 1000, vec![]);
    let _rx = channel.subscribe().unwrap();
    let result = channel.subscribe();
    assert!(matches!(
        result.unwrap_err(),
        SseError::ConnectionLimitExceeded(1)
    ));
}

#[test]
fn channel_push_delivers_to_subscribers() {
    let channel = SseChannel::new(None, 1000, vec![]);
    let mut rx = channel.subscribe().unwrap();

    channel.push(SseEvent::data("test".into())).unwrap();
    let event = rx.try_recv().unwrap();
    assert_eq!(event.data, "test");
}

#[test]
fn channel_push_multiple_subscribers() {
    let channel = SseChannel::new(None, 0, vec![]);
    let mut rx1 = channel.subscribe().unwrap();
    let mut rx2 = channel.subscribe().unwrap();

    let count = channel
        .push(SseEvent::typed("ping".into(), "{}".into()))
        .unwrap();
    assert_eq!(count, 2);

    let e1 = rx1.try_recv().unwrap();
    let e2 = rx2.try_recv().unwrap();
    assert_eq!(e1.event.as_deref(), Some("ping"));
    assert_eq!(e2.event.as_deref(), Some("ping"));
}

#[test]
fn channel_properties() {
    let channel = SseChannel::new(Some(50), 5000, vec!["order.created".into()]);
    assert_eq!(channel.tick_interval_ms(), 5000);
    assert_eq!(channel.trigger_events(), &["order.created"]);
    assert_eq!(channel.max_connections(), Some(50));
}

// ── SseRouteManager ─────────────────────────────────────────────

#[tokio::test]
async fn route_manager_register_and_get() {
    let mgr = SseRouteManager::new();
    let channel = mgr
        .register("sse_view".into(), Some(100), 2000, vec!["tick".into()])
        .await;
    assert_eq!(channel.tick_interval_ms(), 2000);

    let retrieved = mgr.get("sse_view").await;
    assert!(retrieved.is_some());
}

#[tokio::test]
async fn route_manager_get_nonexistent() {
    let mgr = SseRouteManager::new();
    assert!(mgr.get("nope").await.is_none());
}

// ── Integration: Relay lifecycle ─────────────────────────────────

#[tokio::test]
async fn channel_relay_lifecycle_subscribe_push_receive_unsubscribe() {
    let channel = Arc::new(SseChannel::new(Some(10), 0, vec![]));

    // Subscribe 3 clients
    let mut rx1 = channel.subscribe().unwrap();
    let mut rx2 = channel.subscribe().unwrap();
    let mut rx3 = channel.subscribe().unwrap();
    assert_eq!(channel.active_connections(), 3);

    // Push event → all 3 receive
    channel.push(SseEvent::typed("update".into(), r#"{"v":1}"#.into())).unwrap();
    assert_eq!(rx1.try_recv().unwrap().data, r#"{"v":1}"#);
    assert_eq!(rx2.try_recv().unwrap().data, r#"{"v":1}"#);
    assert_eq!(rx3.try_recv().unwrap().data, r#"{"v":1}"#);

    // Unsubscribe one → count drops
    channel.unsubscribe();
    assert_eq!(channel.active_connections(), 2);

    // Push again → remaining 2 still receive
    channel.push(SseEvent::data("second".into())).unwrap();
    assert!(rx2.try_recv().is_ok());
    assert!(rx3.try_recv().is_ok());

    // Unsubscribe all
    channel.unsubscribe();
    channel.unsubscribe();
    assert_eq!(channel.active_connections(), 0);
}

#[tokio::test]
async fn channel_max_connections_frees_after_unsubscribe() {
    let channel = SseChannel::new(Some(2), 0, vec![]);

    let _rx1 = channel.subscribe().unwrap();
    let _rx2 = channel.subscribe().unwrap();

    // At capacity
    assert!(channel.subscribe().is_err());

    // Free one slot
    channel.unsubscribe();
    assert_eq!(channel.active_connections(), 1);

    // Can subscribe again
    let _rx3 = channel.subscribe().unwrap();
    assert_eq!(channel.active_connections(), 2);
}

#[tokio::test]
async fn channel_push_no_subscribers_returns_error() {
    let channel = SseChannel::new(None, 0, vec![]);
    let result = channel.push(SseEvent::data("orphan".into()));
    assert!(matches!(result.unwrap_err(), SseError::NoActiveClients));
}

// ── Integration: Trigger event flow via EventBus ────────────────

#[tokio::test]
async fn trigger_event_flow_eventbus_to_sse_channel() {
    use rivers_runtime::rivers_core::{EventBus, EventHandler, HandlerPriority};
    use rivers_runtime::rivers_core::event::Event;
    use async_trait::async_trait;

    // Create EventBus + SseChannel
    let event_bus = Arc::new(EventBus::new());
    let channel = Arc::new(SseChannel::new(None, 0, vec!["OrderCreated".into()]));

    // SseTriggerHandler equivalent — pushes EventBus events to SseChannel
    struct TestTriggerHandler {
        channel: Arc<SseChannel>,
    }

    #[async_trait]
    impl EventHandler for TestTriggerHandler {
        async fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let sse_event = SseEvent::typed(
                event.event_type.clone(),
                serde_json::to_string(&event.payload).unwrap_or_default(),
            );
            let _ = self.channel.push(sse_event);
            Ok(())
        }
        fn name(&self) -> &str { "TestTriggerHandler" }
    }

    // Subscribe handler to EventBus
    let handler = Arc::new(TestTriggerHandler { channel: channel.clone() });
    let _sub = event_bus.subscribe("OrderCreated", handler, HandlerPriority::Handle).await;

    // Subscribe a client to the SSE channel
    let mut rx = channel.subscribe().unwrap();

    // Publish event on EventBus
    let event = Event::new("OrderCreated", serde_json::json!({"order_id": 42}));
    event_bus.publish(&event).await;

    // Client should receive the SSE event
    let sse_event = rx.try_recv().unwrap();
    assert_eq!(sse_event.event.as_deref(), Some("OrderCreated"));
    assert!(sse_event.data.contains("42"));
}

// ── Integration: Heartbeat push loop ────────────────────────────

#[tokio::test]
async fn drive_sse_push_loop_emits_heartbeats_to_channel() {
    use riversd::sse::drive_sse_push_loop;

    let channel = Arc::new(SseChannel::new(None, 50, vec![]));
    let mut rx = channel.subscribe().unwrap();

    let ch = channel.clone();
    let handle = tokio::spawn(async move {
        drive_sse_push_loop(ch, 50, "heartbeat_test".into(), None, None, None).await;
    });

    // Collect at least 2 heartbeats
    let e1 = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    let e2 = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;

    assert!(e1.is_ok());
    assert!(e2.is_ok());
    let event = e1.unwrap().unwrap();
    assert_eq!(event.event.as_deref(), Some("heartbeat"));
    assert!(event.data.contains("heartbeat_test"));

    handle.abort();
}

// ── Integration: Route manager multiple views ──────────────────

#[tokio::test]
async fn route_manager_isolates_channels_per_view() {
    let mgr = SseRouteManager::new();

    let ch_a = mgr.register("view_a".into(), None, 1000, vec![]).await;
    let ch_b = mgr.register("view_b".into(), None, 2000, vec![]).await;

    let mut rx_a = ch_a.subscribe().unwrap();
    let _rx_b = ch_b.subscribe().unwrap();

    // Push to channel A only
    ch_a.push(SseEvent::data("only-a".into())).unwrap();

    assert_eq!(rx_a.try_recv().unwrap().data, "only-a");

    // Channels are independent
    assert_eq!(ch_a.tick_interval_ms(), 1000);
    assert_eq!(ch_b.tick_interval_ms(), 2000);
}
