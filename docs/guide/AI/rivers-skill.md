---
name: rivers-dev
description: Build Rivers application bundles — declarative REST APIs, WebSocket, SSE, streaming, MCP tools, and GraphQL endpoints using TOML configuration, JSON schemas, and TypeScript/JavaScript/WASM CodeComponent handlers. Use when building Rivers apps, writing handlers, configuring datasources/DataViews/views, or administering riversd. Covers the full Rivers v1 stack.
---

# Rivers Application Development Skill

Build and administer Rivers applications. Rivers is a declarative app-service framework where applications are defined entirely through TOML configuration, JSON schemas, and optional TypeScript/JavaScript/WASM handlers — no Rust code required.

## When to Use

- User asks to build a Rivers app, bundle, or endpoint
- User asks to write a TypeScript, JavaScript, or WASM handler for Rivers
- User asks about Rivers DataViews, views, schemas, or config
- User asks to configure riversd, riversctl, or rivers-lockbox
- User mentions TOML config for REST API, WebSocket, SSE, MCP, or GraphQL

---

## Cookbook — Primary Pattern Reference

**All Rivers patterns, templates, and anti-patterns are defined in the Rivers Cookbook.** Before writing any TOML, handler code, or configuration, consult the cookbook for the correct recipe.

### Cookbook Files

| File | Model | Use |
|------|-------|-----|
| `rivers-cookbook-sonnet.md` | Sonnet | Checklist → Template → Example → Constraints → Escalation |
| `rivers-cookbook-opus.md` | Opus | Decision logic → Composition → Template → Failure modes → Escalation |

### Recipe Selection

1. Identify what you're building (read endpoint, write endpoint, transaction, MCP tool, etc.)
2. Find the matching `RECIPE:` entry in the cookbook
3. Check the match criteria (Sonnet) or decision logic (Opus)
4. Copy the template, fill in the specifics
5. Check the constraints and anti-patterns

### Key Recipes by Task

| Task | Recipe |
|------|--------|
| Start a new app | `RECIPE:NEW-BUNDLE` |
| Simple GET endpoint | `RECIPE:SINGLE-READ` |
| Simple POST/PUT/DELETE | `RECIPE:SINGLE-WRITE` |
| Full CRUD resource | `RECIPE:REST-CRUD` |
| Multiple queries in one response | `RECIPE:MULTI-QUERY-READ` |
| Atomic multi-write | `RECIPE:ATOMIC-MULTI-WRITE` |
| MCP tool (read) | `RECIPE:MCP-READ-TOOL` |
| MCP tool (multi-step write) | `RECIPE:MCP-MULTI-STEP` |
| Connect a datasource | `RECIPE:DATASOURCE-SQL`, `RECIPE:DATASOURCE-REDIS`, etc. |
| Schema file | `RECIPE:SCHEMA-FILE` |
| Validate a bundle | `RECIPE:BUNDLE-VALIDATION` |

### Critical Anti-Patterns

| Anti-Pattern | Rule |
|---|---|
| `ANTI:MULTI-STATEMENT-SQL` | ONE statement per query field. Semicolons → validation error. |
| `ANTI:RAW-SQL-IN-HANDLER` | Handlers call DataViews by name, not raw SQL. |
| `ANTI:SPLIT-TOOL-SEQUENCING` | One logical operation = one tool, one handler, one transaction. |
| `ANTI:HANDLER-FOR-SIMPLE-CRUD` | Don't write a handler that just passes through to a DataView. |
| `ANTI:RETURNING-CLAUSE` | Don't use `RETURNING *` — use a follow-up read DataView. |

---

## Architectural Rules

