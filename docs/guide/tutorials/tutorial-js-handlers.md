# Tutorial: JavaScript Handlers

**Rivers v0.50.1**

## Overview

JavaScript handlers (CodeComponents) let you add custom business logic to Rivers views. Handlers run in V8 isolates — sandboxed, fast, and isolated per request.

Use handlers when a DataView alone isn't enough: validation, transformation, conditional logic, multi-DataView orchestration, or external API calls.

---

## Handler Basics

### View Configuration

```toml
[api.views.process_order]
path      = "orders"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.process_order.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/orders.js"
entrypoint = "processOrder"
resources  = ["orders_db", "inventory_db"]
```

| Field | Description |
|-------|-------------|
| `type` | `"codecomponent"` — runs a JS/WASM handler |
| `language` | `"javascript"`, `"typescript"`, `"js"`, `"ts"` |
| `module` | Path to the JS file (relative to app directory) |
| `entrypoint` | Function name to call |
| `resources` | Datasources the handler can access |

**Rule:** `resources` must list every datasource the handler accesses. Accessing an undeclared datasource throws `CapabilityError`.

---

## The ctx Object

Every handler receives a `ctx` parameter with the full request context.

```javascript
function handler(ctx) {
    // ── Request (read-only) ──
    ctx.request.method        // "GET", "POST", "PUT", "DELETE"
    ctx.request.path          // "/api/orders/123"
    ctx.request.query_params  // { limit: "10", sort: "name" }
    ctx.request.headers       // { "content-type": "application/json" }
    ctx.request.body          // Parsed JSON body (POST/PUT)
    ctx.request.path_params   // { id: "123" }

    // ── Pre-fetched data ──
    ctx.data.list_orders      // DataView results (from resources)

    // ── Dynamic DataView calls ──
    var user = ctx.dataview("get_user", { id: "abc-123" });

    // ── Application KV store ──
    ctx.store.set("key", { data: 42 }, 60000);  // TTL in ms
    ctx.store.get("key");                         // returns { data: 42 }
    ctx.store.del("key");

    // ── Session (when auth = "session") ──
    ctx.session.subject       // User ID
    ctx.session.username      // Username
    ctx.session.groups        // ["admin", "user"]

    // ── Metadata ──
    ctx.trace_id              // Request trace ID
    ctx.app_id                // Application ID
    ctx.env                   // Runtime environment

    // ── Response ──
    ctx.resdata = { result: "success" };  // This becomes the HTTP body
}
```

---

## Rivers Global APIs

### Logging

```javascript
Rivers.log.info("order processed", { order_id: "123", total: 99.50 });
Rivers.log.warn("inventory low", { product_id: "abc", remaining: 3 });
Rivers.log.error("payment failed", { reason: "card declined" });
```

`trace_id` is automatically included in every log entry.

### Crypto

```javascript
// Password hashing (bcrypt-based)
var hash = Rivers.crypto.hashPassword("user-password");
var valid = Rivers.crypto.verifyPassword("user-password", hash);

// Random generation
var hex = Rivers.crypto.randomHex(16);           // 32-char hex string
var token = Rivers.crypto.randomBase64url(32);    // URL-safe base64

// HMAC and timing-safe comparison
var sig = Rivers.crypto.hmac("secret-key", "data-to-sign");
var equal = Rivers.crypto.timingSafeEqual("a", "b");
```

### Outbound HTTP

Requires `allow_outbound_http = true` on the view handler.

```javascript
var resp = await Rivers.http.get("https://api.example.com/data");
var resp = await Rivers.http.post(url, { key: "value" });
var resp = await Rivers.http.put(url, body);
var resp = await Rivers.http.del(url);

// resp = { status: 200, body: {...}, headers: {...} }
```

---

## Patterns

### Input Validation

```javascript
function createUser(ctx) {
    var body = ctx.request.body;

    if (!body) throw new Error("request body required");
    if (!body.email) throw new Error("email is required");
    if (!body.name) throw new Error("name is required");

    // Throwing an Error returns HTTP 500 with the error message
    // The framework wraps it in the standard ErrorResponse format

    var result = ctx.dataview("insert_user", {
        name: body.name,
        email: body.email
    });

    ctx.resdata = result;
}
```

### Multi-DataView Orchestration

```javascript
function getOrderDetails(ctx) {
    var orderId = ctx.request.path_params.id;

    var order = ctx.dataview("get_order", { id: orderId });
    if (!order) throw new Error("order not found");

    var items = ctx.dataview("get_order_items", { order_id: orderId });
    var customer = ctx.dataview("get_customer", { id: order.customer_id });

    ctx.resdata = {
        order: order,
        items: items,
        customer: customer
    };
}
```

### Async / Parallel DataView Calls

```javascript
async function getDashboard(ctx) {
    var [users, orders, metrics] = await Promise.all([
        Promise.resolve(ctx.dataview("recent_users", { limit: 5 })),
        Promise.resolve(ctx.dataview("recent_orders", { limit: 10 })),
        Promise.resolve(ctx.dataview("system_metrics"))
    ]);

    ctx.resdata = { users: users, orders: orders, metrics: metrics };
}
```

### Conditional Response

```javascript
function getItems(ctx) {
    var format = ctx.request.query_params.format;
    var items = ctx.dataview("list_items", { limit: 50 });

    if (format === "ids") {
        ctx.resdata = items.map(function(item) { return item.id; });
    } else if (format === "summary") {
        ctx.resdata = items.map(function(item) {
            return { id: item.id, name: item.name };
        });
    } else {
        ctx.resdata = items;
    }
}
```

---

## Multiple Entrypoints in One File

One JS file can export multiple functions. Each view points to a different `entrypoint`.

```javascript
// libraries/handlers/users.js

function listUsers(ctx) { /* ... */ }
function getUser(ctx) { /* ... */ }
function createUser(ctx) { /* ... */ }
function updateUser(ctx) { /* ... */ }
function deleteUser(ctx) { /* ... */ }
```

```toml
[api.views.list_users.handler]
entrypoint = "listUsers"
module     = "libraries/handlers/users.js"

[api.views.create_user.handler]
entrypoint = "createUser"
module     = "libraries/handlers/users.js"
```

---

## Supported Languages

| Language | Config Values | Runtime |
|----------|--------------|---------|
| JavaScript | `"javascript"`, `"js"`, `"js_v8"` | V8 |
| TypeScript | `"typescript"`, `"ts"`, `"ts_v8"`, `"typescript_strict"` | V8 |
| WASM | `"wasm"` | Wasmtime |

---

## ProcessPool Configuration

Handlers run in a ProcessPool. Configure in `app.toml`:

```toml
[runtime.process_pools.default]
engine              = "v8"
workers             = 4
task_timeout_ms     = 5000
max_heap_mb         = 128
max_queue_depth     = 0           # 0 = workers × 4
recycle_after_tasks = 0           # 0 = never recycle
```
