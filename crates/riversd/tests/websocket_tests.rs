use std::collections::HashMap;

use riversd::websocket::{
    BroadcastHub, ConnectionId, ConnectionInfo, ConnectionRegistry, WebSocketError,
    WebSocketMessage, WebSocketMode, WebSocketRouteManager, WsRateLimiter,
};

// ── WebSocketMode ───────────────────────────────────────────────

#[test]
fn mode_default_is_broadcast() {
    assert_eq!(WebSocketMode::from_str_opt(None), WebSocketMode::Broadcast);
}

#[test]
fn mode_broadcast_from_string() {
    assert_eq!(
        WebSocketMode::from_str_opt(Some("Broadcast")),
        WebSocketMode::Broadcast
    );
}

#[test]
fn mode_direct_from_string() {
    assert_eq!(
        WebSocketMode::from_str_opt(Some("Direct")),
        WebSocketMode::Direct
    );
}

#[test]
fn mode_case_insensitive() {
    assert_eq!(
        WebSocketMode::from_str_opt(Some("direct")),
        WebSocketMode::Direct
    );
}

// ── ConnectionId ────────────────────────────────────────────────

#[test]
fn connection_id_generates_unique() {
    let a = ConnectionId::new();
    let b = ConnectionId::new();
    assert_ne!(a, b);
}

// ── ConnectionRegistry ──────────────────────────────────────────

fn test_info(view_id: &str) -> ConnectionInfo {
    ConnectionInfo {
        id: ConnectionId::new(),
        view_id: view_id.to_string(),
        connected_at: chrono::Utc::now(),
        session_id: None,
        path_params: HashMap::new(),
    }
}

#[tokio::test]
async fn registry_register_and_count() {
    let registry = ConnectionRegistry::new(None);
    let info = test_info("test_view");

    let _rx = registry.register(info).await.unwrap();
    assert_eq!(registry.active_connections(), 1);
}

#[tokio::test]
async fn registry_unregister_decrements() {
    let registry = ConnectionRegistry::new(None);
    let info = test_info("test_view");
    let conn_id = info.id.0.clone();

    let _rx = registry.register(info).await.unwrap();
    assert_eq!(registry.active_connections(), 1);

    registry.unregister(&conn_id).await;
    assert_eq!(registry.active_connections(), 0);
}

#[tokio::test]
async fn registry_unregister_nonexistent_is_noop() {
    let registry = ConnectionRegistry::new(None);
    registry.unregister("nonexistent").await;
    assert_eq!(registry.active_connections(), 0);
}

#[tokio::test]
async fn registry_enforces_max_connections() {
    let registry = ConnectionRegistry::new(Some(1));

    let info1 = test_info("test");
    let _rx1 = registry.register(info1).await.unwrap();

    let info2 = test_info("test");
    let result = registry.register(info2).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        WebSocketError::ConnectionLimitExceeded(1)
    ));
}

#[tokio::test]
async fn registry_send_to_existing() {
    let registry = ConnectionRegistry::new(None);
    let info = test_info("test");
    let conn_id = info.id.0.clone();

    let mut rx = registry.register(info).await.unwrap();

    registry
        .send_to(&conn_id, WebSocketMessage::text("hello".into()))
        .await
        .unwrap();

    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.payload, "hello");
}

#[tokio::test]
async fn registry_send_to_nonexistent_fails() {
    let registry = ConnectionRegistry::new(None);
    let result = registry
        .send_to("ghost", WebSocketMessage::text("hi".into()))
        .await;
    assert!(matches!(
        result.unwrap_err(),
        WebSocketError::ConnectionNotFound(_)
    ));
}

#[tokio::test]
async fn registry_get_info() {
    let registry = ConnectionRegistry::new(None);
    let info = test_info("my_view");
    let conn_id = info.id.0.clone();

    let _rx = registry.register(info).await.unwrap();
    let retrieved = registry.get_info(&conn_id).await.unwrap();
    assert_eq!(retrieved.view_id, "my_view");
}

#[tokio::test]
async fn registry_all_connection_ids() {
    let registry = ConnectionRegistry::new(None);

    let info1 = test_info("v");
    let id1 = info1.id.0.clone();
    let _rx1 = registry.register(info1).await.unwrap();

    let info2 = test_info("v");
    let id2 = info2.id.0.clone();
    let _rx2 = registry.register(info2).await.unwrap();

    let ids = registry.all_connection_ids().await;
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));
}

// ── BroadcastHub ────────────────────────────────────────────────

#[test]
fn hub_subscribe_increments_count() {
    let hub = BroadcastHub::new(None);
    let _rx = hub.subscribe().unwrap();
    assert_eq!(hub.active_connections(), 1);
}

#[test]
fn hub_unsubscribe_decrements_count() {
    let hub = BroadcastHub::new(None);
    let _rx = hub.subscribe().unwrap();
    hub.unsubscribe();
    assert_eq!(hub.active_connections(), 0);
}

