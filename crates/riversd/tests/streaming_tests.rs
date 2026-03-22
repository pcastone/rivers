use riversd::streaming::{
    check_stream_terminated, poison_chunk_ndjson, poison_chunk_sse, validate_streaming,
    StreamChunk, StreamingConfig, StreamingFormat,
};

// ── StreamingFormat ─────────────────────────────────────────────

#[test]
fn format_ndjson_content_type() {
    assert_eq!(StreamingFormat::Ndjson.content_type(), "application/x-ndjson");
}

#[test]
fn format_sse_content_type() {
    assert_eq!(StreamingFormat::Sse.content_type(), "text/event-stream");
}

#[test]
fn format_from_str() {
    assert_eq!(StreamingFormat::from_str_opt(Some("ndjson")), Some(StreamingFormat::Ndjson));
    assert_eq!(StreamingFormat::from_str_opt(Some("NDJSON")), Some(StreamingFormat::Ndjson));
    assert_eq!(StreamingFormat::from_str_opt(Some("sse")), Some(StreamingFormat::Sse));
    assert_eq!(StreamingFormat::from_str_opt(Some("SSE")), Some(StreamingFormat::Sse));
    assert_eq!(StreamingFormat::from_str_opt(None), None);
    assert_eq!(StreamingFormat::from_str_opt(Some("xml")), None);
}

// ── StreamChunk ─────────────────────────────────────────────────

#[test]
fn chunk_to_ndjson() {
    let chunk = StreamChunk::new(serde_json::json!({"id": 1}));
    let ndjson = chunk.to_ndjson();
    assert!(ndjson.ends_with('\n'));
    let parsed: serde_json::Value = serde_json::from_str(ndjson.trim()).unwrap();
    assert_eq!(parsed["id"], 1);
}

#[test]
fn chunk_to_sse_without_event() {
    let chunk = StreamChunk::new(serde_json::json!({"v": 42}));
    let sse = chunk.to_sse(None);
    assert!(sse.starts_with("data: "));
    assert!(sse.ends_with("\n\n"));
    assert!(!sse.contains("event:"));
}

#[test]
fn chunk_to_sse_with_event() {
    let chunk = StreamChunk::new(serde_json::json!({"v": 42}));
    let sse = chunk.to_sse(Some("update"));
    assert!(sse.contains("event: update\n"));
    assert!(sse.contains("data: "));
}

// ── Poison Chunks ───────────────────────────────────────────────

#[test]
fn poison_ndjson() {
    let poison = poison_chunk_ndjson("stream failed");
    let parsed: serde_json::Value = serde_json::from_str(poison.trim()).unwrap();
    assert_eq!(parsed["stream_terminated"], true);
    assert_eq!(parsed["error"], "stream failed");
}

#[test]
fn poison_sse() {
    let poison = poison_chunk_sse("stream failed");
    assert!(poison.contains("event: error\n"));
    assert!(poison.contains("data: "));
    assert!(poison.contains("stream failed"));
}

// ── StreamingConfig ─────────────────────────────────────────────

#[test]
fn default_config() {
    let config = StreamingConfig::default();
    assert_eq!(config.format, StreamingFormat::Ndjson);
    assert_eq!(config.stream_timeout_ms, 120_000);
}

// ── Validation ──────────────────────────────────────────────────

#[test]
fn validate_passes_valid_streaming() {
    let errors = validate_streaming("v", "Rest", true, false, true);
    assert!(errors.is_empty());
}

#[test]
fn validate_non_streaming_always_passes() {
    let errors = validate_streaming("v", "Websocket", false, true, false);
    assert!(errors.is_empty());
}

#[test]
fn validate_rejects_non_rest() {
    let errors = validate_streaming("v", "Websocket", true, false, true);
    assert!(errors.iter().any(|e| e.contains("only supported for Rest")));
}

#[test]
fn validate_rejects_non_codecomponent() {
    let errors = validate_streaming("v", "Rest", true, false, false);
    assert!(errors.iter().any(|e| e.contains("requires a CodeComponent")));
}

