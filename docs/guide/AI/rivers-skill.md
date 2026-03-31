---
name: rivers-dev
description: Build Rivers application bundles — declarative REST APIs, WebSocket, SSE, streaming, and GraphQL endpoints using TOML configuration, JSON schemas, and JavaScript/WASM CodeComponent handlers. Use when building Rivers apps, writing handlers, configuring datasources/DataViews/views, or administering riversd. Covers the full Rivers v0.52.5 stack.
---

# Rivers Application Development Skill

Build and administer Rivers applications. Rivers is a declarative app-service framework where applications are defined entirely through TOML configuration, JSON schemas, and optional JavaScript/WASM handlers — no Rust code required.

## When to Use

- User asks to build a Rivers app, bundle, or endpoint
- User asks to write a JavaScript or WASM handler for Rivers
- User asks about Rivers DataViews, views, schemas, or config
- User asks to configure riversd, riversctl, or rivers-lockbox
- User mentions TOML config for REST API, WebSocket, SSE, or GraphQL

---

## Bundle Structure

```
{bundle-name}/
├── manifest.toml              # bundleName, bundleVersion, apps[]
├── {app-service}/
│   ├── manifest.toml          # appName, appId, type="app-service", entryPoint
│   ├── resources.toml         # [[datasources]], [[services]]
│   ├── app.toml               # [data.datasources.*], [data.dataviews.*], [api.views.*]
│   ├── schemas/               # JSON schema files (.schema.json)
│   └── libraries/             # JS/TS/WASM handlers
└── {app-main}/
    ├── manifest.toml          # type="app-main"
    ├── resources.toml
    ├── app.toml
    └── libraries/spa/         # SPA assets (Svelte, React, etc.)
```

**Rules:**
- `apps` array: services before mains
- `appId` is a stable UUID — NEVER regenerate
- `type = "data_view"` is WRONG — use `type = "dataview"` (lowercase, one word)

---

## DataView Configuration

```toml
[data.dataviews.list_contacts]
name          = "list_contacts"
datasource    = "contacts"
query         = "schemas/contact.schema.json"
return_schema = "schemas/contact.schema.json"

# Cache invalidation: clear these DataView caches on successful execution
invalidates = ["list_contacts", "search_contacts"]

[data.dataviews.list_contacts.caching]
ttl_seconds = 60

[[data.dataviews.list_contacts.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20
```

### CRUD DataViews (per-method queries)

```toml
[data.dataviews.users]
name       = "users"
datasource = "db"
get_query    = "SELECT * FROM users WHERE id = $id"
post_query   = "INSERT INTO users (name, email) VALUES ($name, $email) RETURNING *"
put_query    = "UPDATE users SET name = $name WHERE id = $id RETURNING *"
delete_query = "DELETE FROM users WHERE id = $id"
get_schema    = "schemas/user.schema.json"
post_schema   = "schemas/user-create.schema.json"
```

### Parameter Types

`string`, `integer`, `float`, `boolean`, `array`, `uuid`, `email`, `phone`, `datetime`, `date`, `url`, `json`

---

## View Configuration

### REST View with DataView Handler

```toml
[api.views.list_contacts]
path      = "/api/contacts"
method    = "GET"
view_type = "Rest"
auth      = "none"

[api.views.list_contacts.handler]
type     = "dataview"
dataview = "list_contacts"

[api.views.list_contacts.parameter_mapping.query]
limit  = "limit"
offset = "offset"
```

### REST View with CodeComponent Handler

```toml
[api.views.create_order]
path      = "/api/orders"
method    = "POST"
view_type = "Rest"

[api.views.create_order.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/orders.js"
entrypoint = "createOrder"
resources  = ["orders_db"]
```

### View Types

| Type | Transport |
|------|-----------|
| `Rest` | HTTP request/response |
| `Websocket` | WS upgrade, bidirectional |
| `ServerSentEvents` | HTTP long-lived, server→client |
| `MessageConsumer` | EventBus event (no HTTP route) |

### Auth & Guard Views

```toml
auth  = "none"       # Public
auth  = "session"    # Requires valid session
guard = true         # This IS the login endpoint (creates sessions)
```