1. **ONE statement per query field.** Semicolons in a query string → validation error at Gate 1.
2. **Handlers call DataViews by name** via `Rivers.view.query(name, params)`. SQL lives in TOML.
3. **`query`** is the default query field (handler dispatch). Method-specific variants (`get_query`, `post_query`, `put_query`, `delete_query`) fire on REST dispatch by HTTP method.
4. **Multi-query orchestration belongs in handlers**, not in DataView TOML.
5. **Transactions are sync**: `Rivers.db.tx.begin()` / `tx.query()` / `tx.commit()`. No promises.
6. **`transaction = true`** on a DataView wraps its single query in BEGIN/COMMIT. Independent of handler transactions.
7. **`view_type = "Mcp"`** — CamelCase, NOT all-caps `"MCP"`.
8. **`type = "dataview"`** — lowercase one word, NOT `"data_view"`.
9. **`destructive = false`** must be explicit on non-destructive MCP tools — default is `true`.
10. **`method = "POST"`** required for write MCP tools — without it, engine calls `get_query`.

---

## Handler API Reference

### TypeScript/JavaScript Handler Signature

```typescript
interface ViewContext {
    request: ParsedRequest;
    sources: Record<string, any>;
    meta: Record<string, any>;
    session: SessionContext;
    trace_id: string;
    app_id: string;
    node_id: string;
    env: string;
}

interface ParsedRequest {
    method: string;
    path: string;
    headers: Record<string, string>;
    query_params: Record<string, string>;
    path_params: Record<string, string>;
    body: any | null;
}

interface SessionContext {
    identity: { username: string; groups: string[] };
    apikey?: { key_id: string; scopes: string[] };
}

// Primary handler
export async function handler(ctx: ViewContext): Promise<Rivers.Response> {
    return { status: 200, body: { data: "result" } };
}
```

### Rivers.view — DataView Dispatch (Preferred)

```typescript
// Call a declared DataView by name — goes through DataViewEngine
const result = await Rivers.view.query("get_order", { order_id: "abc-123" });
// result = { rows: [...], affected_rows: 0, last_insert_id: null }
```

### Rivers.db.tx — Sync Transaction API

```typescript
// Transactions are SYNC — no await, no promises
const tx = Rivers.db.tx.begin("datasource_name");

try {
    tx.query("archive_wip", { goal_id });          // sync, returns void
    tx.query("clear_wip", { goal_id, project_id });
    tx.query("mark_goal_complete", { goal_id, project_id });

    // Peek at intermediate results (not final until commit)
    const pending = tx.peek("mark_goal_complete");  // returns Array<QueryResult>
    if (pending[0].affected_rows === 0) {
        tx.rollback();
        return { status: 404, body: { error: "goal not found" } };
    }

    tx.query("get_goal", { goal_id });

    // Commit — returns HashMap<string, Array<QueryResult>>
    const results = tx.commit();

    // Every value is an array — even single calls use [0]
    return { status: 200, body: results["get_goal"][0].rows[0] };

} catch (e) {
    // Auto-rollback on throw, logged at WARN
    return { status: 500, body: { error: e.message } };
}
```

**Transaction rules:**
- `tx.begin(datasource)` — sync, checks out connection, sends BEGIN
- `tx.query(dataview_name, params)` — sync, executes on txn connection, returns void
- `tx.peek(name)` — returns `Array<QueryResult>`, NOT final until commit
- `tx.commit()` — sync, sends COMMIT, returns all results as `HashMap<string, Array<QueryResult>>`
- `tx.rollback()` — sync, sends ROLLBACK, releases connection
- Same DataView called N times: `results["name"][0]`, `results["name"][1]`, etc.
- Auto-rollback if handler exits without commit/rollback — logged at WARN
- All DataViews in a transaction MUST use the same datasource

### Rivers.db.query — Raw SQL (Escape Hatch)

```typescript
// Available but NOT preferred — use Rivers.view.query() instead
const result = await Rivers.db.query("datasource_name", "SELECT * FROM users WHERE id = $1", [userId]);
```

### Rivers.log — Structured Logging

```typescript
Rivers.log.info("user login", { userId: 123 });
Rivers.log.warn("rate limit approaching");
Rivers.log.error("payment failed", { reason: "declined" });
// trace_id auto-included in all log output
```

