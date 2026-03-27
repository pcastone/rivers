# Tutorial: Streaming REST Views

**Rivers v0.50.1**

## Overview

Streaming REST views send chunked HTTP responses — the server writes data incrementally instead of buffering the full response. Useful for large exports, real-time data feeds over HTTP, and long-running operations.

Rivers supports two streaming formats:
- **ndjson** — Newline-delimited JSON. Each line is a valid JSON object.
- **sse** — Server-Sent Events format over a standard REST endpoint.

## When to Use

- CSV/JSON data exports
- Large query result streaming
- Progress updates for long-running operations
- Any response too large to buffer in memory

---

## Step 1: Configure the View

File: `app.toml`

```toml
[api.views.export_data]
path              = "data/export"
method            = "POST"
view_type         = "Rest"
streaming         = true
streaming_format  = "ndjson"
stream_timeout_ms = 60000
auth              = "none"

[api.views.export_data.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/export.js"
entrypoint = "exportRows"
resources  = ["my_datasource"]
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `streaming` | yes | `false` | Enable chunked streaming |
| `streaming_format` | yes (when streaming) | — | `"ndjson"` or `"sse"` |
| `stream_timeout_ms` | no | — | Maximum stream duration (ms) |

---

## Step 2: Write the Handler

Streaming handlers use the **{chunk, done} protocol**. Rivers calls your function repeatedly until you return `{ done: true }`.

Between calls:
- `__args.iteration` — call counter (0, 1, 2, ...)
- `__args.state` — your previous return value's `state` field (for passing data between iterations)

File: `libraries/handlers/export.js`

### Basic: Fixed Row Count

```javascript
function exportRows(ctx) {
    var iteration = __args.iteration || 0;
    var totalRows = 100;

    // Signal completion
    if (iteration >= totalRows) {
        return { done: true };
    }

    // Each chunk becomes one ndjson line
    return {
        chunk: {
            row: iteration + 1,
            id: Rivers.crypto.randomHex(8),
            value: "data-" + iteration,
            exported_at: new Date().toISOString()
        },
        done: false
    };
}
```

### Advanced: Stateful Pagination

```javascript
function exportWithCursor(ctx) {
    var state = __args.state || { cursor: 0 };
    var batchSize = 50;

    // Fetch a batch using a DataView
    var batch = ctx.dataview("get_records", {
        offset: state.cursor,
        limit: batchSize
    });

    // No more data — done
    if (!batch || batch.length === 0) {
        Rivers.log.info("export complete", { total: state.cursor });
        return { done: true };
    }

    // Send batch and advance cursor
    return {
        chunk: { records: batch, offset: state.cursor },
        done: false,
        state: { cursor: state.cursor + batch.length }
    };
}
```

---

## Step 3: Test

### ndjson format

```bash
curl -X POST http://localhost:8080/data/export

# Output (one JSON object per line):
{"row":1,"id":"a3f2b1c0","value":"data-0","exported_at":"2026-03-24T10:00:00.000Z"}
{"row":2,"id":"b4e3c2d1","value":"data-1","exported_at":"2026-03-24T10:00:00.001Z"}
{"row":3,"id":"c5f4d3e2","value":"data-2","exported_at":"2026-03-24T10:00:00.002Z"}
...
```

### SSE format

If using `streaming_format = "sse"`:

```bash
curl -X POST http://localhost:8080/data/export

# Output:
data: {"row":1,"id":"a3f2b1c0","value":"data-0"}

data: {"row":2,"id":"b4e3c2d1","value":"data-1"}

data: {"row":3,"id":"c5f4d3e2","value":"data-2"}
```

---

## Handler Protocol Reference

### Input (per iteration)

| Variable | Type | Description |
|----------|------|-------------|
| `__args.iteration` | integer | Call count (starts at 0) |
| `__args.state` | any | Previous iteration's `state` return value |
| `ctx` | ViewContext | Standard handler context |

### Return Values

| Return | Effect |
|--------|--------|
| `{ chunk: data, done: false }` | Send `data` to client, continue |
| `{ chunk: data, done: false, state: obj }` | Send, continue, pass `obj` as `__args.state` next call |
| `{ done: true }` | End the stream |

### Stream Lifecycle

```
Client sends POST /data/export
  → Rivers calls handler (iteration=0) → { chunk: row1, done: false }
  → Rivers sends row1 to client
  → Rivers calls handler (iteration=1) → { chunk: row2, done: false }
  → Rivers sends row2 to client
  → ...
  → Rivers calls handler (iteration=N) → { done: true }
  → Rivers closes the response
```

If `stream_timeout_ms` is reached before `{ done: true }`, Rivers closes the stream automatically.