### SSE Views

```toml
[api.views.events]
view_type              = "ServerSentEvents"
path                   = "/api/events"
sse_tick_interval_ms   = 5000
sse_trigger_events     = ["OrderCreated", "OrderUpdated"]
sse_event_buffer_size  = 200    # Last-Event-ID replay buffer (default 100)
```

### WebSocket Views with Lifecycle Hooks

```toml
[api.views.chat]
view_type      = "Websocket"
path           = "/ws/chat"
websocket_mode = "Broadcast"    # or "Direct"

[api.views.chat.ws_hooks]
on_connect.module       = "handlers/chat.js"
on_connect.entrypoint   = "onConnect"
on_message.module       = "handlers/chat.js"
on_message.entrypoint   = "onMessage"
on_disconnect.module    = "handlers/chat.js"
on_disconnect.entrypoint = "onDisconnect"
```

### Streaming REST Views

```toml
[api.views.export]
view_type         = "Rest"
path              = "/api/export"
method            = "POST"
streaming         = true
streaming_format  = "ndjson"    # or "sse"
stream_timeout_ms = 120000
```

Handler uses `{chunk, done}` protocol: return `{chunk: <data>, done: false}` per yield, `{done: true}` to complete.

### Polling on SSE/WS Views

```toml
[api.views.dashboard.polling]
tick_interval_ms = 3000
diff_strategy    = "hash"       # hash | null | change_detect
poll_state_ttl_s = 300
```

---

## GraphQL Configuration

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

- **Query fields** auto-generated from DataViews
- **Mutation fields** auto-generated from CodeComponent POST/PUT/DELETE views
- **Subscription fields** auto-generated from SSE trigger events via EventBus
- Playground at `/graphql/playground` when introspection enabled

---

## JavaScript Handler API

Handlers receive a `ctx` object:

```javascript
function handler(ctx) {
    // Request data (read-only)
    const method = ctx.request.method;
    const body = ctx.request.body;
    const id = ctx.request.params.id;
    const query = ctx.request.query;

    // Pre-fetched DataView results
    const users = ctx.data.list_users;

    // Call DataView dynamically
    const orders = ctx.dataview("get_orders", { user_id: id });

    // Application KV store
    ctx.store.set("cache:key", { data: 42 }, 60000);  // TTL in ms
    const cached = ctx.store.get("cache:key");
    ctx.store.del("cache:key");
    // Reserved prefixes blocked: session:, csrf:, cache:, raft:, rivers:

    // Set response
    ctx.resdata = { users, orders };
}
```

### Context Properties

| Property | Type | Description |
|----------|------|-------------|
| `ctx.request` | Object | `{method, path, headers, query, body, path_params}` (read-only) |
| `ctx.resdata` | Any | Response payload (set this to return data) |
| `ctx.data` | Object | Pre-fetched DataView results keyed by name |
| `ctx.store` | Object | Application KV store (`get`, `set`, `del`) |
| `ctx.dataview()` | Function | Call DataView dynamically |
| `ctx.ws` | Object | WebSocket context (`connection_id`, `message`) — only in WS hooks |
| `ctx.trace_id` | String | Request trace ID |
| `ctx.app_id` | String | Application ID |
| `ctx.node_id` | String | Node identifier |
| `ctx.env` | String | Runtime environment |

### Rivers Global APIs

```javascript
// Structured logging (trace_id auto-included)
Rivers.log.info("user login", { userId: 123 });
Rivers.log.warn("rate limit approaching");
Rivers.log.error("payment failed", { reason: "declined" });

// Crypto
var hash = Rivers.crypto.hashPassword("secret");
var valid = Rivers.crypto.verifyPassword("secret", hash);
var hex = Rivers.crypto.randomHex(16);
var token = Rivers.crypto.randomBase64url(32);
var sig = Rivers.crypto.hmac("key", "data");
var eq = Rivers.crypto.timingSafeEqual("a", "b");

```

### Guard Handler (Authentication)

