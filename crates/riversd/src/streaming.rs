//! Streaming REST view support.
//!
//! Per `rivers-streaming-rest-spec.md`.
//!
//! Streaming responses allow REST views to return chunked data
//! using NDJSON or SSE wire formats from AsyncGenerator handlers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError};

// ── Streaming Format ────────────────────────────────────────

/// Wire format for streaming REST responses.
///
/// Per spec: NDJSON or SSE.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamingFormat {
    /// Newline-delimited JSON (`application/x-ndjson`).
    Ndjson,
    /// Server-Sent Events (`text/event-stream`).
    Sse,
}

impl StreamingFormat {
    pub fn content_type(&self) -> &'static str {
        match self {
            StreamingFormat::Ndjson => "application/x-ndjson",
            StreamingFormat::Sse => "text/event-stream",
        }
    }

    pub fn from_str_opt(s: Option<&str>) -> Option<Self> {
        match s {
            Some(s) if s.eq_ignore_ascii_case("ndjson") => Some(StreamingFormat::Ndjson),
            Some(s) if s.eq_ignore_ascii_case("sse") => Some(StreamingFormat::Sse),
            _ => None,
        }
    }
}

// ── Streaming Chunk ─────────────────────────────────────────

/// A single chunk in a streaming response.
#[derive(Debug, Clone, Serialize)]
pub struct StreamChunk {
    pub data: serde_json::Value,
}

impl StreamChunk {
    pub fn new(data: serde_json::Value) -> Self {
        Self { data }
    }

    /// Serialize to NDJSON wire format (one JSON object per line).
    pub fn to_ndjson(&self) -> String {
        let mut s = serde_json::to_string(&self.data).unwrap_or_else(|_| "null".to_string());
        s.push('\n');
        s
    }

    /// Serialize to SSE wire format.
    pub fn to_sse(&self, event_type: Option<&str>) -> String {
        let data_str = serde_json::to_string(&self.data).unwrap_or_else(|_| "null".to_string());
        let mut out = String::new();
        if let Some(evt) = event_type {
            out.push_str(&format!("event: {}\n", evt));
        }
        out.push_str(&format!("data: {}\n\n", data_str));
        out
    }
}

// ── Poison Chunk ────────────────────────────────────────────

/// A poison chunk sent on mid-stream errors.
///
/// Per spec: NDJSON uses `stream_terminated` field, SSE uses `event: error`.
pub fn poison_chunk_ndjson(error: &str) -> String {
    let chunk = serde_json::json!({
        "stream_terminated": true,
        "error": error,
    });
    let mut s = serde_json::to_string(&chunk).unwrap_or_else(|_| "null".to_string());
    s.push('\n');
    s
}

pub fn poison_chunk_sse(error: &str) -> String {
    format!("event: error\ndata: {}\n\n", serde_json::json!({"error": error}))
}

// ── stream_terminated Guard (SHAPE-15) ─────────────────────

/// Check if a yielded chunk contains the reserved `stream_terminated` key.
///
/// Per SHAPE-15: if a CodeComponent yields JSON with `stream_terminated`,
/// the chunk is blocked (not sent to the client), a poison chunk is emitted,
/// and the stream is terminated. This prevents application code from
/// spoofing the framework's termination signal.
pub fn check_stream_terminated(data: &serde_json::Value) -> bool {
    match data {
        serde_json::Value::Object(map) => map.contains_key("stream_terminated"),
        _ => false,
    }
}

// ── Streaming Config ────────────────────────────────────────

/// Streaming configuration for a REST view.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    pub format: StreamingFormat,
    pub stream_timeout_ms: u64,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            format: StreamingFormat::Ndjson,
            stream_timeout_ms: 120_000, // 2 minutes per spec
        }
    }
}

// ── Validation ──────────────────────────────────────────────

