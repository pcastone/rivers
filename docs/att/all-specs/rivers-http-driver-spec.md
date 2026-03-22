# Rivers HTTP Driver Specification

**Document Type:** Spec Addition  
**Scope:** HttpDriver trait, HTTP/HTTP2/SSE/WebSocket datasources, auth models, DataView contract, retry and circuit breaker  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-driver-spec.md`, `rivers-data-layer-spec.md`  
**Depends On:** Epic 5 (LockBox), Epic 6 (Driver SDK), Epic 8 (Pool Manager), Epic 10 (DataView Engine)

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [HttpDriver Trait](#2-httpdriver-trait)
3. [Protocol Activation](#3-protocol-activation)
4. [Auth Models](#4-auth-models)
5. [Connection Pooling](#5-connection-pooling)
6. [DataView Contract](#6-dataview-contract)
7. [Request/Response Path](#7-requestresponse-path)
8. [Streaming Path](#8-streaming-path)
9. [Retry and Circuit Breaker](#9-retry-and-circuit-breaker)
10. [Response Mapping](#10-response-mapping)
11. [Validation Rules](#11-validation-rules)
12. [Configuration Reference](#12-configuration-reference)
13. [Examples](#13-examples)

---

## 1. Design Rationale

### 1.1 Why a New Trait

The HTTP driver does not fit `DatabaseDriver` or `MessageBrokerDriver`:

**`DatabaseDriver`** — models a query against a static endpoint. HTTP DataViews have parameterized paths, methods, headers, and bodies. The request itself is data, not just the parameters to a fixed query. Forcing HTTP into `DatabaseDriver` means losing method, path templating, and header control.

**`MessageBrokerDriver`** — models a continuous inbound stream with ack/nack semantics. SSE and WebSocket connections over HTTP are persistent inbound streams, but they have no ack/nack — they are read-only push delivery. The broker delivery model is wrong.

**`HttpDriver`** — a purpose-built trait that owns path templating, auth token lifecycle, protocol negotiation, and the dual activation path (request/response or streaming) from a single config block.

### 1.2 Driver Registration

`HttpDriver` registers alongside `DatabaseDriver` and `MessageBrokerDriver` in the `DriverRegistrar`. The Rivers core recognizes it as a first-class driver kind. Built-in — no plugin crate needed.

---

## 2. HttpDriver Trait

```rust
#[async_trait]
pub trait HttpDriver: Send + Sync {
    fn name(&self) -> &str;

