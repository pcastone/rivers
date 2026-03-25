# Tutorial: Server-Sent Events (SSE) Views

**Rivers v0.50.1**

## Overview

SSE views maintain a long-lived HTTP connection where the server pushes events to the client. Unlike WebSocket, SSE is one-way (server → client) and uses standard HTTP — no upgrade needed.

Rivers SSE views support:
- **Polling** — periodically query a DataView and push changes
- **Diff strategies** — only push when data actually changes
- **Trigger events** — push when specific EventBus events fire
- **Last-Event-ID** — automatic reconnection with replay

## When to Use

- Live dashboards and metrics
- Notification feeds
- Activity streams
- Stock tickers / real-time data feeds
- Any use case where the server pushes updates and the client just listens

---

## Option A: Polling-Based SSE

Rivers polls a DataView at a regular interval and pushes updates to connected clients.

### Step 1: Create a DataView

```toml
[data.dataviews.dashboard_metrics]
name          = "dashboard_metrics"
datasource    = "metrics_db"
query         = "schemas/metric.schema.json"
return_schema = "schemas/metric.schema.json"

[data.dataviews.dashboard_metrics.caching]
ttl_seconds = 5

[[data.dataviews.dashboard_metrics.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 10
```

### Step 2: Create the SSE View with Polling

```toml
[api.views.metrics_stream]
path                  = "events/metrics"
method                = "GET"
view_type             = "ServerSentEvents"
auth                  = "none"
sse_event_buffer_size = 100

[api.views.metrics_stream.handler]
type     = "dataview"
dataview = "dashboard_metrics"

[api.views.metrics_stream.polling]
tick_interval_ms = 3000
diff_strategy    = "hash"
poll_state_ttl_s = 300
```

### Polling Configuration

| Field | Default | Description |
|-------|---------|-------------|
| `tick_interval_ms` | 5000 | How often to poll the DataView (ms) |
| `diff_strategy` | `"hash"` | When to push updates (see below) |
| `poll_state_ttl_s` | 300 | How long to keep per-client poll state (seconds) |

### Diff Strategies

| Strategy | Behavior |
|----------|----------|
| `"hash"` | Compute a hash of the result. Only push if the hash changed since last poll. Best for most use cases. |
| `"null"` | Always push every poll result, even if unchanged. Use when you want every tick. |
| `"change_detect"` | Use driver-level change detection (if supported). Most efficient but driver-dependent. |

---

## Option B: Event-Triggered SSE

Push events when specific things happen in the system, powered by the EventBus.

```toml
[api.views.order_events]
path                  = "events/orders"
method                = "GET"
view_type             = "ServerSentEvents"
auth                  = "session"
sse_trigger_events    = ["OrderCreated", "OrderUpdated", "OrderCancelled"]
sse_event_buffer_size = 200
```

When `OrderCreated`, `OrderUpdated`, or `OrderCancelled` events are published to the EventBus, they are pushed to all connected SSE clients.

---

## Option C: CodeComponent SSE Handler

Use a handler to generate custom SSE events.

```toml
[api.views.custom_events]
path      = "events/custom"
method    = "GET"
view_type = "ServerSentEvents"
auth      = "none"

[api.views.custom_events.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/events.js"
entrypoint = "generateEvents"
resources  = ["metrics"]
```

```javascript
function generateEvents(ctx) {
    var metrics = ctx.dataview("system_metrics");
    ctx.resdata = {
        type: "metrics_update",
        data: metrics,
        timestamp: new Date().toISOString()
    };
}
```

---

## Last-Event-ID Reconnection

Rivers supports automatic reconnection replay. When a client disconnects and reconnects with the `Last-Event-ID` header, Rivers replays all buffered events after that ID.

```
Client connects → receives events with IDs 1, 2, 3, 4, 5
Client disconnects
Client reconnects with Last-Event-ID: 3
Server replays events 4, 5, then continues live
```

The buffer size is controlled by `sse_event_buffer_size` (default 100).

---

## Testing

### curl

```bash
# Connect to SSE stream
curl -N http://localhost:8080/events/metrics

# Output (one event every 3 seconds):
# data: {"host":"server-1","cpu_percent":42.3,...}
#
# data: {"host":"server-2","cpu_percent":78.1,...}

# Reconnect with Last-Event-ID
curl -N -H "Last-Event-ID: 5" http://localhost:8080/events/metrics
```

### JavaScript (browser)

```javascript
var source = new EventSource("http://localhost:8080/events/metrics");

source.onmessage = function(event) {
    var data = JSON.parse(event.data);
    console.log("metric update:", data);
};

source.onerror = function() {
    console.log("connection lost — browser will auto-reconnect");
};
```

---

## SSE View Configuration Reference

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `view_type` | yes | — | Must be `"ServerSentEvents"` |
| `method` | yes | — | Must be `"GET"` |
| `sse_event_buffer_size` | no | 100 | Events buffered for Last-Event-ID replay |
| `sse_trigger_events` | no | — | EventBus events that trigger a push |
| `sse_tick_interval_ms` | no | 5000 | Shorthand for polling.tick_interval_ms |

### Polling Sub-Config

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `tick_interval_ms` | no | 5000 | Poll interval (ms) |
| `diff_strategy` | no | `"hash"` | `"hash"`, `"null"`, or `"change_detect"` |
| `poll_state_ttl_s` | no | 300 | Per-client state TTL (seconds) |
