# Rivers V1 Administration — Operations Spec

**Rivers v0.50.1**

## Environment

Single-node Rivers V1 deployment. No clustering. RPS not applicable for app operations.

---

## Starting Rivers

```bash
riversd --config {app}/app.toml
riversd --version                    # Print version and exit
riversd --no-ssl --port 8080         # Plain HTTP (development only)
```

### Startup Order

1. app-services start first (declared in bundle `apps` array)
2. app-mains wait for service health checks to pass
3. Bundle `apps` array order is authoritative

### Startup Sequence

```
1.  Load and validate config
2.  Initialize EventBus
3.  Load plugins (driver plugins from plugin directory)
4.  Configure Lockbox resolver
5.  Configure session manager
6.  Configure pool manager (per-datasource connection pools)
7.  Configure DataView engine
8.  Configure runtime factory (ProcessPool — V8 + Wasmtime)
9.  Build main router (all routes registered)
10. Maybe spawn admin server (if admin_api.enabled)
11. Bind HTTP server
12. Wait for shutdown signal
```

### Startup Log Output

```
INFO  rivers::deploy: deploying bundle "orders-platform" v1.4.2
INFO  rivers::deploy: appDeployId assigned — orders-service:    deploy-7f3a1b2c
INFO  rivers::deploy: appDeployId assigned — app-main:          deploy-2b5f8e3d

INFO  rivers::deploy: resolving resources — orders-service
INFO  rivers::lockbox: resolved lockbox://postgres/orders-prod
WARN  rivers::lockbox: optional resource 'cache' lockbox alias 'redis/prod' not found — starting degraded

INFO  rivers::deploy: starting app-services (phase 2)
INFO  rivers::app: orders-service → STARTING (port 9001)
INFO  rivers::app: orders-service → RUNNING  (health check passed)

INFO  rivers::deploy: starting app-mains (phase 3)
INFO  rivers::app: app-main → STARTING (port 8080)
INFO  rivers::app: app-main → RUNNING

INFO  rivers::deploy: bundle "orders-platform" v1.4.2 deployed successfully
```

---

## Configuration

File: `app.toml` or `riversd.conf`

### Main Server

```toml
[base]
host    = "0.0.0.0"
port    = 8080
workers = 4                        # Tokio worker threads (default: num_cpus)
request_timeout_seconds = 30       # default

[base.backpressure]
enabled          = true
queue_depth      = 512
queue_timeout_ms = 100
```

### TLS Configuration

```toml
[base.tls]
cert = "/etc/rivers/tls/server.crt"    # optional — auto-generates if absent
key  = "/etc/rivers/tls/server.key"     # optional — auto-generates if absent
redirect = true                          # HTTP→HTTPS redirect on port 80
redirect_port = 80
```

TLS is mandatory. When cert/key paths are absent, Rivers auto-generates self-signed certificates in `{data_dir}/tls/`.

### Static Files

```toml
[static_files]
enabled       = true
root_path     = "/var/rivers/app/dist"
index_file    = "index.html"
spa_fallback  = true
max_age       = 86400
exclude_paths = [".env", "config.toml"]
```

### Rate Limiting

```toml
[security]
rate_limit_per_minute   = 120
rate_limit_burst_size   = 60
rate_limit_strategy     = "ip"           # ip | custom_header
rate_limit_custom_header = "X-Api-Key"   # required if strategy = custom_header
```

### CORS

```toml
[security]
cors_enabled           = true
cors_allowed_origins   = ["https://app.example.com"]
cors_allowed_methods   = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
cors_allowed_headers   = ["Content-Type", "Authorization", "X-Trace-Id"]
cors_allow_credentials = false
```

### Storage Engine Configuration

```toml
[storage_engine]
backend = "sqlite"               # memory | sqlite | redis
path = "/var/data/rivers.db"     # sqlite path
url = "redis://localhost:6379"   # redis URL
retention_ms = 172800000         # 2 days
```

Required for sessions, CSRF, DataView L2 cache, polling state persistence. InMemory for development, SQLite for single-node, Redis for multi-node.

### GraphQL Configuration

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

### Environment Overrides

