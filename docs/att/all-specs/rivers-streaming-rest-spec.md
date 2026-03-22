# Rivers Streaming REST Specification

**Document Type:** Spec Addition / Patch  
**Scope:** Streaming REST responses — View Layer, ProcessPool Runtime, HTTPD  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-view-layer-spec.md`, `rivers-processpool-runtime-spec-v2.md`, `rivers-httpd-spec.md`  
**Depends On:** Epic 4 (EventBus), Epic 12 (ProcessPool), Epic 13 (View Layer)

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [Wire Formats](#2-wire-formats)
3. [View Layer Changes](#3-view-layer-changes)
4. [ProcessPool Runtime Changes](#4-processpool-runtime-changes)
5. [HTTPD Changes](#5-httpd-changes)
6. [Error Handling Mid-Stream](#6-error-handling-mid-stream)
7. [Timeout Semantics](#7-timeout-semantics)
8. [Validation Rules](#8-validation-rules)
9. [Configuration Reference](#9-configuration-reference)
10. [Examples](#10-examples)

---

## 1. Design Rationale

### 1.1 Why Not SSE for Streaming POST

SSE (`view_type = ServerSentEvents`) is constrained to GET in the Rivers spec. This reflects the browser `EventSource` API limitation — the browser standard only supports GET with no request body. This constraint is correct for browser clients but is wrong for AI agentic workflows.

Agentic callers — orchestrators, agent runtimes, backend services — use raw HTTP clients. They have no `EventSource` constraint. They issue POST requests with large request bodies (prompts, documents, context windows) and expect a streaming response body over the same connection.

Forcing these callers to use SSE requires:
1. POST to create a stream session → receive a `stream_id`  
2. GET to subscribe to the stream by ID  

That is two round trips, cross-request state, stream ID lifecycle management, and session cleanup on disconnect. It is the wrong abstraction for a call that is logically one request with a streaming response.

### 1.2 Scope

Streaming REST is a property of REST views only. It has no interaction with WebSocket, SSE, or MessageConsumer view types. Those view types own their own streaming semantics and are unchanged.

### 1.3 Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Wire format | Both ndjson and SSE format — declared per view | ndjson is the de facto standard for AI APIs; SSE format adds event typing for structured streams |
| Declaration | Explicit `streaming = true` config flag | Consistent with Rivers' declarative config philosophy |
| Timeout | `stream_timeout_ms` replaces `task_timeout_ms` for streaming handlers | Total lifetime timeout is appropriate; per-chunk timeouts add complexity without benefit |
| Error mid-stream | Poison chunk — final structured error object in stream | Headers already committed on first chunk; poison chunk is parseable without ambiguity |

---

## 2. Wire Formats

### 2.1 NDJSON (`application/x-ndjson`)

Each yielded value is serialized as a JSON object on a single line, terminated by `\n`. The client reads line by line. This is the format used by the OpenAI and Anthropic streaming APIs.

```
HTTP/1.1 200 OK
Content-Type: application/x-ndjson
Transfer-Encoding: chunked

{"token":"Hello"}\n
{"token":" world"}\n
{"token":"!"}\n
{"done":true,"total_tokens":42}\n
```

**Error chunk (terminal):**
```
{"error":"handler threw after 3 chunks","error_type":"HandlerError","stream_terminated":true}\n
```

The `stream_terminated` field signals abnormal termination. Non-error final chunks do not include this field. Clients MUST check for `stream_terminated` on every chunk.

### 2.2 SSE Format (`text/event-stream`)

Standard SSE wire format (`data:`, `event:`, `id:` fields) over any HTTP method. Compatible with any HTTP client that can read chunked responses. Not restricted to browser `EventSource`.

```
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
Transfer-Encoding: chunked

data: {"token":"Hello"}\n\n
data: {"token":" world"}\n\n
event: done\ndata: {"total_tokens":42}\n\n
```

**Error event (terminal):**
```
event: error\n
data: {"error":"handler threw after 3 chunks","error_type":"HandlerError"}\n\n
```

The `event: error` terminates the stream. The connection closes after this event is flushed.

### 2.3 Format Selection

The format is declared per view in config (`streaming_format = "ndjson"` or `"sse"`). The `Content-Type` response header is set by Rivers at stream open — the handler does not set it. If `streaming_format` is omitted when `streaming = true`, it defaults to `ndjson`.

---

## 3. View Layer Changes

### 3.1 ApiViewConfig additions

The following fields are added to `ApiViewConfig`:

```rust
pub struct ApiViewConfig {
    // ... existing fields unchanged ...

