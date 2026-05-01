# Rivers Technology Path Specification

**Document Type:** Architecture Specification  
**Version:** 1.0  
**Status:** Locked — Implementation-Ready  
**Scope:** JS API, Schemas, DataView CRUD, Handler Pipeline, Auth & Sessions, CORS, EventBus, Streaming REST, WebSocket, Application Architecture, StorageEngine, Rate Limiting, Logging  
**Supersedes:** Prior inline spec fragments in ProcessPool, View Layer, and Auth specs where conflicts exist  
**Context:** Designed for two primary consumers — (1) web developers building SPAs and microservices rapidly, (2) agentic projects that need RESTful services with minimal token spend

---

## Table of Contents

1. [Design Philosophy](#1-design-philosophy)
2. [Handler Context (`ctx`)](#2-handler-context-ctx)
3. [Handler Contracts](#3-handler-contracts)
4. [Handler Pipeline](#4-handler-pipeline)
5. [DataView CRUD Model](#5-dataview-crud-model)
6. [Pseudo DataViews](#6-pseudo-dataviews)
7. [Schema System](#7-schema-system)
8. [Driver Contract](#8-driver-contract)
9. [Authentication & Sessions](#9-authentication--sessions)
10. [CORS](#10-cors)
11. [Application Init Handler](#11-application-init-handler)
12. [EventBus](#12-eventbus)
13. [Streaming REST](#13-streaming-rest)
14. [WebSocket Views](#14-websocket-views)
15. [StorageEngine](#15-storageengine)
16. [Caching](#16-caching)
17. [Rate Limiting](#17-rate-limiting)
18. [Logging](#18-logging)
19. [Application Architecture](#19-application-architecture)
20. [Appendix A — Full `ctx` Reference](#appendix-a--full-ctx-reference)
21. [Appendix B — Declarative vs Handler Coverage](#appendix-b--declarative-vs-handler-coverage)

---

## 1. Design Philosophy

### 1.1 The Collapse Principle

Find the simplest, best way to do something, then collapse everything to that one path. Every data operation in Rivers — whether it originates from a TOML declaration, a REST request, a handler's inline query, or a pseudo DataView — flows through the same pipeline. One path. One set of behaviors. One thing to debug.

### 1.2 Declarative First, Code When Needed

The framework eats the boilerplate so the developer writes only the business logic that matters. A typical backend requires ~3.5K lines of infrastructure code (HTTP setup, connection management, error handling, auth, serialization). Rivers reduces the developer's responsibility to ~500 lines of actual business logic.

The declarative surface covers full CRUD — reads, writes, updates, deletes — without a single line of JS. The JS environment exists for the 10% that config cannot express: cross-datasource joins, conditional logic, data transformations that depend on runtime state.

### 1.3 Two Personas, One System

**Web developer building SPAs/microservices:** Declares DataViews in TOML, binds them to views, and ships. Full CRUD API from config files. Drops into JS only for business logic.

**Agentic project minimizing token spend:** Generates TOML and schema files (structured config, not code). Token cost drops dramatically because the LLM fills in a schema, not writing a program. When logic is needed, the handler is small and the API surface is minimal.

### 1.4 Architecture Made Visible

The pseudo DataView builder chain is the TOML declaration expressed in code. A developer who starts with pseudo DataViews accidentally learns how Rivers works internally. When they promote to TOML, they already understand what every field means. One mental model generates both.

---

## 2. Handler Context (`ctx`)

All handlers receive a single `ctx` object. This is the handler's entire world — reads come from it, writes go through it, the response is built on it.

### 2.1 Full Interface

```typescript
interface Context {
    // ── Metadata ──
    trace_id:    string;
    node_id:     string;
    app_id:      string;
    env:         string;           // "dev" | "staging" | "prod"

    // ── Identity (populated by guard, immutable) ──
    session?:    Session;

    // ── Request (read-only) ──
    request:     Request;

    // ── Pre-fetched DataView results (read-only, nested namespace) ──
    data:        Record<string, any>;

    // ── Response payload (mutable — this is what goes to the client) ──
    resdata:     any;

    // ── DataView calls ──
    dataview(name: string, params?: Record<string, any>): Promise<any>;

    // ── Streaming DataView (returns AsyncGenerator) ──
    streamDataview(name: string, params?: Record<string, any>): AsyncGenerator<StreamChunk>;

    // ── Datasource factory → pseudo DataView builder ──
    datasource(name: string): DatasourceBuilder;

    // ── Application KV store ──
    store:       Store;

    // ── WebSocket context (WebSocket views only, mode-dependent) ──
    ws?:         WebSocketContext;
}
```

### 2.2 Session

Light, bounded, immutable for the life of the session.

```typescript
interface Session {
    id:        string;                    // session ID
    identity: {
        username:  string;                // authenticated principal
        groups:    string[];              // roles, permissions, memberships
    };
    apikey?:   string;                    // if authenticated via API key
    tenant?:   string;                    // tenant scope
    cdate:     string;                    // ISO 8601 — session creation time
}
```

### 2.3 Request

```typescript
interface Request {
    method:  string;
    path:    string;
    headers: Record<string, string>;
    query:   Record<string, string>;
    body:    any;
    params:  Record<string, string>;      // path params
}
```

### 2.4 Store

Application KV access. Scoped to `app:{app_id}` automatically. Reserved namespace prefixes (`session:`, `csrf:`, `cache:`, `raft:`, `rivers:`) are rejected by the host.

```typescript
interface Store {
    get(key: string): Promise<any>;
    set(key: string, value: any, ttl_seconds?: number): Promise<void>;
    del(key: string): Promise<void>;
}
```

### 2.5 Global Utilities

Present on the `Rivers` namespace, not on `ctx`:

```typescript
// Always available
Rivers.log        // structured logging (info, warn, error)
Rivers.crypto     // crypto utilities (hashPassword, verifyPassword, hmac, etc.)

// Conditional — only if declared in view config
Rivers.http?      // outbound HTTP (when allow_outbound_http = true)
Rivers.env?       // env vars (when allow_env_vars = true)
```

### 2.6 Nested Data Namespace

Pre-fetched DataView results live under `ctx.data` to avoid collision with methods and framework properties on `ctx`. DataView names from the TOML declaration become keys:

```typescript
// TOML declares: dataviews = ["list_orders", "get_customer"]
ctx.data.list_orders      // pre-fetched result
ctx.data.get_customer     // pre-fetched result
```

### 2.7 `$resdata` — The Mutable Response

`ctx.resdata` is what goes to the client. The framework populates it from the primary DataView result. Handlers modify it. When all handlers return, `ctx.resdata` is serialized and sent.

For declarative views (no handler), the primary DataView result becomes `ctx.resdata` directly. For handler-backed views, handlers read from `ctx.data` and build `ctx.resdata`.

---

## 3. Handler Contracts

Three distinct contracts. Same `ctx`, same capabilities, different return values.

### 3.1 Standard Handler

The pipeline participant. Receives `ctx`, modifies `ctx.resdata`, returns void.

```typescript
function handler(ctx): void | Promise<void> { }
```

Framework reads `ctx.resdata` after all handlers complete.

### 3.2 Guard Handler

The auth entry point. Receives `ctx`, returns `IdentityClaims`. Can accept parameters from the request.

```typescript
async function authenticate(ctx): Promise<IdentityClaims> {
    const { username, password } = ctx.request.body;
    // validate credentials
    return { subject: user.id, ... };
}
```

Framework consumes the return value to create a session.

### 3.3 Streaming Handler

The generator. Receives `ctx`, yields chunks. Can accept parameters.

```typescript
async function* generate(ctx): AsyncGenerator<any> {
    for await (const chunk of ctx.streamDataview("completion", { prompt })) {
        yield { token: chunk.data };
    }
}
```

Framework drives the generator, flushes chunks to the wire.

### 3.4 Capability Parity

All three contracts can:

- Read `ctx.data` (pre-fetched DataViews)
- Call `ctx.dataview()` for additional reads/writes
- Call `ctx.streamDataview()` for streaming reads
- Call `ctx.datasource().build()` for pseudo DataViews
- Read `ctx.session`
- Access `ctx.store` for KV
- Use `Rivers.log`, `Rivers.crypto`

---

## 4. Handler Pipeline

### 4.1 Collapsed Model

The six-stage pipeline from the prior spec collapses to a simple ordered model:

```
pre_process handlers    →  ctx available, resdata empty
DataViews execute       →  results land on ctx.data
                        →  primary DataView populates ctx.resdata
handlers                →  execute in declared order, modify ctx.resdata
post_process handlers   →  ctx.resdata is final, side effects only
on_error                →  fires on failure at any step above
```

### 4.2 One Contract, Four Labels

Every handler — `pre_process`, `handlers`, `post_process`, `on_error` — receives the same `ctx` with the same fields. The label determines timing, not capability.

```typescript
// All four use the same signature
function handler(ctx): void | Promise<void> { }
```

### 4.3 What Was Absorbed

| Old stage | Absorbed by |
|---|---|
| `on_request` accumulators | Declared DataViews pre-fetching onto `ctx.data` |
| Primary execution | Handler receiving `ctx` with pre-fetched data |
| `transform` | Handlers in the ordered chain modifying `ctx.resdata` |
| `on_response` | Handler calling `ctx.dataview()` for enrichment |

### 4.4 View Declaration

```toml
[api.views.order_summary]
path      = "/api/orders/{id}"
method    = "GET"
view_type = "Rest"
dataviews = ["get_order", "get_customer", "get_shipping"]
primary   = "get_order"

[[api.views.order_summary.pre_process]]
module     = "handlers/audit.ts"
entrypoint = "logInbound"

[[api.views.order_summary.handlers]]
module     = "handlers/orders.ts"
entrypoint = "buildSummary"

[[api.views.order_summary.handlers]]
module     = "handlers/security.ts"
entrypoint = "redactSensitive"

[[api.views.order_summary.post_process]]
module     = "handlers/metrics.ts"
entrypoint = "recordLatency"
```

### 4.5 Primary DataView Binding

The view binds to exactly one DataView for `$resdata` via the `primary` field. Even when multiple DataViews are declared for pre-fetch, the primary binding is explicit. No guessing.

```toml
dataviews = ["get_order", "get_customer", "get_shipping"]
primary   = "get_order"    # this populates ctx.resdata
```

---

## 5. DataView CRUD Model

### 5.1 One Resource, Four Operations

A DataView is a REST resource. Each HTTP method gets its own query, schema, and parameters. Only one operation is active per request — the HTTP method determines which.

```toml
[data.dataviews.orders]
datasource = "orders_db"

get_query    = "SELECT * FROM orders WHERE id = $id"
get_schema   = "schemas/order_get.schema.json"

post_query   = "INSERT INTO orders (customer_id, amount, status) VALUES ($customer_id, $amount, $status) RETURNING *"
post_schema  = "schemas/order_post.schema.json"

put_query    = "UPDATE orders SET amount = $amount, status = $status WHERE id = $id RETURNING *"
put_schema   = "schemas/order_put.schema.json"

delete_query = "DELETE FROM orders WHERE id = $id"
delete_schema = "schemas/order_delete.schema.json"
```

### 5.2 Per-Method Parameters

Parameters are scoped per method. A GET needs `$id`, a POST needs `$customer_id, $amount, $status`, a PUT needs all of them.

```toml
[[data.dataviews.orders.get.parameters]]
name     = "id"
type     = "uuid"
required = true

[[data.dataviews.orders.post.parameters]]
name     = "customer_id"
type     = "uuid"
required = true

[[data.dataviews.orders.post.parameters]]
name     = "amount"
type     = "decimal"
required = true

[[data.dataviews.orders.post.parameters]]
name     = "status"
type     = "string"
required = false
default  = "pending"

[[data.dataviews.orders.put.parameters]]
name     = "id"
type     = "uuid"
required = true

[[data.dataviews.orders.put.parameters]]
name     = "amount"
type     = "decimal"
required = true

[[data.dataviews.orders.put.parameters]]
name     = "status"
type     = "string"
required = true

[[data.dataviews.orders.delete.parameters]]
name     = "id"
type     = "uuid"
required = true
```

### 5.3 Variable Binding

`$variables` in query strings bind against the parameters declared for that method. The SchemaSyntaxChecker verifies at build time that every `$variable` has a matching parameter declaration, and flags orphans in either direction.

### 5.4 Backward Compatibility

For read-only DataViews, the shorthand fields are aliases:

- `query` → `get_query`
- `return_schema` → `get_schema`
- `parameters` → `get.parameters`

### 5.5 View Binding

One view, one DataView, full CRUD:

```toml
[api.views.orders]
path      = "/api/orders/{id}"
view_type = "Rest"
dataview  = "orders"

[api.views.orders.parameter_mapping.path]
id = "id"

[api.views.orders.parameter_mapping.body]
customer_id = "customer_id"
amount      = "amount"
status      = "status"
```

The runtime resolves: `DELETE /api/orders/abc-123` pulls `id` from the path, matches to `delete.parameters`, validates against `delete_schema`, executes `delete_query`.

### 5.6 Fully Declarative CRUD

No handler needed for standard CRUD:

```toml
# POST — schema validates input, DataView executes INSERT, return schema validates output
# GET — DataView executes SELECT, return schema validates output
# PUT — schema validates input, DataView executes UPDATE, return schema validates output
# DELETE — parameters validated, DataView executes DELETE
```

One DataView declaration and a handful of schema files produce an entire REST resource.

---

## 6. Pseudo DataViews

### 6.1 Purpose

When a handler needs a one-off query that isn't worth declaring in TOML, it builds a pseudo DataView at runtime. The builder chain mirrors the TOML declaration — same steps, same order, same pipeline under the covers.

### 6.2 Builder API

```typescript
interface DatasourceBuilder {
    fromQuery(sql: string, params?: any[]): DatasourceBuilder;
    fromSchema(schema: object | string, params?: Record<string, any>): DatasourceBuilder;
    withGetSchema(schema: object | string): DatasourceBuilder;
    withPostSchema(schema: object | string): DatasourceBuilder;
    withPutSchema(schema: object | string): DatasourceBuilder;
    withDeleteSchema(schema: object | string): DatasourceBuilder;
    build(): DataView;
}
```

### 6.3 Usage

```typescript
// Build the pseudo DataView
const transferView = ctx.datasource("primary_db")
    .fromQuery("INSERT INTO transfers (from_id, to_id, amount) VALUES ($1, $2, $3)")
    .withPostSchema({
        driver: "postgresql",
        type: "object",
        fields: [
            { name: "from_id", type: "uuid",    required: true },
            { name: "to_id",   type: "uuid",    required: true },
            { name: "amount",  type: "decimal",  required: true }
        ]
    })
    .build();

// Use it like a declared DataView
const result = await transferView({ from_id: fromId, to_id: toId, amount: 100.00 });

// Build once, call multiple times
for (const transfer of transfers) {
    await transferView(transfer);
}
```

### 6.4 Deliberate Limitations

Pseudo DataViews are local, disposable, single-handler scope.

| Feature | Declared DataView | Pseudo DataView |
|---|---|---|
| Caching | Yes | No |
| Cache invalidation | Yes | No |
| Streaming | Yes | No |
| EventBus registration | Yes | No |

The pseudo DataView has no name. The infrastructure can't track it, cache it, or invalidate it. If you need any of those, promote to TOML.

### 6.5 `.build()` Creates, Doesn't Execute

`.build()` produces a DataView object. It doesn't run the query. The schema is syntax-checked at build time. The handler calls the built object with parameters to execute.

### 6.6 Promotion Path

1. **Prototype** — developer builds pseudo DataViews, iterates fast
2. **Harden** — queries that stuck get promoted to TOML, gain deploy-time validation, caching
3. **Simplify** — with enough declared DataViews, the handler may disappear entirely

---

## 7. Schema System

### 7.1 Driver Field

Every schema file includes a `driver` field that routes to the driver's validation engine. A Redis schema and a Postgres schema are fundamentally different shapes.

**PostgreSQL schema (rows and columns):**

```json
{
  "driver": "postgresql",
  "type": "object",
  "description": "Order record",
  "fields": [
    { "name": "id",     "type": "uuid",    "required": true },
    { "name": "amount", "type": "decimal",  "required": true, "min": 0 },
    { "name": "status", "type": "string",  "required": true, "pattern": "^(active|closed)$" }
  ]
}
```

**Redis schema (key structure):**

```json
{
  "driver": "redis",
  "type": "hash",
  "description": "User session data",
  "key_pattern": "session:{session_id}",
  "fields": [
    { "name": "user_id",    "type": "string",  "required": true },
    { "name": "claims",     "type": "json",    "required": true },
    { "name": "expires_at", "type": "datetime", "required": true }
  ]
}
```

**Kafka schema (messages):**

```json
{
  "driver": "kafka",
  "type": "message",
  "description": "Order event",
  "topic": "orders",
  "key":   { "type": "uuid" },
  "value": {
    "fields": [
      { "name": "order_id", "type": "uuid",   "required": true },
      { "name": "action",   "type": "string",  "required": true }
    ]
  }
}
```

### 7.2 Per-Method Schemas

Each HTTP method has its own schema:

- `get_schema` — validates output (what comes back from a read)
- `post_schema` — validates input for creation
- `put_schema` — validates input for replacement
- `delete_schema` — validates input for deletion

A DataView only declares the schemas it uses. No dead config.

### 7.3 Inline Schemas

Pseudo DataViews use the same JSON format as file-based schemas. The `driver` field routes to the correct validator:

```typescript
.withPostSchema({
    driver: "postgresql",
    type: "object",
    fields: [
        { name: "amount", type: "decimal", required: true, min: 0.01 }
    ]
})
```

### 7.4 Schema File Location

Schemas are top-level in the app directory, peer to `libraries/`:

```
orders-service/
├── manifest.toml
├── resources.toml
├── app.toml
├── schemas/
│   ├── order_get.schema.json
│   ├── order_post.schema.json
│   └── order_put.schema.json
└── libraries/
    └── handlers/
```

---

## 8. Driver Contract

### 8.1 Three Responsibilities

Every driver ships three things:

**SchemaSyntaxChecker** — "Is this schema well-formed for my driver?" Runs at build and deploy time. Pure structural validation of the schema document itself. A Redis schema with `type: "string"` and a `fields` array is malformed. A Postgres schema with `key_pattern` makes no sense.

**Validator** — "Does this data match this schema?" Runs at request time. Input body on POST, output rows on GET. Type checking, required fields, min/max, pattern matching. Catches "your SELECT returned 5 columns but your schema only defines 3."

**Executor** — "Run this operation." The actual driver talking to the underlying store. Doesn't care about schemas. Gets a query and params, executes, returns raw results.

### 8.2 Execution Order

```
Build time:   SchemaSyntaxChecker  →  "your schema is valid for this driver"
Deploy time:  SchemaSyntaxChecker  →  re-verified against real driver
Request time: Validator            →  "this input matches the POST schema"
              Executor             →  "query executed, here are results"
              Validator            →  "these results match the GET schema"
```

### 8.3 Trait

```rust
pub trait Driver: Send + Sync {
    fn driver_type(&self) -> DriverType;

    // Build/deploy time — is this schema structurally valid for me?
    fn check_schema_syntax(&self, schema: &SchemaDefinition) -> Result<(), SchemaSyntaxError>;

    // Request time — does this data conform to this schema?
    fn validate(&self, data: &Value, schema: &SchemaDefinition) -> Result<(), ValidationError>;

    // Request time — execute the operation
    async fn execute(&self, dataview: &DataView, params: &QueryParams) -> Result<QueryResult>;

    async fn connect(&mut self, config: &DatasourceConfig) -> Result<()>;
    async fn health_check(&self) -> Result<HealthStatus>;
}
```

---

## 9. Authentication & Sessions

### 9.1 Design Principles

- Rivers owns session lifecycle. The application owns credential validation.
- One guard view per app — sole entry point for credential validation.
- All views are protected by default. `auth = "none"` opts out.
- Session is five fields. Light, bounded, immutable.

### 9.2 Guard View

```toml
[api.views.auth]
path      = "/auth"
method    = "POST"
view_type = "Rest"
guard     = true

[api.views.auth.handler]
type       = "codecomponent"
module     = "handlers/auth.ts"
entrypoint = "authenticate"
dataviews  = ["lookup_user"]

[api.views.auth.guard]
valid_session_url   = "/app/dashboard"
invalid_session_url = "/auth/login"
```

### 9.3 Guard Handler

```typescript
async function authenticate(ctx): Promise<IdentityClaims> {
    const { username, password } = ctx.request.body;
    const user = ctx.data.lookup_user;

    if (!user) throw new Error("invalid credentials");

    const valid = await Rivers.crypto.verifyPassword(password, user.password_hash);
    if (!valid) throw new Error("invalid credentials");

    return {
        subject:  user.id,
        username: user.username,
        groups:   user.roles,
        tenant:   user.tenant_id
    };
}
```

### 9.4 Session — Five Fields

```typescript
interface Session {
    id:        string;
    identity: {
        username:  string;
        groups:    string[];
    };
    apikey?:   string;
    tenant?:   string;
    cdate:     string;    // ISO 8601
}
```

### 9.5 Guard Lifecycle Hooks

Three optional hooks. All return void. Side effects only. Cannot influence auth flow.

```typescript
async function onSessionValid(ctx): Promise<void> { }
async function onInvalidSession(ctx): Promise<void> { }
async function onFailed(ctx): Promise<void> { }
```

TOML is authoritative for all routing decisions (`valid_session_url`, `invalid_session_url`). Hooks cannot override redirects or responses.

### 9.6 CSRF Protection

- Double-submit cookie pattern, auto-validated on state-changing methods
- Bearer token requests exempt
- Framework-managed — no developer configuration needed
- Cookie-based sessions get CSRF automatically

### 9.7 Session Carry-Over (Multi-App)

V1 is single node. All apps in the bundle share the same `riversd` process and the same StorageEngine instance. App-main creates the session, app-services validate against the same StorageEngine. No header propagation needed, no Redis required.

---

## 10. CORS

### 10.1 App-Level, Init Handler

CORS is an application concern, configured once at startup in the init handler. Not per-view, not global server config.

```typescript
export async function init(app: Rivers.Application): Promise<void> {
    app.cors({
        origins:     app.config.allowed_origins,
        methods:     ["GET", "POST", "PUT", "DELETE"],
        headers:     ["Content-Type", "Authorization", "X-CSRF-Token"],
        credentials: true,
        max_age:     3600
    });
}
```

### 10.2 No Environment Branching

The init handler has no access to `env`. Developers define policy structure, ops define policy values via environment variable substitution in TOML:

```toml
[app.config]
allowed_origins = ["${APP_ORIGIN}", "${ADMIN_ORIGIN}"]
```

One CORS policy per app. Same code everywhere. Dev sets `APP_ORIGIN=http://localhost:3000`, prod sets `APP_ORIGIN=https://myapp.example.com`.

### 10.3 CorsPolicy Interface

```typescript
interface CorsPolicy {
    origins:      string[];
    methods:      string[];
    headers:      string[];
    credentials:  boolean;
    max_age?:     number;
}
```

---

## 11. Application Init Handler

### 11.1 Purpose

The init handler is the application bootstrap. It fires after resource resolution, before the app accepts traffic. Each app (app-main and app-service) gets its own.

### 11.2 Rivers.Application Context

```typescript
interface Application {
    app_id:     string;
    app_name:   string;
    config:     Record<string, any>;    // from [app.config] in TOML

    cors(policy: CorsPolicy): void;
    health(check: (ctx) => Promise<string>): void;
    dataview(name: string, params?: Record<string, any>): Promise<any>;
    onShutdown(handler: () => Promise<void>): void;
}
```

No `env`, no `node_id`, no runtime metadata that would tempt branching.

### 11.3 TOML Declaration

```toml
[app]
init_handler    = "handlers/init.ts"
init_entrypoint = "init"

[app.config]
allowed_origins = ["${APP_ORIGIN}"]
seed_on_start   = false
seed_version    = "1.0"
```

### 11.4 Example

```typescript
export async function init(app: Rivers.Application): Promise<void> {
    Rivers.log.info("initializing", { app: app.app_name });

    app.cors({
        origins:     app.config.allowed_origins,
        methods:     ["GET", "POST", "PUT", "DELETE"],
        headers:     ["Content-Type", "Authorization", "X-CSRF-Token"],
        credentials: true
    });

    app.health(async (ctx) => {
        const db = await ctx.dataview("health_ping");
        return db ? "healthy" : "degraded";
    });

    if (app.config.seed_on_start) {
        await app.dataview("seed_defaults", { version: app.config.seed_version });
    }

    app.onShutdown(async () => {
        Rivers.log.info("shutting down");
    });
}
```

---

## 12. EventBus

### 12.1 Architecture

A single dedicated Rust thread (V1) that manages all persistent connections and polling. V2 will support multiple named EventBus pools following the ProcessPool pattern.

### 12.2 Responsibilities

| Concern | EventBus Role |
|---|---|
| Broker connections | Holds Kafka, Redis pub/sub, RabbitMQ, NATS consumers |
| Poll loops | Timer-driven DataView execution for SSE views |
| WebSocket connections | Holds upgraded connections, routes inbound messages |
| SSE connections | Holds long-lived HTTP responses, pushes events |
| Client broadcast | Fan-out to connected SSE/WebSocket clients |

### 12.3 Dispatch Model

Everything persistent lives on the EventBus thread. Everything short-lived dispatches to the ProcessPool. The boundary is absolute:

| EventBus (Rust thread) | ProcessPool (V8/WASM) |
|---|---|
| Holds connections | Executes handlers |
| Manages timers | Short-lived invocations |
| Runs diffs | Business logic |
| Routes messages | Returns results |
| Broadcasts to clients | Never touches connections |

### 12.4 Registry

Maps sources to handlers. Built at deploy time from view declarations. When an event arrives (broker message, timer tick, WebSocket message), the registry determines which handler to dispatch.

### 12.5 Poll Loop

The poll loop is EventBus-owned. Timer ticks on the Rust thread, DataView executes, diff runs in-memory, changed results dispatch to the ProcessPool for `on_change` handling, then broadcast to connected clients.

Previous state is held in-memory on the EventBus thread. No StorageEngine involvement. Restart means fresh state, which is correct since all clients reconnect anyway.

### 12.6 `instance_ack`

Declared on MessageConsumer views:

```toml
[api.views.process_orders]
view_type    = "MessageConsumer"
instance_ack = true
```

- `instance_ack = false` (default) — EventBus acks broker immediately on receipt. Fast, lossy.
- `instance_ack = true` — EventBus holds ack until handler reports success. Handler failure = nack/no-ack. Broker redelivers.

### 12.7 Developer Invisibility

The developer never sees the EventBus. They declare views, write handlers. The framework connects them. No `Rivers.eventbus.publish()`, no topic registry API, no special event syntax.

Writing to a broker (Kafka produce, Redis publish) is a POST through a DataView with a broker datasource. Same `ctx.dataview()` verb as everything else.

---

## 13. Streaming REST

### 13.1 Handler Receives `ctx`

Streaming handlers get the same `ctx` as standard handlers. Pre-fetched DataViews are available before the generator starts.

```typescript
export async function* generate(ctx): AsyncGenerator<any> {
    const limits = ctx.data.user_rate_limits;  // pre-fetched

    if (limits.remaining <= 0) {
        throw new Error("rate limit exceeded");  // before first yield = 500
    }

    for await (const chunk of ctx.streamDataview("completion", { prompt: ctx.request.body.prompt })) {
        yield { token: chunk.data.delta.text };
    }

    await ctx.dataview("record_usage", { tokens: totalTokens });
    yield { done: true, total_tokens: totalTokens };
}
```

### 13.2 Two Verbs

```typescript
ctx.dataview("name", params)              // non-streaming — returns complete result
ctx.streamDataview("name", params)        // streaming — returns AsyncGenerator
```

### 13.3 Streaming DataViews Must Be Declared

Streaming only works through declared DataViews. Pseudo DataViews cannot stream — they have no name, no EventBus registration. Same limitation as caching.

```toml
[data.dataviews.generate_completion]
datasource  = "anthropic_api"
post_query  = "/v1/messages"
post_schema = "schemas/llm_request.schema.json"
get_schema  = "schemas/llm_stream_chunk.schema.json"
streaming   = true
```

### 13.4 Wire Formats

- **NDJSON** (`application/x-ndjson`) — newline-delimited JSON, de facto standard for AI APIs
- **SSE** (`text/event-stream`) — standard SSE format over any HTTP method

Declared per view: `streaming_format = "ndjson"` (default) or `"sse"`.

### 13.5 Error Handling

- Before first yield: standard HTTP 500, no streaming begins
- After first yield: poison chunk emitted as final entry, connection closed
- Poison chunk contains `stream_terminated: true` (NDJSON) or `event: error` (SSE)

### 13.6 Timeout

`stream_timeout_ms` covers full generator lifetime. Independent of `task_timeout_ms`. Defaults to 120000 (2 minutes).

---

## 14. WebSocket Views

### 14.1 Two Modes, Clean Boundaries

**Broadcast mode** — all connections on the route share one channel:

```typescript
ctx.ws.send(data)       // send to this connection
ctx.ws.broadcast(data)  // send to all connections on this view
ctx.ws.close()          // close this connection
```

**Direct mode** — messages routed to specific connection IDs:

```typescript
ctx.ws.send(data)                    // send to this connection
ctx.ws.sendTo(connectionId, data)    // send to specific connection
ctx.ws.close()                       // close this connection
```

No `broadcast()` in Direct. No `sendTo()` in Broadcast. Wrong verb = `CapabilityError`.

### 14.2 Three Hooks

All optional. All short-lived ProcessPool invocations.

```toml
[api.views.chat.ws]
on_connect    = { module = "handlers/chat.ts", entrypoint = "onConnect" }
on_message    = { module = "handlers/chat.ts", entrypoint = "onMessage" }
on_disconnect = { module = "handlers/chat.ts", entrypoint = "onDisconnect" }
```

### 14.3 Connection Rejection

`on_connect` can reject the upgrade:

```typescript
export async function onConnect(ctx): Promise<void> {
    const membership = ctx.data.check_membership;

    if (!membership) {
        ctx.ws.reject(403, "not a member");
        return;
    }

    ctx.ws.send({ type: "init", history: ctx.data.room_history });
}
```

No explicit `accept()` — if the handler returns without calling `reject()`, the connection is accepted.

### 14.4 WebSocketContext

```typescript
interface WebSocketContext {
    message:        any;          // inbound (on_message only)
    connection_id:  string;
    close(code?: number, reason?: string): void;
    send(data: any): void;
    reject?(code: number, reason?: string): void;  // on_connect only
    broadcast?(data: any): void;                    // Broadcast mode only
    sendTo?(connectionId: string, data: any): void; // Direct mode only
}
```

### 14.5 Architecture

The EventBus thread holds the actual WebSocket connections. When a message arrives, it dispatches to the ProcessPool. The handler fires, sends/broadcasts through `ctx.ws`, returns. No long-running JS.

---

## 15. StorageEngine

### 15.1 Invisible Infrastructure

The StorageEngine is a read/write-through layer in the data pipeline. Everything above it — DataViews, Datasources, Drivers — doesn't know it exists.

```
Read:   DataView → StorageEngine (cache check) → Datasource → Driver → Data
Write:  DataView → StorageEngine (invalidate)  → Datasource → Driver → Data
```

### 15.2 Pipeline Behavior

On read: checks L1 cache before the request reaches the Datasource. Cache hit = short circuit, Driver never fires. Cache miss = pass through, cache result on return.

On write: passes the write through unchanged, invalidates related cache entries on success. Never modifies, batches, or reorders data.

### 15.3 Pure KV

No queue operations. Write batching belongs in drivers (InfluxDB, Kafka batch produces). The StorageEngine maintains identical read/write characteristics from datasource to view.

```rust
#[async_trait]
pub trait StorageEngine: Send + Sync {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, StorageError>;
    async fn set(&self, namespace: &str, key: &str, value: Bytes, ttl_ms: Option<u64>) -> Result<(), StorageError>;
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError>;
    async fn list_keys(&self, namespace: &str, prefix: Option<&str>) -> Result<Vec<String>, StorageError>;
    async fn flush_expired(&self) -> Result<u64, StorageError>;
}
```

### 15.4 Reserved Namespaces

| Prefix | Owner | Handler access |
|---|---|---|
| `session:` | Session lifecycle | None |
| `csrf:` | CSRF tokens | None |
| `cache:` | DataView L1 cache | None |
| `raft:` | Raft consensus | None |
| `rivers:` | Internal use | None |
| `app:{app_id}` | Application KV via `ctx.store` | Read/write |

### 15.5 Application KV

Handlers access StorageEngine through `ctx.store`. The host automatically prefixes keys with `app:{app_id}` and rejects reserved namespace prefixes.

```typescript
ctx.store.set("user:prefs:123", data)
// Host resolves → StorageEngine.set("app:{app_id}", "user:prefs:123", data)
```

App name to `app_id` resolution is available via API.

### 15.6 Double Duty — Session Management

StorageEngine manages sessions. V1 is single node — all apps share the same in-process StorageEngine. No Redis required. Session carry-over between app-main and app-services is automatic.

### 15.7 Backends

| Deployment | Backend | Durability |
|---|---|---|
| Development | `memory` | None — lost on restart |
| Single-node production | `sqlite` | WAL mode, durable |
| Multi-node (V2) | `redis` | Shared across nodes |

---

## 16. Caching

### 16.1 StorageEngine Owns Policy

Caching configuration lives on the StorageEngine, not on DataViews. Datasource-level defaults with DataView-level overrides.

```toml
[storage_engine.cache.datasources.orders_db]
enabled     = true
ttl_seconds = 120

[storage_engine.cache.dataviews.get_stock_price]
ttl_seconds = 5
```

### 16.2 L1 Only in V1

L1 is in-process LRU, single node. L2 (shared tier via Redis) deferred to V2 when multi-node requires shared cache.

### 16.3 Write Invalidation

POST/PUT/DELETE through a DataView automatically triggers cache invalidation:

**DataView-level** — write to DataView X invalidates GET cache for DataView X.

**Datasource-level** — configurable broader invalidation strategy:

```toml
[storage_engine.cache.datasources.orders_db]
invalidation_strategy = "dataview"    # or "datasource"
```

### 16.4 Pseudo DataViews Excluded

No name, no tracking, no cache, no invalidation. The system tells you the right thing to do — promote to TOML if you need caching.

---

## 17. Rate Limiting

### 17.1 TOML Only

Rate limiting is purely declarative. No init handler involvement.

### 17.2 Two Levels

**App default** in `app.toml`:

```toml
[app.rate_limit]
per_minute = 120
burst_size = 60
strategy   = "ip"          # "ip" | "header" | "session"
```

**Per-view override** in view declaration:

```toml
[api.views.search]
rate_limit_per_minute = 60
rate_limit_burst_size = 20
```

### 17.3 Three Strategies

- **`ip`** (default) — rate limit by remote IP
- **`header`** — rate limit by custom header value (API key)
- **`session`** — rate limit by `session.identity.username`. Falls back to `ip` on `auth = "none"` views

### 17.4 In-Memory State

Token bucket state lives in memory. Resets on restart. No StorageEngine overhead.

### 17.5 Per-App, Not Per-Server

Each app owns its own rate limit policy. Different apps in the same bundle can have different defaults.

---

## 18. Logging

### 18.1 Architecture

Direct emission via `tracing` crate. No EventBus involvement. The logging system exists before any app loads and records everything from process start.

### 18.2 Configuration

```toml
[base.logging]
level  = "info"    # debug | info | warn | error
format = "json"    # json | text
```

Two fields. Stdout only. Operators handle routing via journald, Docker log driver, etc.

### 18.3 Runtime Level Control

```bash
riversctl log levels                        # view current levels
riversctl log set RequestCompleted debug     # change specific event
riversctl log set datasource warn           # change category
riversctl log reset                         # reset to TOML defaults
```

Changes are in-memory. Restart resets to defaults.

### 18.4 `Rivers.log`

Available everywhere — init handlers, standard handlers, guard handlers, streaming handlers.

```typescript
Rivers.log.info("processing order", { order_id: "123", amount: 100.00 });
Rivers.log.warn("slow upstream", { latency_ms: 1200 });
Rivers.log.error("validation failed", { field: "email" });
```

Host attaches `trace_id`, `app_id`, `node_id`, `handler`, `module` automatically.

### 18.5 Structured JSON Format

```json
{
  "timestamp": "2026-03-16T14:23:01.847Z",
  "level": "info",
  "message": "request completed",
  "trace_id": "a1b2c3d4-e5f6-...",
  "app_id": "f47ac10b-...",
  "node_id": "node-1",
  "event_type": "RequestCompleted",
  "method": "GET",
  "path": "/api/orders/42",
  "status": 200,
  "latency_ms": 14
}
```

### 18.6 Trace Correlation

- `trace_id` on all request-scoped logs
- `init:{app_id}` on startup logs (no HTTP request context)
- W3C `traceparent` header extraction, `x-trace-id` fallback, generated UUID as last resort

### 18.7 Redaction

Sensitive keyword detection on error messages. LockBox values never logged. Opaque tokens never exposed.

---

## 19. Application Architecture

### 19.1 TOML Everywhere

All configuration files use TOML: `manifest.toml`, `resources.toml`, `app.toml`.

### 19.2 Bundle Structure

```
bundle/
├── manifest.toml
├── app-main/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   ├── order_get.schema.json
│   │   └── order_post.schema.json
│   └── libraries/
│       ├── handlers/
│       │   ├── init.ts
│       │   └── auth.ts
│       └── spa/
│           ├── index.html
│           └── main.js
├── orders-service/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   └── libraries/
│       └── handlers/
└── inventory-service/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    ├── schemas/
    └── libraries/
        └── handlers/
```

### 19.3 File Roles

**`manifest.toml`** — identity. App name, type, version, stable `appId`, entry point.

**`resources.toml`** — bill of materials for ops. What datasources and services the app needs. Ops reads this to know what to provision.

**`app.toml`** — how the app works. Datasource config, DataViews, views, handlers. Developer's domain.

**`schemas/`** — top-level, peer to `libraries/`. Config artifacts validated at deploy time.

### 19.4 Deployment Lifecycle

```
 1. Deploy bundle.zip → PENDING state
 2. Gate 2 validation → VALIDATING state
    a. Layer 1: Structural TOML — parse all manifest.toml, resources.toml, app.toml with deny_unknown_fields
    b. Layer 2: Resource existence — all referenced files exist (modules, schemas, SPA assets, WASM)
    c. Layer 3: Logical cross-references — datasource refs, DataView refs, service appIds, uniqueness, consistency
    d. Layer 4: Syntax verification — V8 compile check (TS/JS), Wasmtime validation (WASM), schema JSON structure, entrypoint export verification
    e. Live check: LockBox alias resolution for all lockbox:// URIs
    f. Live check: registered driver matching
    g. Live check: SchemaSyntaxChecker validates schema files against live driver
    h. Live check: x-type matches registered driver
    i. Any failure → FAILED state, structured error logged, deployment aborted
 3. Per app: resolve resources (LockBox, datasource connections) → RESOLVING state
 4. Per app: run init handler (CORS, health, seeding, lifecycle hooks)
 5. Start app-services (parallel, respecting dependency graph) → STARTING state
 6. Health check app-services
 7. Start app-main
 8. Health check app-main
 9. Bundle RUNNING
```

Validation logic is implemented in `rivers_runtime` and shared with `riverpackage validate` (Gate 1). See `rivers-bundle-validation-spec.md` for the full error catalog and layer definitions.

### 19.5 Service Resolution

App-main references app-services through HTTP datasources:

```toml
[data.datasources.orders-api]
driver  = "http"
service = "orders-service"
```

Rivers resolves the service name to the running endpoint. DataViews against this datasource are regular REST calls.

### 19.6 Init Handler Per App

App-main and each app-service get their own init handler independently. Fires after resource resolution, before traffic.

---

## Appendix A — Full `ctx` Reference

```typescript
interface Context {
    trace_id:       string;
    node_id:        string;
    app_id:         string;
    env:            string;
    session?:       Session;
    request:        Request;
    data:           Record<string, any>;
    resdata:        any;
    dataview(name: string, params?: Record<string, any>): Promise<any>;
    streamDataview(name: string, params?: Record<string, any>): AsyncGenerator<StreamChunk>;
    datasource(name: string): DatasourceBuilder;
    store:          Store;
    ws?:            WebSocketContext;
}

interface Session {
    id:        string;
    identity:  { username: string; groups: string[]; };
    apikey?:   string;
    tenant?:   string;
    cdate:     string;
}

interface Request {
    method:  string;
    path:    string;
    headers: Record<string, string>;
    query:   Record<string, string>;
    body:    any;
    params:  Record<string, string>;
}

interface Store {
    get(key: string): Promise<any>;
    set(key: string, value: any, ttl_seconds?: number): Promise<void>;
    del(key: string): Promise<void>;
}

interface DatasourceBuilder {
    fromQuery(sql: string, params?: any[]): DatasourceBuilder;
    fromSchema(schema: object | string, params?: Record<string, any>): DatasourceBuilder;
    withGetSchema(schema: object | string): DatasourceBuilder;
    withPostSchema(schema: object | string): DatasourceBuilder;
    withPutSchema(schema: object | string): DatasourceBuilder;
    withDeleteSchema(schema: object | string): DatasourceBuilder;
    build(): DataView;
}

interface WebSocketContext {
    message:        any;
    connection_id:  string;
    close(code?: number, reason?: string): void;
    send(data: any): void;
    reject?(code: number, reason?: string): void;
    broadcast?(data: any): void;
    sendTo?(connectionId: string, data: any): void;
}

interface Application {
    app_id:     string;
    app_name:   string;
    config:     Record<string, any>;
    cors(policy: CorsPolicy): void;
    health(check: (ctx) => Promise<string>): void;
    dataview(name: string, params?: Record<string, any>): Promise<any>;
    onShutdown(handler: () => Promise<void>): void;
}

interface CorsPolicy {
    origins:      string[];
    methods:      string[];
    headers:      string[];
    credentials:  boolean;
    max_age?:     number;
}

// Global utilities
Rivers.log:    { info, warn, error }
Rivers.crypto: { hashPassword, verifyPassword, hmac, timingSafeEqual, randomHex, randomBase64url }
Rivers.http?:  { get, post, put, del }    // conditional
Rivers.env?:   Record<string, string>      // conditional
```

---

## Appendix B — Declarative vs Handler Coverage

| Operation | Declarative (TOML only) | Handler needed |
|---|---|---|
| GET single resource | ✓ DataView + get_schema | |
| GET list with pagination | ✓ DataView + parameters | |
| POST create | ✓ DataView + post_schema | |
| PUT update | ✓ DataView + put_schema | |
| DELETE | ✓ DataView + delete_schema | |
| Cross-datasource join | | ✓ Handler reads ctx.data, builds ctx.resdata |
| Conditional write | | ✓ Handler applies business rules |
| Data transformation | | ✓ Handler reshapes ctx.resdata |
| Auth credential validation | | ✓ Guard handler |
| SSE with polling (hash diff) | ✓ DataView + polling config | |
| SSE with custom diff | | ✓ change_detect CodeComponent |
| WebSocket message handling | | ✓ on_message handler |
| Streaming REST (LLM proxy) | | ✓ Streaming generator handler |
| Broker produce (Kafka write) | ✓ DataView POST to broker datasource | |
| Broker consume | | ✓ MessageConsumer handler |

**Estimated declarative coverage for typical REST APIs: ~80-90%.** The JS environment handles the remaining 10-20% that config cannot express.

---

## Revision History

| Version | Date | Changes |
|---|---|---|
| 1.0 | 2026-03-16 | Initial specification — all sections locked from technology path discussion |
