# Admin and Operations Reference

**Rivers v0.54.0**

This document covers administration, security, monitoring, and operational management of a Rivers deployment. All section identifiers follow the AW3 numbering scheme.

> **v0.54.0 operator notes:** (1) cdylib driver plugins are disabled — all drivers are now statically compiled into `riversd`. (2) `[plugins] dir` config is deprecated and logs a warning. (3) Apps with unresolvable drivers no longer crash the whole bundle — they are isolated and return 503. (4) Metrics are now actually emitted (previously the feature scaffolding existed but no data flowed). See AW3.16, AW3.17, AW3.18 below.

---

## AW3.1 Two-Server Architecture

Rivers runs two independent HTTP servers on separate sockets:

- **Main server** -- serves application traffic: view routes, static files, health endpoints, gossip receiver. Full middleware stack applied.
- **Admin server** -- serves operational endpoints on a separate port. Disabled by default (`admin_api.enabled = false`). Subset middleware stack: `trace_id`, `timeout`, `admin_auth`, `security_headers`.

Both are Axum-based, built on `hyper` and `tokio`. TLS is mandatory on both servers. The admin server auto-generates a self-signed certificate when no cert/key pair is configured.

```toml
[base]
host = "0.0.0.0"
port = 8080

[base.admin_api]
enabled     = true
host        = "127.0.0.1"
port        = 9443
public_key  = "/etc/rivers/admin/admin.pub"
private_key = "/etc/rivers/admin/admin.key"
```

---

## AW3.2 Admin API Endpoints

All admin endpoints are served on the admin server port. They require Ed25519 authentication (unless `--no-admin-auth` is active).

### Status and introspection

| Method | Path | Description |
|--------|------|-------------|
| GET | `/admin/status` | Server status, draining state, inflight count |
| GET | `/admin/drivers` | All registered driver names |
| GET | `/admin/datasources` | Pool snapshots for all datasources |

**GET /admin/status**

```json
{
  "status": "ok",
  "draining": false,
  "inflight": 5
}
```

**GET /admin/drivers**

```json
{
  "drivers": ["postgres", "mysql", "redis", "faker", "http"],
  "count": 5
}
```

**GET /admin/datasources**

```json
{
  "datasources": [
    {
      "name": "orders_db",
      "driver": "postgres",
      "active": 3,
      "idle": 7,
      "max": 20,
      "circuit_state": "closed"
    }
  ],
  "count": 1
}
```

### Deployment lifecycle

| Method | Path | Description |
|--------|------|-------------|
| POST | `/admin/deploy` | Upload and begin deployment |
| POST | `/admin/deploy/test` | Run deployment test/validation stage |
| POST | `/admin/deploy/approve` | Approve a tested deployment |
| POST | `/admin/deploy/reject` | Reject a deployment |
| POST | `/admin/deploy/promote` | Promote approved deployment to active |
| GET | `/admin/deployments` | List all deployments and their status |

**POST /admin/deploy**

Request body:

```json
{
  "bundle_path": "/var/rivers/bundles/orders-platform-v1.4.2.zip",
  "app_id": "orders-service"
}
```

Response:

```json
{
  "status": "accepted",
  "deploy_id": "deploy-7f3a1b2c"
}
```

**POST /admin/deploy/test**

```json
// Request
{ "deploy_id": "deploy-7f3a1b2c" }

// Response — validation results
{
  "deploy_id": "deploy-7f3a1b2c",
  "status": "tested",
  "checks": [
    { "name": "schema_validation", "passed": true },
    { "name": "datasource_connectivity", "passed": true },
    { "name": "lockbox_resolution", "passed": true }
  ]
}
```

**POST /admin/deploy/approve**

```json
// Request
{ "deploy_id": "deploy-7f3a1b2c" }

// Response
{ "status": "approved" }
```

**POST /admin/deploy/reject**

```json
// Request
{ "deploy_id": "deploy-7f3a1b2c" }

// Response
{ "status": "rejected" }
```

**POST /admin/deploy/promote**

```json
// Request
{ "deploy_id": "deploy-7f3a1b2c" }

// Response
{ "status": "promoted" }
```

**GET /admin/deployments**

```json
{
  "deployments": [
    {
      "deploy_id": "deploy-7f3a1b2c",
      "app_id": "orders-service",
      "bundle": "orders-platform",
      "version": "1.4.2",
      "status": "RUNNING",
      "deployed_at": "2026-03-20T14:30:00Z"
    }
  ],
  "count": 1
}
```

### Runtime log management

| Method | Path | Description |
|--------|------|-------------|
| GET | `/admin/log/levels` | Current log level configuration |
| POST | `/admin/log/set` | Change log level at runtime |
| POST | `/admin/log/reset` | Reset log levels to config defaults |

**GET /admin/log/levels**

```json
{
  "levels": {
    "global": "info"
  }
}
```