    /// Enables streaming response mode. Only valid for view_type = Rest.
    pub streaming: bool,                              // default: false

    /// Wire format for streaming. Only relevant when streaming = true.
    pub streaming_format: StreamingFormat,            // default: Ndjson
}

pub enum StreamingFormat {
    Ndjson,  // application/x-ndjson
    Sse,     // text/event-stream
}
```

### 3.2 Streaming view constraints

- `streaming = true` is only valid for `view_type = Rest`. Config validation rejects it on WebSocket, SSE, and MessageConsumer views.
- `streaming = true` requires `handler = CodeComponent`. DataView handlers do not support streaming — they execute as a unit and return a complete `QueryResult`. Config validation rejects a DataView handler on a streaming view.
- The handler pipeline (`pre_process`, `on_request`, `transform`, `on_response`, `post_process`) is **not supported** on streaming views. The CodeComponent is the sole handler. Config validation rejects `event_handlers` on a streaming view.

**Rationale for pipeline exclusion:** The pipeline model accumulates data into `ctx.sources` before returning a complete response. Streaming bypasses this — the handler yields chunks as they are produced. Combining the two models creates ambiguity about when pipeline stages execute relative to yielded chunks. Streaming views are the handler and nothing else.

### 3.3 Streaming view request flow

```
HTTP Request (any method — GET, POST, PUT, PATCH)
    │
    ▼
Router  (matches path + method to ApiViewConfig)
    │
    ▼
Middleware stack  (rate limit, auth, CORS, backpressure, trace)
    │
    ▼
ProcessPool dispatch  (streaming = true → StreamingTaskContext)
    │
    ▼
CodeComponent executes as AsyncGenerator
    │
    ├─ First yield → response headers committed (200 OK + Content-Type)
    │              → Transfer-Encoding: chunked enabled
    │
    ├─ Each subsequent yield → chunk flushed to client immediately
    │
    ├─ Generator return → stream closed cleanly
    │
    └─ Generator throw → poison chunk emitted → stream closed
```

Headers are not committed until the first chunk is yielded. If the handler throws before yielding anything, a standard 500 response is returned — no poison chunk needed, since no streaming has started.

### 3.4 Backpressure handling

The stream write is async. If the client is consuming slowly, the write await blocks. The backpressure semaphore permit (from the HTTPD backpressure middleware) is held for the full stream lifetime — the same as SSE and WebSocket connections.

A slow consumer that causes the write buffer to fill will block the handler at the next `yield`. The `stream_timeout_ms` watchdog covers total lifetime, not individual write wait, so a slow consumer cannot cause a watchdog termination independently of the stream lifetime.

---

## 4. ProcessPool Runtime Changes

### 4.1 New handler contract

Streaming CodeComponent handlers return an `AsyncGenerator` instead of `Promise<Rivers.Response>`:

```typescript
// Non-streaming — unchanged
export async function handler(
    req: Rivers.Request
): Promise<Rivers.Response> { }

// Streaming
export async function* handler(
    req: Rivers.Request
): AsyncGenerator<any> {
    yield { token: "Hello" };
    yield { token: " world" };
    yield { done: true };
}
```

The yielded value can be any JSON-serializable value. Rivers serializes it to the declared wire format. The handler does not set `Content-Type`, status code, or headers — those are owned by Rivers.

If the handler needs to communicate a final summary alongside the last chunk, yield it as the last value before returning:

```typescript
export async function* generate(
    req: Rivers.Request
): AsyncGenerator<any> {
    let total = 0;
    for await (const token of callLLM(req.body.prompt)) {
        yield { token };
        total++;
    }
    yield { done: true, total_tokens: total };
}
```

### 4.2 StreamingTaskContext

When the ProcessPool dispatches a streaming task, it uses `StreamingTaskContext` instead of `TaskContext`:

```rust
pub struct StreamingTaskContext {
    pub request:           ParsedRequest,
    pub resources:         HashMap<String, ResourceToken>,
    pub stream_timeout_ms: u64,
    pub format:            StreamingFormat,
    pub trace_id:          String,
}
```

The resource token model is unchanged — the handler calls `Rivers.db.query(token, ...)` the same way. The only differences are timeout (`stream_timeout_ms` instead of `task_timeout_ms`) and the execution model (generator drive loop instead of `Promise` await).

### 4.3 Generator drive loop

The ProcessPool drives the generator via a host-side loop:

```
loop:
    call generator.next()
    if done → break
    serialize yielded value to wire format
    write chunk to response body (async, blocks on backpressure)
    if write error → terminate generator, log, exit
    check watchdog → if stream_timeout_ms exceeded → TerminateExecution
