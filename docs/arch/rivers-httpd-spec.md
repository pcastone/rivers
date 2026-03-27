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
20. [riversctl tls Commands](#20-riversctl-tls-commands)

---

## 1. Architecture Overview

Rivers runs two HTTP servers on separate sockets:

**Main server** — serves application traffic. All View routes, static files, health endpoint, gossip receiver. Full middleware stack applied.

**Admin server** — serves operational endpoints. Separate socket, separate middleware stack (subset), optional separate TLS. Spawned only when `admin_api.enabled = true`.

Both are Axum-based, built on `hyper` and `tokio`. TLS via `rustls` through `tokio-rustls`, with connections accepted by `hyper_util::server::conn::auto::Builder`.

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
2.  validate_server_tls(&config, no_ssl)
       │  TLS config validation — [base.tls] presence, cert/key pairing, admin TLS
       │  --no-ssl bypasses main server TLS check only (admin TLS always enforced)
       │
3.  validate_tls_config(&config)
       │  [base.tls] absent → hard startup error ("TLS is required")
       │  cert set without key (or vice versa) → hard startup error
       │  http2.enabled = true without [base.tls] → hard startup error
       │
4.  maybe_autogen_tls_cert(&config)
       │  cert + key paths absent → generate self-signed cert from [base.tls.x509]
       │  write to {data_dir}/tls/auto-{appId}.crt / .key
       │  files already exist → reuse (no re-gen)
       │  log WARN: "TLS: using auto-generated self-signed cert — not for production"
       │
4a. validate_admin_tls_config(&config)
       │  admin_api.enabled = true → admin TLS config checked
       │  admin TLS validation runs regardless of --no-ssl
       │
4b. maybe_autogen_admin_tls_cert(&config)
       │  admin cert + key absent → generate self-signed admin cert
       │  write to {data_dir}/tls/auto-admin-{app_id}.crt / .key
       │  existing valid pair → reuse; invalid pair → regenerate
       │
5.  initialize_global(config.clone())
       │  OnceLock-based global state singleton
       │
6.  configure_metrics_registry()
       │  Prometheus-compatible metrics registry
       │
7.  configure_event_bus(&config, metrics_registry)
       │  EventBus singleton: priority tiers, LogHandler subscription at Observe
       │
8.  load_plugins(&config, event_bus)
       │  Plugin directory scan → ABI check → catch_unwind registration
       │
9.  configure_lockbox_resolver(&config, event_bus)
       │  Lockbox backend (InMemory/SOPS/Vault/AWS/Infisical)
       │
10. configure_discovery_runtime / consensus_runtime / gossip_runtime
       │  Cluster layer (no-op for single-node)
       │
11. configure_session_manager(&config, &cluster_state, &gossip_state)
       │
12. configure_pool_manager(&config, event_bus)
       │  Per-datasource connection pools, circuit breakers
       │
13. configure_dataview_engine(&config, pool_manager, event_bus)
       │  DataViewRegistry + DataViewEngine + TieredDataViewCache
       │
14. configure_graphql_runtime(&config, dataview_engine)
       │
15. configure_runtime_factory(&config, pool_manager)
       │  ProcessPool (V8 + Wasmtime executors)
       │
16. configure_topic_registry (optional internal bookkeeping — no validation enforced)  <!-- SHAPE-17 amendment: topic validation dropped -->
       │
17. Build AppContext — all subsystems wired together
       │
18. Build main router — all routes registered
       │
19. maybe_spawn_admin_server — admin router on separate socket
       │
20. maybe_spawn_http_redirect_server — port 80 → HTTPS redirect (unless redirect = false)
       │  bind failure on port 80 → log WARN, continue (redirect does not run)
       │
21. maybe_spawn_hot_reload_watcher (dev mode only)
       │
22. Bind TcpListener (or use pre-bound listener from test harness)
       │
23. tokio_rustls TlsAcceptor — always TLS, no plain HTTP main server
       │
24. shutdown_signal task — waits for SIGTERM/SIGINT/watch channel
```

---

## 3. Router Structure

### 3.1 URL routing scheme

All app routes are namespaced by bundle and app entry point:

```
<host>:<port>/[route_prefix]/<bundle_entryPoint>/<entryPoint>/<view_name>
```

| Segment | Source | Required |
|---------|--------|----------|
| `route_prefix` | Operator-configured in `riversd.toml` (`route_prefix = "v1"`) | No |
| `bundle_entryPoint` | `bundleName` from bundle `manifest.toml` | Yes |
| `entryPoint` | `entryPoint` from app `manifest.toml` (a name, not a URL) | Yes |
| `view_name` | View name from `app.toml` (a name, not a full path) | Yes |

Example routes for an `address-book` bundle with apps `service` and `main`:

```
/address-book/service/contacts           → list_contacts view
/address-book/service/contacts/{id}      → get_contact view
/address-book/service/contacts/search    → search_contacts view
/address-book/main/                      → SPA index.html
/address-book/main/services              → service discovery JSON
```

This eliminates route collisions between apps — each app has its own namespace.

### 3.2 Service discovery endpoint

Every `app-main` automatically exposes a `services` endpoint under its namespace:

```
GET /[route_prefix]/<bundle_entryPoint>/<main_entryPoint>/services
```

Returns a JSON array of services declared in the app's `resources.toml`:

```json
[
  { "name": "address-book-service", "url": "/address-book/service" }
]
```

The SPA fetches this to discover service routes. No HTTP proxy — the SPA calls service endpoints directly.

### 3.3 Route registration order

Route registration order within the main router:

```
1. /health                                          GET  (health)
2. /health/verbose                                  GET  (health_verbose)
3. /gossip/receive                                  POST (gossip_receive_handler)
4. GraphQL routes                                   (if enabled)
5. /[prefix]/<bundle>/<app>/services                GET  (service discovery — app-main only)
6. /[prefix]/<bundle>/<app>/<view_name>             ALL  (view routes from app.toml)
7. /[prefix]/<bundle>/<main_app>/{*path}            GET  (SPA static files / fallback)
```

System routes (`/health`, `/gossip`) are outside the bundle namespace and are always registered first. View routes are registered per-app with their full namespaced path. SPA fallback is registered last under the main app's namespace.

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

TLS is mandatory by default. `[base.tls]` must be present — absence is a hard startup error unless `--no-ssl` is passed. All application traffic is encrypted in normal operation.

### 5.0 --no-ssl debug flag

```
riversd --no-ssl [--port PORT]
```

Disables TLS on the **main server only** for this process lifetime. The admin server always runs TLS regardless of `--no-ssl`. Session-scoped — does not persist across restarts.

When `--no-ssl` is active:
- Main server binds plain HTTP (no TLS acceptor, no redirect server)
- `--port` flag is permitted (only valid with `--no-ssl`; rejected otherwise)
- `[base.tls]` validation is skipped for the main server
- Admin TLS validation still runs — admin server requires TLS unconditionally
- Emits `WARN: "--no-ssl: main server TLS is DISABLED for this session"`

Not for production. Intended for local development and debugging only.

### 5.1 Main server TLS

```toml
[base.tls]
cert          = "/etc/rivers/tls/server.crt"   # omit → auto-gen self-signed on startup
key           = "/etc/rivers/tls/server.key"   # omit → auto-gen self-signed on startup
# redirect = false                              # uncomment to disable HTTP → HTTPS redirect
# redirect_port = 80                            # default: 80 — port for HTTP → HTTPS redirect listener
```

`cert` and `key` are both optional. When absent, Rivers auto-generates a self-signed certificate at startup (see §5.4). When present, both must be set — one without the other is a startup error.

Implementation: `tokio_rustls::TlsAcceptor` built from `rustls::ServerConfig::builder().with_single_cert(certs, key)`.

### 5.2 x509 Certificate Fields

Used when auto-generating a self-signed cert (`cert`/`key` absent) and by `riversctl tls gen` and `riversctl tls request`.

```toml
[base.tls.x509]
common_name  = "localhost"
organization = "Acme Corp"
country      = "US"              # ISO 3166-1 alpha-2
state        = "California"
locality     = "San Francisco"
san          = ["localhost", "127.0.0.1"]   # Subject Alternative Names
days         = 365               # certificate validity period
```

All fields are optional and have defaults. `san` defaults to `["localhost", "127.0.0.1"]`. `days` defaults to `365`.

### 5.3 TLS Engine

Controls cipher suites and minimum TLS version. Leave `ciphers` empty to use rustls secure defaults — recommended for most deployments. Explicit cipher configuration is for compliance requirements (FIPS, PCI-DSS) that mandate specific suites.

```toml
[base.tls.engine]
min_version = "tls12"    # tls12 | tls13  (default: tls12)
ciphers     = []         # empty = rustls defaults (recommended)
                         # explicit suite names (IANA/rustls format):
                         #   "TLS13_AES_256_GCM_SHA384"
                         #   "TLS13_CHACHA20_POLY1305_SHA256"
                         #   "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384"
                         #   "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256"
```

**MAC note:** In TLS 1.3, MAC is embedded in the AEAD cipher (AES-GCM uses GHASH, ChaCha20-Poly1305 uses Poly1305) — there is no separate MAC configuration. In TLS 1.2, MAC is part of the cipher suite designation (e.g. `SHA384`). `ciphers` accepts standard IANA/rustls cipher suite names only.

### 5.4 Auto-gen Self-Signed Certificate

When `cert` and `key` are absent from `[base.tls]`, Rivers generates a self-signed certificate at startup:

```
cert + key absent → generate from [base.tls.x509] fields
                  → write to {data_dir}/tls/auto-{appId}.crt and .key
                  → log WARN: "TLS: using auto-generated self-signed cert — not for production"
                  → existing valid pair reused on subsequent restarts
       → existing invalid pair (bad cert, mismatched key) triggers regeneration with WARN log
       → purge via `riversctl tls expire` to force re-gen
```

Auto-gen is for development only. In production, always provide `cert` and `key` explicitly, or use `riversctl tls import` after signing with a CA. See §20 for `riversctl tls` subcommands.

`riversctl tls expire` purges the auto-gen (or operator-provided) cert files from disk. Next startup will re-generate (if no cert/key paths are set) or error (if cert/key paths are set but files are missing).

### 5.5 HTTP Redirect Server

When `redirect = false` is not set (the default), Rivers spawns a second listener on **port 80** that redirects all HTTP traffic to the HTTPS main server.

```
GET http://host:80/any/path?q=1
→ HTTP 301  Location: https://host:[base.port]/any/path?q=1
```

**Properties:**
- Port defaults to 80. Configurable via `redirect_port` in `[base.tls]`.
- Response is always `301 Moved Permanently`. `Host` header and query string are preserved.
- No middleware stack — no rate limiting, no session, no trace ID. Single redirect handler only.
- Shares the same shutdown signal as the main server — drains and exits on SIGTERM/SIGINT.
- Port 80 bind failure (e.g. permission denied on Linux without `CAP_NET_BIND_SERVICE`) → **warning, not error**. Main HTTPS server still starts; redirect simply does not run.

**To disable:**

```toml
[base.tls]
redirect = false
# redirect_port = 80    # default: 80, configurable
```

Internal services (not publicly accessible) must set `redirect = false` — they must not compete for port 80.

### 5.6 Admin server TLS

Admin server has independent TLS configuration:

```rust
pub struct AdminTlsConfig {
    pub ca_cert: Option<String>,          // CA cert for mTLS client verification
    pub server_cert: Option<String>,
    pub server_key: Option<String>,
    pub require_client_cert: bool,        // default: false
}
```

When `require_client_cert = true`, mTLS is enforced — clients without a valid cert signed by `ca_cert` are rejected at the TLS handshake. This is the recommended production configuration.

When `server_cert` and `server_key` are set but `require_client_cert = false`: one-way TLS. Traffic is encrypted but no client cert is required.

When neither cert is set: admin server auto-generates a self-signed certificate in `{data_dir}/tls/auto-admin-{app_id}.crt`. TLS is **always** required on the admin server — there is no plain-HTTP fallback.

### 5.7 rustls configuration for mTLS

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

HTTP/2 requires TLS (ALPN negotiation). `[base.tls]` must be configured before HTTP/2 can be enabled — enabling HTTP/2 without `[base.tls]` is rejected at startup with a validation error.

```rust
pub struct Http2Config {
    pub enabled: bool,
    pub initial_window_size: Option<u32>,    // flow control window
    pub max_concurrent_streams: Option<u32>, // per-connection stream limit
}
```

TLS certificate paths are not part of `Http2Config` — they belong to `TlsConfig` under `[base.tls]`. `Http2Config` is a protocol toggle only.

When `http2.enabled = true`, Rivers serves h2 + h1.1 via ALPN negotiation. Rivers does not support h2c (HTTP/2 cleartext).

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

When `spa_fallback = true`, any path under the app-main's namespace that does not resolve to an existing file returns `root/index_file` instead of 404. This allows client-side routers (React Router, Vue Router, Angular) to handle routing.

SPA assets are scoped to the app-main's namespace:

```
/[route_prefix]/<bundle_entryPoint>/<main_entryPoint>/           → index.html
/[route_prefix]/<bundle_entryPoint>/<main_entryPoint>/some/path  → index.html (SPA fallback)
/[route_prefix]/<bundle_entryPoint>/<main_entryPoint>/bundle.js  → static file
/[route_prefix]/<bundle_entryPoint>/<main_entryPoint>/services   → discovery JSON (not SPA)
```

**Ordering guarantee**: The `services` discovery endpoint and view routes are registered before the SPA fallback catch-all. They are never shadowed by SPA fallback.

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

Bundle deployment applies this config into the app-main's static file serving scope.

---

## 9. CORS

CORS is an application concern configured once at startup in the application init handler — not per-view and not in server config. See `rivers-view-layer-spec.md` for the full `app.cors()` API.

### 9.1 Policy injection

CORS headers are applied per-request based on the policy registered by `app.cors()` in the init handler. The policy is evaluated against the `Origin` header on each request.

Handler-set CORS headers are blocked (SEC-8) — CodeComponent handlers cannot set `access-control-*` headers. Rivers sets them based on the registered policy, not handler output.

### 9.2 Origin matching

```
app.cors({ origins: ["*"] })
    + any Origin                      → Access-Control-Allow-Origin: *

app.cors({ origins: ["https://app.example.com"] })
    + Origin: https://app.example.com → Access-Control-Allow-Origin: https://app.example.com
    + Origin: https://evil.com        → no CORS headers set (request proceeds, but no CORS header)
```

`origins: ["*"]` is incompatible with `credentials: true` — validation rejects this combination at startup.

### 9.3 SecurityConfig

```rust
pub struct SecurityConfig {
    pub admin_ip_allowlist: Vec<String>,
}
```

CORS configuration is not part of `SecurityConfig` — it belongs to the application init handler. Rate limit configuration is not part of `SecurityConfig` — it belongs to `[app.rate_limit]` in `app.toml`.

---

## 10. Rate Limiting

Token bucket algorithm. Per-key state stored in `RateLimiter` (in-process, not shared across nodes).

Rate limiting is configured at the application level in `[app.rate_limit]` within `app.toml` — not in server-level config. Each app in a bundle has independent rate limit policy.

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

Determined by `strategy` in `[app.rate_limit]`:

- `ip` (default) — remote IP from connection. Does not inspect `X-Forwarded-For` (not trusted without explicit proxy config).
- `header` — value of a configured custom header. Useful for API keys passed in a header. If header is absent, falls back to IP.
- `session` — `session.identity.username`. Falls back to `ip` on `auth = "none"` views.

### 10.3 Bucket eviction

`RATE_LIMIT_MAX_BUCKETS = 10_000`. When the bucket map reaches this size:
1. Evict buckets last seen > 5 minutes ago
2. If still over limit after stale eviction: sort by `last_refill_epoch_ms`, remove oldest 50% of entries

This bounds memory regardless of the number of unique clients.

### 10.4 Per-view override

```rust
pub rate_limit_per_minute: Option<u32>,   // overrides [app.rate_limit].per_minute
pub rate_limit_burst_size: Option<u32>,   // overrides [app.rate_limit].burst_size
```

`None` means use the app default. The per-view bucket key includes the view ID to keep per-view budgets isolated from the app-level budget.

### 10.5 Response

<!-- SHAPE-2 amendment: ErrorResponse envelope -->
```
HTTP 429 Too Many Requests
Retry-After: {seconds}
{"code": 429, "message": "rate limit exceeded", "trace_id": "{trace_id}"}
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

<!-- SHAPE-2 amendment: ErrorResponse envelope -->
```
HTTP 503 Service Unavailable
Retry-After: 1
{"code": 503, "message": "server overloaded; retry later", "trace_id": "{trace_id}"}
```

---

## 12. Session Management

Cookie-based session, backed by StorageEngine.

### 12.1 Session middleware flow

```
request arrives
    │
    ├─ parse cookie: rivers_session (default cookie name)
    │
    ├─ if cookie present:
    │       StorageEngine::get("session:{id}")
    │       → session expired or not found: clear_cookie = true, id = None
    │       → valid: inject session data into request extensions
    │
    ├─ if no cookie:
    │       session identity remains None — no auto-creation
    │       guard views handle session creation via CodeComponent handlers
    │
    └─ after handler:
            set_cookie → Set-Cookie: {cookie_name}={id}; HttpOnly; SameSite=Lax; Path=/
            clear_cookie → Set-Cookie: {cookie_name}=; Max-Age=0; ...
```

Sessions are **never** auto-created by the middleware. Session creation is the exclusive responsibility of guard view CodeComponent handlers (see `rivers-auth-session-spec.md`). Requests without a session cookie proceed with `session = None` in request context.

### 12.2 Session storage

Sessions are stored in and read from StorageEngine under key `session:{session_id}`. StorageEngine is the canonical session store. Cluster-wide session consistency depends on the configured backend — a Redis backend provides immediate cross-node consistency; in-memory or SQLite is node-local and appropriate only for single-node deployments.

Gossip-based session propagation (`GossipPayload::SessionUpserted`, `GossipPayload::SessionDeleted`) is not used.

### 12.3 SessionConfig

```rust
pub struct SessionConfig {
    pub enabled: bool,
    pub cookie_name: String,   // default: "rivers_session"
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
        // <!-- SHAPE-2 amendment: ErrorResponse envelope -->
        return (StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"code": 503, "message": "server is shutting down", "trace_id": trace_id}))).into_response();
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
| POST | `/admin/shutdown` | Gracefully shutdown the server |

### 15.2 Ed25519 authentication enforcement

Ed25519 authentication is **required unconditionally** when `admin_api.enabled = true` — regardless of bind address. The former plain-HTTP exception for `127.0.0.1` is removed (SHAPE-25).

`public_key` must be set in `[base.admin_api]`. Omitting it is a validation error at startup, regardless of `host` value.

The admin server always runs TLS. There is no plain-HTTP fallback, even on localhost.

### 15.3 Ed25519 request authentication

When `public_key` is configured, every admin request must include an Ed25519 signature.

**Signing** (done by `riversctl`):
```
payload   = "{method}\n{path}\n{unix_timestamp_ms}\n{body_sha256_hex}"
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

<!-- SHAPE-2 amendment: ErrorResponse envelope replaces simple error string -->
All error responses from the Rivers server (not from CodeComponent handlers) use the canonical `ErrorResponse` envelope:

```json
{
  "code": 500,
  "message": "human-readable error message",
  "details": "optional diagnostic info",
  "trace_id": "abc-123"
}
```

Status code → ErrorResponse mapping (for `map_runtime_error_to_response` and `map_dataview_error_to_response`):

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
data_dir     = "data"      # default: "data" — relative to CWD; used for auto-gen cert paths
app_id       = "default"   # default: "default" — used in auto-gen cert filenames (auto-{app_id}.crt)
route_prefix = ""          # optional — prepended to all bundle routes (e.g. "v1" → /v1/<bundle>/...)

[base]
host    = "0.0.0.0"    # default
port    = 8080          # default: 8080
workers = 4             # Tokio worker threads, default: auto (num_cpus)

request_timeout_seconds = 30    # default — 0 disallowed

[base.backpressure]
enabled          = true
queue_depth      = 512     # default
queue_timeout_ms = 100     # default

[base.tls]
cert          = "/etc/rivers/tls/server.crt"   # omit → auto-gen self-signed
key           = "/etc/rivers/tls/server.key"   # omit → auto-gen self-signed
# redirect = false                              # uncomment to disable HTTP → HTTPS redirect
# redirect_port = 80                            # default: 80 — port for HTTP → HTTPS redirect listener

[base.tls.x509]
common_name  = "localhost"
organization = "My Org"
country      = "US"
state        = "California"
locality     = "San Francisco"
san          = ["localhost", "127.0.0.1"]
days         = 365

[base.tls.engine]
min_version = "tls12"    # tls12 | tls13
ciphers     = []         # empty = rustls defaults (recommended)

[base.http2]
enabled                = true
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

### 19.3 Security / rate limiting

```toml
# Server-level security (admin IP allowlist only)
[security]
admin_ip_allowlist = ["10.0.0.0/8"]

# App-level rate limiting (in app.toml, per app)
[app.rate_limit]
per_minute = 120      # default
burst_size = 60       # default
strategy   = "ip"     # ip | header | session
```

CORS is not configured here — it is registered in the application init handler via `app.cors()`. See `rivers-view-layer-spec.md`.

### 19.4 Admin API

```toml
[base.admin_api]
enabled     = true
host        = "127.0.0.1"   # bind address for admin server
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
[session]
enabled     = true
cookie_name = "rivers_session"   # default
```

Cookie attributes (`HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` when TLS active) are hardcoded and not configurable. See §12.3.

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

---

## 20. riversctl tls Commands

CLI commands for certificate lifecycle management. All commands read `cert`/`key` paths from the server config. Write commands (`tls gen`, `tls import`) write to those paths.

| Command | Action |
|---|---|
| `riversctl tls gen` | Generate self-signed cert from `[base.tls.x509]` fields, write to configured `cert`/`key` paths |
| `riversctl tls request` | Generate a CSR from `[base.tls.x509]` fields, print to stdout for CA submission |
| `riversctl tls import <cert> <key>` | Validate and copy a CA-signed cert and intermediate key into configured paths |
| `riversctl tls show` | Display cert details: subject, issuer, SANs, fingerprint, validity window, time remaining |
| `riversctl tls list` | List all cert files managed by Rivers with their paths and expiry dates |
| `riversctl tls expire` | Purge cert files from disk — next startup triggers auto-gen or hard error if paths were operator-provided |

### 20.1 tls show output format

```
Subject:     CN=localhost, O=Acme Corp, C=US
Issuer:      CN=localhost (self-signed)
SANs:        localhost, 127.0.0.1
Valid:       2026-03-18 → 2027-03-18
Expires:     365 days left
Fingerprint: SHA256:4A:3B:...
```

`Expires` shows human-readable time remaining (e.g. `42 days left`, `3 hours left`). When expired: `EXPIRED 5 days ago`.

### 20.2 tls expire behavior

`tls expire` is purely destructive and requires the `--yes` flag to execute. Without `--yes`, the command prints a warning and exits without action.

```
riversctl tls expire --yes              # main server cert
riversctl tls expire --port 9443 --yes  # admin server cert
```

It removes the cert and key files at the configured paths. If `cert`/`key` paths are not set (auto-gen mode), it removes the auto-gen files from `{data_dir}/tls/`. The running server is not affected — TLS changes require restart (see §16).

### 20.3 skip_verify on HTTP datasources

When `address-book-main` (or any app) calls an internal service over HTTPS with a self-signed cert, the HTTP driver must be configured to skip certificate verification:

```toml
[data.datasources.address-book-api]
driver      = "http"
service     = "address-book-service"
nopassword  = true
skip_verify = true    # allow self-signed certs — dev only
```

`skip_verify` is a datasource-level field on the HTTP driver. It is not a server-level TLS setting. Remove it in production when using CA-signed certificates.

---

## Shaping Amendments

The following changes were applied to this spec per decisions in `rivers-shaping-and-gap-analysis.md`:

### SHAPE-2: ErrorResponse Envelope

All error responses from the Rivers server use the canonical `ErrorResponse` envelope (not the previous `{"error": "..."}` format):

```json
{
  "code": 500,
  "message": "human-readable error message",
  "details": "optional diagnostic info",
  "trace_id": "abc-123"
}
```

Affected sections (amended inline above):
- **§10.5** — Rate limit 429 response uses `{"code": 429, "message": "rate limit exceeded", "trace_id": "..."}`
- **§11.3** — Backpressure 503 response uses `{"code": 503, "message": "server overloaded; retry later", "trace_id": "..."}`
- **§13.3** — Shutdown guard 503 response uses `{"code": 503, "message": "server is shutting down", "trace_id": "..."}`
- **§18** — Error format section rewritten to document the canonical ErrorResponse envelope

### SHAPE-17: EventBus Topic Validation Dropped

EventBus is a dumb pipe. No topic registry validation is enforced at publish time. Any topic string is accepted.

Affected sections (amended inline above):
- **§2 step 15** — `configure_topic_registry` demoted to optional internal bookkeeping with no validation enforced

### HTTPD-RC1: Phase AD Spec Reconciliation

14 amendments applied to reconcile spec with Phase AD implementation. Source: changelog, `admin_auth.rs`, `server.rs`, `tls.rs`, `cli.rs`, `config.rs`, `tls_cmd.rs`.

| AMD | Section | Change |
|-----|---------|--------|
| 1 | §15.3 | **CRITICAL** — Signature payload field order fixed: timestamp before body_hash (matches `admin_auth.rs:41`) |
| 2 | §15.2 | Heading renamed from "Localhost binding enforcement" to "Ed25519 authentication enforcement" |
| 3 | §1 | TLS library corrected: `tokio-rustls` + `hyper_util::server::conn::auto::Builder` (not `axum-server`) |
| 4 | §2 | Step 2 corrected: `validate_server_tls(&config, no_ssl)` replaces `config.validate()` |
| 5 | §2 | Steps 4a/4b added: admin TLS validation and admin cert auto-gen |
| 6 | §5 | New §5.0: `--no-ssl` debug flag documented (main server only, admin TLS unaffected) |
| 7 | §5.4 | Reuse policy corrected: invalid existing pair triggers regeneration (not silent reuse) |
| 8 | §19.1 | Default port corrected: 8080 (not 443) |
| 9 | §5.1, §19.1 | `redirect_port` added to `[base.tls]` config blocks |
| 10 | §19.1 | `data_dir` and `app_id` added to config reference (top-level `ServerConfig` fields) |
| 11 | §19.4 | Stale host comment removed ("must be localhost if no public_key" → "bind address for admin server") |
| 12 | §19.5 | Session config path fixed: `[session]` (not `[base.cluster.session_store]`); added §12.3 cross-ref |
| 13 | §20.2 | `--yes` flag requirement documented for `tls expire` |
| 14 | §9, §19.3 | Broken cross-reference fixed: `rivers-view-layer-spec.md` (not `rivers-technology-path-spec.md §10`) |