/// Validate streaming configuration.
///
/// Per spec: streaming is REST-only, CodeComponent-only, no pipeline stages.
pub fn validate_streaming(
    view_id: &str,
    view_type: &str,
    streaming: bool,
    has_pipeline_stages: bool,
    is_codecomponent: bool,
) -> Vec<String> {
    let mut errors = Vec::new();

    if !streaming {
        return errors;
    }

    if view_type != "Rest" {
        errors.push(format!(
            "view '{}': streaming is only supported for Rest views",
            view_id
        ));
    }

    if !is_codecomponent {
        errors.push(format!(
            "view '{}': streaming requires a CodeComponent handler",
            view_id
        ));
    }

    if has_pipeline_stages {
        errors.push(format!(
            "view '{}': streaming views must not have pipeline stages (pre_process, transform, etc.)",
            view_id
        ));
    }

    errors
}

// ── Streaming Generator Loop (D12) ─────────────────────────

/// Maximum iterations for the generator dispatch loop (safety valve).
const MAX_GENERATOR_ITERATIONS: u64 = 100_000;

/// Run a streaming generator loop — multi-chunk CodeComponent execution.
///
/// Dispatches to the ProcessPool repeatedly in a loop. The JS handler follows
/// a yield protocol: return `{ chunk: <data>, done: false }` per chunk, and
/// `{ done: true }` to signal completion. State is passed between iterations
/// so the handler can track cursor position, counters, etc.
///
/// The stream ends when:
/// - Handler returns `{ done: true }`
/// - Handler returns `{ stream_terminated: true }` (SHAPE-15 guard)
/// - Total stream lifetime exceeds `stream_timeout_ms`
/// - Client disconnects (sender channel closed)
/// - Handler throws an error
/// - `MAX_GENERATOR_ITERATIONS` safety limit reached
pub async fn run_streaming_generator(
    pool: &ProcessPoolManager,
    entrypoint: &Entrypoint,
    config: &StreamingConfig,
    sender: mpsc::Sender<StreamChunk>,
    trace_id: &str,
) -> Result<(), StreamingError> {
    let timeout = tokio::time::Duration::from_millis(config.stream_timeout_ms);
    let format_str = match config.format {
        StreamingFormat::Ndjson => "ndjson",
        StreamingFormat::Sse => "sse",
    };

    let mut iteration: u64 = 0;
    let mut previous_result = serde_json::Value::Null;

    let loop_result = tokio::time::timeout(timeout, async {
        loop {
            if iteration >= MAX_GENERATOR_ITERATIONS {
                return Err(StreamingError::GeneratorError(
                    format!("generator exceeded {} iterations", MAX_GENERATOR_ITERATIONS),
                ));
            }

            let args = serde_json::json!({
                "format": format_str,
                "stream_timeout_ms": config.stream_timeout_ms,
                "iteration": iteration,
                "state": previous_result,
            });

            let builder = TaskContextBuilder::new()
                .entrypoint(entrypoint.clone())
                .args(args)
                .trace_id(format!("{}-{}", trace_id, iteration));
            let builder = crate::task_enrichment::enrich(builder, "");
            let ctx = builder
                .build()
                .map_err(|e| StreamingError::GeneratorError(e.to_string()))?;

            let result = match pool.dispatch("default", ctx).await {
                Ok(r) => r,
                Err(TaskError::EngineUnavailable(_)) => {
                    return Err(StreamingError::CodeComponentRequired);
                }
                Err(e) => {
                    return Err(StreamingError::GeneratorError(e.to_string()));
                }
            };

            // Check for completion signal
            if result.value.get("done").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(());
            }

            // Check SHAPE-15 guard on the result
            if check_stream_terminated(&result.value) {
                return Err(StreamingError::GeneratorError(
                    "stream_terminated by application".to_string(),
                ));
            }

            // Extract chunk data — use "chunk" field if present, otherwise the whole value
            let chunk_data = result.value.get("chunk").cloned().unwrap_or(result.value.clone());

            // Check SHAPE-15 guard on the chunk data too
            if check_stream_terminated(&chunk_data) {
                return Err(StreamingError::GeneratorError(
                    "stream_terminated by application".to_string(),
                ));
            }

            if !chunk_data.is_null() {
                let chunk = StreamChunk::new(chunk_data);
                if sender.send(chunk).await.is_err() {
                    return Err(StreamingError::ClientDisconnected);
                }
            }

            previous_result = result.value;
            iteration += 1;
        }
    })
    .await;

    match loop_result {
        Ok(result) => result,
        Err(_) => Err(StreamingError::Timeout(config.stream_timeout_ms)),
    }
}