#[test]
fn validate_rejects_pipeline_stages() {
    let errors = validate_streaming("v", "Rest", true, true, true);
    assert!(errors.iter().any(|e| e.contains("must not have pipeline stages")));
}

#[test]
fn validate_multiple_errors() {
    let errors = validate_streaming("v", "Websocket", true, true, false);
    assert!(errors.len() >= 3);
}

// ── stream_terminated guard ─────────────────────────────────────

#[test]
fn stream_terminated_detected_in_object() {
    let data = serde_json::json!({"stream_terminated": true, "result": 42});
    assert!(check_stream_terminated(&data));
}

#[test]
fn stream_terminated_not_present() {
    let data = serde_json::json!({"result": 42, "status": "ok"});
    assert!(!check_stream_terminated(&data));
}

#[test]
fn stream_terminated_non_object_ignored() {
    assert!(!check_stream_terminated(&serde_json::json!([1, 2, 3])));
    assert!(!check_stream_terminated(&serde_json::json!("string")));
    assert!(!check_stream_terminated(&serde_json::json!(null)));
}

// ── Integration: Multiple chunks serialized in sequence ──────────

#[test]
fn multiple_ndjson_chunks_each_on_own_line() {
    let chunks = vec![
        StreamChunk::new(serde_json::json!({"id": 1, "name": "Alice"})),
        StreamChunk::new(serde_json::json!({"id": 2, "name": "Bob"})),
        StreamChunk::new(serde_json::json!({"id": 3, "name": "Carol"})),
    ];

    let combined: String = chunks.iter().map(|c| c.to_ndjson()).collect();
    let lines: Vec<&str> = combined.trim().split('\n').collect();
    assert_eq!(lines.len(), 3);

    // Each line is valid JSON
    for line in &lines {
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(parsed.get("id").is_some());
    }
}

#[test]
fn multiple_sse_chunks_separated_by_double_newlines() {
    let chunks = vec![
        StreamChunk::new(serde_json::json!({"v": 1})),
        StreamChunk::new(serde_json::json!({"v": 2})),
    ];

    let combined: String = chunks.iter().map(|c| c.to_sse(Some("update"))).collect();

    // Each SSE event block ends with \n\n
    let blocks: Vec<&str> = combined.split("\n\n").filter(|b| !b.is_empty()).collect();
    assert_eq!(blocks.len(), 2);
    assert!(blocks[0].contains("event: update"));
    assert!(blocks[1].contains("event: update"));
}

// ── Integration: Poison + normal chunk sequence ──────────────────

#[test]
fn poison_chunk_after_normal_chunks() {
    let normal = StreamChunk::new(serde_json::json!({"status": "ok"}));
    let normal_wire = normal.to_ndjson();

    let poison_wire = poison_chunk_ndjson("connection lost");

    let combined = format!("{}{}", normal_wire, poison_wire);
    let lines: Vec<&str> = combined.trim().split('\n').collect();
    assert_eq!(lines.len(), 2);

    // Last line is the poison
    let poison_parsed: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(poison_parsed["stream_terminated"], true);
    assert_eq!(poison_parsed["error"], "connection lost");
}

// ── Integration: StreamingConfig deserialization ─────────────────

#[test]
fn streaming_config_defaults_are_sensible() {
    let config = StreamingConfig::default();
    assert_eq!(config.format, StreamingFormat::Ndjson);
    assert!(config.stream_timeout_ms > 0);
}

// ── Integration: Validation accumulates all errors ───────────────

#[test]
fn validate_streaming_all_errors_at_once() {
    // Non-REST, has pipeline, no CodeComponent
    let errors = validate_streaming("bad_view", "ServerSentEvents", true, true, false);
    assert!(errors.len() >= 3, "should accumulate all errors: {:?}", errors);
    assert!(errors.iter().any(|e| e.contains("only supported for Rest")));
    assert!(errors.iter().any(|e| e.contains("requires a CodeComponent")));
    assert!(errors.iter().any(|e| e.contains("must not have pipeline stages")));
}