```toml
[environment_overrides.prod.base]
host                    = "0.0.0.0"
port                    = 443
request_timeout_seconds = 60

[environment_overrides.prod.base.backpressure]
queue_depth = 1024

[environment_overrides.prod.security]
rate_limit_per_minute = 300
```

---

## Bundle Validation

`riversctl validate <bundle_path>` runs 9 checks against a bundle directory or archive.

`riversctl validate --schema server|app|bundle` outputs the corresponding JSON Schema.

### Validation Checks

1. View types — validates view type values are recognized
2. Driver names — validates driver names match registered drivers
3. Datasource refs — validates all datasource references resolve
4. DataView refs — validates all DataView references resolve
5. Invalidates targets — validates invalidation targets exist
6. Duplicate names — detects duplicate DataView/View/datasource names
7. Schema file existence — verifies all referenced schema files exist on disk
8. Cross-app service refs — validates inter-app service references resolve within the bundle
9. TOML parse error context — provides line/column context for TOML syntax errors

---

## Logging

### Configuration

```toml
[base.logging]
level           = "info"     # debug | info | warn | error
format          = "json"     # json | text
local_file_path = "/var/log/rivers/riversd.log"   # optional
```

Defaults: `level = "info"`, `format = "json"`, `local_file_path = null`.

### Log Levels

| Level | Use |
|-------|-----|
| `debug` | Verbose, development only |
| `info` | Normal operational events |
| `warn` | Degraded state, not fatal |
| `error` | Failure requiring attention |

### JSON Log Format

```json
{
  "timestamp": "2026-03-11T14:23:01.847Z",
  "level": "info",
  "message": "request completed",
  "trace_id": "a1b2c3d4-e5f6-...",
  "app_id": "riversd",
  "node_id": "node-1",
  "event_type": "RequestCompleted",
  "method": "GET",
  "path": "/api/orders/42",
  "status": 200,
  "latency_ms": 14
}
```

### Event-to-Level Mapping

| Event | Level |
|-------|-------|
| `RequestCompleted` | Info |
| `DataViewExecuted` | Info |
| `WebSocketConnected` / `Disconnected` | Info |
| `DatasourceConnected` | Info |
| `ConnectionPoolExhausted` | Warn |
| `DatasourceCircuitOpened` | Warn |
| `DatasourceDisconnected` | Warn |
| `DatasourceHealthCheckFailed` | Error |
| `PluginLoadFailed` | Error |

### Log Query Patterns

```
# All errors in the last hour
level = "error" | timestamp > now() - 1h

# All requests with latency > 500ms
event_type = "RequestCompleted" | latency_ms > 500

# Trace reconstruction
trace_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"

# Circuit breaker events
event_type = "DatasourceCircuitOpened"
```

---

## Health Endpoints

### Basic Health

```bash
curl http://localhost:8080/health
```

Response: `200 OK` with body `{"status": "healthy"}`

### Verbose Health

```bash
curl http://localhost:8080/health/verbose
```

Response includes datasource status, pool stats, circuit breaker state, and datasource probes:

```json
{
  "datasource_probes": [
    {"name": "pg", "driver": "postgres", "status": "ok", "latency_ms": 3},
    {"name": "cache", "driver": "redis", "status": "error", "latency_ms": 5000, "error": "timeout"}
  ]
}
```

MUST: Verbose health may require AuthZ if admin ACL is set.

---

## Admin API

### Configuration

```toml
[base.admin_api]
enabled     = true
host        = "127.0.0.1"
port        = 9443
public_key  = "/etc/rivers/admin/admin.pub"
private_key = "/etc/rivers/admin/admin.key"
```

### Admin TLS (mTLS)

```toml
[base.admin_api.tls]
ca_cert             = "/etc/rivers/admin/ca.crt"
server_cert         = "/etc/rivers/admin/server.crt"
server_key          = "/etc/rivers/admin/server.key"
require_client_cert = true
```

### RBAC Configuration

```toml
[base.admin_api.rbac.roles]
operator = ["status", "datasources", "drivers"]
deployer = ["status", "datasources", "drivers", "deploy"]

[base.admin_api.rbac.bindings]
"CN=admin-client" = "deployer"
```