```javascript
function authenticate(ctx) {
    var user = ctx.dataview("get_user_by_username", { username: ctx.request.body.username });
    if (!user || !Rivers.crypto.verifyPassword(ctx.request.body.password, user.password_hash)) {
        throw new Error("invalid credentials");
    }
    // Return claims — framework creates session
    return { subject: user.id, username: user.username, groups: user.groups };
}
```

### Streaming Handler ({chunk, done} Protocol)

```javascript
function generate(ctx) {
    var iteration = __args.iteration || 0;
    if (iteration >= 100) return { done: true };
    return {
        chunk: { index: iteration, data: "row-" + iteration },
        done: false
    };
}
```

State is passed between iterations via `__args.state` (previous return value) and `__args.iteration` (counter).

### WebSocket Lifecycle Hooks

```javascript
// on_connect: return non-null to send welcome, return false to reject
function onConnect(ctx) {
    Rivers.log.info("client connected", { connection_id: ctx.ws.connection_id });
    return { welcome: "Connected to chat" };
}

// on_message: receives message, return non-null to reply
function onMessage(ctx) {
    var msg = ctx.ws.message;
    Rivers.log.info("message received", { text: msg.text });
    return { echo: msg.text };
}

// on_disconnect: cleanup
function onDisconnect(ctx) {
    Rivers.log.info("client disconnected", { connection_id: ctx.ws.connection_id });
}
```

---

## WASM Handler API

WASM handlers run in Wasmtime. Write in any language with WASM target (Rust, C, Go/TinyGo, AssemblyScript, Zig).

### WAT Example

```wat
(module
  (func (export "handler") (result i32)
    (i32.add (i32.const 17) (i32.const 25))
  )
)
```

### Configuration

```toml
[api.views.compute.handler]
type       = "codecomponent"
language   = "wasm"
module     = "libraries/compute.wat"    # or .wasm
entrypoint = "handler"

[runtime.process_pools.wasm]
engine          = "wasmtime"
workers         = 2
task_timeout_ms = 5000
```

### WASM Runtime Limits

| Config | Default | Description |
|--------|---------|-------------|
| `fuel_limit` | 1,000,000 | CPU instruction budget |
| `memory_pages` | 256 | WASM memory (256 × 64KB = 16MB) |
| `instance_pool_size` | 4 | Pre-compiled instance pool |

---

## ProcessPool Configuration

```toml
[runtime.process_pools.default]
engine                   = "v8"         # v8 | wasmtime
workers                  = 4
task_timeout_ms          = 5000
max_heap_mb              = 128
max_queue_depth          = 0            # 0 = workers × 4
recycle_after_tasks      = 0            # 0 = never recycle
heap_recycle_threshold   = 0.8          # recycle isolate if heap > 80%

[runtime.process_pools.wasm]
engine                   = "wasmtime"
workers                  = 2
task_timeout_ms          = 5000
```

- V8 isolates are pooled and reused (SHAPE-9)
- No V8 snapshots — state injected via globals (SHAPE-10)
- Per-request isolation via context unbinding
- Watchdog thread terminates timed-out tasks

---

## Datasource Drivers

| Driver | Type | Credentials | Use Case |
|--------|------|-------------|----------|
| `faker` | Database | `nopassword = true` | Synthetic test data |
| `postgres` | Database | lockbox | Relational data |
| `mysql` | Database | lockbox | Relational data |
| `sqlite` | Database | `nopassword = true` | Embedded relational |
| `redis` | Database | lockbox | Cache, sessions, KV |
| `mongodb` | Database (plugin) | lockbox | Document store |
| `elasticsearch` | Database (plugin) | lockbox | Search |
| `cassandra` | Database (plugin) | lockbox | Wide-column |
| `couchdb` | Database (plugin) | lockbox | Document store |
| `influxdb` | Database (plugin) | lockbox | Time series |
| `ldap` | Database (plugin) | lockbox | Directory |
| `rivers-exec` | Database (plugin) | `nopassword = true` | Script execution |
| `kafka` | Broker (plugin) | lockbox | Message streaming |
| `rabbitmq` | Broker (plugin) | lockbox | Message queuing |
| `nats` | Broker (plugin) | lockbox | Message pub/sub |
| `redis-streams` | Broker (plugin) | lockbox | Stream processing |
| `http` | HTTP | optional | Inter-service proxy |
| `eventbus` | Internal | none | Pub/sub via standard interface |

