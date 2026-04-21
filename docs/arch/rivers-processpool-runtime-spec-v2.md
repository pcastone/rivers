# Rivers ProcessPool Runtime Specification

**Document Type:** Implementation Specification  
**Version:** 2.0  
**Scope:** V8 Isolate Pool, Wasmtime Instance Pool, Capability Model, Preemption, Context Construction, Pool Configuration  
**Status:** Design / Pre-Implementation  
**Depends On:** Epic 1 (Workspace), Epic 19 (App Bundles), Epic 20 (Plugin Loading), Epic 6 (Driver SDK)  
**Supersedes:** Inline JS sandbox documentation (in-process blocklist model, now removed)  
**Addresses:** SEC-10 (JS sandbox prototype chain escape), SEC-11 (SSRF in outbound HTTP from handlers), SEC-8 (security-sensitive response header injection)

---

## Table of Contents

1. [Problem Statement — Why ProcessPool](#1-problem-statement--why-processpool)
2. [Architecture Overview](#2-architecture-overview)
3. [Capability Model](#3-capability-model)
4. [TaskContext — Capability Injection](#4-taskcontext--capability-injection)
5. [Engine: V8 Isolate](#5-engine-v8-isolate)
6. [Engine: Wasmtime](#6-engine-wasmtime)
7. [Preemption](#7-preemption)
8. [Worker Lifecycle](#8-worker-lifecycle)
9. [Pool Configuration](#9-pool-configuration)
10. [Handler API (Rivers JS/TS Surface)](#10-handler-api-rivers-jsts-surface)
11. [Error Model](#11-error-model)
12. [RPS and ProcessPool Integration](#12-rps-and-processpool-integration)
13. [Security Properties](#13-security-properties)
14. [Design Patterns](#14-design-patterns)
15. [Open Questions](#15-open-questions)

---

## 1. Problem Statement — Why ProcessPool

### 1.1 The Blocklist Failure Mode

The previous JavaScript sandbox deleted dangerous globals from the V8 context using a blocklist:

```
delete globalThis.fetch;
delete globalThis.XMLHttpRequest;
delete globalThis.eval;
// ... etc.
```

This approach is fundamentally broken. JavaScript's prototype chain makes deleted globals recoverable:

```javascript
// Recover deleted fetch via prototype traversal
const iframe = document.createElement('iframe');
const fetch = iframe.contentWindow.fetch;   // not deleted
```

In Node.js / V8 without a DOM, the equivalent is recovery via `Object` and `Reflect`:

```javascript
const obj = {};
const proto = Object.getPrototypeOf(obj);
// ...traverse chain to recover process, require, etc.
```

`Object` and `Reflect` are not on the blocklist — and cannot be, because they break nearly all real code. The blocklist is security theater.

### 1.2 The V8 JIT Surface

V8 with JIT enabled has a substantially larger attack surface than an interpreter. JIT-compiled code runs at native speed, meaning a JIT exploitation attempt has a larger reward. Running V8 in-process means a successful JIT exploit compromises the `riversd` process directly.

### 1.3 The Correct Model

The correct model is allowlist capability injection, not blocklist denial. The isolate starts with an empty context — not `globalThis` minus a list of things. The handler receives a clean `ObjectTemplate`-based global with only the objects explicitly injected by the host. If `Rivers.http` is not injected, the object does not exist. There is nothing to recover.

The ProcessPool model operationalizes this:
- Workers hold clean contexts, reset between tasks
- `TaskContext` carries the approved capability set
- The host injects capabilities as opaque tokens — never raw connections
- Time-based preemption terminates runaway handlers without handler cooperation

---

## 2. Architecture Overview

### 2.1 Component Diagram

```
                          ┌──────────────────────────────────────────┐
                          │  riversd Process                         │
                          │                                          │
  HTTP Request            │  ┌──────────────┐  TaskContext           │
  ───────────────────────►│  │ on_request   ├──────────────────────┐ │
                          │  │ handler      │                      │ │
                          │  │ (host Rust)  │                      ▼ │
                          │  └──────────────┘   ┌─────────────────────────────┐
                          │                     │   ProcessPool               │
                          │                     │                             │
                          │                     │  ┌────────┐  ┌────────┐    │
                          │                     │  │Worker  │  │Worker  │ …  │
                          │                     │  │(V8)    │  │(WASM)  │    │
                          │                     │  └────────┘  └────────┘    │
                          │                     │                             │
                          │                     │  Task Queue                 │
                          │                     │  Watchdog Thread            │
                          │                     └──────────┬──────────────────┘
                          │                                │ TaskResult
                          │  ┌──────────────┐             │
                          │  │ on_response  │◄────────────┘
                          │  │ handler      │
                          │  │ (host Rust)  │
                          │  └──────┬───────┘
                          │         │  HTTP Response
                          └─────────┼────────────────────────────────────────┘
                                    │
                                    ▼ Client
```

### 2.2 Key Principles

- **The pool is engine-agnostic.** V8 and Wasmtime workers live in the same pool infrastructure. Task dispatch, queuing, preemption, and error handling are uniform across engines.
- **Multiple named pools.** A `riversd` instance can run multiple pools with different worker counts, heap limits, and engine types. Views declare which pool they use.
- **Workers are reused with fresh contexts.** Isolates are pooled and reused across requests. Per-request isolation is achieved by unbinding the application context between executions, not by resetting or recreating the isolate. The security boundary is the context, not the isolate. <!-- SHAPE-9 amendment: context reuse model -->
- **Host side owns resources.** The isolate never holds a database connection, a credential, or a raw file handle. It holds opaque tokens. The host resolves token → resource for each operation.

---

## 3. Capability Model

### 3.1 Static Declaration

The view declaration is the complete and static capability graph for a handler:

```toml
[api.views.order_handler]
path                = "/api/orders"
view_type           = "Rest"
process_pool        = "default"         # which pool to use
libs                = ["lodash.js", "validator.wasm"]
datasources         = ["primary-db", "redis-cache"]
dataviews           = ["order_query", "user_lookup"]
allow_outbound_http = false             # default
allow_env_vars      = false             # default — no process.env access
methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/orders.ts",
    entrypoint_function = "onCreateOrder",
    resources           = ["primary-db", "redis-cache"]
}}
```

| Attribute | Default | Description |
|---|---|---|
| `process_pool` | `"default"` | Named pool to use for this handler |
| `libs` | `[]` | JS/TS/WASM libraries available inside the isolate |
| `datasources` | `[]` | Datasource aliases the handler may query (via opaque token) |
| `dataviews` | `[]` | DataViews the handler may call |
| `allow_outbound_http` | `false` | Inject `Rivers.http` into isolate |
| `allow_env_vars` | `false` | Expose declared env vars via `Rivers.env` |

### 3.2 Dispatch Validation

Before a task is dispatched to the pool, the host validates:

1. All declared `libs` are present in the node's library set (provisioned by RPS)
2. All declared `datasources` are provisioned for this node's roles
3. All declared `dataviews` exist and are bound

If validation fails, the task fails at dispatch with a `CapabilityError` — not mid-execution. This makes capability failures predictable and testable.

### 3.3 No Dynamic Imports

Inside JS/TS handlers, dynamic import is not available:

```typescript
// This will throw CapabilityError — Rivers.import is not injected
const lib = await import('some-lib');

// This works — lib was declared in the view definition
const result = xzzy.process(input);   // xzzy.js is in scope from view declaration
```

WASM modules similarly cannot load secondary modules at runtime. All imports are resolved at dispatch.

---

## 4. TaskContext — Capability Injection

### 4.1 Structure

The `TaskContext` is a Rust struct built by the `on_request` handler before dispatch. It carries:

```rust
pub struct TaskContext {
    /// Opaque tokens, not real connections
    pub datasources: HashMap<String, DatasourceToken>,
    /// DataView handles (resolves queries back to host)
    pub dataviews:   HashMap<String, DataViewToken>,
    /// Libraries to load into the isolate before execution
    pub libs:        Vec<ResolvedLib>,
    /// HTTP client handle — only present if allow_outbound_http = true
    pub http:        Option<HttpToken>,
    /// Env vars — only present if allow_env_vars = true and vars declared
    pub env:         HashMap<String, String>,
    /// Which module + function to call
    pub entrypoint:  Entrypoint,
    /// Serialized function arguments
    pub args:        serde_json::Value,
}
```

### 4.2 Token Model

Tokens are opaque identifiers. The isolate holds a token string. When the handler calls `Rivers.db.query(token, sql, params)`, the host receives the token, resolves it to the actual connection from the host-side connection pool, executes the query, and returns the result to the isolate. The isolate never sees the connection object, the connection string, or the credentials.

**Handlers always pass the datasource name as declared in the view config** — e.g., `"primary-db"`, `"redis-cache"`. The `tok:` prefix (e.g., `tok:mysql-1`) is an internal host-side representation shown here for illustration only. Handler code never constructs or sees `tok:` strings. Passing a name not declared in the view's `datasources` array returns `CapabilityError` immediately.

```
Isolate                              Host
──────────────────────────────────────────────────────
Rivers.db.query("primary-db",        →  resolve name → actual PgPool
    "SELECT * FROM orders WHERE id=$1",      execute query
    [orderId])                       ←  return rows as JSON
```

This model is enforced structurally. There is no mechanism by which the isolate can obtain the raw connection — it is not a matter of policy, it is a matter of what objects exist inside the V8 context.

### 4.3 Four-Scope Variable Injection Model

<!-- SHAPE-10 amendment: four-scope injection, no snapshots -->
Variables are injected into the isolate context at creation time using four scopes. No V8 snapshots are used — all state is injected via globals.

| Scope | Lifetime | Contents | Available in |
|---|---|---|---|
| **Application** (permanent) | `riversd` process lifetime | `Rivers.*` APIs, app config, shared constants | All requests |
| **Session** | Per-user session | Session variables, identity | Requests with active session |
| **Connection** | Per WS/SSE connection | Connection-specific state | WS/SSE handlers only |
| **Request** | Single request | Capability tokens, request data, trace ID, caller context | Current request only |

Narrower scope shadows broader on name collision. REST handlers see: application + session + request. WebSocket/SSE handlers see: all four scopes.

### 4.4 Building a TaskContext

```rust
// In the on_request handler (host Rust):
let ctx = TaskContext::builder()
    .datasource("primary-db", datasource_registry.token_for("primary-db")?)
    .datasource("redis-cache", datasource_registry.token_for("redis-cache")?)
    .dataview("order_query", dataview_registry.token_for("order_query")?)
    .lib("lodash.js", lib_registry.resolve("lodash.js")?)
    .lib("validator.wasm", lib_registry.resolve("validator.wasm")?)
    // allow_outbound_http = false → no http token injected
    .entrypoint("handlers/orders.ts", "onCreateOrder")
    .args(serde_json::to_value(&request_body)?)
    .build()?;

let result = process_pool.dispatch(ctx).await?;
```

---

## 5. Engine: V8 Isolate

### 5.1 Isolate Construction

Each V8 worker holds a single `v8::Isolate`. On worker startup, the isolate is created with an empty context. No V8 snapshots are used — all APIs are injected via globals at context creation time. This avoids snapshot versioning complexity and ensures the API surface is always consistent with the current runtime. <!-- SHAPE-10 amendment: no snapshots, injection model -->

**Context construction per task:**

```rust
let scope = v8::HandleScope::new(&mut isolate);
let context = v8::Context::new(&mut scope, v8::ContextOptions {
    global_template: Some(build_global_template(&mut scope, &task_ctx)),
    ..Default::default()
});
```

`build_global_template` constructs an `ObjectTemplate` containing exactly and only the Rivers API objects derived from the `TaskContext`. No prototype chain inheritance from a full `globalThis`. No `process`. No `require`. No `fetch` unless `allow_outbound_http = true`.

### 5.2 Rivers API Surface (V8)

Inside the isolate, the global object contains:

```typescript
declare const Rivers: {
    // Present if datasource declared in view
    db: {
        query(token: string, sql: string, params?: any[]): Promise<QueryResult>;
        execute(token: string, sql: string, params?: any[]): Promise<ExecuteResult>;
    };
    // Present if dataview declared in view
    view: {
        query(token: string, params?: Record<string, any>): Promise<any>;
    };
    // Present only if allow_outbound_http = true
    http?: {
        get(url: string, opts?: RequestInit): Promise<Response>;
        post(url: string, body: any, opts?: RequestInit): Promise<Response>;
        put(url: string, body: any, opts?: RequestInit): Promise<Response>;
        del(url: string, opts?: RequestInit): Promise<Response>;
    };
    // Present only if allow_env_vars = true and var declared
    env?: Record<string, string>;
    // Always present — for logging (goes to Rivers structured log, not console.log)
    log: {
        info(msg: string, fields?: Record<string, any>): void;
        warn(msg: string, fields?: Record<string, any>): void;
        error(msg: string, fields?: Record<string, any>): void;
    };
};
```

`console.log` is **not** available. All logging goes through `Rivers.log` which is structured, correlated with the request trace ID, and routes to the configured log output. This prevents log injection and ensures all handler output is visible in the standard observability stack.

### 5.3 TypeScript Compilation

> **Superseded by [`rivers-javascript-typescript-spec.md`](./rivers-javascript-typescript-spec.md)** (v1.0, 2026-04-21). See that spec for the authoritative swc compiler integration, module resolution algorithm, entrypoint lookup semantics, source maps, and `ctx.transaction()` API. The paragraph below is preserved for historical context.

TypeScript handlers are compiled at bundle load time (not at request time) using the embedded `swc` compiler. The compiled JS is stored in the bundle cache. At dispatch, the worker loads pre-compiled JS — there is no per-request transpilation overhead.

### 5.4 Isolate Reuse and Context Unbinding

<!-- SHAPE-9,10 amendment: reuse with context unbinding, no snapshots -->
After a task completes, the V8 worker unbinds the current context rather than destroying and recreating the isolate. The worker lifecycle is: pick up task, bind fresh context, execute, unbind context, return to pool.

1. The task context (global object) is unbound — all request-scoped variables are destroyed and zeroized
2. The isolate itself stays warm in the pool for the next task
3. The heap is not cleared (V8 manages heap through GC)
4. If heap usage after unbinding exceeds `max_heap_mb * 0.8`, the isolate is destroyed and recreated (without snapshots)

Streaming handlers (WebSocket, SSE) receive a long-lived context that persists for the duration of the stream. The context is unbound and zeroized when the stream terminates.

This gives fast per-request turnover with occasional full recycling for long-lived workers.

---

## 6. Engine: Wasmtime

### 6.1 Instance Construction

Each WASM worker holds a `wasmtime::Store` and a pool of pre-compiled `wasmtime::Module` instances. Modules are compiled once at bundle load time using Wasmtime's ahead-of-time (AOT) compilation and cached.

**Per-task instance:**

```rust
let mut store = wasmtime::Store::new(&engine, WasiCtx::new());
// Epoch increment configured — see Section 7.2
store.set_epoch_deadline(epoch_ticks_for_timeout);

// Instantiate from pre-compiled module
let instance = linker.instantiate(&mut store, &module)?;
```

WASI capabilities are restricted per TaskContext. WASI stdio is redirected to `Rivers.log`. WASI file access is denied. WASI network access is gated by `allow_outbound_http`.

### 6.2 WASM Host Function Bindings

WASM handlers interact with the host through imported host functions. The linker registers these functions, which are the WASM equivalent of the Rivers JS API:

```rust
// Host functions registered in the WASM linker
linker.func_wrap("rivers", "db_query",
    |mut caller: Caller<'_, WasiCtx>, token_ptr: u32, sql_ptr: u32, params_ptr: u32| -> u32 {
        // Resolve token, execute query, write result to WASM memory
        // Returns pointer to result in WASM linear memory
    }
)?;

linker.func_wrap("rivers", "log_info",
    |mut caller: Caller<'_, WasiCtx>, msg_ptr: u32, fields_ptr: u32| {
        // Structured log output — same as V8 Rivers.log.info
    }
)?;
```

The WASM module declares its imports in the standard WASM binary format:
```wat
(import "rivers" "db_query" (func $db_query (param i32 i32 i32) (result i32)))
(import "rivers" "log_info" (func $log_info (param i32 i32)))
```

### 6.3 Multi-Language WASM

Any language with a WASM compilation target (Rust, C, C++, Go via TinyGo, AssemblyScript, Zig) can produce handlers for the WASM pool. The host function interface is the only contract. Language choice is a developer preference.

---

## 7. Preemption

### 7.1 Problem

Without preemption, a handler can run indefinitely. An infinite loop, a deliberate denial of service, or a pathological algorithmic complexity issue will hold a worker permanently, starving all other tasks.

Cooperative preemption (requiring handlers to yield) is useless — a malicious handler simply doesn't yield. Preemption must be transparent and mandatory.

### 7.2 V8 — TerminateExecution

The watchdog thread tracks active V8 workers. When a worker exceeds its `task_timeout_ms`:

```rust
// Watchdog thread
if worker.start_time.elapsed() > Duration::from_millis(config.task_timeout_ms) {
    worker.isolate.terminate_execution();
    worker.timed_out.store(true, Ordering::SeqCst);
}
```

`v8::Isolate::TerminateExecution()` is the official V8 API for external termination. It causes the isolate to throw a `TerminationException` at the next safe point — typically within microseconds of the call. No changes to handler code are required. The exception cannot be caught by the handler (it is a distinct exception class from regular JavaScript exceptions).

The worker then:
1. Marks the task as `TaskError::TimedOut`
2. Resets the isolate context (or full recycle if the timeout was significant)
3. Becomes available for the next task

### 7.3 Wasmtime — Epoch Interruption

Wasmtime uses an epoch-based interruption model. During AOT compilation, Wasmtime injects epoch check instructions at function entry points and back-edges (loop headers). These checks are invisible to the `.wasm` binary source — they are injected at the native compilation layer.

```rust
// Engine configuration — epoch interrupts enabled at compile time
let mut config = wasmtime::Config::new();
config.epoch_interruption(true);
let engine = wasmtime::Engine::new(&config)?;
```

The watchdog thread increments the engine epoch counter on a fixed interval:

```rust
// Watchdog thread — increments global epoch
loop {
    thread::sleep(Duration::from_millis(epoch_interval_ms));
    engine.increment_epoch();
}
```

Each `Store` has an epoch deadline. When the global epoch surpasses the store's deadline, the next epoch-check instruction in the running WASM code traps, returning `Err(Trap::Interrupt)` to the host.

**Key properties:**
- Standard unmodified `.wasm` binaries get preemption for free — no source changes
- Epoch injection is done by Wasmtime during native compilation
- The overhead of epoch checks is negligible (branch prediction eliminates most cost)
- The `epoch_interval_ms` and deadline determine granularity vs overhead tradeoff

### 7.4 Timeout Configuration

```toml
[runtime.process_pools.default]
task_timeout_ms   = 5000      # wall clock timeout per task

[runtime.process_pools.wasm]
task_timeout_ms   = 10000     # WASM workers often do heavier computation
epoch_interval_ms = 10        # epoch tick frequency (WASM only)
```

The watchdog uses a single thread per pool, not per worker. It scans active workers on a fixed interval and calls `terminate_execution` or increments the epoch as appropriate.

---

## 8. Worker Lifecycle

### 8.1 Pool Startup

On `riversd` start (after bundle load), each ProcessPool initializes its configured number of workers:

```
ProcessPool "default" startup:
  1. Spawn N worker threads (config: workers = 4)
  2. Each worker thread: create empty Isolate, inject Rivers API stubs, enter idle state
  3. Spawn 1 watchdog thread for this pool
  4. Pool enters READY state — begins accepting tasks
```
<!-- SHAPE-10 amendment: no snapshot loading step -->

### 8.2 Task Dispatch

```
Incoming CodeComponent task:
  1. on_request handler builds TaskContext (host Rust)
  2. TaskContext pushed to pool task queue
  3. Idle worker picks up task
  4. Worker builds isolate context from TaskContext
  5. Worker loads declared libs into context
  6. Worker calls entrypoint function with args
  7. Worker awaits result (with watchdog monitoring)
  8. Worker returns TaskResult to awaiting on_response handler
  9. Worker resets context, returns to idle
```

If all workers are busy when a task arrives, the task waits in queue. Queue depth is bounded by `max_queue_depth` (default: `workers * 4`). If the queue is full, the task receives `TaskError::QueueFull` immediately — backpressure to the caller.

### 8.3 Worker Health

Workers that crash (panic, OOM, unrecoverable state) are replaced automatically:

```
Worker crash detected by pool:
  1. Mark worker slot as unavailable
  2. Spawn replacement worker
  3. Emit WorkerCrash event to EventBus (Observe tier)
  4. If crash rate exceeds threshold → emit WorkerPoolDegraded alert
```

A pool that loses all workers emits a `ProcessPoolFailed` critical event and the associated views begin returning 503.

### 8.4 Graceful Shutdown

On `riversd` shutdown signal:
1. Pool stops accepting new tasks
2. In-flight tasks complete (or are terminated if shutdown deadline exceeded)
3. Workers perform final cleanup
4. Watchdog thread exits
5. Pool reports clean shutdown

---

## 9. Pool Configuration

### 9.1 Full Configuration Reference

```toml
# Named pool — multiple pools allowed, each with different characteristics
[runtime.process_pools.<name>]

# Engine selection
engine             = "v8"        # "v8" or "wasmtime"

# Worker count — number of concurrent isolates
workers            = 4           # recommendation: CPU count for compute, higher for I/O-bound

# Memory limits
max_heap_mb        = 128         # V8: heap limit per isolate (MiB)
max_memory_mb      = 64          # Wasmtime: linear memory limit per instance (MiB)

# Preemption
task_timeout_ms    = 5000        # wall-clock timeout per task (ms)
epoch_interval_ms  = 10          # Wasmtime only: epoch tick interval (ms)

# Backpressure
max_queue_depth    = 0           # 0 = workers * 4 (auto)

# Recycling — V8 only
recycle_after_tasks = 0          # 0 = never force-recycle (GC manages heap)
recycle_heap_threshold_pct = 80  # recycle isolate when heap > this % of max after reset
```

### 9.2 Example: Multiple Pools

```toml
# Fast pool for lightweight request handlers
[runtime.process_pools.default]
engine          = "v8"
workers         = 8
max_heap_mb     = 64
task_timeout_ms = 2000

# Heavy pool for expensive computation (ML scoring, PDF generation, etc.)
[runtime.process_pools.heavy]
engine          = "v8"
workers         = 2
max_heap_mb     = 512
task_timeout_ms = 30000

# WASM pool for compute-intensive native-speed tasks
[runtime.process_pools.wasm]
engine             = "wasmtime"
workers            = 4
max_memory_mb      = 128
task_timeout_ms    = 10000
epoch_interval_ms  = 10
```

View declarations choose a pool:

```toml
[api.views.ml_score]
process_pool = "heavy"
...

[api.views.image_process]
process_pool = "wasm"
...

[api.views.api_handler]
# process_pool not specified → uses "default"
...
```

---

## 10. Handler API (Rivers JS/TS Surface)

### 10.1 Handler Function Signature

Every JS/TS CodeComponent handler exports a single async function:

```typescript
// TypeScript handler
export async function onCreateOrder(
    request: Rivers.Request,
    context: Rivers.Context
): Promise<Rivers.Response> {
    // handler body
}
```

The `Rivers` namespace is globally available inside the isolate (injected by the host, not imported). There is no explicit import for `Rivers`.

### 10.2 Request and Response Types

```typescript
namespace Rivers {
    interface Request {
        method:  string;
        path:    string;
        headers: Record<string, string>;
        query:   Record<string, string>;
        body:    any;               // pre-parsed JSON or null
        params:  Record<string, string>;  // path params
    }

    interface Context {
        trace_id:  string;
        node_id:   string;
        app_id:    string;
        env:       string;          // "dev" | "staging" | "prod"
    }

    interface Response {
        status:   number;
        headers?: Record<string, string>;
        body?:    any;              // will be JSON-serialized
    }
}
```

### 10.3 Database Access

```typescript
export async function onGetUser(req: Rivers.Request): Promise<Rivers.Response> {
    const rows = await Rivers.db.query(
        "primary-db",              // datasource token (string alias)
        "SELECT id, name, email FROM users WHERE id = $1",
        [req.params.id]
    );

    if (rows.length === 0) {
        return { status: 404, body: { error: "not found" } };
    }

    return { status: 200, body: rows[0] };
}
```

The datasource name (`"primary-db"`) must be declared in the view's `datasources` array. If the handler passes a name not in the declared set, the host returns a `CapabilityError` — the token does not resolve.

### 10.4 DataView Access

```typescript
export async function onListOrders(req: Rivers.Request): Promise<Rivers.Response> {
    const orders = await Rivers.view.query(
        "order_query",             // dataview token
        {
            user_id:  req.query.user_id,
            status:   req.query.status ?? "active",
            limit:    parseInt(req.query.limit ?? "20"),
        }
    );

    return { status: 200, body: orders };
}
```

DataView queries use the standard DataView parameter model. Cache behavior (L1/L2) is transparent to the handler.

### 10.5 Outbound HTTP (Conditional)

`Rivers.http` is an escape hatch for use cases not expressible through the DataView/driver model — calling a third-party webhook, proxying to an external API, or consuming a service with no Rivers driver. It is not the default HTTP client. Views that use `Rivers.http` should document why a DataView over the HTTP driver does not suffice.

When `allow_outbound_http = true`, Rivers emits `tracing::warn!` at startup identifying the view and module that declared the capability. Each call to `Rivers.http` at runtime is logged at `INFO` with the destination host (not full URL) and the view trace ID.

Only available when `allow_outbound_http = true`:

```typescript
export async function onWebhookRelay(req: Rivers.Request): Promise<Rivers.Response> {
    const result = await Rivers.http.post(
        "https://external-service.example.com/webhook",
        req.body,
        { headers: { "Content-Type": "application/json" } }
    );

    return { status: result.status, body: await result.json() };
}
```

<!-- SHAPE-11 amendment: SSRF prevention via capability model, no IP validation -->
SSRF prevention is handled by the capability model. `Rivers.http` is only injected when `allow_outbound_http = true` is declared, and outbound requests are token-gated to configured datasources. No runtime IP validation or post-DNS RFC 1918 range checking is performed.

### 10.6 Logging

```typescript
export async function onProcessPayment(req: Rivers.Request): Promise<Rivers.Response> {
    Rivers.log.info("processing payment", {
        order_id:  req.body.order_id,
        amount:    req.body.amount,
        // Never log req.body.card_number — structured logging makes accidental
        // sensitive field logging visible in code review
    });

    // ...

    Rivers.log.warn("payment processor slow", { latency_ms: 1200 });

    return { status: 200, body: { ok: true } };
}
```

`Rivers.log` entries are correlated with the request trace ID automatically. They appear in the Rivers structured log output with `handler`, `module`, `view`, and `trace_id` fields populated by the host.

### 10.7 Cryptographic Utilities

`Rivers.crypto` is available in all CodeComponents regardless of pool config — no capability declaration required. The API is intentionally narrow: only operations with safe, hard-to-misuse defaults are exposed.

```typescript
Rivers.crypto = {
    // Password hashing — bcrypt, cost factor 12 (min enforced: 10)
    async hashPassword(password: string): Promise<string>,
    async verifyPassword(password: string, hash: string): Promise<boolean>,

    // Random bytes — URL-safe, cryptographically secure
    randomHex(bytes: number): string,           // hex string, 2*bytes chars
    randomBase64url(bytes: number): string,     // base64url string

    // HMAC-SHA256 — key resolved via LockBox resource token, never enters isolate
    async hmac(secret_alias: string, data: string): Promise<string>,

    // Constant-time comparison — use for all secret/token comparisons
    timingSafeEqual(a: string, b: string): boolean,
}
```

**`hashPassword` / `verifyPassword`:** bcrypt with cost factor 12 by default. Minimum cost factor of 10 is enforced — values below 10 are rejected at call time with `CryptoError`. The cost factor is not configurable per-call to prevent handlers from inadvertently weakening it.

**`hmac`:** The signing key is resolved on the host side via LockBox alias. The raw key never enters the isolate. The handler passes the LockBox alias name (must be declared in the view's `resources`), not the key value.

**`timingSafeEqual`:** Handlers that compare secrets, API keys, or tokens must use this, not `===`. Standard string equality is not constant-time and is vulnerable to timing oracle attacks.

```typescript
export async function onVerifyWebhook(req: Rivers.Request): Promise<Rivers.Response> {
    const expected = await Rivers.crypto.hmac("webhook_signing_key", req.body.payload);
    const provided  = req.headers["x-signature"] ?? "";

    if (!Rivers.crypto.timingSafeEqual(expected, provided)) {
        return { status: 401, body: { error: "invalid signature" } };
    }

    return { status: 200, body: { ok: true } };
}
```

### 10.8 Streaming DataView Consumption

`Rivers.view.stream()` consumes a DataView backed by a streaming HTTP datasource (`streaming_response = true`). Returns an `AsyncIterable` — the handler iterates chunks as they arrive from the upstream.

```typescript
Rivers.view.stream = async function*(
    dataview_name: string,
    params?: Record<string, any>
): AsyncIterable<StreamChunk>
```

The DataView must be declared in the view's `dataviews` array (same rule as `Rivers.view.query()`). Calling `Rivers.view.stream()` with an undeclared DataView returns `CapabilityError`.

**Chunk shape** — for `response_format = "sse"` upstreams:

```typescript
interface StreamChunk {
    event?: string;   // SSE event type (if present)
    data:   any;      // parsed JSON payload
    id?:    string;   // SSE id field (if present)
}
```

For `response_format = "ndjson"` upstreams, each chunk is the parsed JSON object directly.

**Example — LLM proxy:**

```typescript
// handlers/llm.ts
export async function* generate(
    req: Rivers.Request
): AsyncGenerator<any> {
    const { prompt, model } = req.body as { prompt: string; model: string };

    Rivers.log.info("starting generation", { model, trace_id: req.trace_id });

    let total_tokens = 0;

    for await (const chunk of Rivers.view.stream("generate_completion", { prompt })) {
        if (chunk.data?.type === "content_block_delta") {
            yield { token: chunk.data.delta.text };
            total_tokens++;
        }
        if (chunk.data?.type === "message_stop") break;
    }

    yield { done: true, total_tokens };
}
```

If the upstream stream terminates with an error mid-iteration, the `AsyncIterable` throws on the next `for await`. The generator's try/catch handles it normally — Rivers emits a poison chunk automatically on unhandled throw.

---

## 11. Error Model

### 11.1 Error Taxonomy

| Error Type | Description | HTTP Outcome |
|---|---|---|
| `CapabilityError` | Handler requested a resource not declared in view | 500 Internal Server Error |
| `TaskError::TimedOut` | Handler exceeded `task_timeout_ms` | 504 Gateway Timeout |
| `TaskError::QueueFull` | Pool queue at capacity | 503 Service Unavailable |
| `TaskError::WorkerCrash` | Worker panicked or OOM during execution | 500 Internal Server Error |
| `HandlerError` | Handler threw an unhandled JS/WASM exception | 500 Internal Server Error |
| `HandlerReturn::Error` | Handler returned `{ status: 4xx/5xx }` deliberately | As declared |
| `DispatchError::LibMissing` | Declared lib not found in node's library set | 500 at startup / deploy time |

### 11.2 Error Propagation

Errors from the pool are returned to the `on_request` / `on_response` pipeline as `Err(TaskError)`. The handler pipeline maps these to HTTP responses. The application author can override default error handling by implementing `on_error` in their handler:

```typescript
export async function onError(
    error: Rivers.TaskError,
    req: Rivers.Request
): Promise<Rivers.Response> {
    if (error.kind === "HandlerError") {
        return { status: 500, body: { message: "Internal error" } };
    }
    // fall through to default
}
```

`on_error` is not available for `TaskError::TimedOut` — the handler is already terminated.

### 11.3 EventBus Error Events

All pool errors emit events to the EventBus (Observe tier — non-blocking):

```rust
EventType::ProcessPoolError {
    pool_name:  String,
    error_kind: TaskErrorKind,
    view:       String,
    trace_id:   String,
    worker_id:  Option<usize>,
}
```

These events surface in metrics and can trigger alerts. `WorkerCrash` events with high frequency trigger `WorkerPoolDegraded`.

---

## 12. RPS and ProcessPool Integration

### 12.1 RPS Handlers Run in ProcessPool

All RPS CodeComponent handlers — alias registry, secret broker, node registry, poll handler, etc. — execute inside the ProcessPool. The RPS is a Rivers application and follows the same rules as all other applications.

This has a specific security implication for the secret broker: the handler that fetches and encrypts secrets runs inside a sandboxed isolate. The raw secret value **never enters the isolate**. The handler triggers a host-side fetch via a lockbox resource token:

```typescript
// Inside rps-master secret_broker.ts
export async function onSecretRequest(
    req: Rivers.Request,
    ctx: Rivers.Context
): Promise<Rivers.Response> {
    const alias = req.params.alias;
    const nodePublicKey = req.body.node_public_key;

    // River.lockbox is a host-side capability — returns encrypted envelope
    // The raw credential never enters this isolate
    const envelope = await Rivers.lockbox.fetchEncrypted(
        "lockbox_backends",    // resource token
        alias,
        nodePublicKey
    );

    return { status: 200, body: envelope };
}
```

The `Rivers.lockbox.fetchEncrypted` call is resolved entirely on the host side. The isolate passes the alias and the requesting node's public key. The host fetches from the backend, encrypts, and returns the encrypted envelope. The raw credential touches the host-side Rust code only, not the TypeScript handler.

### 12.2 RPS Pool Configuration

The RPS primary runs two pools: a standard V8 pool for most handlers, and a WASM pool for the crypto library used in envelope construction:

```toml
[runtime.process_pools.default]
engine          = "v8"
workers         = 4
max_heap_mb     = 128
task_timeout_ms = 5000

[runtime.process_pools.rps_crypto]
engine             = "wasmtime"
workers            = 2
max_memory_mb      = 32
task_timeout_ms    = 1000       # crypto ops should be fast
epoch_interval_ms  = 5
```

The crypto WASM pool (`rps_crypto.wasm`) handles X25519 key exchange and XChaCha20-Poly1305 encryption. Putting this in WASM ensures the crypto implementation is deterministic and auditable — the same binary runs on every RPS instance.

### 12.3 Lib Distribution via RPS

The ProcessPool requires all declared libs to be present locally before tasks can execute. The RPS provisions libs to nodes as part of the role resource set. The lifecycle is:

```
1. View declaration includes: libs = ["lodash.js", "validator.wasm"]
2. RPS role includes these libs as resources
3. At bootstrap, RPS relay delivers lib binaries to node
4. Node installs libs to local lib cache
5. ProcessPool resolves libs from local cache at dispatch
6. If lib missing at dispatch → DispatchError::LibMissing → 500
```

No lib can appear in a view that was not provisioned by the RPS for that node's role. The dependency is enforced at both the provisioning layer (RPS) and the dispatch layer (ProcessPool).

---

## 13. Security Properties

### 13.1 Property Summary

| Property | Mechanism | Addresses |
|---|---|---|
| No prototype chain escape | Clean `ObjectTemplate` global — no `globalThis` inheritance | SEC-10 |
| No SSRF from handlers | `Rivers.http` not injected unless declared; capability model prevents unauthorized outbound access — no runtime IP validation needed | SEC-11 | <!-- SHAPE-11 amendment -->
| No security header injection | Response headers from handler context are filtered — security-sensitive headers (Set-Cookie, Location, X-Frame-Options, etc.) cannot be set by handler code | SEC-8 |
| No credential leakage into isolate | Opaque tokens; host resolves; raw secrets never cross into V8/WASM context | SEC-10, SEC-11 |
| No runaway handler | V8 `TerminateExecution()` / Wasmtime epoch interruption — preemptive, not cooperative | — |
| No undeclared lib | Dispatch validation fails if any lib missing; no dynamic import inside isolate | — |
| No env var leakage | `Rivers.env` not injected unless `allow_env_vars = true` and vars explicitly declared | — |
| Handler logging is structured | `console.log` not available; `Rivers.log` routes to structured OTel log | — |

### 13.2 Remaining Considerations

- **Side-channel via timing:** A handler that performs timing attacks against `Rivers.db` responses could potentially leak information about other tenants' data. This is a long-term isolation concern and not a v1 issue (single-tenant deployment), but noted here.
- **Memory exhaustion via heap:** A handler that allocates up to `max_heap_mb` and holds it will exhaust the worker heap. The V8 GC will collect, but a deliberate allocation-and-hold pattern can force GC pressure. `recycle_after_tasks` and `recycle_heap_threshold_pct` are the mitigations.
- **Queue saturation:** A spike of slow handlers can fill the task queue. `max_queue_depth` and backpressure to the caller are the mitigations.

---

## 14. Design Patterns

| Pattern | Application |
|---|---|
| **Strategy** | ProcessPool is engine-agnostic. V8Worker and WasmWorker both implement the `Worker` trait. Pool dispatches to either transparently. Preemption strategy varies per engine type. |
| **Builder** | `TaskContext::builder()` accumulates capabilities before dispatch. The `ProcessPool` itself is configured via a builder at startup. |
| **Factory** | `WorkerFactory` creates workers of the correct type (V8 or WASM) based on pool config. Workers are created from factory on pool startup and on crash recovery. |
| **Adapter** | `WasmWorker` adapts Wasmtime's instance model to the Rivers `Worker` trait. `V8Worker` adapts the V8 Isolate model. The pool sees a uniform interface. |
| **Facade** | `ProcessPool` presents a single `dispatch(TaskContext) → TaskResult` interface over the complexity of isolate lifecycle, preemption, and queuing. |
| **Observer** | Pool errors and metrics are emitted as EventBus events (Observe tier). No caller needs to poll — the EventBus delivers to metrics collectors and alerting handlers. |
| **Singleton** | Each named pool is a singleton within a `riversd` instance. Multiple views share the same pool instance — they do not each get their own pool. |

---

## 15. Open Questions

| # | Question | Options | Status |
|---|---|---|---|
| 1 | ~~V8 snapshot content~~ | ~~What should the base snapshot include?~~ | **Closed** — No snapshots. Pure injection model at context creation. <!-- SHAPE-10 amendment --> |
| 2 | ~~Isolate-per-request vs reuse~~ | ~~Current design reuses isolates with context reset.~~ | **Closed** — Isolates reused. Fresh context per request; context unbound between executions. Streaming handlers get long-lived context. <!-- SHAPE-9 amendment --> |
| 3 | WASM threading | Wasmtime supports WASM threads (wasm-threads proposal). Should the WASM pool support multi-threaded WASM modules? Significant complexity increase. | **Deferred to v3** |
| 4 | ~~Shared V8 Heap Snapshot~~ | ~~The V8 heap snapshot is shared read-only across workers.~~ | **Closed** — No snapshots used. Moot. <!-- SHAPE-10 amendment --> |
| 5 | ~~TypeScript source maps~~ | ~~When a TS handler throws an exception, stack traces reference compiled JS line numbers.~~ | **Closed** — Resolved by `rivers-javascript-typescript-spec.md §5`. swc emits v3 source maps at bundle load; a V8 `PrepareStackTraceCallback` remaps frames to original `.ts:line:col` on `Error.stack` access. Always logged to per-app log; exposed in error envelope under `details.stack` in debug builds. |
