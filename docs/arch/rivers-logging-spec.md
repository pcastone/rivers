# Rivers Logging Specification

**Document Type:** Implementation Specification
**Scope:** Log levels, structured format, EventBus-driven logging, destinations, trace correlation, security boundaries
**Status:** Reference / Ground Truth
**Source audit:** `crates/riversd/src/lib.rs` (LogHandler), `crates/rivers-core/src/config.rs`

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Log Levels](#2-log-levels)
3. [Structured Log Format](#3-structured-log-format)
4. [EventBus-Driven Logging](#4-eventbus-driven-logging)
5. [Trace Correlation](#5-trace-correlation)
6. [Destinations](#6-destinations)
7. [Security Boundaries](#7-security-boundaries)
8. [Configuration Reference](#8-configuration-reference)

---

## 1. Architecture Overview

Rivers logging is a single plane driven by EventBus events. Every meaningful server event passes through the EventBus before being logged. `LogHandler` subscribes at `Observe` priority — fire-and-forget, never blocks the request path.

```
Request path
    │
    ├─ EventBus.publish(RequestCompleted) ──→ LogHandler (Observe tier)
    │                                               │
    │                                               ├─ format = JSON: write JSON record to stdout (+ file if configured)
    │                                               └─ format = Text: tracing::info! to stdout (+ file if configured)
    │
    └─ tracing spans (axum, tokio, sqlx internals) → tracing subscriber → stdout
```

There is no distributed tracing export. No OTLP. No external collector. Log aggregation is the operator's responsibility — pipe stdout to journald, a Docker log driver, Loki, or any standard log shipper.

---

## 2. Log Levels

```rust
pub enum LogLevel {
    Debug,  // 0 — verbose, development only
    Info,   // 1 — normal operational events
    Warn,   // 2 — degraded state, not fatal
    Error,  // 3 — failure requiring attention
}
```

Level hierarchy: `Debug < Info < Warn < Error`. Setting `min_level = Warn` suppresses `Debug` and `Info`.

### Event-to-level mapping

This mapping is fixed — operators cannot reclassify individual events:

| Event | Level |
|---|---|
| `RequestCompleted` | Info |
| `DataViewExecuted` | Info |
| `WebSocketConnected` / `Disconnected` | Info |
| `WebSocketMessageIn` / `Out` | Info |
| `SseStreamOpened` / `Closed` | Info |
| `SseEventSent` | Info |
| `DriverRegistered` | Info |
| `DeploymentStatusChanged` | Info |
| `ConfigFileChanged` | Info |
| `CacheInvalidation` | Info |
| `DatasourceConnected` | Info |
| `BrokerConsumerStarted` / `Stopped` | Info |
| `BrokerMessageReceived` / `Published` | Info |
| `EventBusTopicPublished` | **Debug** |
| `EventBusTopicSubscribed` / `Unsubscribed` | Info |
| `ConnectionPoolExhausted` | **Warn** |
| `DatasourceCircuitOpened` | **Warn** |
| `DatasourceDisconnected` | **Warn** |
| `NodeHealthChanged` | **Warn** |
<!-- SHAPE-5 amendment: CredentialRotated event removed -->
| `DatasourceHealthCheckFailed` | **Error** |
| `BrokerConsumerError` | **Error** |
| `PluginLoadFailed` | **Error** |

---

## 3. Structured Log Format

Two formats are supported. Format is selected at startup from config and does not change at runtime.

### 3.1 JSON format

Emitted as newline-delimited JSON. One JSON object per log record.

**Mandatory fields** — always present:

```json
{
  "timestamp": "2026-03-11T14:23:01.847Z",
  "level": "info",
  "message": "request completed",
  "trace_id": "a1b2c3d4-e5f6-...",
  "app_id": "riversd",
  "node_id": "node-1",
  "event_type": "RequestCompleted"
}
```

**Payload fields** — merged from `event.payload`, vary by event type:

```json
// RequestCompleted
{
  "method": "GET",
  "path": "/api/orders/42",
  "status": 200,
  "latency_ms": 14,
  "datasource": "orders_db"
}

// DataViewExecuted
{
  "dataview_name": "get_order",
  "datasource": "orders_db",
  "latency_ms": 8,
  "success": true,
  "cache_hit": false,
  "error": null
}

// DatasourceCircuitOpened
{
  "datasource_id": "orders_db",
  "failure_threshold": 5,
  "state": "open"
}

// PluginLoadFailed
{
  "path": "/var/rivers/plugins/neo4j.so",
  "reason": "ABI version mismatch: expected 3, got 2"
}
```

### 3.2 Text format

Emitted via `tracing::info!("{}", record)`. The JSON record is stringified and passed to the tracing subscriber. The tracing subscriber's formatter controls the final output — typically `tracing_subscriber::fmt` with compact or pretty formatting.

Text format is intended for development. JSON format is intended for production log aggregation (Loki, Elasticsearch, CloudWatch).

---

## 4. EventBus-Driven Logging

`LogHandler` is registered as an `EventHandler` at `HandlerPriority::Observe` on the EventBus. It receives every event published to the bus.

```rust
struct LogHandler {
    format: LogFormat,   // Json | Text
    min_level: LogLevel,
    app_id: String,
    node_id: String,
    file_writer: Option<AsyncFileWriter>,
}
```

The handler:
1. Maps `event.event_type` to a `LogLevel`
2. Checks if that level is enabled (≥ min_level)
3. If suppressed, returns early — no allocation
4. Builds the JSON record from fixed fields + `event.payload`
5. Emits to stdout via `println!` (JSON) or `tracing::info!` (Text)
6. If `file_writer` is configured, enqueues the record to the async buffered writer

Because `LogHandler` runs at `Observe` priority, it is fire-and-forget. Log I/O does not delay the request or any Critical/Standard-tier event handlers.

### 4.1 Events NOT logged by LogHandler

Not all EventBus events map to `LogHandler`. The following are consumed by other handlers and are not duplicated in LogHandler:

- Internal Rivers events (cluster gossip, Raft state machine transitions)
- Handler execution events for CodeComponent invocations (traced, not logged as events)

---

## 5. Trace Correlation

Every log record includes a `trace_id`. The trace ID originates in the HTTP request middleware stack and propagates through the entire request lifecycle.

### 5.1 Trace ID extraction

`trace_id_middleware` runs on every inbound request (both main server and admin server). Extraction priority:

1. **W3C `traceparent` header** — format `00-{trace_id}-{span_id}-{flags}`. The `trace_id` segment (32 hex chars, positions 3–34) is extracted.
2. **`x-trace-id` header** — custom Rivers header, used for backwards compatibility.
3. **Generated UUID** — `Uuid::new_v4()` if neither header is present.

### 5.2 Trace ID propagation

The extracted trace ID is:
- Stored in request extensions as `TraceId(String)`
- Injected into `x-trace-id` request header (for downstream middleware access)
- Emitted in response headers: `x-trace-id` and `traceparent` (synthesized)
- Passed into every `Event` via `event.trace_id`
- Included in every `LogHandler` JSON record as `"trace_id"`
- Attached to the active tracing span via `tracing::Span::current().record("trace_id", &trace_id)`

### 5.3 Traceparent response format

```
traceparent: 00-{trace_id_hex_32chars}-0000000000000000-01
```

The span ID segment is zeroed — Rivers does not generate W3C-compliant 8-byte span IDs. The flags byte `01` indicates sampled. Downstream consumers should use the trace ID segment only.

### 5.4 Trace ID in DataView events

`DataViewRequest` carries `trace_id` through execution. `DataViewExecuted` events include the trace ID, so DataView execution can be correlated to the originating HTTP request in log queries.

---

## 6. Destinations

### 6.1 Stdout (always on)

All log output goes to stdout. Rivers makes no attempt to manage log files, log rotation, or log retention — that is the operator's responsibility (journald, Docker log driver, Kubernetes log aggregator, Loki promtail, etc.).

JSON format → `println!("{}", json_record)` → stdout.
Text format → `tracing::info!` → tracing subscriber → stdout.

### 6.2 Local file (optional)

When `local_file_path` is set, log records are written to a file in addition to stdout. The file is opened in append mode at startup. No rotation is performed by Rivers — use logrotate or equivalent.

```toml
[base.logging]
local_file_path = "/var/log/rivers/riversd.log"
```

File writes use an async buffered writer (`tokio::io::BufWriter` over a `tokio::fs::File`). Records are enqueued into the writer from `LogHandler` without blocking. The writer flushes on a configurable interval or when the buffer is full.

This means file writes do not add latency to the `LogHandler` execution path under normal load. On shutdown, the writer flushes and closes before the process exits.

### 6.3 Per-Application Log Files

When `app_log_dir` is configured in `[base.logging]`, each loaded application receives a dedicated log file:

```toml
[base.logging]
app_log_dir = "/opt/rivers/log/apps"
```

**Behavior:**
- On bundle load, `AppLogRouter` creates `<app_log_dir>/<entry_point>.log` for each app
- `Rivers.log.info/warn/error` calls in V8 and WASM handlers write to both:
  1. The central tracing subscriber (stdout + `local_file_path`)
  2. The per-app log file via `AppLogRouter`
- EventBus events with `app_id` context also route to per-app files
- Log files rotate at 10MB (`<name>.log.1`, single backup)
- Writers use `BufWriter` buffering; flush on rotation, shutdown, and Drop

**Implementation:**
- `rivers-core/src/app_log_router.rs` — `AppLogRouter` file writer registry (global via `OnceLock`)
- `riversd/src/process_pool/v8_engine/task_locals.rs` — `TASK_APP_NAME` thread-local
- `riversd/src/process_pool/v8_engine/rivers_global.rs` — `write_to_app_log()` in V8 callbacks
- `riversd/src/process_pool/wasm_engine.rs` — equivalent WASM logging
- `rivers-core/src/logging.rs` — EventBus `LogHandler` per-app routing

---

## 7. Security Boundaries

<!-- SHAPE-4 amendment: all string-scanning redaction removed; LockBox + capability model is the security boundary -->

Rivers does not perform string-scanning redaction of log messages. The security boundary is structural:

- **LockBox** ensures secret values are never loaded into long-lived memory and never reach the driver layer in plaintext form accessible to logs
- **Capability model** restricts what data handlers can access — unauthorized access is blocked before any value is produced
- **Driver authors** are responsible for their own error message hygiene

### 7.1 What is never logged

- Raw credential values from LockBox
- Resource handle contents (opaque tokens, not values)
- PASETO tokens in full (only expiry and node_id metadata)
- Admin API private keys
- WebSocket session payloads (only size and type metadata)

---

## 8. Configuration Reference

### 8.1 Logging config

```toml
[base.logging]
level           = "info"                           # debug | info | warn | error
format          = "json"                           # json | text
local_file_path = "/var/log/rivers/riversd.log"   # optional
app_log_dir     = "/opt/rivers/log/apps"           # optional — per-app log files
```

Defaults: `level = "info"`, `format = "json"`, `local_file_path = null`, `app_log_dir = null`.

Environment override:

```toml
[environment_overrides.dev.logging]
level  = "debug"
format = "text"
```

### 8.2 Log query patterns

For JSON format logs shipped to a log aggregation system:

```
# All errors in the last hour
level = "error" | timestamp > now() - 1h

# All requests to /api/orders with latency > 500ms
event_type = "RequestCompleted" | path starts_with "/api/orders" | latency_ms > 500

# Trace reconstruction — all events for a trace_id
trace_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"

# Circuit breaker events
event_type = "DatasourceCircuitOpened" | datasource_id = "orders_db"

# Plugin load failures
event_type = "PluginLoadFailed"
```

---

## Shaping Amendments

The following changes were applied to this spec per decisions in `rivers-shaping-and-gap-analysis.md`:

### SHAPE-4: Credential Redaction Dropped

- **S7** (formerly S7.1/S7.2) — Removed all string-scanning redaction requirements. The previous S7.1 (DataViewEngine 4-keyword redaction) and S7.2 (V8/WASM 15-keyword `redact_sensitive_text`) have been removed entirely. Section renamed from "Redaction" to "Security Boundaries" to reflect the structural security model.
- LockBox + capability model is the security boundary. Driver authors own their own error message hygiene.

### SHAPE-5: LockBox Index-Only Resolver

- **S2 event table** — `CredentialRotated` event row removed from the event-to-level mapping. This event no longer exists — LockBox rotation writes to disk and connections cycle naturally via `max_lifetime` or health check failure.
