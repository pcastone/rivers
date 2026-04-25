//! Structured logging via EventBus.
//!
//! Per `rivers-logging-spec.md`.
//!
//! `LogHandler` subscribes to EventBus at `Observe` priority.
//! It maps events to log levels, filters by `min_level`, formats
//! as JSON or Text, and emits to stdout (or optional file).

use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

use crate::config::LoggingConfig;
use crate::event::{Event, LogLevel};
use crate::eventbus::{event_log_level, EventBus, EventHandler, HandlerPriority};

// ── LogHandler ──────────────────────────────────────────────────────

/// EventBus handler that emits structured log records.
///
/// Runs at `Observe` tier — fire-and-forget, never blocks requests.
pub struct LogHandler {
    /// Output format (JSON or text).
    pub format: LogFormat,
    /// Minimum severity — events below this level are discarded.
    pub min_level: LogLevel,
    /// Application ID included in every log record.
    pub app_id: String,
    /// Node ID included in every log record.
    pub node_id: String,
    /// Optional async-buffered file writer for local log persistence.
    file_writer: Option<std::sync::Mutex<std::io::BufWriter<std::fs::File>>>,
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Structured JSON — one record per line.
    Json,
    /// Human-readable single-line text.
    Text,
}

impl LogFormat {
    /// Parse a format string (`"json"` or `"text"`). Defaults to JSON.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "text" => LogFormat::Text,
            _ => LogFormat::Json,
        }
    }
}

impl LogHandler {
    /// Create a LogHandler from config.
    pub fn from_config(config: &LoggingConfig, app_id: String, node_id: String) -> Self {
        let file_writer = config.local_file_path.as_ref().and_then(|path| {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => {
                    tracing::info!(path = %path, "log file writer opened");
                    Some(std::sync::Mutex::new(std::io::BufWriter::new(file)))
                }
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "failed to open log file — stdout only");
                    None
                }
            }
        });

        Self {
            format: LogFormat::parse(&config.format),
            min_level: config.level,
            app_id,
            node_id,
            file_writer,
        }
    }

    /// Register this handler on the EventBus for all known event types.
    ///
    /// Subscribes to a wildcard-like catch-all topic "*" if supported,
    /// or alternatively subscribes to each known event type.
    /// For simplicity, we subscribe to a single "*" topic and the
    /// EventBus publish path also publishes to "*".
    pub async fn register(self: Arc<Self>, bus: &EventBus) {
        // Subscribe to the wildcard topic that receives all events.
        // Permanent (lives for the lifetime of the bus) — use subscribe_static.
        bus.subscribe_static("*", self, HandlerPriority::Observe).await;
    }

    /// Check if an event's log level passes the min_level filter.
    pub fn should_log(&self, event: &Event) -> bool {
        let level = event_log_level(&event.event_type);
        level_ordinal(level) >= level_ordinal(self.min_level)
    }

    /// Format an event as a JSON log record string.
    pub fn format_json(&self, event: &Event) -> String {
        let level = event_log_level(&event.event_type);
        let mut record = serde_json::json!({
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "level": level_str(level),
            "message": event_message(&event.event_type),
            "trace_id": event.trace_id,
            "app_id": self.app_id,
            "node_id": self.node_id,
            "event_type": event.event_type,
        });

        // Merge payload fields into the record
        if let serde_json::Value::Object(payload) = &event.payload {
            if let serde_json::Value::Object(ref mut obj) = record {
                for (k, v) in payload {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        serde_json::to_string(&record).unwrap_or_default()
    }

    /// Format an event as a human-readable text line.
    pub fn format_text(&self, event: &Event) -> String {
        let level = event_log_level(&event.event_type);
        let trace = event.trace_id.as_deref().unwrap_or("-");
        format!(
            "{} [{}] {} trace_id={} event_type={}",
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level_str(level).to_uppercase(),
            event_message(&event.event_type),
            trace,
            event.event_type,
        )
    }
}

#[async_trait]
impl EventHandler for LogHandler {
    async fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.should_log(event) {
            return Ok(());
        }

        let line = match self.format {
            LogFormat::Json => self.format_json(event),
            LogFormat::Text => self.format_text(event),
        };

        // Stdout
        println!("{}", line);

        // File writer (if configured)
        if let Some(ref writer) = self.file_writer {
            if let Ok(mut w) = writer.lock() {
                let _ = writeln!(w, "{}", line);
                let _ = w.flush();
            }
        }

        // Per-app log routing: prefer app_id from event payload, fall back to self.app_id
        let effective_app_id = event
            .payload
            .get("app_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.app_id);

        if !effective_app_id.is_empty() && effective_app_id != "default" {
            if let Some(router) = crate::app_log_router::global_router() {
                router.write(effective_app_id, &line);
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "LogHandler"
    }
}

// ── Level helpers ───────────────────────────────────────────────────

/// Numeric ordinal for level comparison. Higher = more severe.
fn level_ordinal(level: LogLevel) -> u8 {
    match level {
        LogLevel::Trace => 0,
        LogLevel::Debug => 1,
        LogLevel::Info => 2,
        LogLevel::Warn => 3,
        LogLevel::Error => 4,
    }
}

fn level_str(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// Generate a human-readable message from an event type.
fn event_message(event_type: &str) -> String {
    // Convert PascalCase to lowercase space-separated
    let mut msg = String::new();
    for (i, ch) in event_type.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            msg.push(' ');
        }
        msg.push(ch.to_lowercase().next().unwrap_or(ch));
    }
    msg
}

// ── Trace ID ────────────────────────────────────────────────────────

/// Extract or generate a trace ID from request headers.
///
/// Priority:
/// 1. W3C `traceparent` header — extract 32 hex chars
/// 2. `x-trace-id` header — use as-is
/// 3. Generate UUID v4
pub fn extract_trace_id(
    traceparent: Option<&str>,
    x_trace_id: Option<&str>,
) -> String {
    // 1. W3C traceparent: 00-{32hex}-{16hex}-{2hex}
    if let Some(tp) = traceparent {
        if let Some(trace_id) = parse_traceparent(tp) {
            return trace_id;
        }
    }

    // 2. x-trace-id header
    if let Some(tid) = x_trace_id {
        if !tid.is_empty() {
            return tid.to_string();
        }
    }

    // 3. Generate
    uuid::Uuid::new_v4().to_string()
}

/// Parse W3C traceparent header format: `00-{trace_id}-{span_id}-{flags}`
pub fn parse_traceparent(header: &str) -> Option<String> {
    let parts: Vec<&str> = header.split('-').collect();
    if parts.len() >= 4 && parts[0] == "00" && parts[1].len() == 32 {
        // Validate hex
        if parts[1].chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(parts[1].to_string());
        }
    }
    None
}

/// Synthesize a W3C traceparent response header from a trace ID.
///
/// Span ID is zeroed (Rivers doesn't generate W3C span IDs).
/// Flags byte `01` indicates sampled.
pub fn synthesize_traceparent(trace_id: &str) -> String {
    // Pad or truncate to 32 hex chars
    let hex: String = trace_id
        .replace('-', "")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(32)
        .collect();
    let padded = format!("{:0<32}", hex);
    format!("00-{}-0000000000000000-01", padded)
}