### Admin Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/admin/status` | GET | Server status |
| `/admin/drivers` | GET | Registered drivers |
| `/admin/datasources` | GET | Datasource status |
| `/admin/deploy` | POST | Deploy bundle |
| `/admin/deploy/test` | POST | Test deployment |
| `/admin/deploy/approve` | POST | Approve staged deploy |
| `/admin/deploy/reject` | POST | Reject staged deploy |
| `/admin/deploy/promote` | POST | Promote approved deployment |
| `/admin/deployments` | GET | List deployments |
| `/admin/log/levels` | GET | Current log levels |
| `/admin/log/set` | POST | Change log level at runtime. Body: `{"target": "global", "level": "debug"}` |
| `/admin/log/reset` | POST | Reset log levels to defaults |

### Admin Error Responses

Admin errors use the same ErrorResponse format as the main server: `{code, message, trace_id}`.

| Status | Condition |
|--------|-----------|
| 400 | Bad request (malformed body, invalid parameters) |
| 404 | Deployment not found |
| 503 | Log controller unavailable |

### Emergency Access

```bash
riversd --no-admin-auth
```

Disables Ed25519 signature verification for this process lifetime only. Session-scoped — does not persist across restarts.

MUST: Emits `tracing::warn!` at startup.
MUST: Use only for initial setup and break-glass scenarios.

---

## Middleware Stack

Main server middleware order (outermost to innermost):

```
0. compression              — gzip/deflate/br compression (outermost)
1. cors                     — CORS preflight and response headers
2. body_limit (16 MiB)      — hard body size cap
3. trace_id                  — extract/generate trace_id
4. security_headers          — inject security response headers
5. shutdown_guard            — reject new requests during drain
6. backpressure              — semaphore-based queue
7. timeout                   — per-request timeout (30s default)
8. request_observer          — publish RequestCompleted to EventBus (innermost)
   └─ route handler
```

Rate limiting and session validation are handled per-view at dispatch time, not as global middleware layers.

### Security Headers

Set on every response:

| Header | Value |
|--------|-------|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `X-XSS-Protection` | `1; mode=block` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |

MUST NOT: Set `Strict-Transport-Security` automatically — set at reverse proxy.

---

## Graceful Shutdown

On SIGTERM/SIGINT:

```
1. Stop accepting new connections
2. shutdown_guard_middleware returns 503 for new requests
3. Wait for in-flight requests to complete (drain timeout)
4. Close datasource connections
5. Stop ProcessPool workers
6. Exit
```

### Drain Behavior

- In-flight requests complete normally
- New requests receive 503 Service Unavailable
- WebSocket connections receive close frame
- SSE connections close gracefully

---

## Circuit Breaker

Per-datasource circuit breaker protects against cascading failures.

### States

| State | Behavior |
|-------|----------|
| Closed | Normal operation |
| Open | All requests fail immediately |
| Half-Open | Single test request allowed |

### Configuration

```toml
[data.datasources.orders_db.circuit_breaker]
failure_threshold    = 5      # failures before opening
cooldown_seconds     = 30     # time in open state before half-open
success_threshold    = 2      # successes in half-open before closing
```

### Events

- `DatasourceCircuitOpened` — logged at Warn level
- `DatasourceCircuitClosed` — logged at Info level

---

## Connection Pool

Per-datasource connection pool configuration:

```toml
[data.datasources.orders_db.connection_pool]
min_idle           = 2
max_size           = 20
connection_timeout = 5000      # ms
idle_timeout       = 600000    # ms
test_query         = "SELECT 1"
```

### Events

- `ConnectionPoolExhausted` — logged at Warn level
- `DatasourceConnected` — logged at Info level
- `DatasourceDisconnected` — logged at Warn level

---

## Hot Reload (Dev Mode Only)

Development mode only. Disabled in production.

```toml
[hot_reload]
enabled    = true
watch_path = "./app.toml"
```

### What Hot Reload Does

- Reloads View routes
- Reloads DataView configs
- Reloads DataView engine
- Reloads static file config
- Reloads security config
- Reloads GraphQL schema
- Reloads bundle validation

