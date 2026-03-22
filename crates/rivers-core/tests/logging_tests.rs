//! Logging and trace ID tests.

use rivers_core::event::Event;
use rivers_core::logging::{
    extract_trace_id, parse_traceparent,
    synthesize_traceparent, LogFormat, LogHandler,
};
use rivers_core::LogLevel;

fn make_handler(level: LogLevel, format: LogFormat) -> LogHandler {
    LogHandler::from_config(
        &rivers_core::config::LoggingConfig {
            level,
            format: match format {
                LogFormat::Json => "json".into(),
                LogFormat::Text => "text".into(),
            },
            local_file_path: None,
        },
        "test-app".into(),
        "node-1".into(),
    )
}

fn test_event(event_type: &str) -> Event {
    Event::new(event_type, serde_json::json!({"method": "GET", "path": "/api/test"}))
}

// ── LogHandler filtering ────────────────────────────────────────────

#[test]
fn log_handler_filters_below_min_level() {
    let handler = make_handler(LogLevel::Warn, LogFormat::Json);
    // RequestCompleted is Info, which is below Warn
    let event = test_event("RequestCompleted");
    assert!(!handler.should_log(&event));
}

#[test]
fn log_handler_passes_at_min_level() {
    let handler = make_handler(LogLevel::Warn, LogFormat::Json);
    // DatasourceCircuitOpened is Warn
    let event = test_event("DatasourceCircuitOpened");
    assert!(handler.should_log(&event));
}

#[test]
fn log_handler_passes_above_min_level() {
    let handler = make_handler(LogLevel::Info, LogFormat::Json);
    // DatasourceHealthCheckFailed is Error
    let event = test_event("DatasourceHealthCheckFailed");
    assert!(handler.should_log(&event));
}

// ── JSON format ─────────────────────────────────────────────────────

#[test]
fn json_format_contains_mandatory_fields() {
    let handler = make_handler(LogLevel::Debug, LogFormat::Json);
    let event = test_event("RequestCompleted").with_trace_id("abc-123");
    let json = handler.format_json(&event);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(parsed["timestamp"].is_string());
    assert_eq!(parsed["level"], "info");
    assert!(parsed["message"].as_str().unwrap().contains("request"));
    assert_eq!(parsed["trace_id"], "abc-123");
    assert_eq!(parsed["app_id"], "test-app");
    assert_eq!(parsed["node_id"], "node-1");
    assert_eq!(parsed["event_type"], "RequestCompleted");
}

#[test]
fn json_format_merges_payload_fields() {
    let handler = make_handler(LogLevel::Debug, LogFormat::Json);
    let event = test_event("RequestCompleted");
    let json = handler.format_json(&event);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["method"], "GET");
    assert_eq!(parsed["path"], "/api/test");
}

// ── Text format ─────────────────────────────────────────────────────

#[test]
fn text_format_contains_key_info() {
    let handler = make_handler(LogLevel::Debug, LogFormat::Text);
    let event = test_event("RequestCompleted").with_trace_id("xyz");
    let text = handler.format_text(&event);

    assert!(text.contains("[INFO]"));
    assert!(text.contains("trace_id=xyz"));
    assert!(text.contains("event_type=RequestCompleted"));
}

// ── Trace ID extraction ─────────────────────────────────────────────

#[test]
fn extract_from_traceparent() {
    let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let id = extract_trace_id(Some(tp), None);
    assert_eq!(id, "0af7651916cd43dd8448eb211c80319c");
}

#[test]
fn extract_from_x_trace_id() {
    let id = extract_trace_id(None, Some("my-custom-trace-id"));
    assert_eq!(id, "my-custom-trace-id");
}

#[test]
fn traceparent_priority_over_x_trace_id() {
    let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let id = extract_trace_id(Some(tp), Some("should-be-ignored"));
    assert_eq!(id, "0af7651916cd43dd8448eb211c80319c");
}

#[test]
fn generates_uuid_when_no_headers() {
    let id = extract_trace_id(None, None);
    assert!(!id.is_empty());
    // Should be a valid UUID format (contains hyphens)
    assert!(id.contains('-'));
}

#[test]
fn invalid_traceparent_falls_through() {
    let id = extract_trace_id(Some("invalid-header"), Some("fallback-id"));
    assert_eq!(id, "fallback-id");
}

#[test]
fn parse_traceparent_valid() {
    let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    assert_eq!(
        parse_traceparent(tp),
        Some("0af7651916cd43dd8448eb211c80319c".to_string())
    );
}

#[test]
fn parse_traceparent_invalid() {
    assert_eq!(parse_traceparent("garbage"), None);
    assert_eq!(parse_traceparent("01-short-id-01"), None);
}

#[test]
fn synthesize_traceparent_from_uuid() {
    let tp = synthesize_traceparent("a1b2c3d4-e5f6-7890-abcd-ef1234567890");
    assert!(tp.starts_with("00-"));
    assert!(tp.ends_with("-0000000000000000-01"));
    assert!(tp.len() > 50);
}

// ── LogFormat ───────────────────────────────────────────────────────

#[test]
fn log_format_from_str() {
    assert_eq!(LogFormat::parse("json"), LogFormat::Json);
    assert_eq!(LogFormat::parse("JSON"), LogFormat::Json);
    assert_eq!(LogFormat::parse("text"), LogFormat::Text);
    assert_eq!(LogFormat::parse("Text"), LogFormat::Text);
    assert_eq!(LogFormat::parse("unknown"), LogFormat::Json); // default
}
