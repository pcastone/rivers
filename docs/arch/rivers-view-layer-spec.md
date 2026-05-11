# Rivers View Layer Specification

**Document Type:** Implementation Specification  
**Scope:** View types, handler pipeline, routing, WebSocket, SSE, MessageConsumer, GraphQL  
**Status:** Reference / Ground Truth  
**Source audit:** `crates/rivers-core/src/config.rs`, `crates/riversd/src/handler_pipeline.rs`, `crates/riversd/src/lib.rs`

---

## Table of Contents

1. [Architectural Overview](#1-architectural-overview)
2. [View Types](#2-view-types)
3. [Handler Definitions](#3-handler-definitions)
4. [Handler Pipeline](#4-handler-pipeline)
5. [REST Views](#5-rest-views)
6. [WebSocket Views](#6-websocket-views)
7. [Server-Sent Events Views](#7-server-sent-events-views)
8. [MessageConsumer Views](#8-messageconsumer-views)
9. [GraphQL](#9-graphql)
10. [Per-View Rate Limiting](#10-per-view-rate-limiting)
11. [EventBus Integration](#11-eventbus-integration)
12. [Configuration Reference](#12-configuration-reference)
13. [Validation Rules](#13-validation-rules)

---

## 1. Architectural Overview

A View is a named, routed endpoint. Every View has exactly one `view_type`, exactly one `handler` (either a DataView reference or a CodeComponent), and an optional handler pipeline that wraps the primary execution.

```
Incoming request (HTTP / WS upgrade / EventBus event)
    │
    ▼
Router  (matches path + method to ApiViewConfig)
    │
    ▼
Middleware stack  (rate limit, auth, CORS, backpressure, trace)
    │
    ▼
View Handler
    │
    ├─ [on_session_valid]  — session handler, runs after session validation
    ├─ [pre_process]       — observer, fire-and-forget
    ├─ [on_request]        — accumulator, deposits into ctx.sources
    ├─  Primary execution  (DataView or CodeComponent)
    │     result → ctx.sources["primary"]
    ├─ [transform]         — chained pipeline, shapes data
    ├─ [on_response]       — accumulator, merges/collapses sources
    └─ [post_process]      — observer, fire-and-forget
    │
    ▼
HTTP Response / WS frame / SSE event
```

`on_session_valid` fires after session validation succeeds and before any other pipeline stage. Session identity is available in all subsequent stages including `pre_process` observers. Position is configurable via `session_stage` — valid values: `"before_pre_process"` (default) or `"after_on_request"`. `session_stage` is an advanced option; the default is correct for almost all use cases.

`on_session_valid` is skipped entirely on `auth = "none"` views and MessageConsumer views with no `auth` declaration.

The View layer never accesses drivers or pools directly. All data access goes through DataView names (resolved by the DataView engine) or through resource handles passed to CodeComponent handlers.

---

## 2. View Types

```rust
pub enum ApiViewType {
    Rest,
    Websocket,
    ServerSentEvents,
    MessageConsumer,
    Mcp,
}
```

| Type | Transport | Direction | HTTP method constraint |
|---|---|---|---|
| `Rest` | HTTP | Request → Response | Any |
| `Websocket` | WS upgrade | Bidirectional | GET only |
| `ServerSentEvents` | HTTP long-lived | Server → Client | GET only |
| `MessageConsumer` | EventBus event | Event → Handler | No HTTP route |
| `Mcp` | HTTP (JSON-RPC + SSE) | Bidirectional via session | POST only — see `rivers-mcp-view-spec.md` |

`view_type` is a closed enum at structural validation. Values outside the
list above emit `S005` with a did-you-mean hint (Sprint 2026-05-09 Track 2).
The same hardening applies to `auth` — accepted values are `"none"` and
`"session"` only. Bearer-token authentication is not a built-in `auth`
mode; use the `guard_view` named-guard recipe in `rivers-auth-session-spec.md`
§11.5.

---

## 3. Handler Definitions

Every View specifies exactly one handler. The handler determines what executes as the primary data step inside the pipeline.

```rust
pub enum HandlerDefinition {
    DataView { dataview: String },
    CodeComponent {
        language: String,      // "javascript" | "js" | "typescript" | "ts" | "wasm"
        module: String,        // path within the App Bundle
        entrypoint: String,    // exported function name
        resources: Vec<String>, // datasource names this component may access
    },
}
```

### 3.1 DataView handler

References a named DataView from `config.data.dataviews`. The DataView engine handles parameter resolution, caching, pool acquire, execution, and schema validation. The View layer passes query parameters from the request into the DataView request.

Only valid for `view_type = Rest`. WebSocket, SSE, and MessageConsumer views require a CodeComponent.

### 3.2 CodeComponent handler

A sandboxed JS/TS/WASM module executed in the ProcessPool. The handler receives the full request context and declared resource handles. It is responsible for all data access and response construction.

`resources` is a static declaration — the complete set of datasources the component may access. The runtime validates this list before dispatch. A component that attempts to access an undeclared datasource gets a `CapabilityError`, not a runtime exception.

`language` maps to a runtime alias in `RuntimeFactory`. Valid aliases: `javascript`, `js`, `typescript`, `ts`, `typescript_strict`, `ts_strict`, `wasm`, and V8-explicit variants `javascript_v8`, `js_v8`, `typescript_v8`, `ts_v8`.

---

## 4. Handler Pipeline

Defined in `crates/rivers-core/src/handler_pipeline.rs` (types) and `crates/riversd/src/handler_pipeline.rs` (execution).

### 4.1 The boundary rule

**Inside the lifecycle → has a return value → can affect data.**  
**Outside the lifecycle → observer only → void, fire-and-forget.**

This boundary never moves. Observers cannot block or modify the response. Accumulators cannot affect infrastructure behavior.

```
Outside lifecycle                    Inside lifecycle                     Outside lifecycle
      │                                    │                                    │
[pre_process]  →  [on_request]  →  [Primary]  →  [transform]  →  [on_response]  →  [post_process]
   observer         accumulator     execute        pipeline         accumulator        observer
    void           {key,data}|null             TransformResult    {key,data}|null       void
```

### 4.2 ViewContext

Passed through the entire pipeline. Accumulates as handlers deposit data.

```rust
pub struct ViewContext {
    pub request: ParsedRequest,
    pub sources: HashMap<String, serde_json::Value>,
    pub meta: HashMap<String, serde_json::Value>,
    pub trace_id: String,
}
```

`sources["primary"]` is reserved for the DataView or CodeComponent output. All other keys are declared by `on_request` and `on_response` handler configs.

### 4.3 ParsedRequest

```rust
pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub query_params: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
    pub path_params: HashMap<String, String>,
}
```

### 4.4 Stage types

#### pre_process — Observer (outside lifecycle)

Runs before any data access. Fire-and-forget. Result is discarded. Errors are logged; the pipeline continues regardless. Typical uses: audit logging, inbound metrics, rate-limit pre-check side effects.

```typescript
function handler(ctx: ViewContext): void { }
```

#### on_request — Accumulator (inside lifecycle)

All declared stages execute sequentially in declaration order. Each deposits data into `ctx.sources[key]`. No short-circuit — a failing stage deposits nothing and logs, but subsequent stages still run. Typical uses: fetching foreign data (pricing, inventory, permissions) that will be merged with primary results. <!-- SHAPE-12 amendment: parallel removed -->

```typescript
interface OnRequestResult { key: string; data: any; }
function handler(ctx: ViewContext): OnRequestResult | null { }
```

#### Primary execution (DataView or CodeComponent)

Result is deposited into `ctx.sources["primary"]`. Skipped if `datasource = "none"` — in that case `on_request` handlers are the sole data providers, and `ctx.sources["primary"]` starts as `[]`.

#### transform — Pipeline (inside lifecycle)

Chained — output of stage N is input of stage N+1. Each stage receives `TransformContext` (current data + read-only sources + read-only request). Cannot deposit new keys into sources. On failure, behavior is per-stage: `abort` returns an error to the caller, `skip` passes the current data through unchanged.

```typescript
interface TransformContext {
    data: any;                // output of previous transform (or primary execution)
    sources: ViewSources;     // read-only
    request: ParsedRequest;   // read-only
}
interface TransformResult { data: any; }
function handler(ctx: TransformContext): TransformResult { }
```

#### on_response — Accumulator (inside lifecycle)

Same contract as `on_request` but runs after primary execution and transforms. Typical uses: collapsing multiple sources into a single response shape, fetching additional data based on primary results, applying response-time enrichment.

```typescript
interface OnResponseResult { key: string; data: any; }
function handler(ctx: ViewContext): OnResponseResult | null { }
```

#### post_process — Observer (outside lifecycle)

Runs after the response has been determined. Fire-and-forget. Sees final `ctx` including all accumulated sources. Typical uses: response metrics, audit trail, cache warming side effects.

```typescript
function handler(ctx: ViewContext): void { }
```

#### on_error — Observer (outside lifecycle)

Fires when the primary execution or a transform returns an error. Receives `ErrorContext` — `ViewContext` plus `error`, `error_type`, and `status_code`.

```typescript
interface ErrorContext extends ViewContext {
    error: string;
    error_type: string;
    status_code: number;
}
function handler(ctx: ErrorContext): void { }
```

#### on_timeout — Observer (outside lifecycle)

Fires when the pipeline exceeds the configured execution timeout. Receives `TimeoutContext` — `ViewContext` plus `elapsed_ms` and `timeout_ms`.

```typescript
interface TimeoutContext extends ViewContext {
    elapsed_ms: number;
    timeout_ms: number;
}
function handler(ctx: TimeoutContext): void { }
```

### 4.5 HandlerStageConfig

```rust
pub struct HandlerStageConfig {
    pub module: String,
    pub entrypoint: String,
    pub key: String,               // for accumulators — where data lands in ctx.sources
    pub on_failure: TransformFailureAction,  // Abort | Skip (transform only)
}
```
<!-- SHAPE-12 amendment: parallel field removed -->

### 4.6 Sequential execution

All pipeline stages run sequentially in declaration order, always. The `parallel` option has been removed. Each stage sees the results of all prior stages. <!-- SHAPE-12 amendment -->

```toml
on_request = [
    { module = "h/pricing.ts",   entrypoint = "fetch", key = "pricing" },
    { module = "h/inventory.ts", entrypoint = "fetch", key = "inventory" },
    { module = "h/audit.ts",     entrypoint = "log",   key = "" }
]
```

Stages execute in order. The inventory stage sees `ctx.sources["pricing"]`. The audit stage sees both.

### 4.7 Null datasource pattern

```toml
[api.views.live_pricing]
datasource = "none"
```

The DataView execution step is skipped entirely. `ctx.sources["primary"]` starts as `[]`. `on_request` handlers are the sole data providers. The pipeline continues normally through transform and on_response stages.

### 4.8 Data collapse pattern

`on_response` is the idiomatic place to collapse multiple sources:

```typescript
// ctx.sources["primary"]   = order record from DataView
// ctx.sources["customer"]  = from on_response handler 1
// ctx.sources["shipping"]  = from on_response handler 2
// transform then reduces all to the final response shape
function buildOrderResponse(ctx: TransformContext): TransformResult {
    return {
        data: {
            order:    ctx.sources.primary,
            customer: ctx.sources.customer,
            shipping: ctx.sources.shipping,
        }
    };
}
```

---

## 5. REST Views

The default view type. Maps HTTP method + path to a handler.

### 5.1 ApiViewConfig fields relevant to REST

```rust
pub struct ApiViewConfig {
    pub view_type: ApiViewType,    // Rest
    pub path: String,              // "/api/orders/{id}"
    pub method: String,            // "GET" | "POST" | "PUT" | "DELETE" | "PATCH"
    pub handler: HandlerDefinition,
    pub event_handlers: Option<ViewEventHandlers>,
    pub rate_limit_per_minute: Option<u32>,
    pub rate_limit_burst_size: Option<u32>,
    pub middleware: Vec<String>,
}
```

### 5.2 Parameter mapping

Path parameters (`{id}` in path pattern) are extracted and made available in `ctx.request.path_params`. Query string parameters land in `ctx.request.query_params`. Request body (for POST/PUT) is in `ctx.request.body` as `serde_json::Value`.

For DataView handlers, parameter mapping is declared explicitly in config — each DataView parameter maps to a request source (path, query, body field).

### 5.3 Response serialization

The final value in `ctx.sources["primary"]` after all pipeline stages is serialized to JSON and returned. Status code is 200 by default. CodeComponent handlers can return explicit `{ status, headers, body }` envelopes. DataView handlers always return 200 with the `QueryResult` rows serialized.

### 5.4 Static response headers (CB-P1.11)

Every view type — REST, WebSocket, SSE, MCP — supports an optional
`[api.views.*.response_headers]` table. Entries are appended to every
HTTP response from the view after handler-set headers; **handler
overrides win** when the same name is set on both sides.

```toml
[api.views.legacy_mcp.response_headers]
"Deprecation" = "true"
"Sunset"      = "Wed, 31 Dec 2026 23:59:59 GMT"
"Link"        = "</mcp/advisor>; rel=\"successor-version\""
```

**Validation rules** (Layer 1, structural):

- Header names match RFC 7230 token grammar: alphanumerics + `-`.
- Header values must be ASCII-printable (`\x20`–`\x7E`); no control
  characters.
- The framework manages four header names — they are rejected at
  bundle-load time with `S005`: `Content-Type`, `Content-Length`,
  `Transfer-Encoding`, `Mcp-Session-Id`.

**Runtime semantics:**

- Applied in `combined_fallback_handler` once per request — the same
  intercept point covers all view types.
- A configured header is inserted only if the response does not already
  carry one with the same name (case-insensitive). This preserves
  handler intent: if a handler sets `Cache-Control: no-store`, a
  configured `Cache-Control: max-age=60` is dropped.
- Defense-in-depth: any entry that survives validation but trips axum's
  stricter runtime parser is logged at WARN and skipped — failure to
  attach a deprecation header never turns a 200 into a 500.

---

## 6. WebSocket Views

### 6.1 Constraints

- `view_type = Websocket` requires `method = "GET"` — HTTP GET triggers the upgrade
- `handler` must be `CodeComponent` (DataView handlers are not valid for WebSocket views)
- `on_stream` handler required for bidirectional messaging (see 6.3)

### 6.2 WebSocket mode

```rust
pub enum WebSocketMode {
    Broadcast,  // default — shared hub, all connections on this route share one channel
    Direct,     // per-connection routing — each connection has its own message path
}
```

`Broadcast` mode: all active connections on the route receive every message published to the hub. Suitable for chat rooms, live dashboards, feed subscriptions.

`Direct` mode: messages are routed to specific connection IDs. The `ConnectionRegistry` maps connection ID → sender. Suitable for session-specific push, private notifications.

### 6.3 on_stream handler

Inbound WebSocket client messages are routed through the EventBus to the `on_stream` handler. The DataView config must declare `on_stream` for the view to accept client messages:

```rust
pub struct OnStreamConfig {
    pub module: String,
    pub entrypoint: String,
    pub handler_mode: HandlerMode,
}

pub enum HandlerMode {
    Stream,  // always invoked for each message
    Normal,  // invoked for structured request-shaped messages
    Auto,    // Rivers infers mode from message shape
}
```

SSE views must NOT declare `on_stream` — they are unidirectional server → client only.

### 6.4 Connection limits

```rust
pub max_connections: Option<usize>
```

When the limit is reached, new upgrade requests receive `503 Service Unavailable`. Active connection count is tracked per route via atomic counter. Decrement on disconnect uses `saturating_sub`.

### 6.5 Rate limiting

WebSocket rate limiting is token bucket per connection.

```rust
pub rate_limit_per_minute: Option<u32>  // maps to messages_per_sec internally
pub rate_limit_burst_size: Option<u32>
```

Defaults when rate limiting is enabled but values are not specified: `messages_per_sec = 100`, `burst = 20`.

### 6.6 Lag handling

`RecvError::Lagged` on the broadcast receiver is handled explicitly — log warning and continue (the connection is not closed). `RecvError::Closed` breaks the connection loop cleanly. This prevents a slow consumer from being silently dropped without diagnostic output.

### 6.7 Binary frames

<!-- SHAPE-13 amendment: rate-limited logging for binary frames -->
Rivers WebSocket views operate on text frames only. `Message::Binary` frames are handled with rate-limited logging to avoid log flooding:

1. First binary frame per connection: logged as `WARN` with connection ID and view name
2. Subsequent binary frames: increment an internal counter (no log per frame)
3. Every 60 seconds, if counter > 0: emit a summary `WARN` log with the count and reset the counter

Binary frames are discarded after logging. `Message::Ping` and `Message::Pong` are handled by the WS layer transparently.

### 6.8 Session revalidation on persistent connections

WebSocket connections are long-lived. A session validated at connect time may expire or be invalidated (logout from another tab, admin revocation) while the connection remains open. Rivers does not terminate the connection on expiry by default — the session was valid at connect time.

For views that require ongoing session validity, declare `session_revalidation_interval_s`:

```toml
[api.views.live_feed]
view_type                       = "Websocket"
session_revalidation_interval_s = 300   # re-check session every 5 minutes
```

Rivers re-validates the session against StorageEngine at the configured interval. On failure (session expired or destroyed), Rivers sends a `{ "rivers_session_expired": true }` JSON frame and closes the connection cleanly. The client is expected to redirect to the guard view.

`session_revalidation_interval_s = 0` is the default — session is validated at connect time only.

### 6.9 Datasource capability propagation (CB-P1.13)

WebSocket codecomponent handlers (`on_stream`, `on_connect`,
`on_message`, `on_disconnect`) receive the same per-app datasource set
as REST and MCP handlers — calls to `Rivers.db.execute('<datasource>',
...)` resolve against the app's datasource declarations identically.
The dispatch path snapshots `ctx.dataview_executor` once per message
and threads it through `task_enrichment::wire_datasources` before the
handler runs.

The handler also sees `ctx.app_id` as the entry-point slug (e.g.
`cb-service`), not the manifest UUID. Same convention as REST and MCP.

---

## 7. Server-Sent Events Views

### 7.1 Constraints

- `view_type = ServerSentEvents` requires `method = "GET"`
- `handler` must be `CodeComponent`
- `on_stream` must NOT be declared — SSE is server → client only

### 7.2 Push model

SSE views use a hybrid push model controlled by two config fields:

```rust
pub sse_tick_interval_ms: u64,          // default: 1000. 0 = pure event-driven
pub sse_trigger_events: Vec<String>,    // EventBus event types that trigger immediate push
```

The SSE handler loop uses `tokio::select!` between:
- The tick timer (fires every `sse_tick_interval_ms` milliseconds)
- Any `sse_trigger_events` event arriving on the EventBus

On either trigger, the CodeComponent handler executes and its output is pushed as an SSE event. This eliminates unnecessary polling for event-sourced views while preserving a heartbeat tick option.

### 7.3 Connection health

The SSE loop checks `tx.is_closed()` at the top of each iteration before executing the callback. A closed channel (client disconnect) exits the loop cleanly without executing the handler.

### 7.4 Session revalidation on persistent connections

Same model as WebSocket (§6.8). Declare `session_revalidation_interval_s` to re-check session validity at an interval. On failure, Rivers sends a final SSE event `data: {"rivers_session_expired":true}` and closes the stream. Default is 0 — connect-time validation only.

```toml
[api.views.notifications]
view_type                       = "ServerSentEvents"
session_revalidation_interval_s = 600   # re-check every 10 minutes
```

### 7.5 Datasource capability propagation (CB-P1.13)

SSE codecomponent handlers receive the same per-app datasource set as
REST and MCP handlers — same convention as WebSocket §6.9. The
push-loop dispatch path passes the executor + entry-point slug into
`task_enrichment::wire_datasources` before each handler tick fires, so
`Rivers.db.execute('<datasource>', ...)` resolves identically to a
REST handler in the same app.

`ctx.app_id` is the entry-point slug (matches REST/WS/MCP).

---

## 8. MessageConsumer Views

### 8.1 Purpose

`MessageConsumer` views are driven by EventBus events originating from the `BrokerConsumerBridge`. They have no HTTP route — they are not accessible via HTTP. They process broker messages asynchronously.

### 8.2 Constraints

- No HTTP route is registered for `MessageConsumer` views
- `handler` must be `CodeComponent`
- `event_name` must match an event published by the configured broker datasource

### 8.3 on_event config

```rust
pub struct OnEventConfig {
    pub topic: String,          // EventBus topic / event name
    pub handler: String,        // module path
    pub handler_mode: HandlerMode,
}
```

The CodeComponent handler receives the EventBus event payload as the request body. It can access declared datasources via resource handles. Return value is not delivered to an HTTP client — it is either discarded or used to publish a follow-on event.

### 8.4 HTTP response

Requests to a `MessageConsumer` route path (if somehow reached) receive `400 Bad Request` with body: `{"error": "event-driven view — not accessible via HTTP"}`.

---

## 9. GraphQL

### 9.1 Integration

`async-graphql` integrated with Axum. Single GraphQL endpoint at the configured path (default `/graphql`).

### 9.2 Schema generation

Schema is generated from DataView configurations at startup. Each DataView with a `return_schema` generates a corresponding GraphQL object type. DataViews without return schemas generate generic scalar response types.

### 9.3 Resolver bridge

GraphQL resolvers delegate to the DataView engine. A GraphQL query field maps to a DataView name. Arguments map to DataView parameters with the same type conversion as REST parameter mapping.

### 9.4 Limitations

- Mutations require a CodeComponent resolver, not a DataView
- Subscriptions route through the EventBus
- Schema introspection is enabled by default; disable via config for production

---

## 10. Per-View Rate Limiting

Rate limiting is configurable at two levels:

**Global** — `config.security.rate_limit_per_minute`, `rate_limit_burst_size`, `rate_limit_strategy` (IP, custom header). Applies to all views.

**Per-view** — `ApiViewConfig.rate_limit_per_minute` and `rate_limit_burst_size`. Overrides global for that specific view.

For WebSocket views, per-view rate limiting applies per connection (token bucket on inbound messages). For REST and SSE views, it applies per IP (or custom header value).

Rate limit strategy options:

```rust
pub enum RateLimitStrategy {
    Ip,            // default — uses remote IP
    CustomHeader,  // uses value from a declared header
}
```

`CustomHeader` requires `rate_limit_custom_header` to be set. It is used for API keys or other client identifiers passed via header.

---

## 11. EventBus Integration

### 11.1 TopicRegistry

<!-- SHAPE-17 amendment: topic validation dropped, EventBus is a dumb pipe -->
Per-topic `tokio::sync::broadcast` channels managed by `TopicRegistry`. Topics are created on demand when first published to — no upfront registration or validation is required. The EventBus is a dumb pipe; any topic string is valid. Each topic has a configurable buffer size (defaults apply for auto-created topics).

Backend selection per topic:
- `Memory` — in-process broadcast only
- `Broker` — passthrough to a configured broker datasource

Cross-node delivery via `GossipPayload::EventBusEvent` + `GossipEffect::EventBusPublish`. When a node publishes to a topic, it gossips the event to peers, which re-publish locally.

### 11.2 ConnectionRegistry

Tracks active WebSocket and SSE connections. Lookup by DataView name or node ID. Used by Direct mode WebSocket for targeted message delivery.

### 11.3 Deploy-time validation

<!-- SHAPE-17 amendment: topic existence validation removed -->
At deploy time, 5 rules are enforced against EventBus view configs:

1. ~~Referenced topic must exist in `TopicRegistry`~~ — **removed** (topics are created on demand)
2. `datasource` field (if set) must reference an `eventbus` driver datasource
3. `on_event.handler` module must exist in the App Bundle
4. Broker passthrough datasource (if topic uses Broker backend) must be a valid broker datasource
5. `MessageConsumer` views must have `on_event` config

---

## 12. Configuration Reference

### 12.1 REST view — DataView handler

```toml
[api.views.get_order]
path       = "/api/orders/{id}"
method     = "GET"
view_type  = "Rest"

[api.views.get_order.handler]
type     = "dataview"
dataview = "get_order"           # references data.dataviews.get_order

# Optional pipeline
[api.views.get_order.event_handlers]
on_request = [
    { module = "handlers/auth.ts", entrypoint = "checkPermission", key = "authz", parallel = false }
]
transform = [
    { module = "handlers/orders.ts", entrypoint = "redactSensitiveFields", on_failure = "abort" }
]
post_process = [
    { module = "handlers/metrics.ts", entrypoint = "recordLatency", key = "" }
]
```

### 12.2 REST view — CodeComponent handler

```toml
[api.views.create_order]
path      = "/api/orders"
method    = "POST"
view_type = "Rest"

[api.views.create_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "createOrder"
resources  = ["orders_db", "inventory_db", "events_queue"]
```

### 12.3 WebSocket view

```toml
[api.views.order_updates]
path             = "/ws/orders/{customer_id}"
method           = "GET"
view_type        = "Websocket"
websocket_mode   = "Direct"
max_connections  = 1000
rate_limit_per_minute = 300
rate_limit_burst_size = 50

[api.views.order_updates.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "onOrderConnection"
resources  = ["orders_db"]

[api.views.order_updates.on_stream]
module       = "handlers/orders.ts"
entrypoint   = "onOrderMessage"
handler_mode = "auto"
```

### 12.4 SSE view

```toml
[api.views.order_feed]
path                 = "/sse/orders"
method               = "GET"
view_type            = "ServerSentEvents"
sse_tick_interval_ms = 0                        # pure event-driven
sse_trigger_events   = ["order.created", "order.updated"]

[api.views.order_feed.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "streamOrders"
resources  = ["orders_db"]
```

### 12.5 MessageConsumer view

```toml
[api.views.process_order]
view_type  = "MessageConsumer"

[api.views.process_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "onOrderCreated"
resources  = ["orders_db", "notifications_queue"]

[api.views.process_order.on_event]
topic        = "order.created"
handler      = "handlers/orders.ts"
handler_mode = "stream"
```

### 12.6 Full pipeline example

```toml
[api.views.order_summary]
path      = "/api/orders/{id}/summary"
method    = "GET"
view_type = "Rest"

[api.views.order_summary.handler]
type     = "dataview"
dataview = "get_order"

[api.views.order_summary.event_handlers]

pre_process = [
    { module = "h/audit.ts",   entrypoint = "logInbound",  key = "" },
    { module = "h/metrics.ts", entrypoint = "startTimer",  key = "" }
]

on_request = [
    { module = "h/orders.ts", entrypoint = "fetchCustomer",    key = "customer" },
    { module = "h/orders.ts", entrypoint = "fetchShipping",    key = "shipping" },
    { module = "h/orders.ts", entrypoint = "fetchFulfillment", key = "fulfillment" }
]
# <!-- SHAPE-12 amendment: parallel = true removed, stages run sequentially -->

transform = [
    { module = "h/orders.ts", entrypoint = "buildSummary",        on_failure = "abort" },
    { module = "h/orders.ts", entrypoint = "attachComputedFields", on_failure = "skip"  }
]

on_response = [
    { module = "h/orders.ts", entrypoint = "addResponseMetadata", key = "meta" }
]

post_process = [
    { module = "h/metrics.ts", entrypoint = "recordLatency", key = "" },
    { module = "h/audit.ts",   entrypoint = "logOutbound",   key = "" }
]

on_error = [
    { module = "h/ops.ts", entrypoint = "alertOnError" }
]

on_timeout = [
    { module = "h/ops.ts", entrypoint = "logTimeout" }
]
```

---

## 13. Validation Rules

Enforced at config load time. Failures are reported as structured `RiversError::Validation` before the server binds.

| Rule | Error message pattern |
|---|---|
| `DataView` handler on non-REST view | `dataview is only supported for view_type=rest` |
| WebSocket view method != GET | `method must be GET when view_type=websocket` |
| SSE view method != GET | `method must be GET when view_type=server_sent_events` |
| SSE view with `on_stream` declared | `on_stream is not valid for server_sent_events views` |
| `MessageConsumer` view with HTTP path | `MessageConsumer views must not declare a path` |
| DataView reference not in `data.dataviews` | `unknown dataview '{name}'` |
| Resource declared in CodeComponent not in `data.datasources` | `unknown datasource '{name}' in resources` |
| ~~EventBus topic in `sse_trigger_events` not registered~~ | ~~`unknown topic '{name}'`~~ — **removed** <!-- SHAPE-17 amendment --> |
| ~~`on_event.topic` not registered in TopicRegistry~~ | ~~`unknown topic '{name}'`~~ — **removed** <!-- SHAPE-17 amendment --> |
| `rate_limit_per_minute = 0` | `rate_limit_per_minute must be greater than 0` |
| ~~Parallel stage with `on_failure` set~~ | ~~`on_failure is only valid for transform stages`~~ — **removed** (parallel stages removed) <!-- SHAPE-12 amendment --> |

---

## 14. Named Guards (CB-P1.10)

`[api.views.X.guard_view] = "name"` references another view in the same
app whose codecomponent runs as a pre-flight before view `X` dispatches.
The named view's response (`{ allow: bool }`) decides whether the
request proceeds. Honoured uniformly across REST, streaming REST, MCP,
WebSocket, and SSE.

### 14.1 Why two guard mechanisms

The framework already has `guard = true` for the server-wide auth gate
(per `rivers-auth-session-spec.md` §3). Named guards complement that:

| Concern | `guard = true` | `guard_view = "name"` |
|---|---|---|
| Cardinality | exactly one per server | many per app |
| Use case | session-cookie / OAuth login flow | per-route bearer / API-key / multi-tenant auth |
| Result on success | establishes a session | proceeds with request (and may project claims into `ctx.session`) |
| Result on failure | redirect to login URL | HTTP 401 |

Multi-tenant deployments are the canonical case for named guards: each
tenant gets a distinct guard view that validates its bearer token and
projects tenant-scoped identity claims.

### 14.2 Runtime contract

The framework runs the named-guard preflight in `view_dispatch_handler`
between rate limiting and the view-type dispatch switch. Order of
operations:

1. Rate limit (cheap; rejects before guard work)
2. **Named-guard preflight** (this section)
3. Existing security pipeline (session validation, CSRF, server-wide guard)
4. View-type dispatch (REST handler, MCP JSON-RPC parse, WS upgrade, SSE attach)

The single intercept means WS never starts a half-upgrade and SSE never
attaches a stream when the guard rejects — the 401 response materialises
before any view-type-specific work.

The guard handler receives a `ParsedRequest` with the original method,
path, headers, and matched `path_params`. The body has not been read
yet (auth-shape decisions only). On `{ allow: true, session_claims: {...} }`,
the claims propagate into `ctx.session` for the protected view's
handler — same shape as the server-wide guard's output. On
`{ allow: false }` (or any other shape, missing field, dispatcher
error), the framework rejects with HTTP 401.

### 14.3 Configuration

```toml
# The guard view (no auth — it IS the auth boundary).
[api.views.tenant_guard]
view_type = "Rest"
path      = "/internal/tenant-guard"
method    = "POST"
auth      = "none"

[api.views.tenant_guard.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/auth.ts"
entrypoint = "validate_tenant"

# A protected REST route.
[api.views.tenant_orders]
view_type  = "Rest"
path       = "/api/orders"
method     = "GET"
guard_view = "tenant_guard"
```

Per-tenant scopes are typical:

```toml
[api.views.admin_orders]
guard_view = "admin_guard"

[api.views.user_orders]
guard_view = "user_guard"

[api.views.public_health]
# no guard_view — public.
```

### 14.4 Chain composition

A guard view may itself declare `guard_view` to compose multi-stage
auth. Chains run **inside-out** — deepest leaf first, then each layer
back up to the protected view. Every level must return
`{ allow: true }` for the request to proceed; the first rejection
short-circuits with HTTP 401.

```toml
# Multi-tenant: validate token → check role → reach the protected route.
[api.views.tenant_auth]
view_type = "Rest"
auth      = "none"
[api.views.tenant_auth.handler]
type       = "codecomponent"
module     = "handlers/tenant.ts"
entrypoint = "validate_token"
language   = "typescript"

[api.views.tenant_admin_check]
view_type  = "Rest"
auth       = "none"
guard_view = "tenant_auth"            # depth 1
[api.views.tenant_admin_check.handler]
type       = "codecomponent"
module     = "handlers/tenant.ts"
entrypoint = "require_admin"
language   = "typescript"

[api.views.admin_orders]
guard_view = "tenant_admin_check"     # depth 2
```

For a request to `admin_orders`:

1. `tenant_auth` runs (deepest leaf). On `allow: true`, proceed.
2. `tenant_admin_check` runs. On `allow: true`, proceed.
3. `admin_orders` handler runs.

Any level returning `allow: false` rejects the request immediately.

**Chain limits:**

- Maximum depth: **5** hops past the protected view (a constant in
  `crates/riversd/src/security_pipeline.rs`). Validator rejects deeper
  chains at config load with `X014`. Runtime check is
  defense-in-depth.
- Cycles forbidden. Validator catches self-reference (V → V), mutual
  recursion (A → B → A), and longer cycles via DFS visited tracking.
  Runtime check is defense-in-depth.

**Claims propagation (v1):** `session_claims` returned by guards in
the chain are **not** propagated to the protected view's
`ctx.session`. The chain composes allow/deny decisions only. Cross-level
claim merging is a separate feature; if a chain needs to share data
between levels, use `ctx.store` or pass via headers.

### 14.5 Constraints (validator-enforced)

| Code | Severity | Catches |
|---|---|---|
| `X014` | error | `guard_view` (or any chain hop) references a missing view |
| `X014` | error | `guard_view` (or any chain hop) target is not a codecomponent (DataView / `none` handlers can't return the `{ allow }` envelope) |
| `X014` | error | Chain forms a cycle — self-reference (V → V), mutual recursion (A → B → A), or a longer cycle |
| `X014` | error | Chain exceeds `MAX_GUARD_CHAIN_DEPTH` (5 hops) |
| `W009` | warning | Any chain hop has `auth = "session"` — sessions don't exist when the guard runs |
| `W010` | warning | Protected view has both `guard = true` (server-wide gate) and `guard_view = "..."` (per-view gate) |

### 14.6 Cross-references

- `rivers-mcp-view-spec.md` §13.5 — MCP-specific config example.
- `rivers-auth-session-spec.md` §11.5 — bearer-token recipe via a named guard (closes CB-P1.12).