    /// Build and return a connection for request/response use.
    async fn connect(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<Box<dyn HttpConnection>, HttpDriverError>;

    /// Build and return a persistent stream connection (SSE or WebSocket).
    async fn connect_stream(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<Box<dyn HttpStreamConnection>, HttpDriverError>;

    /// Refresh auth credentials if needed. Called by the pool manager
    /// on a background interval for oauth2_client_credentials.
    async fn refresh_auth(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<AuthState, HttpDriverError>;
}

#[async_trait]
pub trait HttpConnection: Send + Sync {
    async fn execute(
        &mut self,
        request: &HttpRequest,
    ) -> Result<HttpResponse, HttpDriverError>;
}

#[async_trait]
pub trait HttpStreamConnection: Send + Sync {
    async fn next(&mut self) -> Result<Option<HttpStreamEvent>, HttpDriverError>;
    async fn close(&mut self) -> Result<(), HttpDriverError>;
}
```

### 2.1 HttpConnectionParams

```rust
pub struct HttpConnectionParams {
    pub base_url:    String,
    pub protocol:    HttpProtocol,
    pub auth:        AuthConfig,
    pub tls:         TlsConfig,
    pub timeout_ms:  u64,
    pub pool_size:   u32,
}

pub enum HttpProtocol {
    Http1,
    Http2,
    Sse,
    WebSocket,
}
```

### 2.2 HttpRequest

```rust
pub struct HttpRequest {
    pub method:      HttpMethod,
    pub path:        String,          // fully resolved — {params} already substituted
    pub headers:     HashMap<String, String>,
    pub query:       HashMap<String, String>,
    pub body:        Option<serde_json::Value>,
    pub timeout_ms:  Option<u64>,     // per-request override
}

pub enum HttpMethod {
    Get, Post, Put, Patch, Delete, Head,
}
```

### 2.3 HttpResponse

```rust
pub struct HttpResponse {
    pub status:   u16,
    pub headers:  HashMap<String, String>,
    pub body:     serde_json::Value,
}
```

Non-2xx responses are not automatically errors. The DataView config declares what status codes are acceptable via `success_status` (see §7). Unacceptable status codes return `HttpDriverError::UnexpectedStatus { status, body }`.

### 2.4 HttpStreamEvent

```rust
pub struct HttpStreamEvent {
    pub event_type: Option<String>,   // SSE "event:" field — None for data-only events
    pub data:       serde_json::Value,
    pub id:         Option<String>,   // SSE "id:" field
}
```

WebSocket text frames are deserialized as JSON. Binary frames are rejected with `HttpDriverError::UnsupportedFrameType`.

---

## 3. Protocol Activation

The `protocol` field on the datasource config determines activation path:

| Protocol | Activation | Trait path |
|---|---|---|
| `http` | Request/response, HTTP/1.1 | `HttpConnection.execute()` |
| `http2` | Request/response, HTTP/2 | `HttpConnection.execute()` |
| `sse` | Persistent inbound stream | `HttpStreamConnection` |
| `websocket` | Persistent bidirectional stream | `HttpStreamConnection` |

`sse` and `websocket` protocols activate the `MessageBrokerDriver`-compatible delivery path in Rivers core — the `BrokerConsumerBridge` drives the stream, routing events to `MessageConsumer` views via the EventBus. The `HttpDriver` provides the stream; the bridge handles delivery.

A single driver crate implements both paths. Which activates at runtime is determined entirely by the `protocol` config field.

---

## 4. Auth Models

Auth is configured per datasource. Credentials always come from LockBox — no plaintext credentials in config.

### 4.1 `bearer`

Static bearer token. Injected as `Authorization: Bearer <token>` on every request. Token fetched from LockBox at datasource init and cached for the datasource lifetime.

```toml
[data.datasources.openai]
driver      = "http"
base_url    = "https://api.openai.com"
auth        = "bearer"
credentials = "lockbox://openai/api_key"
```

### 4.2 `basic`

HTTP Basic auth. LockBox secret must be a JSON object `{ "username": "...", "password": "..." }`. Injected as `Authorization: Basic <base64(user:pass)>` on every request.

```toml
[data.datasources.internal_api]
driver      = "http"
base_url    = "https://api.internal"
auth        = "basic"
credentials = "lockbox://internal/basic_creds"
```

### 4.3 `api_key`

API key injected as a named header. Requires `auth_header` field to specify the header name.

```toml
[data.datasources.weather_api]
driver      = "http"
base_url    = "https://api.weather.example.com"
auth        = "api_key"
auth_header = "X-Api-Key"
credentials = "lockbox://weather/api_key"
```

Header name is configurable — covers `X-Api-Key`, `X-Access-Token`, `api-key`, and any other convention the upstream uses.

### 4.4 `oauth2_client_credentials`

OAuth2 client credentials flow. Rivers fetches and manages the access token automatically. LockBox secret must be a JSON object:

```json
{
    "client_id":     "...",
    "client_secret": "...",
    "token_url":     "https://auth.example.com/oauth/token",
    "scope":         "read write"
}
```

**Token lifecycle:**
1. On datasource init, Rivers fetches an access token from `token_url`
2. Token stored in pool manager alongside connection pool
3. Background refresh task runs at `(expires_in - refresh_buffer_s)` seconds
4. All requests in-flight during refresh complete with the old token
5. New token injected for all subsequent requests
6. If refresh fails, Rivers retries with exponential backoff up to `auth_retry_attempts`

```toml
[data.datasources.salesforce]
driver               = "http"
base_url             = "https://myorg.salesforce.com"
auth                 = "oauth2_client_credentials"
credentials          = "lockbox://salesforce/oauth_creds"
refresh_buffer_s     = 60       # refresh this many seconds before expiry (default: 60)
auth_retry_attempts  = 3        # retry attempts on token refresh failure (default: 3)
```

`scope` in the LockBox secret is optional — omit if the upstream does not require it.

---

## 5. Connection Pooling

HTTP connections are pooled per datasource. The pool manager maintains a connection pool of keep-alive HTTP connections to the upstream.

```toml
[data.datasources.openai]
pool_size        = 10       # max concurrent connections (default: 10)
connect_timeout_ms = 5000   # connection establishment timeout (default: 5000)
request_timeout_ms = 30000  # per-request timeout (default: 30000)
```

HTTP/2 multiplexes multiple requests over a single connection. For `protocol = "http2"`, `pool_size` controls the number of HTTP/2 connections (not streams). Each connection supports many concurrent streams. Adjust downward for HTTP/2 upstreams — a pool of 2-3 connections is often sufficient.

SSE and WebSocket datasources do not use a connection pool — each stream subscription creates a persistent dedicated connection managed by the `BrokerConsumerBridge`.

---

## 6. DataView Contract

HTTP DataViews differ from SQL DataViews in structure. The "query" is a full HTTP request template.

```rust
pub struct HttpDataViewConfig {
    pub datasource:     String,
    pub method:         HttpMethod,
    pub path:           String,                    // path template with {param} placeholders
    pub headers:        HashMap<String, String>,   // static headers merged with auth headers
    pub query_params:   HashMap<String, String>,   // static query params
    pub body_template:  Option<serde_json::Value>, // body template with {param} placeholders
    pub parameters:     Vec<HttpDataViewParam>,
    pub success_status: Vec<u16>,                  // default: [200, 201, 202, 204]
    pub return_schema:  Option<String>,
}

pub struct HttpDataViewParam {
    pub name:     String,
    pub location: ParamLocation,
    pub required: bool,
    pub default:  Option<serde_json::Value>,
}

pub enum ParamLocation {
    Path,        // substituted into path template
    Query,       // appended to query string
    Body,        // substituted into body template
    Header,      // injected as request header
}
```

### 6.1 Path templating

Path parameters use `{param}` syntax. All declared path parameters must appear in the path template. Extra `{placeholders}` in the path without a declared parameter fail at config validation.

```toml
path = "/v1/users/{user_id}/orders/{order_id}"

parameters = [
    { name = "user_id",  location = "path",  required = true },
    { name = "order_id", location = "path",  required = true },
    { name = "status",   location = "query", required = false, default = "active" }
]
```

Resolved at DataView execution time:
```
GET /v1/users/42/orders/99?status=active
```

### 6.2 Body templating

Body template is a JSON object with `{param}` placeholders as string values. At execution time, placeholders are replaced with actual parameter values, preserving JSON types.

```toml
body_template = { model = "gpt-4", input = "{text}", max_tokens = 1024 }
parameters    = [{ name = "text", location = "body", required = true }]
```

Resolves to:
```json
{ "model": "gpt-4", "input": "the actual text value", "max_tokens": 1024 }
```

Non-string values in the template are passed through unchanged. Only string values containing a single `{param}` placeholder are substituted. String values with no placeholder are passed through as literal strings.

### 6.3 Header injection

Static headers declared in the DataView config are merged with auth headers. Auth headers take precedence over static DataView headers. DataView headers take precedence over datasource-level default headers. Per-request header parameters (declared with `location = "header"`) override all.

---

## 7. Request/Response Path

### 7.1 Execution flow

```
DataView.execute(params)
    │
    ├─ Resolve path template → substitute {params}
    ├─ Resolve body template → substitute {params}
    ├─ Resolve query params  → append to URL
    ├─ Resolve header params → merge with static + auth headers
    │
    ├─ Acquire connection from pool
    │
    ├─ Execute HttpConnection.execute(HttpRequest)
    │
    ├─ Check response status against success_status
    │   ├─ Match     → deserialize body as JSON
    │   └─ No match  → HttpDriverError::UnexpectedStatus
    │
    ├─ Apply return_schema validation (if configured)
    │
    └─ Return HttpResponse body as QueryResult
```

### 7.2 QueryResult mapping

The HTTP driver wraps the response body in a `QueryResult` for compatibility with the DataView engine:

- JSON object response → single row
- JSON array response → one row per element
- Empty body (204) → empty QueryResult

This allows HTTP DataViews to be used identically to SQL DataViews from the View layer's perspective.

---

## 8. Streaming Path

SSE and WebSocket datasources activate the streaming path. The `BrokerConsumerBridge` manages the persistent connection and routes events to `MessageConsumer` views via the EventBus.

### 8.1 SSE datasource

```toml
[data.datasources.upstream_events]
driver   = "http"
base_url = "https://events.upstream.example.com"
protocol = "sse"
auth     = "bearer"
credentials = "lockbox://upstream/api_key"

[data.datasources.upstream_events.consumer]
path       = "/v1/events/stream"
event_type = "upstream.event"     # EventBus event type to publish under
```

The `BrokerConsumerBridge` connects to the SSE endpoint at startup. Each `data:` event received is deserialized as JSON and published to the EventBus under `event_type`. `MessageConsumer` views subscribed to that event type receive delivery.

SSE `event:` field (named events) is preserved in `HttpStreamEvent.event_type`. If the upstream uses named events, the EventBus topic can be qualified: `upstream.{event_type}`.

### 8.2 WebSocket datasource

```toml
[data.datasources.market_feed]
driver   = "http"
base_url = "wss://feed.exchange.example.com"
protocol = "websocket"
auth     = "api_key"
auth_header = "X-Api-Key"
credentials = "lockbox://exchange/api_key"

[data.datasources.market_feed.consumer]
path       = "/v2/stream"
event_type = "market.tick"
```

WebSocket text frames deserialized as JSON → published to EventBus. Binary frames logged and discarded (same policy as Rivers WebSocket views).

The `BrokerConsumerBridge` manages reconnection with exponential backoff on disconnect. WebSocket datasources do not have ack/nack semantics — each event is fire-and-forget delivery to the EventBus.

---

## 9. Retry and Circuit Breaker

Configured per datasource. Applied on the request/response path only — streaming connections have their own reconnect logic in `BrokerConsumerBridge`.

### 9.1 Retry

```toml
[data.datasources.openai.retry]
attempts        = 3           # total attempts including first (default: 3)
backoff         = "exponential"  # "exponential" | "linear" | "none"
base_delay_ms   = 100         # initial delay (default: 100)
max_delay_ms    = 5000        # cap on delay (default: 5000)
retry_on_status = [429, 502, 503, 504]  # retry on these HTTP status codes
retry_on_timeout = true       # retry on request timeout (default: true)
```

`retry_on_status` defaults to `[429, 502, 503, 504]`. 4xx errors other than 429 are not retried by default — they indicate a bad request, not a transient upstream failure.

For 429 responses, Rivers checks for `Retry-After` header and honors it if present, capping at `max_delay_ms`.

### 9.2 Circuit Breaker

```toml
[data.datasources.openai.circuit_breaker]
failure_threshold   = 5        # failures in window before opening (default: 5)
window_ms           = 10000    # rolling window for failure counting (default: 10000)
open_duration_ms    = 30000    # how long circuit stays open (default: 30000)
half_open_attempts  = 1        # probe attempts before closing (default: 1)
```

Circuit breaker states follow the standard model: Closed → Open → Half-Open → Closed.

When open, requests fail immediately with `HttpDriverError::CircuitOpen`. The `on_circuit_open` datasource event handler fires (if declared) — same observer pattern as other datasource lifecycle events.

### 9.3 Timeout hierarchy

Three timeout levels, innermost wins:

```
datasource.request_timeout_ms       (datasource default)
    └─ dataview.timeout_ms          (per-DataView override)
        └─ HttpRequest.timeout_ms   (per-request override — set by DataView engine)
```

---

## 10. Response Mapping

### 10.1 Return schema

`return_schema` applies the same JSON Schema validation as SQL DataViews. The HTTP response body (after JSON deserialization) is validated against the declared schema. Validation failure returns `DataViewError::Schema` — same error type as SQL DataViews.

```toml
[data.dataviews.get_embedding]
datasource    = "openai"
return_schema = "EmbeddingResponse"
```

### 10.2 Non-JSON responses

If the upstream returns a non-JSON `Content-Type`, the driver wraps the raw body string in a JSON object:

```json
{ "raw": "<response body as string>", "content_type": "text/plain" }
```

This preserves the DataView engine's JSON-native contract. Operators who need structured access to non-JSON upstreams should use a transform handler in the View pipeline to parse `raw`.

### 10.3 Empty responses

HTTP 204 No Content returns an empty `QueryResult` (zero rows). This is not an error. Views that call a DataView expecting rows will see an empty result — the same behavior as a SQL query that returns no rows.

---

## 11. Validation Rules

| Rule | Error message |
|---|---|
| `auth = "api_key"` without `auth_header` | `auth_header is required when auth = api_key` |
| Path template contains `{param}` not in `parameters` | `path template references undeclared parameter '{name}'` |
| `parameters` declares `location = "path"` but `{name}` not in path template | `path parameter '{name}' not found in path template` |
| `protocol = "sse"` or `"websocket"` without `consumer` block | `streaming protocol requires a consumer block` |
| `consumer.path` missing | `consumer.path is required` |
| `retry.attempts = 0` | `retry.attempts must be at least 1` |
| `circuit_breaker.failure_threshold = 0` | `failure_threshold must be at least 1` |
| `success_status` empty | `success_status must declare at least one status code` |

---

## 12. Configuration Reference

### 12.1 HTTP/HTTP2 datasource

```toml
[data.datasources.openai]
driver             = "http"
base_url           = "https://api.openai.com"
protocol           = "http2"           # default: "http"
auth               = "bearer"
credentials        = "lockbox://openai/api_key"
pool_size          = 10
connect_timeout_ms = 5000
request_timeout_ms = 30000

[data.datasources.openai.retry]
attempts         = 3
backoff          = "exponential"
base_delay_ms    = 100
max_delay_ms     = 5000
retry_on_status  = [429, 502, 503, 504]
retry_on_timeout = true

[data.datasources.openai.circuit_breaker]
failure_threshold  = 5
window_ms          = 10000
open_duration_ms   = 30000
half_open_attempts = 1
```

### 12.2 OAuth2 datasource

```toml
[data.datasources.salesforce]
driver              = "http"
base_url            = "https://myorg.salesforce.com"
auth                = "oauth2_client_credentials"
credentials         = "lockbox://salesforce/oauth_creds"
refresh_buffer_s    = 60
auth_retry_attempts = 3

[data.datasources.salesforce.retry]
attempts        = 2
backoff         = "linear"
base_delay_ms   = 500
retry_on_status = [503, 504]
```

### 12.3 SSE streaming datasource

```toml
[data.datasources.upstream_feed]
driver      = "http"
base_url    = "https://events.upstream.example.com"
protocol    = "sse"
auth        = "bearer"
credentials = "lockbox://upstream/token"

[data.datasources.upstream_feed.consumer]
path       = "/v1/stream"
event_type = "upstream.event"
```

### 12.4 HTTP DataViews

```toml
[data.dataviews.generate_embedding]
datasource    = "openai"
method        = "POST"
path          = "/v1/embeddings"
return_schema = "EmbeddingResponse"
timeout_ms    = 10000

[data.dataviews.generate_embedding.body_template]
model = "text-embedding-3-small"
input = "{text}"

[[data.dataviews.generate_embedding.parameters]]
name     = "text"
location = "body"
required = true

---

[data.dataviews.get_user]
datasource = "internal_api"
method     = "GET"
path       = "/v2/users/{user_id}"

[[data.dataviews.get_user.parameters]]
name     = "user_id"
location = "path"
required = true

---

[data.dataviews.search_records]
datasource = "salesforce"
method     = "GET"
path       = "/services/data/v57.0/query"

[[data.dataviews.search_records.parameters]]
name     = "q"
location = "query"
required = true
```

---

## 13. Examples

### 13.1 LLM embedding pipeline

```toml
[data.datasources.openai]
driver      = "http"
base_url    = "https://api.openai.com"
protocol    = "http2"
auth        = "bearer"
credentials = "lockbox://openai/api_key"

[data.dataviews.embed]
datasource = "openai"
method     = "POST"
path       = "/v1/embeddings"

[data.dataviews.embed.body_template]
model = "text-embedding-3-small"
input = "{text}"

[[data.dataviews.embed.parameters]]
name     = "text"
location = "body"
required = true

[api.views.embed]
path      = "/api/embed"
method    = "POST"
view_type = "Rest"

[api.views.embed.handler]
type     = "dataview"
dataview = "embed"
```

No CodeComponent needed. A POST to `/api/embed` with `{ "text": "..." }` calls the OpenAI embeddings API and returns the result directly. Connection pooling, retry, circuit breaker, and LockBox credential management all come from the datasource config.

### 13.2 Upstream SSE feed → MessageConsumer

```toml
[data.datasources.market_events]
driver      = "http"
base_url    = "https://feed.exchange.example.com"
protocol    = "sse"
auth        = "api_key"
auth_header = "X-Api-Key"
credentials = "lockbox://exchange/key"

[data.datasources.market_events.consumer]
path       = "/v2/stream"
event_type = "market.tick"

[api.views.process_tick]
view_type  = "MessageConsumer"

[api.views.process_tick.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/market.ts"
entrypoint = "onMarketTick"
resources  = ["trades_db"]

[api.views.process_tick.on_event]
topic        = "market.tick"
handler_mode = "stream"
```

```typescript
// handlers/market.ts
async function onMarketTick(req: Rivers.Request): Promise<void> {
    const tick = req.body as MarketTick;
    await Rivers.db.query(
        Rivers.resources.trades_db,
        "INSERT INTO ticks (symbol, price, volume, ts) VALUES ($1, $2, $3, $4)",
        [tick.symbol, tick.price, tick.volume, tick.timestamp]
    );
}
```

Upstream SSE feed → EventBus → `MessageConsumer` view → database write. No broker infrastructure required.

### 13.3 Multi-datasource view — primary DB + HTTP enrichment

```toml
[api.views.product_detail]
path      = "/api/products/{id}"
method    = "GET"
view_type = "Rest"

[api.views.product_detail.handler]
type     = "dataview"
dataview = "get_product"           # SQL DataView — primary

[api.views.product_detail.event_handlers]

on_request = [
    { module = "handlers/product.ts", entrypoint = "fetchReviews", key = "reviews", parallel = true },
    { module = "handlers/product.ts", entrypoint = "fetchInventory", key = "inventory", parallel = true }
]

on_response = [
    { module = "handlers/product.ts", entrypoint = "mergeProductData", key = "merged" }
]
```

```typescript
// handlers/product.ts
async function fetchReviews(ctx: ViewContext): Promise<OnRequestResult> {
    // HTTP DataView call — reviews service
    const reviews = await Rivers.view.query("get_reviews", {
        product_id: ctx.request.path_params.id
    });
    return { key: "reviews", data: reviews };
}

async function fetchInventory(ctx: ViewContext): Promise<OnRequestResult> {
    // HTTP DataView call — inventory service
    const inventory = await Rivers.view.query("get_inventory", {
        sku: ctx.request.path_params.id
    });
    return { key: "inventory", data: inventory };
}
```

SQL DataView for core product data, two parallel HTTP DataView calls for enrichment. The handler pipeline collapses all three into one response. From the View layer's perspective, SQL and HTTP DataViews are identical.
