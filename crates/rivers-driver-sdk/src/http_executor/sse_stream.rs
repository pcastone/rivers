//! SSE (Server-Sent Events) stream connection.
//!
//! Reads chunked data from an upstream response, parses SSE wire format
//! (`event:`, `data:`, `id:` lines delimited by double newlines),
//! and yields `HttpStreamEvent` values.

use async_trait::async_trait;

use crate::http_driver::{HttpDriverError, HttpStreamConnection, HttpStreamEvent};

/// SSE (Server-Sent Events) stream connection backed by a reqwest streaming response.
///
/// Reads chunked data from the upstream, parses SSE wire format
/// (`event:`, `data:`, `id:` lines delimited by double newlines),
/// and yields `HttpStreamEvent` values.
pub struct SseStreamConnection {
    pub(crate) response: reqwest::Response,
    pub(crate) buffer: String,
}

#[async_trait]
impl HttpStreamConnection for SseStreamConnection {
    async fn next(&mut self) -> Result<Option<HttpStreamEvent>, HttpDriverError> {
        loop {
            // Try to parse a complete SSE event from the buffer first
            if let Some(event) = parse_sse_event(&mut self.buffer) {
                return Ok(Some(event));
            }

            // Read more data from the streaming response
            let chunk = self
                .response
                .chunk()
                .await
                .map_err(|e| HttpDriverError::Request(format!("stream read: {e}")))?;

            match chunk {
                None => return Ok(None), // Stream ended
                Some(bytes) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), HttpDriverError> {
        // Drop the response to close the connection
        Ok(())
    }
}

/// Parse a single SSE event from the buffer.
///
/// SSE events are delimited by a double newline (`\n\n`). Each event
/// consists of lines prefixed with `event: `, `data: `, or `id: `.
pub(crate) fn parse_sse_event(buffer: &mut String) -> Option<HttpStreamEvent> {
    // SSE events are delimited by double newline
    if let Some(pos) = buffer.find("\n\n") {
        let event_text = buffer[..pos].to_string();
        *buffer = buffer[pos + 2..].to_string();

        let mut event_type = None;
        let mut data = String::new();
        let mut id = None;

        for line in event_text.lines() {
            if let Some(val) = line.strip_prefix("event: ") {
                event_type = Some(val.to_string());
            } else if let Some(val) = line.strip_prefix("data: ") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(val);
            } else if let Some(val) = line.strip_prefix("id: ") {
                id = Some(val.to_string());
            }
        }

        if !data.is_empty() || event_type.is_some() {
            return Some(HttpStreamEvent {
                event_type,
                data: serde_json::from_str(&data)
                    .unwrap_or(serde_json::Value::String(data)),
                id,
            });
        }
    }
    None
}
