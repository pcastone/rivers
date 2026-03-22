# Rivers Logging Specification

**Document Type:** Implementation Specification  
**Scope:** Log levels, structured format, EventBus-driven logging, destinations, trace correlation, OTel integration  
**Status:** Reference / Ground Truth  
**Source audit:** `crates/riversd/src/lib.rs` (LogHandler, TracingConfig), `crates/rivers-core/src/config.rs`

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Log Levels](#2-log-levels)
3. [Structured Log Format](#3-structured-log-format)
4. [EventBus-Driven Logging](#4-eventbus-driven-logging)
5. [Trace Correlation](#5-trace-correlation)
6. [Destinations](#6-destinations)
7. [OpenTelemetry Integration](#7-opentelemetry-integration)
8. [Redaction](#8-redaction)
9. [Configuration Reference](#9-configuration-reference)

---

## 1. Architecture Overview

Rivers logging has two planes that are deliberately separate:

**Application log plane** — structured records emitted by `LogHandler`, driven by EventBus events. This is the log stream that operators see. Every meaningful server event (request completed, circuit opened, plugin failed) passes through EventBus before being logged. `LogHandler` subscribes at `Observe` priority tier — fire-and-forget, never blocks the request path.

**Tracing plane** — `tracing` crate spans and events used by the Rust framework layer (Axum, tokio, sqlx, etc.). These feed into the OpenTelemetry pipeline when tracing is enabled. Application log plane records are also emitted through `tracing::info!` in Text format, merging both planes into the OTel pipeline when configured.

```
Request path
    │
    ├─ EventBus.publish(RequestCompleted) ──→ LogHandler (Observe tier)
    │                                               │
    │                                               ├─ format = JSON: println! to stdout
    │                                               └─ format = Text: tracing::info!
    │
    └─ tracing spans (driver.execute, view.dispatch, etc.)
            │
            └─ OTel exporter (when enabled) → OTLP endpoint
```

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
| `CredentialRotated` | **Warn** |
| `DatasourceHealthCheckFailed` | **Error** |
| `BrokerConsumerError` | **Error** |
| `PluginLoadFailed` | **Error** |

---

## 3. Structured Log Format

Two formats are supported. Format is selected at startup from config and does not change at runtime.

### 3.1 JSON format

Emitted to stdout as newline-delimited JSON. One JSON object per log record.

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
}
```

The handler:
1. Maps `event.event_type` to a `LogLevel`
2. Checks if that level is enabled (≥ min_level)
3. If suppressed, returns early — no allocation
4. Builds the JSON record from fixed fields + `event.payload`
5. Emits via `println!` (JSON) or `tracing::info!` (Text)

Because `LogHandler` runs at `Observe` priority, it is fire-and-forget. A slow log write does not delay the request or any other Critical/Standard-tier event handlers.

### 4.1 Events NOT logged by LogHandler

Not all EventBus events map to `LogHandler`. The following are consumed by other handlers (MetricsCollector, DatasourceLifecycleHandler) and are not duplicated in LogHandler:

- Internal Rivers events (cluster gossip, Raft state machine transitions)
- Handler execution events for CodeComponent invocations (traced, not logged as events)

---

## 5. Trace Correlation

Every log record includes a `trace_id`. The trace ID originates in the HTTP request middleware stack and propagates through the entire request lifecycle.

### 5.1 Trace ID extraction

`trace_id_middleware` runs on every inbound request (both main server and admin server). Extraction priority:

1. **W3C `traceparent` header** — format `00-{trace_id}-{span_id}-{flags}`. The `trace_id` segment (32 hex chars, positions 3-34) is extracted.
2. **`x-trace-id` header** — custom Rivers header, used for backwards compatibility.
3. **Generated UUID** — `Uuid::new_v4()` if neither header is present.

### 5.2 Trace ID propagation

The extracted trace ID is:
- Stored in request extensions as `TraceId(String)`
- Injected into `x-trace-id` request header (for downstream middleware access)
- Emitted in response headers: both `x-trace-id` and `traceparent` (synthesized)
- Passed into every `Event` via `event.trace_id`
- Included in every `LogHandler` JSON record as `"trace_id"`
- Attached to the active tracing span via `tracing::Span::current().record("trace_id", &trace_id)`

### 5.3 Traceparent response format

```
traceparent: 00-{trace_id_hex_32chars}-0000000000000000-01
```

The span ID segment is zeroed because Rivers does not generate W3C-compliant 8-byte span IDs in the current implementation. The flags byte `01` indicates sampled. Downstream systems that consume `traceparent` for trace propagation should use the trace ID segment only.

### 5.4 Trace ID in DataView events

`DataViewRequest` carries `trace_id` through execution. `DataViewExecuted` events include the trace ID, so DataView execution can be correlated to the originating HTTP request in log queries.

---

## 6. Destinations

### 6.1 Stdout (default)

All log output goes to stdout. Rivers makes no attempt to manage log files, log rotation, or log retention — that is the operator's responsibility (journald, Docker log driver, Kubernetes log aggregator).

JSON format → `println!("{}", json_record)` → stdout.  
Text format → `tracing::info!` → tracing subscriber → stdout.

### 6.2 Local file

When `local_file_path` is set, log records are written to a file in addition to stdout. The file is opened in append mode. No rotation is performed by Rivers — use logrotate or equivalent.

```toml
[base.logging]
local_file_path = "/var/log/rivers/riversd.log"
```

File writes are synchronous in the current implementation. This adds latency to the `LogHandler` execution path. For high-throughput production deployments, this path should be replaced with an async buffered writer. This is an open item.

### 6.3 OTel logs signal (cluster nodes)

For cluster deployments, log records from cluster nodes are multiplexed over the existing RPS protocol connection. There is no separate TCP log shipper. The OTel logs signal is the transport.

When `performance.tracing.enabled = true`, the `tracing-opentelemetry` layer feeds all `tracing` events (including those from `tracing::info!` in Text format LogHandler) into the OTel pipeline. The OTLP exporter sends spans + logs to the configured endpoint.

For non-cluster single-node deployments, `local_file_path` or stdout is the only destination.

---

## 7. OpenTelemetry Integration

### 7.1 Span hierarchy

When tracing is enabled, the following spans are created per request:

```
http.request  (root span — created by trace_id_middleware)
    │
    └─ view.dispatch  (created by view handler)
            │
            ├─ dataview.execute  (created by DataViewEngine)
            │       │
            │       └─ driver.execute  (created by DataViewEngine inside pool acquire)
            │
            └─ codecomponent.execute  (created by RuntimeFactory, when applicable)
```

Span attributes:

| Span | Key attributes |
|---|---|
| `http.request` | `http.method`, `http.target`, `http.status_code`, `trace_id` |
| `view.dispatch` | `view.id`, `view.type` |
| `dataview.execute` | `dataview.id`, `datasource.driver` |
| `driver.execute` | `dataview.id`, `datasource.driver` |
| `codecomponent.execute` | `component.module`, `component.language` |

### 7.2 Sampling

```rust
pub struct TracingConfig {
    pub enabled: bool,
    pub provider: String,        // "otlp" | "jaeger" | "datadog"
    pub endpoint: Option<String>,
    pub service_name: String,
    pub sampling_rate: Option<f64>,  // 0.0..=1.0
}
```

- `sampling_rate = None` or not set: always-on sampling (development default)
- `sampling_rate = 1.0`: sample every request
- `sampling_rate = 0.1`: sample 10% of requests (production default recommendation)
- `sampling_rate = 0.0`: disable tracing (not recommended — use `enabled = false`)

Sampling is ratio-based. The decision is made at the root span (`http.request`). Child spans inherit the parent's sampling decision.

### 7.3 OTLP exporter

Transport: gRPC via `tonic`. Endpoint must be an OTLP gRPC receiver (Jaeger, Grafana Agent, OpenTelemetry Collector, Datadog Agent with OTLP enabled).

Validation at startup:
- `enabled = true` requires `endpoint` to be set
- `endpoint` must not be empty
- `service_name` must not be empty
- `sampling_rate` (if set) must be in `0.0..=1.0`
- `provider` must be one of `"otlp"`, `"jaeger"`, `"datadog"`

---

## 8. Redaction

Rivers redacts sensitive content at two points:

### 8.1 Error message redaction (DataViewEngine)

Before emitting `DataViewExecuted` events, error strings are checked for sensitive keywords. If any of these appear in the error message, the entire string is replaced with `"sensitive details redacted"`:

- `password`
- `token`
- `secret`
- `authorization`

### 8.2 General text redaction (V8/WASM runtime)

The `redact_sensitive_text` function used in runtime error paths has an expanded keyword list of 15 terms. It uses targeted key=value redaction where possible:

- Targeted: `password=hunter2&host=localhost` → `password=[REDACTED]&host=localhost`
- Full fallback: bare keyword match → entire message is `[REDACTED]`

Keywords: `password`, `passwd`, `token`, `secret`, `authorization`, `api_key`, `apikey`, `api-key`, `access_key`, `private_key`, `credential`, `bearer`, `session_id`, `cookie`, `auth`.

### 8.3 What is never logged

- Raw credential values from Lockbox
- Resource handle contents (opaque tokens, not values)
- PASETO tokens in full (only expiry and node_id metadata)
- Admin API private keys
- WebSocket session payloads (only size and type metadata)

---

## 9. Configuration Reference

### 9.1 Logging config

```toml
[base.logging]
level           = "info"     # debug | info | warn | error
format          = "json"     # json | text
local_file_path = "/var/log/rivers/riversd.log"   # optional
```

Defaults: `level = "info"`, `format = "json"`, `local_file_path = null`.

Environment override:

```toml
[environment_overrides.dev.logging]
level  = "debug"
format = "text"
```

### 9.2 Tracing config

```toml
[performance.tracing]
enabled       = true
provider      = "otlp"                          # otlp | jaeger | datadog
endpoint      = "http://otel-collector:4317"
service_name  = "riversd"
sampling_rate = 0.1                             # 10% sampling in production
```

### 9.3 Log query patterns

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
