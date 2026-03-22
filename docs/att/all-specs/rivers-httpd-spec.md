# Rivers HTTPD Specification

**Document Type:** Implementation Specification  
**Scope:** HTTP server, middleware stack, TLS, HTTP/2, static files, SPA, CORS, rate limiting, backpressure, admin API, health, graceful shutdown  
**Status:** Reference / Ground Truth  
**Source audit:** `crates/riversd/src/lib.rs`, `crates/rivers-core/src/config.rs`

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Startup Sequence](#2-startup-sequence)
3. [Router Structure](#3-router-structure)
4. [Middleware Stack](#4-middleware-stack)
5. [TLS Configuration](#5-tls-configuration)
6. [HTTP/2](#6-http2)
7. [Static File Serving](#7-static-file-serving)
8. [SPA Fallback](#8-spa-fallback)
9. [CORS](#9-cors)
10. [Rate Limiting](#10-rate-limiting)
11. [Backpressure](#11-backpressure)
12. [Session Management](#12-session-management)
13. [Graceful Shutdown](#13-graceful-shutdown)
14. [Health Endpoints](#14-health-endpoints)
15. [Admin API](#15-admin-api)
16. [Hot Reload](#16-hot-reload)
17. [Security Headers](#17-security-headers)
18. [Error Response Format](#18-error-response-format)
19. [Configuration Reference](#19-configuration-reference)

---

## 1. Architecture Overview

Rivers runs two HTTP servers on separate sockets:

**Main server** — serves application traffic. All View routes, static files, health endpoint, gossip receiver. Full middleware stack applied.

**Admin server** — serves operational endpoints. Separate socket, separate middleware stack (subset), optional separate TLS. Spawned only when `admin_api.enabled = true`.

Both are Axum-based, built on `hyper` and `tokio`. TLS via `rustls` through `axum-server`.

```
┌─────────────────────────────────────────────────────────────────┐
│  Main server  (host:port)                                       │
│                                                                 │
│  /health              → health handler                          │
│  /health/verbose      → verbose health handler                  │
│  /gossip/receive      → gossip handler                          │
│  /graphql             → GraphQL handler (if configured)         │
│  /api/...             → View routes (REST, WS, SSE)             │
│  /                    → static file / SPA fallback              │
│  /{*path}             → static file / SPA fallback              │
│                                                                 │
│  Middleware: trace_id → request_observer → timeout →            │
│             backpressure → shutdown_guard → rate_limit →        │
│             session → security_headers → compression            │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  Admin server  (admin_api.host:admin_api.port)                  │
│                                                                 │
│  /admin/status           → status handler                       │
│  /admin/drivers          → drivers handler                      │
│  /admin/datasources      → datasources handler                  │
│  /admin/deploy           → deploy handler                       │
│  /admin/deploy/test      → deploy test handler                  │
│  /admin/deploy/approve   → deploy approve handler               │
│  /admin/deploy/reject    → deploy reject handler                │
│  /admin/deploy/promote   → deploy promote handler               │
│  /admin/deployments      → deployments list handler             │
│                                                                 │
│  Middleware: trace_id → timeout → security_headers              │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Startup Sequence

`run_server_with_listener_with_control` is the single entry point for all server startup paths. Called by `main()`, tests, and integration harnesses.

```
1.  apply_bundle_config(&mut config)
       │  Merge App Bundle manifest into ServerConfig (static files, views, dataviews)
       │
2.  config.validate()
       │  All config validation — fails fast before any resources are allocated
       │
3.  validate_http2_runtime(&config)
       │  HTTP/2 requires TLS certs — rejects HTTP/2 without TLS
       │
4.  initialize_global(config.clone())
       │  OnceLock-based global state singleton
       │
5.  configure_metrics_registry()
       │  Prometheus-compatible metrics registry
       │
6.  configure_event_bus(&config, metrics_registry)
       │  EventBus singleton: priority tiers, LogHandler subscription at Observe
       │
7.  load_plugins(&config, event_bus)
       │  Plugin directory scan → ABI check → catch_unwind registration
       │
8.  configure_lockbox_resolver(&config, event_bus)
       │  Lockbox backend (InMemory/SOPS/Vault/AWS/Infisical)
       │
9.  configure_discovery_runtime / consensus_runtime / gossip_runtime
       │  Cluster layer (no-op for single-node)
       │
10. configure_session_manager(&config, &cluster_state, &gossip_state)
       │
11. configure_pool_manager(&config, event_bus)
       │  Per-datasource connection pools, circuit breakers
       │
12. configure_dataview_engine(&config, pool_manager, event_bus)
       │  DataViewRegistry + DataViewEngine + TieredDataViewCache
       │
13. configure_graphql_runtime(&config, dataview_engine)
       │
14. configure_runtime_factory(&config, pool_manager)
       │  ProcessPool (V8 + Wasmtime executors)
       │
15. configure_topic_registry (if any datasource uses driver = "eventbus")
       │
16. Build AppContext — all subsystems wired together
       │
17. Build main router — all routes registered
       │
18. maybe_spawn_admin_server — admin router on separate socket
       │
19. maybe_spawn_hot_reload_watcher (dev mode only)
       │
20. Bind TcpListener (or use pre-bound listener from test harness)
       │
21. axum::serve (plain) or axum_server::bind_rustls (TLS)
       │
22. shutdown_signal task — waits for SIGTERM/SIGINT/watch channel
```

---

## 3. Router Structure

Route registration order within the main router:

```
1. /health              GET  (health)
2. /health/verbose      GET  (health_verbose — requires AuthZ if admin ACL set)
3. /gossip/receive      POST (gossip_receive_handler)
4. GraphQL routes       (append_graphql_routes — /graphql GET + POST if enabled)
5. View routes          (append_api_view_routes — from config.api.views)
6. Static file routes   (append_static_file_routes — / and /{*path} if enabled)
```

Static file routes are registered last. API view routes take precedence because they are registered first. Axum route matching is first-match within a router — a `/{*path}` catch-all registered after explicit routes does not shadow them.

Gossip endpoint is always registered, regardless of cluster mode. On single-node deployments it receives gossip from RPS during provisioning. It is not authenticated at the HTTP layer — gossip messages carry their own HMAC.

---

## 4. Middleware Stack

Applied via Axum's `layer()` chain. Layers apply in reverse registration order — the last `.layer()` call wraps outermost (first to execute on request, last on response).

**Main server middleware order (outermost to innermost):**

```
1. CompressionLayer          — gzip/deflate/br compression of responses
2. DefaultBodyLimit::max(16 MiB) — hard body size cap (SEC-7)
3. security_headers_middleware — inject security response headers (SEC-12)
4. session_middleware         — session cookie parse and set
5. rate_limit_middleware      — token bucket rate limiting
6. shutdown_guard_middleware  — reject new requests during drain
7. backpressure_middleware    — semaphore-based queue
8. timeout_middleware         — per-request timeout (default: 30s)
9. request_observer_middleware — publish RequestCompleted to EventBus
10. trace_id_middleware        — extract/generate trace_id, inject headers
    │
    └─ route handler
```

**Admin server middleware (outermost to innermost):**

```
1. DefaultBodyLimit::max(16 MiB)
2. security_headers_middleware
3. timeout_middleware
4. trace_id_middleware
    │
    └─ route handler (includes admin auth check inline)
```

Admin server has no session, rate limit, backpressure, or compression layers. Admin requests are low-volume operational calls; throughput optimization is not the goal.

---

## 5. TLS Configuration

### 5.1 Main server TLS

TLS is optional for the main server. When configured:

```toml
[base.http2]
enabled   = true
tls_cert  = "/etc/rivers/tls/server.crt"
tls_key   = "/etc/rivers/tls/server.key"
```

Implementation: `axum_server::bind_rustls(addr, RustlsConfig::from_pem_file(cert, key))`.

When TLS is not configured, Axum serves over plain HTTP via `TcpListener::bind` + `axum::serve`.

### 5.2 Admin server TLS

Admin server has independent TLS configuration:

```rust
pub struct AdminTlsConfig {
    pub ca_cert: String,          // CA cert for mTLS client verification
    pub server_cert: String,
    pub server_key: String,
    pub require_client_cert: bool,
}
```

When `require_client_cert = true`, mTLS is enforced — clients without a valid cert signed by `ca_cert` are rejected at the TLS handshake. This is the recommended production configuration.

When `server_cert` and `server_key` are set but `require_client_cert = false`: one-way TLS. Traffic is encrypted but no client cert is required.

When neither cert is set: admin server binds plain HTTP. Only acceptable with `host = "127.0.0.1"` (enforced — see [§15.2](#152-localhost-binding-enforcement)).

### 5.3 rustls configuration for mTLS

```rust
// From build_admin_tls_config_mtls()
let mut root_store = RootCertStore::empty();
root_store.add_parsable_certificates(&ca_certs);
let config = ServerConfig::builder()
    .with_client_cert_verifier(
        WebPkiClientVerifier::builder(Arc::new(root_store)).build()?
    )
    .with_single_cert(server_certs, private_key)?;
```

---

## 6. HTTP/2

HTTP/2 requires TLS (ALPN negotiation). Enabling HTTP/2 without TLS is rejected at startup with a validation error.

```rust
pub struct Http2Config {
    pub enabled: bool,
    pub initial_window_size: Option<u32>,    // flow control window
    pub max_concurrent_streams: Option<u32>, // per-connection stream limit
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}
```

When `http2.enabled = true` and TLS certs are configured, `axum_server::bind_rustls` is used with `axum_server::Server::http2_only(false)` (h2 + h1.1 via ALPN negotiation). Rivers does not support h2c (HTTP/2 cleartext).

`initial_window_size` and `max_concurrent_streams` are passed directly to the hyper HTTP/2 builder if set. `None` uses hyper defaults (65535 bytes, unlimited streams).

---

## 7. Static File Serving

Enabled when `static_files.enabled = true`. Routes registered: `GET /` and `GET /{*path}`.

### 7.1 Request flow

```
GET /some/path
    │
    ├─ is_static_excluded_path? (path in exclude_paths list)
    │       → 404 if excluded
    │
    ├─ resolve_static_file_path(root, path, index_file, spa_fallback)
    │       → None → 404
    │       → Some(file_path)
    │
    ├─ tokio::fs::metadata(file_path) — file must exist and be a regular file
    │
    ├─ tokio::fs::read(file_path) — reads entire file into memory
    │
    ├─ SHA-256 ETag generation — hex digest of file bytes
    │
    ├─ If-None-Match check — 304 Not Modified if ETag matches
    │
    └─ 200 OK with Content-Type + Cache-Control + ETag headers
```

### 7.2 Path resolution

```rust
fn resolve_static_file_path(
    root: &Path,
    requested: &str,
    index_file: &str,
    spa_fallback: bool,
) -> Option<PathBuf>
```

1. Empty path → `root/index_file`
2. Normalize path components — `ParentDir (..)` and absolute roots are rejected, returning `None` (path traversal prevention)
3. `root/normalized_path` exists → return it
4. Does not exist + `spa_fallback = true` → return `root/index_file`
5. Does not exist + `spa_fallback = false` → `None` → 404

### 7.3 Response headers

```
Content-Type:  inferred from file extension (mime_guess)
Cache-Control: public, max-age={max_age}
ETag:          "{sha256_hex}"
```

`max_age` is in seconds, from `static_files.max_age` config. Default: 3600 (1 hour).

### 7.4 StaticFilesConfig

```rust
pub struct StaticFilesConfig {
    pub enabled: bool,
    pub root_path: String,    // absolute path to static file root
    pub index_file: String,   // default: "index.html"
    pub spa_fallback: bool,   // default: false
    pub max_age: u64,         // Cache-Control max-age seconds
}
```

`exclude_paths` is a `Vec<String>` — paths that return 404 even if the file exists. Used to prevent serving sensitive files (`.env`, config files) that might be co-located with the static root.

---

## 8. SPA Fallback

When `spa_fallback = true`, any path that does not resolve to an existing file returns `root/index_file` instead of 404. This allows client-side routers (React Router, Vue Router, Angular) to handle routing.

**Ordering guarantee**: API view routes are registered before static file routes. A request to `/api/orders/42` matches the view route, not the SPA fallback. The fallback only activates for paths that don't match any registered route and don't correspond to an existing file.

SPA configuration via App Bundle manifest:

```json
{
  "spa_config": {
    "root_path": "dist/",
    "index_file": "index.html",
    "spa_fallback": true,
    "max_age": 86400
  }
}
```

Bundle deployment applies this config into `ServerConfig.static_files` automatically.

---

## 9. CORS

CORS is opt-in per view (via `cors_enabled` on `ApiViewConfig`) or globally via `security.cors_enabled`. Per-view setting overrides global.

### 9.1 CORS header injection

CORS headers are set on the response after handler execution. The CORS decision (`should_enable_cors`) is evaluated per-request from the matched view config.

Handler-set CORS headers are blocked (SEC-8) — CodeComponent handlers cannot set `access-control-*` headers. Rivers sets them based on config, not handler output.

### 9.2 Origin matching

```
cors_allowed_origins = ["*"]        → Access-Control-Allow-Origin: *
cors_allowed_origins = ["https://app.example.com"]
    + Origin: https://app.example.com → Access-Control-Allow-Origin: https://app.example.com
    + Origin: https://evil.com        → no CORS headers set (request proceeds, but no CORS header)
```

Wildcard `"*"` is incompatible with `cors_allow_credentials = true` — validation rejects this combination at startup.

### 9.3 SecurityConfig CORS fields

```rust
pub struct SecurityConfig {
    pub cors_enabled: bool,
    pub cors_allowed_origins: Vec<String>,
    pub cors_allowed_methods: Vec<String>,
    pub cors_allowed_headers: Vec<String>,
    pub cors_allow_credentials: bool,
    pub rate_limit_per_minute: u32,          // default: 120
    pub rate_limit_burst_size: u32,          // default: 60
    pub rate_limit_strategy: RateLimitStrategy,
    pub rate_limit_custom_header: Option<String>,
    pub admin_ip_allowlist: Vec<String>,
}
```

---

## 10. Rate Limiting

Token bucket algorithm. Per-key state stored in `RateLimiter` (in-process, not shared across nodes).

### 10.1 Algorithm

```
capacity      = burst_size
refill_rate   = requests_per_minute / 60_000  (tokens per ms)

on each request:
    tokens += elapsed_ms * refill_rate
    tokens  = min(tokens, capacity)
    if tokens >= 1.0:
        tokens -= 1.0
        → allow
    else:
        retry_after = ceil((1.0 - tokens) / refill_rate) ms
        → 429 Too Many Requests + Retry-After header
```

### 10.2 Rate limit key

Determined by `rate_limit_strategy`:

- `Ip` (default) — remote IP from connection. Does not inspect `X-Forwarded-For` (not trusted without explicit proxy config).
- `CustomHeader` — value of `rate_limit_custom_header`. Useful for API keys passed in a header. If header is absent, falls back to IP.

### 10.3 Bucket eviction

`RATE_LIMIT_MAX_BUCKETS = 10_000`. When the bucket map reaches this size:
1. Evict buckets last seen > 5 minutes ago
2. If still over limit after stale eviction: sort by `last_refill_epoch_ms`, remove oldest 50% of entries

This bounds memory regardless of the number of unique clients.

### 10.4 Per-view override

```rust
pub rate_limit_per_minute: Option<u32>,   // overrides security.rate_limit_per_minute
pub rate_limit_burst_size: Option<u32>,   // overrides security.rate_limit_burst_size
```

`None` means use global config. The per-view bucket key includes the view ID to keep per-view budgets isolated from the global budget.

### 10.5 Response

```
HTTP 429 Too Many Requests
Retry-After: {seconds}
{"error": "rate limit exceeded"}
```

---

## 11. Backpressure

Semaphore-based request queue. Limits total concurrent inflight requests to protect downstream resources (pool connections, ProcessPool slots).

### 11.1 Mechanism

`Arc<Semaphore>` with `queue_depth` permits. On each request:
1. `tokio::time::timeout(queue_timeout_ms, semaphore.acquire())` — attempt to acquire a permit
2. Timeout or semaphore closed → `503 Service Unavailable` + `Retry-After: 1`
3. Permit acquired → run handler → drop permit on response

The semaphore is released via `drop(permit)` at response return, not when the handler finishes writing. For streaming responses (SSE, WebSocket), the permit is held for the connection lifetime.

### 11.2 BackpressureConfig

```rust
pub struct BackpressureConfig {
    pub enabled: bool,
    pub queue_depth: usize,     // default: 512
    pub queue_timeout_ms: u64,  // default: 100
}
```

`queue_depth = 512` means up to 512 concurrent requests. The 513th request waits up to `queue_timeout_ms` for a slot. After that, 503.

`enabled = false` bypasses the middleware entirely — no semaphore acquire, no 503 risk from queue exhaustion.

### 11.3 Response when exhausted

```
HTTP 503 Service Unavailable
Retry-After: 1
{"error": "server overloaded; retry later"}
```

---

## 12. Session Management

Cookie-based session, distributed via gossip.

### 12.1 Session middleware flow

```
request arrives
    │
    ├─ parse cookie: rivers_session_id (default cookie name)
    │
    ├─ if cookie present:
    │       SessionManager::get_session(id)
    │       → session expired or not found: clear_cookie = true, id = None
    │       → valid: inject session data into request extensions
    │
    ├─ if no cookie and session not required:
    │       generate new session_id
    │       SessionManager::create_session(id)
    │       set_cookie = true
    │
    └─ after handler:
            set_cookie → Set-Cookie: {cookie_name}={id}; HttpOnly; SameSite=Lax; Path=/
            clear_cookie → Set-Cookie: {cookie_name}=; Max-Age=0; ...
```

### 12.2 Session propagation

Session creates/updates are broadcast via `GossipPayload::SessionUpserted { session_id }`. Session deletes via `GossipPayload::SessionDeleted { session_id }`. All nodes in the cluster maintain a consistent session store.

### 12.3 SessionConfig

```rust
pub struct SessionConfig {
    pub enabled: bool,
    pub cookie_name: String,   // default: "rivers_session_id"
}
```

Cookie attributes are hardcoded: `HttpOnly`, `SameSite=Lax`, `Path=/`. `Secure` is set when TLS is enabled. These are not configurable — they represent the minimum safe session cookie posture.

---

## 13. Graceful Shutdown

### 13.1 Shutdown signal sources

```rust
// Unix: SIGTERM or SIGINT
// Non-Unix: SIGINT only
// Watch channel: for programmatic shutdown (test harness, riversctl)
tokio::select! {
    _ = ctrl_c => {},
    _ = sigterm => {},
    _ = watch_channel_changed_to_true => {},
}
```

### 13.2 ShutdownCoordinator

```rust
struct ShutdownCoordinator {
    draining: AtomicBool,
    inflight: AtomicUsize,
    notify: Notify,
}
```

On shutdown signal: `coordinator.mark_draining()` sets `draining = true` and logs `"shutdown signal received; entering drain mode"`.

### 13.3 shutdown_guard_middleware

```rust
async fn shutdown_guard_middleware(State(state), request, next) -> Response {
    if state.shutdown.is_draining() {
        return (StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "server is shutting down"}))).into_response();
    }
    state.shutdown.inflight.fetch_add(1, Ordering::AcqRel);
    let response = next.run(request).await;
    state.shutdown.inflight.fetch_sub(1, Ordering::AcqRel);
    state.shutdown.notify.notify_waiters();
    response
}
```

New requests during drain receive `503 Service Unavailable`. Inflight requests complete normally.

### 13.4 Drain wait

After `mark_draining()`, the shutdown task waits for `inflight == 0`:

```rust
while coordinator.inflight.load(Ordering::Acquire) > 0 {
    coordinator.notify.notified().await;
}
```

No hard timeout on drain in the current implementation — if a request never completes, drain waits indefinitely. For production, load balancers should stop sending traffic before the drain window.

### 13.5 Pool and bridge drain

After inflight requests complete:
- All connection pools drain (existing connections complete, no new checkouts)
- `BrokerConsumerBridge` drain loop runs for `drain_timeout_ms`
- InfluxDB write batch flush runs

---

## 14. Health Endpoints

### 14.1 GET /health

Always returns 200. No auth. No rate limit bypass — subject to the full middleware stack.

```json
{
  "status": "ok",
  "service": "riversd",
  "environment": "production",
  "version": "0.9.0"
}
```

Upstream load balancers should use this endpoint for health checks.

### 14.2 GET /health/verbose

Returns extended status. Optionally accepts `?simulate_delay_ms=N` for testing timeout behavior.

```json
{
  "status": "ok",
  "service": "riversd",
  "environment": "production",
  "version": "0.9.0",
  "uptime_seconds": 3721,
  "pool_snapshots": [
    {
      "datasource_id": "orders_db",
      "active_connections": 3,
      "idle_connections": 7,
      "total_connections": 10,
      "checkout_count": 4821,
      "avg_wait_ms": 2,
      "max_size": 10,
      "min_idle": 0
    }
  ],
  "cluster": {
    "node_id": "node-1",
    "peers": ["node-2", "node-3"],
    "role": "leader"
  }
}
```

If `admin_ip_allowlist` is non-empty, `/health/verbose` is restricted to those IPs. `/health` (simple) is always public.

---

## 15. Admin API

The admin API is a separate HTTP server on a separate socket. It is disabled by default (`enabled = false`).

### 15.1 Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/admin/status` | Server status, config summary, driver list |
| GET | `/admin/drivers` | All registered driver names |
| GET | `/admin/datasources` | Pool snapshots for all datasources |
| POST | `/admin/deploy` | Upload and begin deployment |
| POST | `/admin/deploy/test` | Run deployment test stage |
| POST | `/admin/deploy/approve` | Approve a tested deployment |
| POST | `/admin/deploy/reject` | Reject a deployment |
| POST | `/admin/deploy/promote` | Promote approved deployment to active |
| GET | `/admin/deployments` | List all deployments and their status |

### 15.2 Localhost binding enforcement

When `admin_api.enabled = true` but no `public_key` is configured, the admin server **must** bind to `127.0.0.1` or `localhost`. Any other host is rejected at config validation. This prevents accidentally exposing an unauthenticated admin API over the network.

```
admin_api.host = "0.0.0.0" + no public_key → validation error at startup
admin_api.host = "127.0.0.1" + no public_key → allowed (dev mode)
admin_api.host = "0.0.0.0" + public_key set → allowed (Ed25519-authenticated)
```

### 15.3 Ed25519 request authentication

When `public_key` is configured, every admin request must include an Ed25519 signature.

**Signing** (done by `riversctl`):
```
payload   = "{method}\n{path}\n{body_sha256_hex}\n{unix_timestamp_ms}"
signature = ed25519_sign(private_key, payload)
```

**Verification** (done by admin server):
```
X-Rivers-Signature: {hex_signature}
X-Rivers-Timestamp: {unix_ms}
```

Server checks:
1. Timestamp within ±5 minutes of server clock (replay protection)
2. Signature valid for reconstructed payload using configured public key
3. Request rejected with 401 if either check fails

### 15.4 IP allowlist

`security.admin_ip_allowlist` — list of IPs or CIDR ranges allowed to reach the admin server. Enforced at the application layer (not TLS/firewall). If empty, any IP is allowed (subject to auth).

```toml
[security]
admin_ip_allowlist = ["10.0.0.0/8", "192.168.1.50"]
```

### 15.5 RBAC

```rust
pub struct RbacConfig {
    pub roles: HashMap<String, Vec<String>>,    // role → permissions
    pub bindings: HashMap<String, String>,       // identity → role
}
```

`identity` is the client certificate CN when mTLS is used, or a static key otherwise. `permissions` is a list of admin endpoint names (e.g., `["deploy", "status"]`). Validation rejects bindings referencing undefined roles and roles with no permissions.

### 15.6 --no-admin-auth escape hatch

```
riversd --no-admin-auth
```

Disables Ed25519 signature verification for this process lifetime only. Session-scoped — does not persist across restarts. Emits `tracing::warn!("--no-admin-auth: admin API authentication is DISABLED for this session")` at startup. Intended for initial setup and break-glass scenarios only.

---

## 16. Hot Reload

Development mode only. Disabled in production (when `hot_reload` is not configured).

```rust
pub struct HotReloadState {
    pub source: Option<HotReloadSource>,
    pub active_config: Arc<ServerConfig>,
    pub api_views: HashMap<String, ApiViewConfig>,
    pub dataview_engine: Arc<DataViewEngine>,
}
```

`maybe_spawn_hot_reload_watcher` uses the `notify` crate to watch the config file path. On change event:
1. Acquire `hot_reload_lock` (exclusive, prevents concurrent reloads)
2. Reload config from file
3. Validate new config
4. Atomic `RwLock::write()` swap of `HotReloadState`
5. Publish `ConfigFileChanged` event to EventBus
6. Release lock

In-flight requests use the `Arc<ServerConfig>` snapshot they acquired at request start. The swap does not interrupt them. New requests after the swap see the new config.

Hot reload does **not** restart the HTTP server, rebind sockets, re-initialize pools, or reload plugins. It reloads: View routes, DataView configs, DataView engine, static file config, and security config. Pool changes require a full restart.

---

## 17. Security Headers

Set by `security_headers_middleware` on every response (main and admin servers):

| Header | Value |
|---|---|
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `X-XSS-Protection` | `1; mode=block` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |

`Strict-Transport-Security` is **not** set automatically — it requires the operator to know their TLS configuration. Set it at the reverse proxy layer or add it to a future `security.hsts_max_age` config field.

`Content-Security-Policy` is not set by default. Add via a future config field or reverse proxy.

### Handler header blocklist

CodeComponent handlers cannot set the following response headers (SEC-8). Any such headers in handler output are silently dropped before the response is sent:

```
set-cookie
access-control-allow-origin
access-control-allow-credentials
access-control-allow-methods
access-control-allow-headers
access-control-expose-headers
access-control-max-age
host
transfer-encoding
connection
upgrade
x-forwarded-for
x-forwarded-host
x-forwarded-proto
x-content-type-options
x-frame-options
strict-transport-security
content-security-policy
```

CORS headers are set by Rivers based on config. Security headers are set by middleware. Handlers cannot override either.

---

## 18. Error Response Format

All error responses from the Rivers server (not from CodeComponent handlers) use a consistent JSON envelope:

```json
{
  "error": "human-readable error message"
}
```

Status code → error message mapping (for `map_runtime_error_to_response` and `map_dataview_error_to_response`):

| Status | Condition |
|---|---|
| 400 | Invalid request, parameter validation failed, bad request body |
| 401 | Admin auth failed, missing signature |
| 403 | RBAC permission denied, IP allowlist rejected |
| 404 | View not found, static file not found, DataView not found |
| 408 | Request timeout (timeout_middleware) |
| 422 | Schema validation failed on DataView result |
| 429 | Rate limit exceeded |
| 500 | Runtime execution failed, connection error, internal error |
| 503 | Server draining (shutdown), backpressure exhausted, circuit open |

CORS headers are added to error responses from view routes if CORS is enabled for that view.

---

## 19. Configuration Reference

### 19.1 Main server

```toml
[base]
host    = "0.0.0.0"    # default
port    = 8080          # default
workers = 4             # Tokio worker threads, default: auto (num_cpus)

request_timeout_seconds = 30    # default — 0 disallowed

[base.backpressure]
enabled          = true
queue_depth      = 512     # default
queue_timeout_ms = 100     # default

[base.http2]
enabled                = true
tls_cert               = "/etc/rivers/tls/server.crt"
tls_key                = "/etc/rivers/tls/server.key"
initial_window_size    = 65535    # optional
max_concurrent_streams = 100      # optional
```

### 19.2 Static files

```toml
[static_files]
enabled      = true
root_path    = "/var/rivers/app/dist"
index_file   = "index.html"
spa_fallback = true
max_age      = 86400          # Cache-Control max-age seconds
exclude_paths = [".env", "config.toml"]
```

### 19.3 Security / CORS / rate limiting

```toml
[security]
cors_enabled           = true
cors_allowed_origins   = ["https://app.example.com"]
cors_allowed_methods   = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
cors_allowed_headers   = ["Content-Type", "Authorization", "X-Trace-Id"]
cors_allow_credentials = false

rate_limit_per_minute  = 120     # default
rate_limit_burst_size  = 60      # default
rate_limit_strategy    = "ip"    # ip | custom_header
rate_limit_custom_header = "X-Api-Key"  # required if strategy = custom_header

admin_ip_allowlist     = ["10.0.0.0/8"]
```

### 19.4 Admin API

```toml
[base.admin_api]
enabled     = true
host        = "127.0.0.1"   # must be localhost if no public_key
port        = 9443
public_key  = "/etc/rivers/admin/admin.pub"   # Ed25519 public key
private_key = "/etc/rivers/admin/admin.key"   # used by riversctl

[base.admin_api.tls]
ca_cert              = "/etc/rivers/admin/ca.crt"
server_cert          = "/etc/rivers/admin/server.crt"
server_key           = "/etc/rivers/admin/server.key"
require_client_cert  = true   # enforce mTLS

[base.admin_api.rbac.roles]
operator    = ["status", "datasources", "drivers"]
deployer    = ["status", "datasources", "drivers", "deploy"]

[base.admin_api.rbac.bindings]
"CN=admin-client" = "deployer"
```

### 19.5 Session

```toml
[base.cluster.session_store]
enabled     = true
cookie_name = "rivers_session_id"   # default
```

### 19.6 Environment overrides

All main server settings support environment overrides:

```toml
[environment_overrides.prod.base]
host    = "0.0.0.0"
port    = 443
request_timeout_seconds = 60

[environment_overrides.prod.base.backpressure]
queue_depth = 1024

[environment_overrides.prod.security]
rate_limit_per_minute = 300
```