### Rivers.crypto — Cryptography

```typescript
const hash = Rivers.crypto.hashPassword("secret");
const valid = Rivers.crypto.verifyPassword("secret", hash);
const hex = Rivers.crypto.randomHex(16);
const token = Rivers.crypto.randomBase64url(32);
const sig = Rivers.crypto.hmac("key", "data");
const eq = Rivers.crypto.timingSafeEqual("a", "b");
```

### Rivers.http — Outbound HTTP (Escape Hatch)

```typescript
// Only available when allow_outbound_http = true on the handler
const response = await Rivers.http.get("https://external.com/api/data");
const response = await Rivers.http.post("https://external.com/webhook", body, {
    headers: { "Content-Type": "application/json" },
});
```

### ctx.store — Application KV Store

```typescript
ctx.store.set("cache:key", { data: 42 }, 60000);  // TTL in ms
const cached = ctx.store.get("cache:key");
ctx.store.del("cache:key");
// Reserved prefixes blocked: session:, csrf:, cache:, raft:, rivers:
```

### Pseudo DataView Builder

```typescript
// One-off query without TOML declaration — prototype only
const view = ctx.datasource("db")
    .fromQuery("SELECT department, SUM(amount) FROM expenses GROUP BY department")
    .withGetSchema({ driver: "postgresql", type: "object", fields: [...] })
    .build();

const result = await view({ year: 2024 });
// No caching, no invalidation — promote to TOML when stable
```

---

## Pipeline Stages

```
on_session_valid  → pre_process → on_request → Primary → transform → on_response → post_process
```

| Stage | Type | Return |
|-------|------|--------|
| `on_session_valid` | Hook | void — loads permissions into `ctx.meta` |
| `pre_process` | Observer | void — fire-and-forget, errors logged |
| `on_request` | Accumulator | `{ key, data }` or `null` → deposited into `ctx.sources` |
| Primary | Handler/DataView | result → `ctx.sources["primary"]` |
| `transform` | Chained | shapes data |
| `on_response` | Accumulator | `{ key, data }` or `null` → merges/collapses sources |
| `post_process` | Observer | void — fire-and-forget |

---

## Guard Handler (Authentication)

```typescript
export function authenticate(ctx: ViewContext): any {
    const user = ctx.dataview("get_user_by_username", {
        username: ctx.request.body.username,
    });
    if (!user || !Rivers.crypto.verifyPassword(ctx.request.body.password, user.password_hash)) {
        throw new Error("invalid credentials");
    }
    // Return claims — framework creates session
    return { subject: user.id, username: user.username, groups: user.groups };
}
```

---

## Streaming Handler ({chunk, done} Protocol)

```typescript
export function generate(ctx: ViewContext): any {
    const iteration = __args.iteration || 0;
    if (iteration >= 100) return { done: true };
    return {
        chunk: { index: iteration, data: "row-" + iteration },
        done: false,
    };
}
```

State passed between iterations via `__args.state` and `__args.iteration`.

---

## WebSocket Lifecycle Hooks

```typescript
export function onConnect(ctx: ViewContext): any {
    Rivers.log.info("client connected", { connection_id: ctx.ws.connection_id });
    return { welcome: "Connected to chat" };
}

export function onMessage(ctx: ViewContext): any {
    const msg = ctx.ws.message;
    Rivers.log.info("message received", { text: msg.text });
    return { echo: msg.text };
}

export function onDisconnect(ctx: ViewContext): void {
    Rivers.log.info("client disconnected", { connection_id: ctx.ws.connection_id });
}
```

---

## WASM Handler API

WASM handlers run in Wasmtime. Write in any language with WASM target (Rust, C, Go/TinyGo, AssemblyScript, Zig).

```toml
[api.views.compute.handler]
type       = "codecomponent"
language   = "wasm"
module     = "libraries/compute.wasm"
entrypoint = "handler"

[runtime.process_pools.wasm]
engine          = "wasmtime"
workers         = 2
task_timeout_ms = 5000
```

