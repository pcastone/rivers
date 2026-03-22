use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Log severity levels matching the Rivers logging spec (§2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

/// An EventBus event.
///
/// All meaningful server events (request completed, circuit opened, etc.)
/// are published through the EventBus as `Event` instances.
/// Per spec: event_type, payload, trace_id, timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_id: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl Event {
    /// Create a new event with the current timestamp.
    pub fn new(event_type: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            event_type: event_type.into(),
            payload,
            trace_id: None,
            timestamp: Utc::now(),
        }
    }

    /// Attach a trace ID to this event.
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }
}