// ── Client Disconnect Detection (D13) ───────────────────────

/// Monitor for client disconnection during streaming.
///
/// Used by streaming handlers to detect when the client has disconnected
/// so they can clean up resources and stop generating chunks.
pub struct DisconnectMonitor {
    cancelled: Arc<AtomicBool>,
}

impl DisconnectMonitor {
    /// Create a new disconnect monitor.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if the client has disconnected.
    pub fn is_disconnected(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Mark the client as disconnected.
    ///
    /// Called by the connection handler when the client drops.
    pub fn mark_disconnected(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Get a clone of the internal flag for sharing with other tasks.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

impl Default for DisconnectMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rivers.view.stream() Token (D14) ───────────────────────

/// Token representing a stream handle in the CodeComponent API.
///
/// Per spec: the CodeComponent receives this token and calls `write()`
/// to send chunks and `close()` to terminate the stream.
pub struct StreamToken {
    /// Channel for sending chunks to the HTTP response.
    pub sender: mpsc::Sender<StreamChunk>,
    /// Wire format for serialization.
    pub format: StreamingFormat,
}

impl StreamToken {
    /// Create a new stream token.
    pub fn new(sender: mpsc::Sender<StreamChunk>, format: StreamingFormat) -> Self {
        Self { sender, format }
    }

    /// Write a chunk to the stream.
    ///
    /// Returns an error if the client has disconnected.
    pub async fn write(&self, data: serde_json::Value) -> Result<(), StreamingError> {
        // SHAPE-15: block stream_terminated spoofing
        if check_stream_terminated(&data) {
            return Err(StreamingError::GeneratorError(
                "application code cannot use reserved key 'stream_terminated'".to_string(),
            ));
        }

        let chunk = StreamChunk::new(data);
        self.sender
            .send(chunk)
            .await
            .map_err(|_| StreamingError::ClientDisconnected)
    }

    /// Close the stream.
    ///
    /// Drops the sender, signaling to the response handler that the
    /// stream is complete.
    pub async fn close(self) -> Result<(), StreamingError> {
        // Dropping self.sender closes the channel
        drop(self.sender);
        Ok(())
    }
}

// ── Error Types ─────────────────────────────────────────────

/// Streaming errors.
#[derive(Debug, thiserror::Error)]
pub enum StreamingError {
    #[error("stream timeout after {0}ms")]
    Timeout(u64),

    #[error("client disconnected")]
    ClientDisconnected,

    #[error("generator error: {0}")]
    GeneratorError(String),

    #[error("handler requires CodeComponent (not yet available)")]
    CodeComponentRequired,
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── D12: Streaming generator tests ──────────────────────

    #[tokio::test]
    async fn test_streaming_generator_engine_unavailable() {
        // With the Boa engine active, dispatching a task with a missing
        // module file results in a GeneratorError (cannot read file)
        // rather than CodeComponentRequired.
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "stream.js".into(),
            function: "generate".into(),
            language: "javascript".into(),
        };
        let config = StreamingConfig::default();
        let (sender, _receiver) = mpsc::channel(16);

        let result =
            run_streaming_generator(&pool, &entrypoint, &config, sender, "trace-1").await;
        assert!(result.is_err());
        // Now that JS engine is live, missing file produces GeneratorError
        assert!(
            matches!(
                result,
                Err(StreamingError::GeneratorError(_)) | Err(StreamingError::CodeComponentRequired)
            ),
            "expected GeneratorError or CodeComponentRequired"
        );
    }

    #[tokio::test]
    async fn test_streaming_generator_timeout() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "stream.js".into(),
            function: "generate".into(),
            language: "javascript".into(),
        };
        // Very short timeout — handler error or timeout
        let config = StreamingConfig {
            format: StreamingFormat::Ndjson,
            stream_timeout_ms: 1,
        };
        let (sender, _receiver) = mpsc::channel(16);

        let result =
            run_streaming_generator(&pool, &entrypoint, &config, sender, "trace-1").await;
        // Either timeout, generator error, or engine unavailable — all valid
        assert!(result.is_err());
    }