---

## Server Configuration (`riversd.toml`)

```toml
bundle_path = "my-bundle/"

[base]
host = "0.0.0.0"
port = 8080
request_timeout_seconds = 30

[base.tls]
# Optional — auto-generates self-signed certs if absent
cert = "/etc/rivers/tls/server.crt"
key  = "/etc/rivers/tls/server.key"
redirect = true

[base.admin_api]
enabled = true
host    = "127.0.0.1"
port    = 9090

[security]
cors_enabled         = true
cors_allowed_origins = ["https://app.example.com"]
rate_limit_per_minute = 120

[storage_engine]
backend = "memory"              # memory | sled | redis
path    = "data/rivers.db"

[graphql]
enabled = true

[lockbox]
path       = "lockbox/keystore.rkeystore"
key_source = "env"
key_env_var = "RIVERS_LOCKBOX_KEY"
```

---

## CLI Tools

```bash
# Server
riversd --config riversd.toml            # Start server
riversd --version                         # Print version
riversd --no-ssl --port 8080             # Plain HTTP (dev only)
riversd --log-level debug                # Override log level

# Control
riversctl start --config riversd.toml     # Start via helper
riversctl doctor                           # Health check diagnostics
riversctl validate my-bundle/             # Validate bundle (9 checks)
riversctl validate --schema server        # Output JSON Schema
riversctl exec hash <input>               # Hash utility
riversctl admin status                     # Query admin API
riversctl admin deploy my-bundle/         # Deploy via admin API

# Secrets
rivers-lockbox init                       # Create keystore
rivers-lockbox add db-password --value secret
rivers-lockbox list                       # List entries (no values)
rivers-lockbox show db-password           # Decrypt and display
rivers-lockbox alias db-password alt-name # Add alias
rivers-lockbox rotate db-password         # Rotate entry
rivers-lockbox remove db-password         # Remove entry
rivers-lockbox validate                   # Validate keystore integrity

# App Keystore
rivers-keystore init <path>               # Create keystore
rivers-keystore generate <path> <name>    # Generate key
rivers-keystore list <path>               # List keys
rivers-keystore info <path> <name>        # Key metadata
rivers-keystore delete <path> <name>      # Delete key
rivers-keystore rotate <path> <name>      # Rotate key

# Packaging
riverpackage validate my-bundle/          # Validate bundle structure
riverpackage preflight my-bundle/         # Pre-deployment checks
riverpackage pack my-bundle/              # Package for deployment
riverpackage import-exec <path>           # Import exec scripts

# Deployment
cargo deploy <path>                       # Deploy dynamic mode
cargo deploy <path> --static              # Deploy static mode
```

---

## Error Response Format (SHAPE-2)

All server-generated errors use:

```json
{"code": 404, "message": "view not found", "trace_id": "abc-123"}
```

| Code | Condition |
|------|-----------|
| 400 | Bad request, parameter validation |
| 401 | Admin auth failed |
| 403 | RBAC permission denied |
| 404 | View/file not found |
| 405 | Method not allowed |
| 408 | Request timeout |
| 422 | Schema validation failed |
| 429 | Rate limit exceeded |
| 500 | Internal server error |
| 503 | Server draining, backpressure, circuit open |

---

## Validation Rules

| Rule | Error |
|------|-------|
| Handler `type = "data_view"` | WRONG — use `type = "dataview"` |
| `invalidates` target not found | `invalidates target 'X' does not exist` |
| Unknown `view_type` | `unknown view_type 'X' (expected: Rest, Websocket, ServerSentEvents, MessageConsumer)` |
| Unknown driver | warning: `unknown driver 'X'` |
| Duplicate datasource names | `duplicate datasource name 'X'` |
| Schema file not found | `schema file 'X' not found` |
| Service references unknown appId | `service references unknown appId 'X'` |
| `appId` missing or not UUID | `appId is required` |
| Duplicate appId in bundle | `duplicate appId` |