```

The watchdog check is per-iteration, not per-wall-clock interval. This means the timeout is enforced at each yield boundary. A handler that hangs inside a yield (e.g., blocked on an external HTTP call) is covered by the watchdog thread, which calls `v8::Isolate::TerminateExecution()` independently of the drive loop.

### 4.4 Rivers API surface for streaming handlers

All existing `Rivers.*` APIs are available in streaming handlers — `Rivers.db`, `Rivers.view`, `Rivers.http` (if `allow_outbound_http = true`), `Rivers.log`. No new APIs are required.

The handler does not have access to a response object — there is no `Rivers.response` to set headers or status codes. Status is always 200 (committed on first chunk). If the handler needs to signal a non-200 outcome before yielding, it should throw before the first yield — this results in a standard 500 response delivered before streaming begins.

### 4.5 Isolate reuse for streaming handlers

Streaming handlers follow the same isolate reuse policy as non-streaming handlers (Open Question #2 in the processpool spec). Whichever policy is chosen applies uniformly — there is no streaming-specific exception.

### 4.6 Pool config additions

`stream_timeout_ms` is added to pool config alongside `task_timeout_ms`:

```toml
[runtime.process_pools.default]
worker_count       = 4
task_timeout_ms    = 5000     # non-streaming handlers
stream_timeout_ms  = 120000   # streaming handlers — full generator lifetime
memory_limit_mb    = 128
```

`stream_timeout_ms` defaults to `120000` (2 minutes) when not specified. This is intentionally generous — streaming handlers are expected to run for the duration of an inference call or document processing job. Operators tune this per pool based on their workload.

If a streaming handler exceeds `stream_timeout_ms`, the watchdog calls `v8::Isolate::TerminateExecution()`. The drive loop detects termination, emits a poison chunk, and closes the stream.

---

## 5. HTTPD Changes

### 5.1 Transfer-Encoding

Streaming responses use chunked transfer encoding. Rivers sets `Transfer-Encoding: chunked` automatically when the first chunk is flushed. This is handled by the Axum response body type — an `axum::body::Body` constructed from a `tokio::sync::mpsc::Receiver<Bytes>` or equivalent async stream produces chunked encoding automatically when TLS or HTTP/2 is not in use. Under HTTP/2, DATA frames replace chunked encoding transparently.

### 5.2 Cache-Control

Rivers adds `Cache-Control: no-cache, no-store` to all streaming responses. Streaming responses must not be cached by intermediaries.

For SSE-format streaming responses, Rivers additionally adds `X-Accel-Buffering: no` to disable Nginx proxy buffering when the server sits behind Nginx.

### 5.3 Backpressure semaphore

The backpressure semaphore permit is acquired before the handler begins and released when the stream closes (clean, error, or timeout). This is identical to the existing SSE and WebSocket behavior documented in the HTTPD spec (§8). The effect is that a long-running streaming handler holds a backpressure slot for its full duration. Operators should size `queue_depth` with long-running AI workloads in mind, or use a dedicated process pool with its own concurrency limits.

### 5.4 Connection handling

If the client disconnects during streaming, the next write to the response body returns an error. The drive loop detects this, logs it as `"client disconnected during stream"`, terminates the generator, and exits. No poison chunk is emitted — the client is gone.

The watchdog is not involved in client disconnect detection. The drive loop's write-error path is the detection mechanism.

### 5.5 Body size limit

The global `DefaultBodyLimit::max(16 MiB)` applies to the **request** body. There is no limit on the **response** body for streaming views — chunks are flushed as they are produced and are never accumulated.

---

## 6. Error Handling Mid-Stream

### 6.1 Before first yield

If the handler throws before yielding any chunk, no streaming has begun. Headers have not been committed. Rivers returns a standard HTTP 500 response with a JSON error body:

```json
{
    "error": "handler initialization failed",
    "error_type": "HandlerError",
    "trace_id": "trace-abc-123"
}
```

This is identical to non-streaming handler error behavior.

### 6.2 After first yield — poison chunk (ndjson)

Once the first chunk has been yielded, the response headers are committed (200 OK, `Content-Type: application/x-ndjson`). If the generator subsequently throws, Rivers emits a poison chunk as the final line in the stream:

```
{"error":"<message>","error_type":"<type>","stream_terminated":true}\n
```

Then closes the connection.

The `stream_terminated` field is the signal. Clients MUST inspect every chunk for this field. A valid non-error chunk from the application MUST NOT include `stream_terminated`. Rivers validates this at dispatch — if the yielded value contains a `stream_terminated` key, it is an error.

Error types:

| `error_type` | Cause |
|---|---|
| `HandlerError` | Generator threw a JavaScript exception |
| `TimeoutError` | `stream_timeout_ms` exceeded |
| `MemoryError` | Memory limit exceeded in isolate |
| `WriteError` | Response body write failed (unexpected — client disconnect is handled separately) |

### 6.3 After first yield — error event (SSE format)

For SSE-format streams, the terminal error is delivered as a named event:

```
event: error\n
data: {"error":"<message>","error_type":"<type>"}\n\n
```

Clients subscribe to the `error` event type via `eventSource.addEventListener('error', ...)` or by parsing the `event:` field. The connection closes after this event is flushed.

### 6.4 Watchdog termination

When the watchdog calls `v8::Isolate::TerminateExecution()`:

1. The generator terminates with a `TerminationException`
2. The drive loop catches it (host-side, outside the isolate)
3. If before first yield → standard 500 response
4. If after first yield → poison chunk with `error_type: "TimeoutError"` emitted
5. Connection closed
6. `on_timeout` observer fires if declared — but `on_timeout` is not supported on streaming views (pipeline is excluded). The timeout is logged and a `ViewTimeout` internal event is emitted for observability.

---

## 7. Timeout Semantics

### 7.1 `task_timeout_ms` vs `stream_timeout_ms`

| Config | Applies to | Meaning |
|---|---|---|
| `task_timeout_ms` | Non-streaming CodeComponent handlers | Total wall clock time from dispatch to `Promise` resolution |
| `stream_timeout_ms` | Streaming CodeComponent handlers | Total wall clock time from dispatch to generator exhaustion or close |

They are independent. A pool may have different values for each. A non-streaming view on the same pool uses `task_timeout_ms`. A streaming view uses `stream_timeout_ms`.

### 7.2 No time-to-first-chunk timeout

There is no separate time-to-first-chunk timeout in this spec. The `stream_timeout_ms` covers the full lifetime from dispatch. If an operator wants to enforce time-to-first-chunk, they can do so at the load balancer or reverse proxy layer (e.g., Nginx `proxy_read_timeout`).

This is a deliberate simplification. Time-to-first-chunk requires a second watchdog state machine and adds complexity without being universally necessary. Operators with strict TTFC requirements are likely already using a proxy.

---

## 8. Validation Rules

Enforced at config load time. Failures reported as `RiversError::Validation` before the server binds.

| Rule | Error message |
|---|---|
| `streaming = true` on non-REST view | `streaming is only valid for view_type=rest` |
| `streaming = true` with DataView handler | `streaming requires a codecomponent handler` |
| `streaming = true` with `event_handlers` declared | `streaming views do not support handler pipeline stages` |
| `streaming_format` set without `streaming = true` | `streaming_format has no effect when streaming = false` |
| Yielded value contains `stream_terminated` key | `yielded value must not contain reserved key 'stream_terminated'` |
| `stream_timeout_ms = 0` | `stream_timeout_ms must be greater than 0` |

---

## 9. Configuration Reference

### 9.1 Streaming REST view — ndjson

```toml
[api.views.generate]
path              = "/api/generate"
method            = "POST"
view_type         = "Rest"
streaming         = true
streaming_format  = "ndjson"     # default — may be omitted