**POST /admin/log/set**

```json
// Request
{ "target": "global", "level": "debug" }

// Response
{ "status": "updated" }
```

Valid levels: `debug`, `info`, `warn`, `error`. Invalid level values return 400.

**POST /admin/log/reset**

```json
// Response
{ "status": "reset" }
```

Resets all runtime log level overrides back to the values defined in config.

---

## AW3.3 Admin Error Response Envelope

All admin API errors use the standard `ErrorResponse` envelope:

```json
{
  "code": 400,
  "message": "invalid log level: 'verbose'"
}
```

| Status Code | Condition |
|-------------|-----------|
| 400 | Bad request -- invalid level, invalid state transition, malformed body |
| 401 | Missing or invalid Ed25519 signature |
| 403 | RBAC permission denied, IP allowlist rejected |
| 404 | Deployment not found |
| 503 | Log controller unavailable, subsystem not ready |

---

## AW3.4 Admin Authentication -- Ed25519 Signing

Every admin request must include an Ed25519 signature. Signing is performed by `riversctl`.

### Signing protocol

The signature covers a string constructed as:

```
{method}\n{path}\n{body_sha256_hex}\n{unix_timestamp_ms}
```

For GET requests with no body, `body_sha256_hex` is the SHA-256 of an empty string (`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).

### Request headers

| Header | Value |
|--------|-------|
| `X-Rivers-Signature` | Hex-encoded Ed25519 signature |
| `X-Rivers-Timestamp` | Unix timestamp in milliseconds |

### Verification

The admin server:

1. Extracts `X-Rivers-Timestamp` -- rejects if not within +-5 minutes of server clock (replay protection)
2. Reconstructs the signing payload from the request method, path, SHA-256 of body, and the provided timestamp
3. Verifies the signature against the configured `public_key`
4. Rejects with 401 if either check fails

### Configuration

```toml
[base.admin_api]
public_key  = "/etc/rivers/admin/admin.pub"
private_key = "/etc/rivers/admin/admin.key"
```

`public_key` must be set when `admin_api.enabled = true`. Omitting it is a validation error at startup.

### Emergency access

```bash
riversd --no-admin-auth
```

Disables Ed25519 signature verification for the current process lifetime only. Does not persist across restarts. Emits a warning at startup. Intended for initial setup and break-glass scenarios.

---

## AW3.5 RBAC -- Role-Based Access Control

Roles contain a list of permissions. Permissions correspond to admin endpoint names. Identities are bound to roles.

### Configuration

```toml
[base.admin_api.rbac.roles]
operator = ["status", "datasources", "drivers", "log"]
deployer = ["status", "datasources", "drivers", "deploy", "log"]

[base.admin_api.rbac.bindings]
"CN=ops-team"     = "operator"
"CN=admin-client" = "deployer"
```

### Identity sources

Identity is determined from one of:

- **mTLS client certificate CN** -- when `require_client_cert = true` in admin TLS config
- **Ed25519 key fingerprint** -- when using key-based authentication

### Validation rules

- Bindings referencing undefined roles are rejected at startup
- Roles with no permissions are rejected at startup
- Permission names must match known admin endpoint categories (e.g., `deploy`, `status`, `datasources`, `drivers`, `log`)

### Admin TLS (mTLS)

```toml
[base.admin_api.tls]
ca_cert             = "/etc/rivers/admin/ca.crt"
server_cert         = "/etc/rivers/admin/server.crt"
server_key          = "/etc/rivers/admin/server.key"
require_client_cert = true
```

---

## AW3.6 Deployment Lifecycle

Deployments follow a state machine with defined transitions.

### Deployment states

```
PENDING --> RESOLVING --> STARTING --> RUNNING
                                   \-> FAILED
RUNNING --> STOPPING --> STOPPED
```

| State | Description |
|-------|-------------|
| `PENDING` | Bundle received, not yet processed |
| `RESOLVING` | Resolving resources (LockBox aliases, datasource connectivity) |
| `STARTING` | App services starting, health checks running |
| `RUNNING` | All health checks passed, serving traffic |
| `FAILED` | Startup or resource resolution failed |
| `STOPPING` | Graceful shutdown in progress, draining in-flight requests |
| `STOPPED` | Fully stopped |

### Deployment workflow

1. `POST /admin/deploy` -- upload bundle, receive `deploy_id`, status becomes `PENDING`
2. `POST /admin/deploy/test` -- run validation (schema checks, connectivity tests), returns results
3. `POST /admin/deploy/approve` -- approve for promotion
4. `POST /admin/deploy/promote` -- promote to active, begins `RESOLVING` -> `STARTING` -> `RUNNING`
5. `POST /admin/deploy/reject` -- reject a deployment at any pre-promotion stage

### Startup order

1. App-services start first (parallel, respecting dependency graph)
2. App-mains wait for service health checks to pass
3. Bundle `apps` array order is authoritative

### Zero-downtime redeployment

The previous deployment drains in-flight requests before the new deployment takes over. New requests are routed to the new deployment once its health checks pass. If the new deployment fails, the old deployment remains active.

---

## AW3.7 Security

### TLS

TLS is mandatory on both the main server and admin server.

- When cert/key paths are provided, Rivers uses those certificates
- When cert/key paths are absent, Rivers auto-generates a self-signed certificate at startup and writes it to `{data_dir}/tls/`
- Auto-generated certs emit a warning: not suitable for production
- HTTP/2 requires TLS -- enabling HTTP/2 without TLS is a startup error

```toml
[base.tls]
cert = "/etc/rivers/tls/server.crt"
key  = "/etc/rivers/tls/server.key"

[base.tls.x509]
common_name  = "rivers.local"
organization = "Rivers"
```

### CORS

CORS is framework-managed. Handlers cannot override CORS headers (blocked by the handler header blocklist).

```toml
[security]
cors_enabled           = true
cors_allowed_origins   = ["https://app.example.com"]
cors_allowed_methods   = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
cors_allowed_headers   = ["Content-Type", "Authorization", "X-Trace-Id"]
cors_allow_credentials = false
```

| Field | Description |
|-------|-------------|
| `cors_enabled` | Enable CORS handling (preflight + response headers) |
| `cors_allowed_origins` | Allowed origin domains |
| `cors_allowed_methods` | Allowed HTTP methods |
| `cors_allowed_headers` | Allowed request headers |
| `cors_allow_credentials` | Whether `Access-Control-Allow-Credentials` is set |

### Rate limiting

Token bucket algorithm applied globally and per-view.

```toml
[security]
rate_limit_per_minute    = 120
rate_limit_burst_size    = 60
rate_limit_strategy      = "ip"            # ip | custom_header
rate_limit_custom_header = "X-Api-Key"     # required if strategy = custom_header
```

| Strategy | Key source |
|----------|-----------|
| `ip` | Client IP address (default) |
| `custom_header` | Value of the specified header (e.g., API key) |

Bucket eviction occurs at 10,000 entries, removing stale or oldest buckets first.

WebSocket connections use per-connection rate limiting. REST and SSE use per-IP rate limiting. Per-view overrides can be configured in individual view definitions (see below).

### Per-view rate limiting (v0.54.0)

Individual views can override the global rate limit by declaring `rate_limit_per_minute` and `rate_limit_burst_size` directly on the view in `app.toml`:

```toml
[api.views.search]
path                  = "/api/search"
method                = "GET"
view_type             = "Rest"
rate_limit_per_minute = 30     # override global (120)
rate_limit_burst_size = 10     # override global (60)
```

Per-view limits use token-bucket algorithm with **proxy-aware** client IP extraction: when the request arrives from an IP in `trusted_proxies`, the left-most entry in `X-Forwarded-For` is used as the client key. Limiters are cached per view_id.

When a per-view limit is exceeded, the response is `429 Too Many Requests` with a `Retry-After` header set to the seconds remaining until the next token is available.

### CSRF protection

Double-submit cookie pattern with rotation interval.

- Auto-validated on state-changing methods: POST, PUT, PATCH, DELETE
- Bearer token requests are exempt from CSRF validation
- CSRF token stored in StorageEngine (`csrf:` namespace)

### Security headers

Set automatically on every response from both servers:

| Header | Value |
|--------|-------|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `X-XSS-Protection` | `1; mode=block` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |

`Strict-Transport-Security` (HSTS) and `Content-Security-Policy` (CSP) are not set automatically. These are the responsibility of the operator or reverse proxy, since they depend on the specific deployment topology.

### Handler header blocklist

CodeComponent handlers cannot set the following response headers. Any such headers in handler output are silently dropped:

```
set-cookie, access-control-allow-origin, access-control-allow-credentials,
access-control-allow-methods, access-control-allow-headers,
access-control-expose-headers, access-control-max-age,
host, transfer-encoding, connection, upgrade,
x-forwarded-for, x-forwarded-host, x-forwarded-proto,
x-content-type-options, x-frame-options,
strict-transport-security, content-security-policy
```

### IP allowlist

```toml
[security]
admin_ip_allowlist = ["10.0.0.0/8", "192.168.1.50"]
```

Enforced at the application layer. Also gates `/health/verbose` access on the main server. If empty, any IP is allowed (subject to authentication).

---

## AW3.8 Health Endpoints

Health endpoints are served on the **main server** (not the admin server). They require no authentication.

### GET /health

Always returns 200. No authentication required. Subject to the full middleware stack.

```json
{"status": "healthy"}
```

### GET /health/verbose

Extended status including pool snapshots, circuit breaker state, and cluster information. Returns 200.

```json
{
  "status": "healthy",
  "uptime_seconds": 86400,
  "datasources": [
    {
      "name": "orders_db",
      "driver": "postgres",
      "active": 3,
      "idle": 7,
      "max": 20,
      "circuit_state": "closed"
    }
  ],
  "cluster": {
    "node_id": "node-1",
    "peers": ["node-2", "node-3"],
    "role": "leader"
  }
}
```

If `admin_ip_allowlist` is configured, `/health/verbose` is restricted to those IPs. The basic `/health` endpoint is always public.

The verbose endpoint accepts `?simulate_delay_ms=N` for testing timeout behavior.

---

## AW3.9 EventBus Event Types

All system events are published to the EventBus. The `LogHandler` subscribes at `Observe` priority and formats them as log output. These events are also the foundation for SSE triggers, monitoring, and internal coordination.

### Request lifecycle

| Event | Log Level |
|-------|-----------|
| `RequestCompleted` | Info |
| `DataViewExecuted` | Info |
| `CacheInvalidation` | Info |

### WebSocket

| Event | Log Level |
|-------|-----------|
| `WebSocketConnected` | Info |
| `WebSocketDisconnected` | Info |
| `WebSocketMessageIn` | Info |
| `WebSocketMessageOut` | Info |

### Server-Sent Events

| Event | Log Level |
|-------|-----------|
| `SseStreamOpened` | Info |
| `SseStreamClosed` | Info |
| `SseEventSent` | Info |

### Driver and datasource

| Event | Log Level |
|-------|-----------|
| `DriverRegistered` | Info |
| `DatasourceConnected` | Info |
| `DatasourceDisconnected` | Warn |
| `DatasourceConnectionFailed` | Error |
| `DatasourceReconnected` | Info |
| `DatasourceCircuitOpened` | Warn |
| `DatasourceCircuitClosed` | Info |
| `DatasourceHealthCheckFailed` | Error |
| `ConnectionPoolExhausted` | Warn |

### Message broker

| Event | Log Level |
|-------|-----------|
| `BrokerConsumerStarted` | Info |
| `BrokerConsumerStopped` | Info |
| `BrokerConsumerError` | Error |
| `BrokerMessageReceived` | Info |
| `BrokerMessagePublished` | Info |
| `ConsumerLagDetected` | Warn |
| `MessageFailed` | Error |

### Deployment and config

| Event | Log Level |
|-------|-----------|
| `DeploymentStatusChanged` | Info |
| `ConfigFileChanged` | Info |

### EventBus internal

| Event | Log Level |
|-------|-----------|
| `EventBusTopicPublished` | Debug |
| `EventBusTopicSubscribed` | Info |
| `EventBusTopicUnsubscribed` | Info |

### Cluster and health

| Event | Log Level |
|-------|-----------|
| `NodeHealthChanged` | Warn |

### Plugin

| Event | Log Level |
|-------|-----------|
| `PluginLoadFailed` | Error |

### Polling

| Event | Log Level |
|-------|-----------|
| `PollTickFailed` | Error |
| `OnChangeFailed` | Error |
| `PollChangeDetectTimeout` | Warn |

---

## AW3.10 Error Response Format

All error responses from the Rivers framework (not from CodeComponent handlers) use the `ErrorResponse` envelope:

```json
{
  "code": 500,
  "message": "human-readable error message",
  "details": "optional diagnostic info",
  "trace_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

The `details` field is optional and may be omitted. The `trace_id` field links the error to the originating request for log correlation.

### Status code mapping

| Status | Condition |
|--------|-----------|
| 400 | Invalid request, parameter validation failed, bad request body |
| 401 | Authentication failed, missing or invalid signature |
| 403 | RBAC permission denied, IP allowlist rejected |
| 404 | View not found, static file not found, DataView not found, deployment not found |
| 405 | Method not allowed |
| 408 | Request timeout |
| 422 | Schema validation failed |
| 429 | Rate limit exceeded |
| 500 | Runtime execution failed, connection error, internal error |
| 503 | Server draining (shutdown), backpressure exhausted, circuit open, subsystem unavailable |

---

## AW3.11 Logging Configuration

### Configuration

```toml
[base.logging]
level           = "info"                           # debug | info | warn | error
format          = "json"                           # json | text
local_file_path = "/var/log/rivers/riversd.log"   # optional
```

Defaults: `level = "info"`, `format = "json"`, `local_file_path` is null (stdout only).

### Log levels

| Level | Use |
|-------|-----|
| `debug` | Verbose output, development only |
| `info` | Normal operational events |
| `warn` | Degraded state, not fatal |
| `error` | Failure requiring attention |

Level hierarchy: `debug < info < warn < error`. Setting level to `warn` suppresses `debug` and `info` events.

The event-to-level mapping is fixed (see AW3.9). Operators cannot reclassify individual events.

### JSON log format

```json
{
  "timestamp": "2026-03-11T14:23:01.847Z",
  "level": "info",
  "message": "request completed",
  "trace_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "app_id": "riversd",
  "node_id": "node-1",
  "event_type": "RequestCompleted",
  "method": "GET",
  "path": "/api/orders/42",
  "status": 200,
  "latency_ms": 14
}
```

Mandatory fields (`timestamp`, `level`, `message`, `trace_id`, `app_id`, `node_id`, `event_type`) are always present. Payload fields (e.g., `method`, `path`, `status`, `latency_ms`) vary by event type.

### Text format

Uses `tracing::info!` with the tracing subscriber's formatter. Intended for development. JSON is recommended for production log aggregation.

### Destinations

- **Stdout** (always on) -- all log output goes to stdout. Rivers does not manage log files, rotation, or retention. Pipe stdout to journald, Docker log driver, Loki, or any standard log shipper.
- **Local file** (optional) -- when `local_file_path` is set, log records are also written to a file in append mode. Uses async buffered writer. No rotation by Rivers -- use logrotate or equivalent.

### Runtime log level changes

Use the admin API log management endpoints (AW3.2) to change log levels at runtime without restarting the server. Changes via `/admin/log/set` persist until the next `/admin/log/reset` or process restart.

### Per-Application Logging

When `app_log_dir` is configured, each loaded app gets its own log file:

```toml
[base.logging]
level           = "info"
format          = "json"
local_file_path = "/opt/rivers/log/riversd.log"
app_log_dir     = "/opt/rivers/log/apps"
```

Result:
```
log/
├── riversd.log        <- server logs (startup, config, driver loading)
└── apps/
    ├── my-api.log     <- Rivers.log.info/warn/error from my-api handlers
    └── admin.log      <- Rivers.log from admin handlers
```

App log files rotate automatically at 10MB (`<app>.log.1`).

### Prometheus Metrics

Enable the built-in Prometheus metrics exporter:

```toml
[metrics]
enabled = true
port = 9091       # default
```

Scrape endpoint: `http://localhost:9091/metrics`

Available metrics (v0.54.0 — now actually wired to emit data):

| Metric | Type | Labels |
|--------|------|--------|
| `rivers_http_requests_total` | counter | `method`, `status` |
| `rivers_http_request_duration_ms` | histogram | `method` |
| `rivers_active_connections` | gauge | — |
| `rivers_engine_executions_total` | counter | `engine` (`v8` \| `dataview` \| `none`), `success` (`true` \| `false`) |
| `rivers_engine_execution_duration_ms` | histogram | `engine` |
| `rivers_loaded_apps` | gauge | — |

All metrics are behind the `metrics` cargo feature. In deployed builds, the feature is enabled by default.

### Environment overrides

```toml
[environment_overrides.dev.logging]
level  = "debug"
format = "text"
```

---

## AW3.12 Trace Correlation

Every request is assigned a `trace_id` that propagates through the entire lifecycle.

### Trace ID extraction priority

1. **W3C `traceparent` header** -- `trace_id` segment extracted from `00-{trace_id}-{span_id}-{flags}`
2. **`x-trace-id` header** -- custom Rivers header for backwards compatibility
3. **Generated UUID** -- `Uuid::new_v4()` if neither header is present

### Propagation

The trace ID is:

- Stored in request extensions
- Injected into every `Event` published to the EventBus
- Included in every JSON log record
- Emitted in response headers: `x-trace-id` and `traceparent`
- Carried across inter-service HTTP calls

### Log query patterns

For JSON logs shipped to a log aggregation system:

```
# Trace reconstruction -- all events for a single request
trace_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"

# All errors in the last hour
level = "error" | timestamp > now() - 1h

# Slow requests (latency > 500ms)
event_type = "RequestCompleted" | latency_ms > 500

# Circuit breaker events for a specific datasource
event_type = "DatasourceCircuitOpened" | datasource_id = "orders_db"

# Plugin load failures
event_type = "PluginLoadFailed"
```

---

## AW3.13 Graceful Shutdown

On SIGTERM or SIGINT:

1. `ShutdownCoordinator` sets the atomic draining flag
2. `shutdown_guard_middleware` returns 503 for new requests
3. In-flight requests complete normally
4. WebSocket connections receive a close frame
5. SSE connections close gracefully
6. Connection pools drain
7. `BrokerConsumerBridge` drains pending messages
8. Async write batches flush (log file writer, StorageEngine)
9. Process exits

### Drain behavior

- In-flight requests complete normally within the drain timeout
- New requests during drain receive `503 Service Unavailable`
- The `/admin/status` endpoint reflects the draining state (`"draining": true`)

### Backpressure

Semaphore-based request queue limits concurrency.

```toml
[base.backpressure]
enabled          = true
queue_depth      = 512
queue_timeout_ms = 100
```

When the semaphore is exhausted, requests receive `503 Service Unavailable` with `Retry-After: 1`. Streaming responses (SSE, WebSocket) hold their semaphore permit for the connection lifetime.

---

## AW3.14 Circuit Breaker

Per-datasource circuit breaker protects against cascading failures.

### States

| State | Behavior |
|-------|----------|
| Closed | Normal operation -- requests pass through |
| Open | All requests fail immediately -- no backend calls |
| Half-Open | Single test request allowed -- success closes, failure re-opens |

### Configuration

```toml
[data.datasources.orders_db.circuit_breaker]
failure_threshold    = 5       # failures in rolling window before opening
cooldown_seconds     = 30      # time in open state before transitioning to half-open
success_threshold    = 2       # consecutive successes in half-open before closing
```

Uses a rolling window model (not fixed-window).

### Events

- `DatasourceCircuitOpened` -- logged at Warn
- `DatasourceCircuitClosed` -- logged at Info

---

## AW3.15 Common Operations

### Starting Rivers

```bash
riversd --config ./bundle/config.toml
riversd --config ./bundle/config.toml --port 8080
riversd --config ./bundle/config.toml --no-ssl --no-admin-auth   # development only
```

### Verify app health

```bash
curl https://localhost:8080/health
curl https://localhost:8080/health/verbose
```

### Check server status (via admin API)

```bash
riversctl status
# or directly:
curl -H "X-Rivers-Signature: <sig>" -H "X-Rivers-Timestamp: <ts>" \
  https://localhost:9443/admin/status
```

### Check datasource status

```bash
curl --cert client.crt --key client.key \
  https://localhost:9443/admin/datasources
```

### Deploy a bundle

```bash
riversctl deploy ./orders-platform-bundle
# or via curl:
curl --cert client.crt --key client.key \
  -X POST -H "Content-Type: application/json" \
  -d '{"bundle_path": "./orders-platform-bundle", "app_id": "orders-service"}' \
  https://localhost:9443/admin/deploy
```

### Deployment workflow

```bash
# 1. Upload bundle
riversctl deploy ./bundle

# 2. Run validation tests
curl -X POST -d '{"deploy_id": "deploy-7f3a1b2c"}' \
  https://localhost:9443/admin/deploy/test

# 3. Approve
curl -X POST -d '{"deploy_id": "deploy-7f3a1b2c"}' \
  https://localhost:9443/admin/deploy/approve

# 4. Promote to active
curl -X POST -d '{"deploy_id": "deploy-7f3a1b2c"}' \
  https://localhost:9443/admin/deploy/promote
```

### Change log level at runtime

```bash
# Set to debug
curl -X POST -H "Content-Type: application/json" \
  -d '{"target": "global", "level": "debug"}' \
  https://localhost:9443/admin/log/set

# Check current levels
curl https://localhost:9443/admin/log/levels

# Reset to config defaults
curl -X POST https://localhost:9443/admin/log/reset
```

### Check logs

```bash
# JSON logs to stdout (default)
journalctl -u riversd -f

# Local file if configured
tail -f /var/log/rivers/riversd.log

# Filter errors (jq)
journalctl -u riversd | jq 'select(.level == "error")'

# Filter by trace_id
journalctl -u riversd | jq 'select(.trace_id == "a1b2c3d4...")'

# Circuit breaker events
journalctl -u riversd | jq 'select(.event_type == "DatasourceCircuitOpened")'
```

### Hot reload (development mode only)

```toml
[hot_reload]
enabled    = true
watch_path = "./app.toml"
```

Hot reload reloads: view routes, DataView configs, DataView engine, static file config, security config. It does **not** restart the HTTP server, rebind sockets, re-initialize connection pools, or reload plugins. Pool or plugin changes require a full restart.

### Connection pool configuration

```toml
[data.datasources.orders_db.connection_pool]
min_idle           = 2
max_size           = 20
connection_timeout = 5000      # ms
idle_timeout       = 600000    # ms
test_query         = "SELECT 1"
```

### Troubleshooting

**Service not starting:**

```
ERROR rivers::deploy: required resource '{name}' lockbox alias '{alias}' not found
ERROR rivers::deploy: port {port} is already bound
ERROR rivers::schema: attribute_validation_failed
```

Resolution: verify LockBox alias, check port availability, fix schema attribute errors.

**DataView errors:**

```
ERROR rivers::dataview: schema_file_not_found
ERROR rivers::dataview: unsupported_schema_attribute
ERROR rivers::driver: connection_failed
```

Resolution: verify schema file path, check schema attributes match driver, verify datasource connectivity.

**Driver connection failures:**

```
WARN  rivers::datasource: DatasourceCircuitOpened
WARN  rivers::pool: ConnectionPoolExhausted
ERROR rivers::driver: connection_timeout
```

Resolution: check database/service availability, verify credentials in LockBox, check network connectivity, increase pool size if exhausted.

**Rate limiting:**

Response headers indicate rate limit state:

```
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1710342060
```

Resolution: increase `rate_limit_per_minute`, implement client-side backoff, or switch to `custom_header` strategy with API keys.

### Restart

```bash
systemctl restart riversd
```

Full restart is required for: connection pool config changes, plugin changes, TLS certificate changes, admin API config changes.

---

## ExecDriver Operations

### Script Management

Scripts executed by the ExecDriver must be:
- Declared in datasource config with absolute path and SHA-256 hash
- Owned by root or a deploy user (not by `run_as_user`)
- Mode `0555` (read + execute, no write)

Recommended layout:
```
/usr/lib/rivers/scripts/    # scripts (root-owned, 0555)
/etc/rivers/exec-schemas/   # JSON Schema files (root-owned, 0444)
/var/rivers/exec-scratch/   # working directory (rivers-exec-owned, 0700)
```

### Hash Management

When updating a script, update the SHA-256 hash in the datasource config:

```bash
riversctl exec hash /usr/lib/rivers/scripts/netscan.py
# Copy output into your TOML config
```

### Script Contract

Scripts must follow this I/O contract:
- **Input:** Read JSON from stdin (stdin mode) and/or parse argv (args mode)
- **Output:** Write a single JSON document to stdout
- **Errors:** Write diagnostics to stderr, exit with non-zero code
- **No interactivity:** No TTY reads or prompts

### Security Checklist

- [ ] `run_as_user` is a dedicated restricted account (not root, not the riversd user)
- [ ] Scripts are owned by root/deploy user, mode 0555
- [ ] `env_clear = true` (default) — no env leakage
- [ ] `integrity_check = "each_time"` for sensitive commands
- [ ] JSON Schema validation enabled for all commands that accept user input
- [ ] `max_concurrent` limits set to prevent resource exhaustion

---

## AW3.16 Missing Driver Handling (v0.54.0)

When an app declares datasources with drivers that cannot be resolved, the failure is isolated to that app rather than aborting the whole bundle.

### Behavior

- The app is **blocked from loading**. Its views are **not** registered in the router.
- Init handlers for the failed app are **skipped**.
- Requests to endpoints in the failed app return `503 Service Unavailable`:

  ```json
  {
    "code": 503,
    "message": "app 'canary-nosql' is unavailable — missing driver(s): mongodb, elasticsearch"
  }
  ```

- Other apps in the same bundle load and serve traffic normally.
- A structured `AppLoadFailed` event is written to `log/apps/<app>.log` listing the missing driver names and the resources that referenced them.

### Operator checklist

1. Check `log/apps/<app>.log` for the `AppLoadFailed` event.
2. If the missing driver comes from a `[plugins] dir` entry, remove that config key — all drivers are now compiled into `riversd` (see AW3.18).
3. Verify `resources.toml` uses a known driver name (`postgres`, `mysql`, `sqlite`, `redis`, `faker`, `mongodb`, `elasticsearch`, `couchdb`, `cassandra`, `ldap`, `kafka`, `rabbitmq`, `nats`, `neo4j`, `influxdb`, `redis-streams`, `exec`).

### Failed app tracking

`riversd` keeps a `failed_apps` registry in memory. The router middleware consults it before dispatching — any request whose app is in `failed_apps` short-circuits to 503 with the structured error body above. On a successful hot reload that fixes the missing driver, the app is removed from `failed_apps` and its views begin serving normally.

---

## AW3.17 Startup Bundle Validation (v0.54.0)

`riversd` runs the `riverpackage` 4-layer validation pipeline at startup before loading the bundle. Invalid bundles are **rejected** before any driver is initialized.

The pipeline layers:

1. **Structural** — TOML parse of bundle/app manifests, `resources.toml`, `app.toml`
2. **Existence** — all referenced files (schemas, handler modules, libraries) exist
3. **Cross-reference** — DataViews resolve to datasources, views resolve to DataViews, services resolve
4. **Syntax** — JSON schemas parse, TS/JS handler modules compile via V8

Run the same pipeline locally or in CI with:

```bash
riverpackage validate ./my-bundle
riverpackage validate ./my-bundle --format json
riverpackage validate ./my-bundle --config /opt/rivers/config/riversd.toml
```

A bundle that passes `riverpackage validate` on the same Rivers version will load cleanly on the server.

---

## AW3.18 Static Plugin Mode and the Deprecated `[plugins] dir` (v0.54.0)

### Background

Prior to v0.54.0, driver plugins (`mongodb`, `elasticsearch`, `couchdb`, `cassandra`, `ldap`, `kafka`, `rabbitmq`, `nats`, `neo4j`, `influxdb`, `redis-streams`, `exec`) shipped as cdylib files in `plugins/` and were loaded by `riversd` at startup. In testing, this produced SIGABRT crashes caused by a **tokio ABI mismatch across the FFI boundary** — the tokio runtime inside the plugin and the one inside `riversd` did not agree on type layout.

### Current behavior

- cdylib plugin loading is **disabled**.
- All 12 former plugin drivers, plus the 5 built-in drivers (`sqlite`, `postgres`, `mysql`, `redis`, `faker`), are compiled into the `riversd` binary via the `static-plugins` cargo feature.
- The `[plugins] dir` config key is **deprecated**. If set, `riversd` logs a warning at startup and proceeds without loading anything from the directory.

### Config migration

Remove the `[plugins]` section from your `riversd.toml`:

```toml
# DELETE in v0.54.0 — no longer has any effect
# [plugins]
# dir = "/opt/rivers/plugins"
```

Engine dylibs (`librivers_engine_v8`, `librivers_engine_wasm`) are unaffected — they still ship as dylibs in dynamic-mode deploys under `[engines] dir`.

### Future: Plugin ABI v2

A new plugin ABI is planned to re-enable dynamic driver loading. It uses a **synchronous C-ABI** that avoids the tokio-across-FFI problem entirely: plugins expose blocking functions and `riversd` wraps them in `tokio::task::spawn_blocking` on its side. See `docs/arch/rivers-plugin-abi-v2-spec.md` for the design.

---

## AW3.19 Guard Lifecycle Hooks (v0.54.0)

Guard views (views with `guard = true`, typically login endpoints) can now declare fire-and-forget lifecycle hooks that fire at key points in the session lifetime. Hooks are dispatched via `tokio::spawn` and **cannot influence the auth flow** — they run concurrently with the response and are intended for side-effects only (audit logging, metrics, external event emission).

### Configuration

```toml
[api.views.login]
path      = "/api/login"
method    = "POST"
view_type = "Rest"
auth      = "none"
guard     = true

[api.views.login.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/auth.ts"
entrypoint = "login"

[api.views.login.lifecycle_hooks]
on_session_valid.module      = "libraries/handlers/audit.ts"
on_session_valid.entrypoint  = "onSessionValid"
on_invalid_session.module    = "libraries/handlers/audit.ts"
on_invalid_session.entrypoint = "onInvalidSession"
on_failed.module             = "libraries/handlers/audit.ts"
on_failed.entrypoint         = "onLoginFailed"
```

### Hooks

| Hook | Fires when |
|------|-----------|
| `on_session_valid` | A session validation check succeeds (e.g., a protected request arrives with a still-valid session). |
| `on_invalid_session` | A session validation check fails (expired, revoked, unknown token). |
| `on_failed` | Guard credentials are rejected (wrong password, unknown user). |

### Contract

- Hooks are **fire-and-forget**. Return values are ignored.
- Hooks **cannot block** or extend the request. They must not perform long-running work.
- Hooks run in the ProcessPool on a best-effort basis. A hook failure does not affect the originating request.
- Hook handlers receive the same `ctx` shape as normal CodeComponent handlers but should treat it as read-only.

---

## AW3.20 DDL in Init Handlers (v0.54.0)

Init handlers (TypeScript / JavaScript modules that run once per app at load time) can now execute DDL statements against a datasource via `ctx.ddl(datasource, statement)`. This is useful for creating tables, indexes, or other schema objects during app startup — for example, a canary app that wants to seed its own test schema.

### Handler usage

```typescript
// libraries/handlers/init.ts
export function init(ctx: InitContext): void {
  ctx.ddl("orders_db", "CREATE TABLE IF NOT EXISTS audit_log (id SERIAL PRIMARY KEY, message TEXT)");
  ctx.ddl("orders_db", "CREATE INDEX IF NOT EXISTS idx_audit_log_id ON audit_log (id)");
}
```

### Gate 3 whitelist

DDL execution is gated by a `ddl_whitelist` in `[security]`. Each entry is `"<datasource>@<app_id>"`:

```toml
[security]
ddl_whitelist = [
  "orders_db@c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a",
  "audit_db@c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a",
]
```

A `ctx.ddl(...)` call from an app that is not on the whitelist for the requested datasource is **rejected** at the gate. The init handler receives an error and the rejection is logged.

### Events

DDL calls emit structured events to the per-app log (`log/apps/<app>.log`):

| Event | Level | When |
|-------|-------|------|
| `DdlExecuted` | Info | Statement executed successfully. |
| `DdlFailed` | Error | Statement reached the datasource but failed (syntax error, permission denied, etc.). |
| `DdlRejected` | Warn | Statement blocked at Gate 3 — datasource@app_id not in `ddl_whitelist`. |

### Engine support

`ctx.ddl` works in both execution modes:

- **Static builds** — ProcessPool V8 (compiled in)
- **Dynamic builds** — engine dylib V8 (loaded from `lib/librivers_engine_v8.dylib`)
