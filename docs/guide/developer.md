# Rivers Developer Guide

**Rivers v0.54.0**

Build applications with TOML configuration and JSON schemas. No Rust required.

> **v0.54.0:** All driver plugins are now compiled into `riversd`. There is no longer a distinction between "built-in" and "plugin" drivers from the developer's perspective — any driver name you declare will resolve if it's listed in the driver table below. See [Available Drivers](#available-drivers).

---

## Bundle Structure

A Rivers application is packaged as a **bundle** — a directory containing one or more apps.

```
my-bundle/
  manifest.toml                 # Bundle metadata
  my-api/                       # App: backend service
    manifest.toml               # App identity
    resources.toml              # Datasources and dependencies
    app.toml                    # DataViews, views, config
    schemas/                    # JSON schema files
  my-frontend/                  # App: SPA host
    manifest.toml
    resources.toml
    app.toml
    libraries/                  # Static assets (built SPA)
```

### Bundle Manifest

```toml
bundleName    = "my-bundle"
bundleVersion = "1.0.0"
apps          = ["my-api", "my-frontend"]
```

### App Manifest

```toml
appName     = "my-api"
description = "REST API service"
version     = "1.0.0"
type        = "app-service"        # or "app-main" for SPA host
appId       = "a1b2c3d4-..."       # Stable UUID — generate once, never change
entryPoint  = "api"                # URL segment: /my-bundle/api/...
```

- `type = "app-service"` — backend API, starts first
- `type = "app-main"` — SPA host, starts after services are healthy
- `appId` must be a stable UUID. Generate with `uuidgen` and keep it forever.

### Resources Declaration

`resources.toml` declares what the app needs:

```toml
[[datasources]]
name       = "db"
driver     = "postgres"
required   = true

[[datasources]]
name       = "cache"
driver     = "redis"
nopassword = true

[[services]]
name   = "user-service"
app_id = "uuid-of-user-service"
```

---

## Datasource Configuration

In `app.toml`, configure each datasource declared in `resources.toml`:

```toml
[data.datasources.db]
name     = "db"
driver   = "postgres"
host     = "localhost"
port     = 5432
database = "myapp"
username = "app"

# Credential from LockBox (no plaintext passwords in config)
credentials_source = "lockbox://db/myapp-postgres"

[data.datasources.db.connection_pool]
max_size              = 10
min_idle              = 2
connection_timeout_ms = 500
idle_timeout_ms       = 30000
health_check_interval_ms = 5000

[data.datasources.db.connection_pool.circuit_breaker]
enabled            = true
failure_threshold  = 5
open_timeout_ms    = 30000
```

### Available Drivers (v0.54.0)

All drivers are compiled into `riversd` via the `static-plugins` cargo feature. There are no external plugin dylibs in v0.54.0.

| Driver | Type | Notes |
|--------|------|-------|
| `postgres` | Database | Core |
| `mysql` | Database | Core |
| `sqlite` | Database | Core |
| `redis` | Database | Core |
| `memcached` | Database | Core |
| `faker` | Database | Synthetic data generator |
| `eventbus` | Database | Pub/sub |
| `mongodb` | Database | Formerly cdylib, now static |
| `elasticsearch` | Database | Formerly cdylib, now static |
| `cassandra` | Database | Formerly cdylib, now static |
| `couchdb` | Database | Formerly cdylib, now static |
| `influxdb` | Database | Formerly cdylib, now static |
| `neo4j` | Database | Formerly cdylib, now static |
| `ldap` | Database | Formerly cdylib, now static |
| `kafka` | Broker | Formerly cdylib, now static |
| `rabbitmq` | Broker | Formerly cdylib, now static |
| `nats` | Broker | Formerly cdylib, now static |
| `redis-streams` | Broker | Formerly cdylib, now static |
| `exec` | Database | Script runner, formerly cdylib, now static |

If an app declares a datasource whose driver cannot be resolved, the app is isolated: its views are blocked from loading and endpoint requests return `503 Service Unavailable`. Other apps in the bundle load normally. See `admin.md` AW3.16 for operator details.

### Faker Driver (Testing)

No database needed. Generates synthetic data from schema files:

```toml
[data.datasources.contacts]
name       = "contacts"
driver     = "faker"
nopassword = true
```

---

## DataViews

A DataView is a named, parameterized query bound to a datasource. Define in `app.toml`:

```toml
[data.dataviews.list_users]
name          = "list_users"
datasource    = "db"
query         = "SELECT * FROM users LIMIT $limit OFFSET $offset"
return_schema = "schemas/user.schema.json"

[[data.dataviews.list_users.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[[data.dataviews.list_users.parameters]]
name     = "offset"
type     = "integer"
required = false
default  = 0
```

### CRUD DataViews

One DataView can handle all HTTP methods with per-method queries, schemas, and parameters:

```toml
[data.dataviews.users]
name       = "users"
datasource = "db"

# Per-method queries
get_query    = "SELECT * FROM users WHERE id = $id"
post_query   = "INSERT INTO users (name, email) VALUES ($name, $email) RETURNING *"
put_query    = "UPDATE users SET name = $name, email = $email WHERE id = $id RETURNING *"
delete_query = "DELETE FROM users WHERE id = $id"

# Per-method schemas
get_schema    = "schemas/user.schema.json"
post_schema   = "schemas/user-create.schema.json"
put_schema    = "schemas/user-update.schema.json"
delete_schema = "schemas/user-delete.schema.json"
```

### Parameter Types

| Type | Zero-value default | Example |
|------|-------------------|---------|
| `string` | `""` | `"alice"` |
| `integer` | `0` | `42` |
| `float` | `0.0` | `3.14` |
| `boolean` | `false` | `true` |
| `array` | `[]` | `[1, 2, 3]` |
| `uuid` | — | `"a1b2c3d4-..."` |

Required parameters with no value produce a 422 error. Optional parameters use the `default` value if set, otherwise the zero-value for the type.

### Caching

```toml
[data.dataviews.list_users.caching]
ttl_seconds    = 60          # Cache lifetime
l1_enabled     = true        # In-process LRU (default: true)
l1_max_bytes   = 157286400   # L1 memory limit in bytes (default: 150 MB)
l1_max_entries = 100000      # Hard cap on entries (default: 100,000)
l2_enabled     = false       # StorageEngine-backed (requires [storage_engine])
```

### Cache Invalidation

Write DataViews can declare which read DataViews to invalidate on success:

```toml
[data.dataviews.create_user]
name         = "create_user"
datasource   = "db"
post_query   = "INSERT INTO users (name, email) VALUES ($name, $email)"
invalidates  = ["list_users", "search_users"]
```

When `create_user` executes successfully, the cached results for `list_users` and `search_users` are cleared from both L1 and L2 caches.

---

## Views (REST Endpoints)

Views map HTTP endpoints to DataViews or CodeComponent handlers.

```toml
[api.views.list_users]
view_type = "Rest"
path      = "users"
method    = "GET"
auth      = "none"                  # "none" = public, "session" = requires login

[api.views.list_users.handler]
type     = "dataview"
dataview = "list_users"

[api.views.list_users.parameter_mapping.query]
limit  = "limit"                    # ?limit=10 → DataView param "limit"
offset = "offset"
```

### View Types

| Type | Purpose |
|------|---------|
| `Rest` | Standard HTTP request/response |
| `ServerSentEvents` | Server-push streaming |
| `Websocket` | Bidirectional WebSocket |
| `MessageConsumer` | Broker message handler |

### Parameter Mapping

Map HTTP parameters to DataView parameters:

```toml
# Query string: ?page=2&size=10
[api.views.list.parameter_mapping.query]
page = "offset_page"
size = "limit"

# Path parameters: /users/{id}
[api.views.get.parameter_mapping.path]
id = "user_id"

# Body fields (POST/PUT)
[api.views.create.parameter_mapping.body]
name  = "name"
email = "email"
```

### Handler Types

**DataView handler** — dispatches to a declared DataView:
```toml
[api.views.list_users.handler]
type     = "dataview"
dataview = "list_users"
```

**CodeComponent handler** — runs JavaScript/WASM:
```toml
[api.views.process_order.handler]
type       = "codecomponent"
language   = "javascript"
module     = "handlers/order.js"
entrypoint = "processOrder"
```

### Auth

```toml
auth = "none"       # Public — no session required
auth = "session"    # Protected — requires valid session
guard = true        # This view IS the login endpoint
```

### Per-view Rate Limiting (v0.54.0)

Views can override the global rate limit with their own token-bucket settings:

```toml
[api.views.search]
path                  = "search"
method                = "GET"
view_type             = "Rest"
rate_limit_per_minute = 30
rate_limit_burst_size = 10
```

Per-view limiters use proxy-aware client IP extraction (respects `X-Forwarded-For` from `trusted_proxies`). Exceeding the limit returns `429 Too Many Requests` with a `Retry-After` header.

---

## CodeComponent Handlers (JavaScript)

Handlers receive a `ctx` object and modify `ctx.resdata`:

```javascript
function handler(ctx) {
    // Read request
    const name = ctx.request.body.name;
    const id = ctx.request.params.id;

    // Access pre-fetched DataView data
    const users = ctx.data.list_users;

    // Call a DataView dynamically
    const orders = ctx.dataview("get_orders", { user_id: id });

    // Set response
    ctx.resdata = { name, orders };
}
```

### Context Object (`ctx`)

| Property | Type | Description |
|----------|------|-------------|
| `ctx.request` | Object | `{method, path, headers, query, body, params}` (read-only) |
| `ctx.resdata` | Any | Response payload (mutable — set this to return data) |
| `ctx.data` | Object | Pre-fetched DataView results, keyed by name |
| `ctx.session` | Object | Session claims (when `auth = "session"`) |
| `ctx.trace_id` | String | Request trace ID for correlation |
| `ctx.app_id` | String | Application ID |
| `ctx.node_id` | String | Server node ID |
| `ctx.env` | String | Runtime environment (`"dev"`, `"prod"`) |

### `ctx.dataview(name, params?)`

Call a declared DataView from a handler:

```javascript
function handler(ctx) {
    var contacts = ctx.dataview("list_contacts");
    var user = ctx.dataview("get_user", { id: ctx.request.params.id });
    ctx.resdata = { contacts, user };
}
```

Returns `null` if the DataView doesn't exist. When a `DataViewExecutor` is available, executes the query live against the datasource.

### `ctx.store` (Application KV)

Per-app key-value store backed by StorageEngine:

```javascript
function handler(ctx) {
    // Set a value (optional TTL in milliseconds)
    ctx.store.set("user:preferences:123", { theme: "dark" }, 3600000);

    // Get a value
    var prefs = ctx.store.get("user:preferences:123");

    // Delete a value
    ctx.store.del("user:preferences:123");
}
```

Reserved key prefixes (`session:`, `csrf:`, `poll:`, `cache:`, `rivers:`) are blocked.

### Init Handlers and `ctx.ddl` (v0.54.0)

Init handlers run once per app at load time. They can execute DDL statements to create tables, indexes, or other schema objects:

```typescript
// libraries/handlers/init.ts
export function init(ctx) {
  ctx.ddl("orders_db", "CREATE TABLE IF NOT EXISTS audit_log (id SERIAL PRIMARY KEY, message TEXT)");
  ctx.ddl("orders_db", "CREATE INDEX IF NOT EXISTS idx_audit_log_id ON audit_log (id)");
}
```

`ctx.ddl(datasource, statement)` executes the statement against the named datasource. DDL calls are gated by a **Gate 3 whitelist** configured in `riversd.toml`:

```toml
[security]
ddl_whitelist = [
  "orders_db@c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a",
]
```

Calls from apps not on the whitelist for the requested datasource are rejected at the gate.

DDL events are logged to the per-app log:

| Event | Level | When |
|-------|-------|------|
| `DdlExecuted` | info | Statement executed successfully |
| `DdlFailed` | error | Statement reached the datasource but failed |
| `DdlRejected` | warn | Blocked at Gate 3 whitelist |

`ctx.ddl` works in both ProcessPool V8 (static builds) and engine dylib V8 (dynamic builds).

### `Rivers.log` (Structured Logging)

```javascript
function handler(ctx) {
    Rivers.log.info("user login", { userId: 123, action: "login" });
    Rivers.log.warn("rate limit approaching", { remaining: 5 });
    Rivers.log.error("payment failed", { orderId: "abc", reason: "declined" });
}
```

Logs include `trace_id` automatically. Output goes to the server's structured log stream.

### `Rivers.crypto`

```javascript
function handler(ctx) {
    // Password hashing (bcrypt)
    var hash = Rivers.crypto.hashPassword("secret123");
    var valid = Rivers.crypto.verifyPassword("secret123", hash);

    // Random values
    var hex = Rivers.crypto.randomHex(16);          // 32 hex chars
    var token = Rivers.crypto.randomBase64url(32);   // URL-safe base64

    // HMAC-SHA256
    var sig = Rivers.crypto.hmac("secret-key", "data-to-sign");

    // Timing-safe comparison
    var equal = Rivers.crypto.timingSafeEqual("abc", "abc");

    // AES-256-GCM encrypt/decrypt (requires [[keystores]] in resources.toml)
    var enc = Rivers.crypto.encrypt("credential-key", "plaintext");
    // enc = { ciphertext: "base64...", nonce: "base64...", key_version: 1 }

    var dec = Rivers.crypto.decrypt("credential-key", enc.ciphertext, enc.nonce, {
        key_version: enc.key_version
    });
    // dec === "plaintext"

    // With additional authenticated data (AEAD)
    var enc = Rivers.crypto.encrypt("credential-key", "data", { aad: "record-id" });
    var dec = Rivers.crypto.decrypt("credential-key", enc.ciphertext, enc.nonce, {
        key_version: enc.key_version, aad: "record-id"
    });
}
```

### `Rivers.keystore` (Key Metadata)

Only available when `[[keystores]]` is declared in `resources.toml`. Key bytes never exposed to handler code.

```javascript
function handler(ctx) {
    var exists = Rivers.keystore.has("credential-key");     // boolean
    var info = Rivers.keystore.info("credential-key");
    // info = { name: "credential-key", type: "aes-256", version: 2, created_at: "..." }
}
```

### `Rivers.http` (Outbound HTTP)

Only available when `allow_outbound_http = true` on the view:

```toml
[api.views.proxy]
allow_outbound_http = true
```

```javascript
async function handler(ctx) {
    var resp = await Rivers.http.get("https://api.example.com/data");
    ctx.resdata = resp;
}
```

Methods: `get(url)`, `post(url, body)`, `put(url, body)`, `del(url)`.

### ExecDriver (Script Execution)

Execute admin-declared scripts via the standard datasource query pattern. The handler doesn't know it's running a script — it just queries a datasource.

```javascript
function handler(ctx) {
    // Execute a declared command via datasource query
    var result = ctx.dataview("run_network_scan");
    // Or via direct datasource query:
    // var result = ctx.datasource("ops_tools").query({
    //     command: "network_scan",
    //     args: { cidr: "10.0.1.0/24", ports: [22, 80] }
    // });
    ctx.resdata = result;
}
```

The command, script path, integrity hash, and all security guardrails are admin-configured in TOML — not in handler code.

### Async Handlers

```javascript
async function handler(ctx) {
    var [users, orders] = await Promise.all([
        Promise.resolve(ctx.dataview("list_users")),
        Promise.resolve(ctx.dataview("list_orders")),
    ]);
    ctx.resdata = { users, orders };
}
```

### Guard Handlers (Authentication)

A guard view is the login endpoint. The handler validates credentials and returns identity claims:

```javascript
function authenticate(ctx) {
    var username = ctx.request.body.username;
    var password = ctx.request.body.password;

    // Validate credentials (your logic here)
    var user = ctx.dataview("get_user_by_username", { username });
    if (!user || !Rivers.crypto.verifyPassword(password, user.password_hash)) {
        throw new Error("invalid credentials");
    }

    // Return claims — framework creates session automatically
    return {
        subject: user.id,
        username: user.username,
        groups: user.groups,
    };
}
```

```toml
[api.views.login]
view_type = "Rest"
path      = "auth/login"
method    = "POST"
auth      = "none"
guard     = true

[api.views.login.handler]
type       = "codecomponent"
language   = "javascript"
module     = "handlers/auth.js"
entrypoint = "authenticate"
```

### Guard Lifecycle Hooks (v0.54.0)

Guard views can declare fire-and-forget lifecycle hooks that fire when session state changes. Hooks run via `tokio::spawn` and **cannot influence the auth flow** — they exist for audit logging, metrics, and external event emission.

```toml
[api.views.login.lifecycle_hooks]
on_session_valid.module      = "handlers/audit.js"
on_session_valid.entrypoint  = "onSessionValid"
on_invalid_session.module    = "handlers/audit.js"
on_invalid_session.entrypoint = "onInvalidSession"
on_failed.module             = "handlers/audit.js"
on_failed.entrypoint         = "onLoginFailed"
```

| Hook | Fires when |
|------|-----------|
| `on_session_valid` | Session validation check succeeds on a protected request. |
| `on_invalid_session` | Session validation check fails (expired, revoked, unknown). |
| `on_failed` | Guard credentials are rejected. |

Hooks are fire-and-forget. Return values are ignored. Hooks must not block or perform long-running work — they run concurrently with the originating request and their failures do not affect it.

### WASM Handlers

Write handlers in any language that compiles to WebAssembly:

```toml
[api.views.compute]
view_type = "Rest"
path      = "compute"
method    = "POST"

[api.views.compute.handler]
type       = "codecomponent"
language   = "wasm"
module     = "handlers/compute.wat"
entrypoint = "handler"
```

WASM modules run in Wasmtime with configurable fuel limits and memory pages. Configure via `[runtime.process_pools]`:

```toml
[runtime.process_pools.wasm]
engine          = "wasmtime"
workers         = 2
task_timeout_ms = 5000
```

---

## SSE Views (Server-Sent Events)

Push data to clients via EventBus triggers or polling:

```toml
[api.views.order_updates]
view_type           = "ServerSentEvents"
path                = "orders/stream"
auth                = "none"
sse_tick_interval_ms = 5000                          # Poll every 5 seconds
sse_trigger_events   = ["OrderCreated", "OrderUpdated"]  # Push on events
sse_event_buffer_size = 200                          # Replay buffer for reconnection
max_connections      = 100

[api.views.order_updates.handler]
type     = "dataview"
dataview = "list_recent_orders"
```

Clients reconnect with `Last-Event-ID` header — missed events are replayed from the buffer.

---

## WebSocket Views

```toml
[api.views.chat]
view_type      = "Websocket"
path           = "ws/chat"
websocket_mode = "Broadcast"       # or "Direct" for targeted messaging
max_connections = 500

[api.views.chat.handler]
type       = "codecomponent"
language   = "javascript"
module     = "handlers/chat.js"
entrypoint = "onMessage"
```

- **Broadcast**: all connected clients receive every message
- **Direct**: messages routed to specific connections via `ConnectionRegistry`

---

## Streaming REST

Return chunked responses from CodeComponent handlers:

```toml
[api.views.export]
view_type        = "Rest"
path             = "export"
method           = "POST"
streaming        = true
streaming_format = "ndjson"        # or "sse"
stream_timeout_ms = 120000

[api.views.export.handler]
type       = "codecomponent"
language   = "javascript"
module     = "handlers/export.js"
entrypoint = "generate"
```

---

## Polling Views

Attach polling to SSE or WebSocket views for automatic data change detection:

```toml
[api.views.dashboard]
view_type            = "ServerSentEvents"
path                 = "dashboard/stream"
sse_tick_interval_ms = 3000

[api.views.dashboard.handler]
type     = "dataview"
dataview = "dashboard_stats"

[api.views.dashboard.polling]
tick_interval_ms = 3000
diff_strategy    = "hash"          # "hash", "null", or "change_detect"
```

| Strategy | Behavior |
|----------|----------|
| `hash` | SHA-256 of result — push only when hash changes |
| `null` | Push when result transitions from null/empty to non-empty |
| `change_detect` | Custom CodeComponent decides if data changed |

---

## GraphQL

Enable GraphQL to expose DataViews as Query fields and CodeComponent views as Mutation fields:

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

- **Query fields**: auto-generated from all registered DataViews
- **Mutation fields**: auto-generated from CodeComponent views with `method != GET`
- **Playground**: available at `/graphql/playground` when introspection is enabled

```bash
# Query
curl -k -X POST https://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{"query":"{ list_contacts }"}'

# Mutation (dispatches to CodeComponent handler)
curl -k -X POST https://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{"query":"mutation { create_contact(input: \"{\\\"name\\\": \\\"Alice\\\"}\") }"}'
```

---

## Schema Files

Schema files live in `schemas/` and are referenced by path:

```json
{
  "type": "object",
  "properties": {
    "id":    { "type": "integer", "faker": "datatype.uuid" },
    "name":  { "type": "string",  "faker": "person.fullName" },
    "email": { "type": "string",  "faker": "internet.email" },
    "city":  { "type": "string",  "faker": "location.city" }
  },
  "required": ["id", "name"]
}
```

The `faker` attribute is driver-specific — it tells the faker driver how to generate synthetic data. Other drivers ignore it.

### Rivers Primitive Types

`uuid`, `string`, `integer`, `float`, `decimal`, `boolean`, `email`, `phone`, `datetime`, `date`, `url`, `json`, `bytes`

---

## Handler Pipeline

Views can declare pipeline stages that run before and after the main handler:

```toml
[api.views.create_order.event_handlers]
pre_process  = [{ module = "handlers/validate.js", entrypoint = "validateOrder" }]
handlers     = [{ module = "handlers/order.js", entrypoint = "createOrder" }]
post_process = [{ module = "handlers/notify.js", entrypoint = "sendConfirmation" }]
on_error     = [{ module = "handlers/error.js", entrypoint = "handleError" }]
```

Execution order:
1. `pre_process` — ctx available, resdata empty
2. DataViews execute — results land on `ctx.data`
3. `handlers` — modify `ctx.resdata`
4. `post_process` — final side effects only
5. `on_error` — fires on failure at any step

All stages are sequential (no parallel execution).

---

## MessageConsumer Views

Process broker messages (Kafka, RabbitMQ, NATS):

```toml
[api.views.process_orders]
view_type = "MessageConsumer"

[api.views.process_orders.on_event]
module     = "handlers/order.js"
entrypoint = "processOrderEvent"
```

MessageConsumer views are automatically exempt from session requirements. They receive messages via the EventBus when a broker bridge is configured for the datasource.