[api.views.generate.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/llm.ts"
entrypoint = "generate"
resources  = ["llm_cache", "usage_db"]
```

Optional pool assignment for streaming-specific timeout:

```toml
[api.views.generate]
process_pool = "llm_pool"     # uses llm_pool.stream_timeout_ms

[runtime.process_pools.llm_pool]
worker_count       = 8
task_timeout_ms    = 5000
stream_timeout_ms  = 120000
memory_limit_mb    = 256
```

### 9.2 Streaming REST view — SSE format

```toml
[api.views.stream_analysis]
path              = "/api/analyse"
method            = "POST"
view_type         = "Rest"
streaming         = true
streaming_format  = "sse"

[api.views.stream_analysis.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/analysis.ts"
entrypoint = "streamAnalysis"
resources  = ["documents_db"]
```

---

## 10. Examples

### 10.1 LLM token streaming (ndjson)

```typescript
// handlers/llm.ts
export async function* generate(
    req: Rivers.Request
): AsyncGenerator<any> {
    const { prompt, model } = req.body as { prompt: string; model: string };

    Rivers.log.info("starting generation", { model, trace_id: req.trace_id });

    const stream = await Rivers.http.post(
        "https://api.anthropic.com/v1/messages",
        {
            headers: { "x-api-key": await Rivers.lockbox.get("anthropic_key") },
            body: {
                model,
                max_tokens: 1024,
                stream: true,
                messages: [{ role: "user", content: prompt }]
            },
            stream: true
        }
    );

    let total_tokens = 0;

    for await (const event of stream.events()) {
        if (event.type === "content_block_delta") {
            yield { token: event.delta.text };
            total_tokens++;
        }
    }

    yield { done: true, total_tokens };
}
```

Client receives:
```
{"token":"The"}\n
{"token":" answer"}\n
{"token":" is"}\n
{"token":" 42"}\n
{"done":true,"total_tokens":4}\n
```

### 10.2 Document processing pipeline (SSE format)

```typescript
// handlers/analysis.ts
export async function* streamAnalysis(
    req: Rivers.Request
): AsyncGenerator<any> {
    const { document_id } = req.body as { document_id: string };

    // Fetch doc
    yield { stage: "fetch", status: "started" };
    const rows = await Rivers.view.query("get_document", { id: document_id });
    const doc = rows[0];
    yield { stage: "fetch", status: "done", pages: doc.page_count };

    // Chunk
    yield { stage: "chunk", status: "started" };
    const chunks = splitIntoChunks(doc.content, 512);
    yield { stage: "chunk", status: "done", chunk_count: chunks.length };

    // Embed each chunk
    for (let i = 0; i < chunks.length; i++) {
        const embedding = await Rivers.http.post(
            "https://api.openai.com/v1/embeddings",
            { body: { input: chunks[i], model: "text-embedding-3-small" } }
        );
        await Rivers.db.query(
            Rivers.resources.VectorDB,
            "INSERT INTO embeddings (doc_id, chunk_idx, vector) VALUES ($1, $2, $3)",
            [document_id, i, embedding.data[0].embedding]
        );
        yield { stage: "embed", chunk: i, total: chunks.length, pct: Math.round((i + 1) / chunks.length * 100) };
    }

    yield { stage: "complete", document_id, chunks_stored: chunks.length };
}
```

Client receives SSE events:
```
data: {"stage":"fetch","status":"started"}\n\n
data: {"stage":"fetch","status":"done","pages":12}\n\n
data: {"stage":"chunk","status":"started"}\n\n
data: {"stage":"chunk","status":"done","chunk_count":48}\n\n
data: {"stage":"embed","chunk":0,"total":48,"pct":2}\n\n
...
event: error\ndata: {"error":"upstream embedding API failed","error_type":"HandlerError"}\n\n
```

### 10.3 Error before first yield

```typescript
export async function* generate(
    req: Rivers.Request
): AsyncGenerator<any> {
    const { prompt } = req.body as { prompt: string };

    if (!prompt || prompt.length === 0) {
        // Throw before first yield → standard 500 response, no streaming begins
        throw new Error("prompt is required");
    }

    yield { token: "Starting..." };
    // ...
}
```

Client receives a standard non-streaming 500:
```json
{"error": "prompt is required", "error_type": "HandlerError", "trace_id": "..."}
```

### 10.4 Error after first yield (ndjson)

```typescript
export async function* generate(
    req: Rivers.Request
): AsyncGenerator<any> {
    yield { token: "Starting..." };          // committed — 200 on wire

    const data = await Rivers.http.get("https://api.example.com/model");
    if (!data.ok) {
        throw new Error("upstream model API unavailable");
        // Rivers catches this → emits poison chunk → closes connection
    }

    yield { token: "Done" };
}
```

Client receives:
```
{"token":"Starting..."}\n
{"error":"upstream model API unavailable","error_type":"HandlerError","stream_terminated":true}\n
```