    // ── D13: Disconnect monitor tests ───────────────────────

    #[test]
    fn test_disconnect_monitor_initial_state() {
        let monitor = DisconnectMonitor::new();
        assert!(!monitor.is_disconnected());
    }

    #[test]
    fn test_disconnect_monitor_mark_disconnected() {
        let monitor = DisconnectMonitor::new();
        monitor.mark_disconnected();
        assert!(monitor.is_disconnected());
    }

    #[test]
    fn test_disconnect_monitor_shared_flag() {
        let monitor = DisconnectMonitor::new();
        let flag = monitor.flag();

        assert!(!flag.load(Ordering::Relaxed));
        monitor.mark_disconnected();
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn test_disconnect_monitor_default() {
        let monitor = DisconnectMonitor::default();
        assert!(!monitor.is_disconnected());
    }

    // ── D14: StreamToken tests ──────────────────────────────

    #[tokio::test]
    async fn test_stream_token_write() {
        let (sender, mut receiver) = mpsc::channel(16);
        let token = StreamToken::new(sender, StreamingFormat::Ndjson);

        token
            .write(serde_json::json!({"chunk": 1}))
            .await
            .unwrap();
        token
            .write(serde_json::json!({"chunk": 2}))
            .await
            .unwrap();

        let c1 = receiver.recv().await.unwrap();
        assert_eq!(c1.data, serde_json::json!({"chunk": 1}));
        let c2 = receiver.recv().await.unwrap();
        assert_eq!(c2.data, serde_json::json!({"chunk": 2}));
    }

    #[tokio::test]
    async fn test_stream_token_blocks_stream_terminated() {
        let (sender, _receiver) = mpsc::channel(16);
        let token = StreamToken::new(sender, StreamingFormat::Ndjson);

        // SHAPE-15: should block stream_terminated key
        let result = token
            .write(serde_json::json!({"stream_terminated": true, "data": "hacked"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stream_token_close() {
        let (sender, mut receiver) = mpsc::channel(16);
        let token = StreamToken::new(sender, StreamingFormat::Ndjson);

        token.close().await.unwrap();

        // After close, receiver should get None
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn test_stream_token_write_after_receiver_drop() {
        let (sender, receiver) = mpsc::channel(16);
        let token = StreamToken::new(sender, StreamingFormat::Ndjson);

        drop(receiver);

        let result = token.write(serde_json::json!({"data": 1})).await;
        assert!(matches!(result, Err(StreamingError::ClientDisconnected)));
    }

    // ── AY1: Multi-chunk generator loop tests ──────────────

    #[tokio::test]
    async fn test_generator_multi_chunk_loop() {
        // JS handler yields 3 chunks then signals done
        let dir = std::env::temp_dir();
        let js_path = dir.join("ay1_multi_chunk.js");
        std::fs::write(&js_path, r#"
            function generate(ctx) {
                var i = __args.iteration || 0;
                if (i >= 3) return { done: true };
                return { chunk: { index: i, token: "chunk-" + i }, done: false };
            }
        "#).unwrap();

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "generate".into(),
            language: "javascript".into(),
        };
        let config = StreamingConfig { format: StreamingFormat::Ndjson, stream_timeout_ms: 10000 };
        let (sender, mut receiver) = mpsc::channel(16);

        let result = run_streaming_generator(&pool, &entrypoint, &config, sender, "ay1-test").await;
        assert!(result.is_ok(), "generator should complete: {:?}", result.err());

        // Collect all chunks
        let mut chunks = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            chunks.push(chunk);
        }
        assert_eq!(chunks.len(), 3, "should yield 3 chunks, got {}", chunks.len());
        assert_eq!(chunks[0].data["index"], 0);
        assert_eq!(chunks[1].data["index"], 1);
        assert_eq!(chunks[2].data["index"], 2);

        let _ = std::fs::remove_file(&js_path);
    }

    #[tokio::test]
    async fn test_generator_immediate_done() {
        let dir = std::env::temp_dir();
        let js_path = dir.join("ay1_immediate_done.js");
        std::fs::write(&js_path, r#"
            function generate(ctx) { return { done: true }; }
        "#).unwrap();

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "generate".into(),
            language: "javascript".into(),
        };
        let config = StreamingConfig::default();
        let (sender, mut receiver) = mpsc::channel(16);

        let result = run_streaming_generator(&pool, &entrypoint, &config, sender, "ay1-done").await;
        assert!(result.is_ok());

        // No chunks should have been sent
        assert!(receiver.try_recv().is_err());

        let _ = std::fs::remove_file(&js_path);
    }

    #[tokio::test]
    async fn test_generator_state_passed_between_iterations() {
        // Handler returns state that should be passed to next iteration
        let dir = std::env::temp_dir();
        let js_path = dir.join("ay1_state_pass.js");
        std::fs::write(&js_path, r#"
            function generate(ctx) {
                var iteration = __args.iteration || 0;
                var prev = __args.state || {};
                if (iteration >= 2) return { done: true };
                return {
                    chunk: { iter: iteration, saw_prev: prev.chunk ? prev.chunk.iter : null },
                    done: false
                };
            }
        "#).unwrap();

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "generate".into(),
            language: "javascript".into(),
        };
        let config = StreamingConfig { format: StreamingFormat::Ndjson, stream_timeout_ms: 10000 };
        let (sender, mut receiver) = mpsc::channel(16);

        let result = run_streaming_generator(&pool, &entrypoint, &config, sender, "ay1-state").await;
        assert!(result.is_ok());

        let mut chunks = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            chunks.push(chunk);
        }
        assert_eq!(chunks.len(), 2);
        // Second chunk should see previous iteration's result
        assert_eq!(chunks[0].data["iter"], 0);
        assert!(chunks[0].data["saw_prev"].is_null());
        assert_eq!(chunks[1].data["iter"], 1);
        assert_eq!(chunks[1].data["saw_prev"], 0);

        let _ = std::fs::remove_file(&js_path);
    }

    #[tokio::test]
    async fn test_generator_client_disconnect() {
        let dir = std::env::temp_dir();
        let js_path = dir.join("ay1_disconnect.js");
        std::fs::write(&js_path, r#"
            function generate(ctx) {
                return { chunk: { data: "infinite" }, done: false };
            }
        "#).unwrap();

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "generate".into(),
            language: "javascript".into(),
        };
        let config = StreamingConfig { format: StreamingFormat::Ndjson, stream_timeout_ms: 10000 };
        let (sender, receiver) = mpsc::channel(1); // Small buffer

        // Drop receiver immediately to simulate disconnect
        drop(receiver);

        let result = run_streaming_generator(&pool, &entrypoint, &config, sender, "ay1-dc").await;
        assert!(
            matches!(result, Err(StreamingError::ClientDisconnected)),
            "expected ClientDisconnected, got: {:?}",
            result
        );

        let _ = std::fs::remove_file(&js_path);
    }
}
