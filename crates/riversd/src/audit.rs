//! Framework audit event bus.
//!
//! Emits structured events for observable framework operations.
//! Enabled via `[audit] enabled = true` in `riversd.toml`.
//! Events are broadcast on a `tokio::sync::broadcast` channel and
//! exposed as newline-delimited JSON SSE at `GET /admin/audit/stream`.

use tokio::sync::broadcast;

/// Structured event emitted for observable framework operations.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditEvent {
    /// A REST or streaming handler completed execution.
    HandlerInvoked {
        /// Stable appId UUID from the app manifest.
        app_id: String,
        /// View identifier (slug).
        view: String,
        /// HTTP method (GET, POST, …).
        method: String,
        /// Request path.
        path: String,
        /// Wall-clock duration of the handler in milliseconds.
        duration_ms: u64,
        /// HTTP response status code.
        status: u16,
    },
    /// An MCP tool was dispatched via `tools/call`.
    McpToolCalled {
        /// Stable appId UUID from the app manifest.
        app_id: String,
        /// Tool name.
        tool: String,
        /// Wall-clock duration of the tool dispatch in milliseconds.
        duration_ms: u64,
        /// Whether the response carries `"isError": true`.
        is_error: bool,
    },
    /// A DataView query completed.
    DataViewRead {
        /// Stable appId UUID from the app manifest.
        app_id: String,
        /// DataView name (without app namespace prefix).
        dataview: String,
        /// Number of rows returned.
        row_count: usize,
        /// Wall-clock duration of the query in milliseconds.
        duration_ms: u64,
    },
    /// An auth/session resolution completed.
    AuthResolved {
        /// Stable appId UUID from the app manifest.
        app_id: String,
        /// HTTP method of the request.
        method: String,
        /// Request path.
        path: String,
        /// Outcome string: `"allowed"`, `"denied"`, `"anonymous"`, etc.
        outcome: String,
    },
}

/// Broadcast sender for audit events.
///
/// Clone to emit; `subscribe()` to receive.
pub type AuditBus = broadcast::Sender<AuditEvent>;

/// Create a new `AuditBus` with capacity 512.
///
/// The capacity is intentionally generous so that a burst of events
/// does not block the sender even when there are no active subscribers.
pub fn new_bus() -> AuditBus {
    let (tx, _) = broadcast::channel(512);
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn audit_bus_emits_to_subscriber() {
        let bus = new_bus();
        let mut rx = bus.subscribe();
        bus.send(AuditEvent::McpToolCalled {
            app_id: "test".into(),
            tool: "search".into(),
            duration_ms: 5,
            is_error: false,
        })
        .unwrap();
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, AuditEvent::McpToolCalled { .. }));
    }

    #[tokio::test]
    async fn audit_bus_lagged_receiver_does_not_block_sender() {
        let bus = new_bus();
        let _rx = bus.subscribe(); // subscriber that never reads
        // Overflow the buffer — should not block
        for i in 0..600u64 {
            let _ = bus.send(AuditEvent::McpToolCalled {
                app_id: "a".into(),
                tool: "t".into(),
                duration_ms: i,
                is_error: false,
            });
        }
        // If we get here, the sender was not blocked
    }
}