#[test]
fn hub_enforces_max_connections() {
    let hub = BroadcastHub::new(Some(1));
    let _rx = hub.subscribe().unwrap();
    let result = hub.subscribe();
    assert!(matches!(
        result.unwrap_err(),
        WebSocketError::ConnectionLimitExceeded(1)
    ));
}

#[test]
fn hub_broadcast_delivers_to_subscribers() {
    let hub = BroadcastHub::new(None);
    let mut rx1 = hub.subscribe().unwrap();
    let mut rx2 = hub.subscribe().unwrap();

    let count = hub
        .broadcast(WebSocketMessage::text("hello all".into()))
        .unwrap();
    assert_eq!(count, 2);

    let msg1 = rx1.try_recv().unwrap();
    let msg2 = rx2.try_recv().unwrap();
    assert_eq!(msg1.payload, "hello all");
    assert_eq!(msg2.payload, "hello all");
}

// ── WebSocketMessage ────────────────────────────────────────────

#[test]
fn message_text() {
    let msg = WebSocketMessage::text("payload".into());
    assert_eq!(msg.payload, "payload");
    assert!(msg.connection_id.is_none());
}

#[test]
fn message_directed() {
    let msg = WebSocketMessage::directed("payload".into(), "conn_1".into());
    assert_eq!(msg.payload, "payload");
    assert_eq!(msg.connection_id.unwrap(), "conn_1");
}

// ── WsRateLimiter ───────────────────────────────────────────────

#[test]
fn rate_limiter_none_when_no_limit() {
    assert!(WsRateLimiter::new(None, None).is_none());
}

#[test]
fn rate_limiter_none_when_zero() {
    assert!(WsRateLimiter::new(Some(0), None).is_none());
}

#[test]
fn rate_limiter_allows_within_burst() {
    let limiter = WsRateLimiter::new(Some(60), Some(5)).unwrap();
    // Should allow burst_size messages immediately
    for _ in 0..5 {
        assert!(limiter.check());
    }
}

#[test]
fn rate_limiter_blocks_over_burst() {
    let limiter = WsRateLimiter::new(Some(60), Some(2)).unwrap();
    assert!(limiter.check()); // token 1
    assert!(limiter.check()); // token 2
    assert!(!limiter.check()); // should be blocked
}

#[test]
fn rate_limiter_has_correct_rate() {
    let limiter = WsRateLimiter::new(Some(120), Some(10)).unwrap();
    assert!((limiter.messages_per_sec() - 2.0).abs() < f64::EPSILON);
    assert_eq!(limiter.burst(), 10);
}

// ── WebSocketRouteManager ───────────────────────────────────────

#[tokio::test]
async fn route_manager_register_broadcast() {
    let mgr = WebSocketRouteManager::new();
    let hub = mgr.register_broadcast("ws_view".into(), Some(100)).await;
    assert_eq!(hub.active_connections(), 0);

    let retrieved = mgr.get_broadcast("ws_view").await;
    assert!(retrieved.is_some());
}

#[tokio::test]
async fn route_manager_register_direct() {
    let mgr = WebSocketRouteManager::new();
    let registry = mgr.register_direct("ws_view".into(), Some(50)).await;
    assert_eq!(registry.active_connections(), 0);
    assert_eq!(registry.max_connections(), Some(50));

    let retrieved = mgr.get_direct("ws_view").await;
    assert!(retrieved.is_some());
}

#[tokio::test]
async fn route_manager_get_nonexistent_returns_none() {
    let mgr = WebSocketRouteManager::new();
    assert!(mgr.get_broadcast("nope").await.is_none());
    assert!(mgr.get_direct("nope").await.is_none());
}

// ── Binary Frame Tracker ─────────────────────────────────────────

#[test]
fn binary_tracker_first_frame_returns_true() {
    let tracker = riversd::websocket::BinaryFrameTracker::new();
    assert!(tracker.record_binary_frame(), "first binary frame should return true");
    assert!(!tracker.record_binary_frame(), "subsequent frames should return false");
}

#[test]
fn binary_tracker_counts_frames() {
    let tracker = riversd::websocket::BinaryFrameTracker::new();
    tracker.record_binary_frame();
    tracker.record_binary_frame();
    tracker.record_binary_frame();
    assert_eq!(tracker.total(), 3);
}

#[test]
fn binary_tracker_drain_resets_count() {
    let tracker = riversd::websocket::BinaryFrameTracker::new();
    tracker.record_binary_frame();
    tracker.record_binary_frame();
    let drained = tracker.drain_count();
    assert_eq!(drained, 2);
    assert_eq!(tracker.total(), 0);
}

// ── Session Expired Message ─────────────────────────────────────

#[test]
fn session_expired_message_format() {
    let msg = riversd::websocket::session_expired_message();
    let parsed: serde_json::Value = serde_json::from_str(&msg.payload).unwrap();
    assert_eq!(parsed["rivers_session_expired"], true);
}

// ── Integration: Multi-client broadcast ──────────────────────────