### What Hot Reload Does NOT Do

- Restart HTTP server
- Rebind sockets
- Re-initialize connection pools
- Reload plugins
- Re-resolve LockBox credentials

MUST: Pool changes require full restart.

---

## OpenTelemetry Tracing

### Configuration

```toml
[performance.tracing]
enabled       = true
provider      = "otlp"                        # otlp | jaeger | datadog
endpoint      = "http://otel-collector:4317"
service_name  = "riversd"
sampling_rate = 0.1                           # 10% sampling in production
```

### Span Hierarchy

```
http.request  (root span)
    └─ view.dispatch
            ├─ dataview.execute
            │       └─ driver.execute
            └─ codecomponent.execute
```

---

## Error Response Format

All error responses from Rivers (not from CodeComponent handlers) use the SHAPE-2 format:

```json
{
  "code": 500,
  "message": "human-readable error message",
  "details": "optional diagnostic info",
  "trace_id": "abc-123"
}
```

### Status Code Mapping

| Status | Condition |
|--------|-----------|
| 400 | Invalid request, parameter validation failed |
| 401 | Admin auth failed, missing signature |
| 403 | RBAC permission denied, IP allowlist rejected |
| 404 | View not found, static file not found |
| 405 | Method not allowed |
| 408 | Request timeout |
| 422 | Schema validation failed, unprocessable entity |
| 429 | Rate limit exceeded |
| 500 | Runtime execution failed, internal error |
| 503 | Server draining, backpressure exhausted, circuit open |

---

## Troubleshooting

### Service Not Starting

Check logs for:
```
ERROR rivers::deploy: required resource '{name}' lockbox alias '{alias}' not found
ERROR rivers::deploy: port {port} is already bound
ERROR rivers::schema: attribute_validation_failed
```

Resolution:
1. Verify Lockbox alias is provisioned
2. Check port availability
3. Fix schema attribute errors

### DataView Errors

Check logs for:
```
ERROR rivers::dataview: schema_file_not_found
ERROR rivers::dataview: unsupported_schema_attribute
ERROR rivers::driver: connection_failed
```

Resolution:
1. Verify schema file path exists
2. Check schema attributes match driver
3. Verify datasource connectivity

### Driver Connection Failures

Check logs for:
```
WARN  rivers::datasource: DatasourceCircuitOpened
WARN  rivers::pool: ConnectionPoolExhausted
ERROR rivers::driver: connection_timeout
```

Resolution:
1. Check database/service availability
2. Verify credentials in Lockbox
3. Check network connectivity
4. Increase pool size if exhausted

### Rate Limiting

Check response headers:
```
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1710342060
```

Resolution:
1. Increase `rate_limit_per_minute` in config
2. Implement client-side backoff
3. Use `rate_limit_strategy = "custom_header"` for API keys

### Plugin Load Failures

Check logs for:
```
ERROR rivers::plugin: PluginLoadFailed
  path   = "/var/rivers/plugins/neo4j.so"
  reason = "ABI version mismatch: expected 3, got 2"
```

Resolution:
1. Rebuild plugin against current Rivers SDK
2. Verify plugin file permissions
3. Check plugin path in config

---

## Common Operations

### Verify App Health

```bash
curl http://localhost:8080/health
curl http://localhost:8080/health/verbose
```

### Check Logs

```bash
# JSON logs to stdout (default)
journalctl -u riversd -f

# Or if local_file_path is set
tail -f /var/log/rivers/riversd.log

# Filter by level
journalctl -u riversd | jq 'select(.level == "error")'

# Filter by trace_id
journalctl -u riversd | jq 'select(.trace_id == "a1b2c3d4...")'
```

### Check Datasource Status

```bash
curl --cert client.crt --key client.key \
  https://localhost:9443/admin/datasources
```

### Deploy Bundle

```bash
curl --cert client.crt --key client.key \
  -X POST \
  -F "bundle=@bundle.zip" \
  https://localhost:9443/admin/deploy
```

### Restart Service

```bash
systemctl restart riversd
```

MUST: Full restart required for pool config changes.
MUST: Full restart required for plugin changes.