| Config | Default | Description |
|--------|---------|-------------|
| `fuel_limit` | 1,000,000 | CPU instruction budget |
| `memory_pages` | 256 | WASM memory (256 × 64KB = 16MB) |
| `instance_pool_size` | 4 | Pre-compiled instance pool |

---

## ProcessPool Configuration

```toml
[runtime.process_pools.default]
engine                 = "v8"
workers                = 4
task_timeout_ms        = 5000
max_heap_mb            = 128
max_queue_depth        = 0            # 0 = workers × 4
recycle_after_tasks    = 0            # 0 = never recycle
heap_recycle_threshold = 0.8          # recycle isolate if heap > 80%
```

- V8 isolates are pooled and reused
- Per-request isolation via context unbinding
- Watchdog thread terminates timed-out tasks

---

## Supported Languages

| Language | Aliases | Runtime |
|----------|---------|---------|
| JavaScript | `javascript`, `js`, `js_v8` | V8 |
| TypeScript | `typescript`, `ts`, `ts_v8`, `typescript_strict` | V8 |
| WASM | `wasm` | Wasmtime |

---

## Parameter Types

`string`, `integer`, `float`, `boolean`, `array`, `uuid`, `email`, `phone`, `datetime`, `date`, `url`, `json`

---

## CLI Tools

```bash
# Server
riversd --config riversd.toml            # Start server
riversd --version                        # Print version
riversd --no-ssl --port 8080             # Plain HTTP (dev only)
riversd --log-level debug                # Override log level

# Control
riversctl start --config riversd.toml    # Start via helper
riversctl stop                           # Graceful shutdown
riversctl status                         # Check if running
riversctl doctor                         # Health check diagnostics
riversctl doctor --fix                   # Auto-fix common issues
riversctl admin status                   # Query admin API
riversctl admin deploy my-bundle/        # Deploy via admin API

# Validation & Packaging
riverpackage init my-bundle/             # Scaffold new bundle
riverpackage validate my-bundle/         # Validate bundle (4-layer pipeline)
riverpackage validate --format json      # JSON output for CI/CD
riverpackage preflight my-bundle/        # Pre-deployment checks
riverpackage pack my-bundle/             # Package for deployment

# Secrets
rivers-lockbox init                      # Create keystore
rivers-lockbox add db-password --value secret
rivers-lockbox list                      # List entries (no values)
rivers-lockbox show db-password          # Decrypt and display
rivers-lockbox alias db-password alt-name
rivers-lockbox rotate db-password        # Rotate entry
rivers-lockbox remove db-password        # Remove entry
rivers-lockbox validate                  # Validate integrity

# App Keystore
rivers-keystore init <path>
rivers-keystore generate <path> <name>
rivers-keystore list <path>
rivers-keystore rotate <path> <name>
rivers-keystore delete <path> <name>
```

---

## Validation Rules (Quick Reference)

| Rule | Error |
|------|-------|
| `type = "data_view"` | WRONG — use `type = "dataview"` |
| `view_type = "MCP"` | WRONG — use `view_type = "Mcp"` |
| `invalidates` target not found | `invalidates target 'X' does not exist` |
| Unknown `view_type` | `unknown view_type 'X'` |
| Unknown driver | warning: `unknown driver 'X'` |
| Duplicate datasource names | `duplicate datasource name 'X'` |
| Schema file not found | `schema file 'X' not found` |
| Service references unknown appId | `service references unknown appId 'X'` |
| `appId` missing or not UUID | `appId is required` |
| Duplicate appId in bundle | `duplicate appId` |
| Semicolons in query field | Multi-statement SQL rejected |
| Write MCP tool without `method = "POST"` | Engine calls `get_query` which doesn't exist |
| `destructive` not set on read tool | Defaults to `true` — explicitly set `false` |