#[tokio::test]
async fn hub_broadcast_to_three_clients_all_receive() {
    let hub = BroadcastHub::new(None);
    let mut rx1 = hub.subscribe().unwrap();
    let mut rx2 = hub.subscribe().unwrap();
    let mut rx3 = hub.subscribe().unwrap();

    assert_eq!(hub.active_connections(), 3);

    let count = hub.broadcast(WebSocketMessage::text(r#"{"event":"ping"}"#.into())).unwrap();
    assert_eq!(count, 3);

    for rx in [&mut rx1, &mut rx2, &mut rx3] {
        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.payload, r#"{"event":"ping"}"#);
    }
}

#[tokio::test]
async fn hub_unsubscribe_cleanup_reduces_count() {
    let hub = BroadcastHub::new(Some(3));

    let _rx1 = hub.subscribe().unwrap();
    let _rx2 = hub.subscribe().unwrap();
    let _rx3 = hub.subscribe().unwrap();
    assert_eq!(hub.active_connections(), 3);

    // At capacity
    assert!(hub.subscribe().is_err());

    // Free 2 slots
    hub.unsubscribe();
    hub.unsubscribe();
    assert_eq!(hub.active_connections(), 1);

    // Can subscribe 2 more
    let _rx4 = hub.subscribe().unwrap();
    let _rx5 = hub.subscribe().unwrap();
    assert_eq!(hub.active_connections(), 3);
}

// ── Integration: Direct mode send_to targeted ────────────────────

#[tokio::test]
async fn registry_send_to_targets_specific_connection() {
    let registry = ConnectionRegistry::new(None);

    let info1 = test_info("chat");
    let _id1 = info1.id.0.clone();
    let mut rx1 = registry.register(info1).await.unwrap();

    let info2 = test_info("chat");
    let id2 = info2.id.0.clone();
    let mut rx2 = registry.register(info2).await.unwrap();

    // Send only to connection 2
    registry.send_to(&id2, WebSocketMessage::directed("secret".into(), id2.clone())).await.unwrap();

    // Connection 1 should NOT have received
    assert!(rx1.try_recv().is_err());

    // Connection 2 should have received
    let msg = rx2.try_recv().unwrap();
    assert_eq!(msg.payload, "secret");
    assert_eq!(msg.connection_id.unwrap(), id2);
}

#[tokio::test]
async fn registry_unregister_then_send_fails() {
    let registry = ConnectionRegistry::new(None);

    let info = test_info("chat");
    let id = info.id.0.clone();
    let _rx = registry.register(info).await.unwrap();

    registry.unregister(&id).await;

    let result = registry.send_to(&id, WebSocketMessage::text("late".into())).await;
    assert!(matches!(result.unwrap_err(), WebSocketError::ConnectionNotFound(_)));
}

// ── Integration: Route manager isolates modes ────────────────────

#[tokio::test]
async fn route_manager_broadcast_and_direct_independent() {
    let mgr = WebSocketRouteManager::new();

    let _hub = mgr.register_broadcast("ws_broadcast".into(), Some(100)).await;
    let _reg = mgr.register_direct("ws_direct".into(), Some(50)).await;

    // Broadcast view should not appear in direct and vice versa
    assert!(mgr.get_direct("ws_broadcast").await.is_none());
    assert!(mgr.get_broadcast("ws_direct").await.is_none());

    // Each found in its own registry
    assert!(mgr.get_broadcast("ws_broadcast").await.is_some());
    assert!(mgr.get_direct("ws_direct").await.is_some());
}

// ── Integration: Rate limiter refill ─────────────────────────────

#[test]
fn rate_limiter_refills_tokens_over_time() {
    let limiter = WsRateLimiter::new(Some(6000), Some(3)).unwrap(); // 100/sec

    // Exhaust burst
    assert!(limiter.check());
    assert!(limiter.check());
    assert!(limiter.check());
    assert!(!limiter.check());

    // Wait for refill (at 100/sec, 10ms = 1 token)
    std::thread::sleep(std::time::Duration::from_millis(15));
    assert!(limiter.check());
}

// ── Integration: Lag detector lifecycle ──────────────────────────

#[test]
fn lag_detector_full_lifecycle() {
    use riversd::websocket::{LagDetector, LagStatus};

    let detector = LagDetector::new(100);

    // Normal initially
    assert_eq!(detector.check_lag(), LagStatus::Normal);
    assert_eq!(detector.depth(), 0);

    // Send 51 messages → Warning
    for _ in 0..51 {
        detector.on_send();
    }
    assert_eq!(detector.check_lag(), LagStatus::Warning);

    // Ack 10 → still warning
    for _ in 0..10 {
        detector.on_ack();
    }
    assert_eq!(detector.depth(), 41);

    // Ack all → back to normal
    for _ in 0..41 {
        detector.on_ack();
    }
    assert_eq!(detector.check_lag(), LagStatus::Normal);
    assert_eq!(detector.depth(), 0);
}
