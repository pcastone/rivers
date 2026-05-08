use std::collections::HashMap;

use rivers_runtime::view::{ApiViewConfig, HandlerConfig, OnEventConfig};
use riversd::message_consumer::{
    validate_message_consumers, MessageConsumerConfig, MessageConsumerRegistry, MessageEventPayload,
};

// ── Helper ───────────────────────────────────────────────────────

fn consumer_view(topic: &str, handler: &str) -> ApiViewConfig {
    ApiViewConfig {
        view_type: "MessageConsumer".to_string(),
        path: None,
        method: None,
        handler: HandlerConfig::Codecomponent {
            language: "javascript".to_string(),
            module: "handlers/consumer.js".to_string(),
            entrypoint: handler.to_string(),
            resources: vec![],
        },
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        event_handlers: None,
        on_stream: None,
        ws_hooks: None,
        on_event: Some(OnEventConfig {
            topic: topic.to_string(),
            handler: handler.to_string(),
            handler_mode: None,
        }),
        polling: None,
        tools: HashMap::new(),
        resources: HashMap::new(),
        prompts: HashMap::new(),
        instructions: None,
        session: None,
        federation: vec![],
        response_headers: None,
    }
}

// ── MessageConsumerConfig ───────────────────────────────────────

#[test]
fn config_from_valid_consumer_view() {
    let view = consumer_view("orders.new", "onNewOrder");
    let config = MessageConsumerConfig::from_view("order_consumer", &view, "test-app");
    assert!(config.is_some());
    let config = config.unwrap();
    assert_eq!(config.view_id, "order_consumer");
    assert_eq!(config.entry_point, "test-app");
    assert_eq!(config.topic, "orders.new");
    assert_eq!(config.handler, "onNewOrder");
}

#[test]
fn config_returns_none_for_non_consumer() {
    let mut view = consumer_view("t", "h");
    view.view_type = "Rest".to_string();
    assert!(MessageConsumerConfig::from_view("v", &view, "test-app").is_none());
}

#[test]
fn config_returns_none_without_on_event() {
    let mut view = consumer_view("t", "h");
    view.on_event = None;
    assert!(MessageConsumerConfig::from_view("v", &view, "test-app").is_none());
}

// ── MessageConsumerRegistry ─────────────────────────────────────

#[test]
fn registry_from_mixed_views() {
    let mut views = HashMap::new();
    views.insert("consumer1".to_string(), consumer_view("topic.a", "handlerA"));
    views.insert("consumer2".to_string(), consumer_view("topic.b", "handlerB"));

    // Non-consumer view should be ignored
    let mut rest = consumer_view("x", "y");
    rest.view_type = "Rest".to_string();
    rest.path = Some("/api/test".to_string());
    views.insert("rest_view".to_string(), rest);

    let registry = MessageConsumerRegistry::from_views(&views, "test-app");
    assert_eq!(registry.len(), 2);
    assert!(!registry.is_empty());
    assert!(registry.get("consumer1").is_some());
    assert!(registry.get("consumer2").is_some());
    assert!(registry.get("rest_view").is_none());
    // Every MC config in the registry carries the app's entry point —
    // code-review §5 fix so the downstream `ctx.store` namespace is
    // the owning app, not `app:default`.
    assert_eq!(registry.get("consumer1").unwrap().entry_point, "test-app");
}

#[test]
fn registry_topics() {
    let mut views = HashMap::new();
    views.insert("c1".to_string(), consumer_view("orders.new", "h1"));
    views.insert("c2".to_string(), consumer_view("payments.done", "h2"));

    let registry = MessageConsumerRegistry::from_views(&views, "test-app");
    let topics = registry.topics();
    assert_eq!(topics.len(), 2);
    assert!(topics.contains(&"orders.new".to_string()));
    assert!(topics.contains(&"payments.done".to_string()));
}

#[test]
fn registry_empty() {
    let registry = MessageConsumerRegistry::from_views(&HashMap::new(), "test-app");
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

// ── Validation ──────────────────────────────────────────────────

#[test]
fn validate_valid_consumer() {
    let mut views = HashMap::new();
    views.insert("c".to_string(), consumer_view("topic", "handler"));
    let errors = validate_message_consumers(&views);
    assert!(errors.is_empty());
}

#[test]
fn validate_consumer_with_path() {
    let mut view = consumer_view("topic", "handler");
    view.path = Some("/should/not/exist".to_string());
    let mut views = HashMap::new();
    views.insert("bad".to_string(), view);
    let errors = validate_message_consumers(&views);
    assert!(errors.iter().any(|e| e.contains("must not declare a path")));
}

#[test]
fn validate_consumer_without_on_event() {
    let mut view = consumer_view("topic", "handler");
    view.on_event = None;
    let mut views = HashMap::new();
    views.insert("bad".to_string(), view);
    let errors = validate_message_consumers(&views);
    assert!(errors.iter().any(|e| e.contains("on_event configuration is required")));
}

#[test]
fn validate_consumer_with_on_stream() {
    let mut view = consumer_view("topic", "handler");
    view.on_stream = Some(rivers_runtime::view::OnStreamConfig {
        module: "m".to_string(),
        entrypoint: "e".to_string(),
        handler_mode: None,
    });
    let mut views = HashMap::new();
    views.insert("bad".to_string(), view);
    let errors = validate_message_consumers(&views);
    assert!(errors
        .iter()
        .any(|e| e.contains("on_stream is not valid")));
}

#[test]
fn validate_skips_non_consumer_views() {
    let mut view = consumer_view("t", "h");
    view.view_type = "Rest".to_string();
    view.path = Some("/api".to_string());
    let mut views = HashMap::new();
    views.insert("rest".to_string(), view);
    let errors = validate_message_consumers(&views);
    assert!(errors.is_empty());
}

// ── MessageEventPayload ─────────────────────────────────────────

#[test]
fn event_payload_serialization() {
    let payload = MessageEventPayload {
        data: serde_json::json!({"order_id": 42}),
        topic: "orders.new".to_string(),
        partition: Some("0".to_string()),
        offset: Some("1234".to_string()),
        trace_id: Some("trace-abc".to_string()),
        timestamp: Some("2026-01-01T00:00:00Z".to_string()),
    };

    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["topic"], "orders.new");
    assert_eq!(json["data"]["order_id"], 42);
    assert_eq!(json["partition"], "0");
}

#[test]
fn event_payload_roundtrip() {
    let payload = MessageEventPayload {
        data: serde_json::json!({"key": "value"}),
        topic: "test".to_string(),
        partition: None,
        offset: None,
        trace_id: None,
        timestamp: None,
    };

    let json = serde_json::to_string(&payload).unwrap();
    let deserialized: MessageEventPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.topic, "test");
    assert!(deserialized.partition.is_none());
}
