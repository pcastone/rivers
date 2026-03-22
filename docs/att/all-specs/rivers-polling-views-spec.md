# Rivers Polling Views Specification

**Document Type:** Spec Addition / Patch  
**Scope:** Polling configuration for REST and SSE views — diff strategies, change detection, on_change handler, poll loop lifecycle  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-view-layer-spec.md`, `rivers-httpd-spec.md`, `rivers-storage-engine-spec.md`  
**Depends On:** Epic 4 (EventBus), Epic 10 (DataView Engine), Epic 13 (View Layer), StorageEngine

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [Mental Model](#2-mental-model)
3. [Poll Loop Lifecycle](#3-poll-loop-lifecycle)
4. [Diff Strategies](#4-diff-strategies)
5. [Handlers](#5-handlers)
6. [Poll Loop Deduplication](#6-poll-loop-deduplication)
7. [StorageEngine Integration](#7-storageengine-integration)
8. [View Type Behavior](#8-view-type-behavior)
9. [Validation Rules](#9-validation-rules)
10. [Configuration Reference](#10-configuration-reference)
11. [Examples](#11-examples)

---

## 1. Design Rationale

### 1.1 The Problem

SSE and WebSocket views are event-driven — they push when something is published to the EventBus. But not all data sources are event-driven. A relational database has no native change notification. A REST datasource has no webhook. The only way to detect change in these systems is to poll.

Without a native polling primitive, every application that needs "push on database change" must:
1. Run its own polling loop in a CodeComponent
2. Manage previous state manually
3. Implement its own diff logic
4. Handle connection lifecycle, reconnection, and multi-client fan-out independently

This is reinvented badly every time. Rivers owns it.

### 1.2 Design Principles

**Rivers owns the loop, user owns the logic.** Rivers manages tick scheduling, prev state storage, client fan-out, and loop lifecycle. The user declares what to poll (a DataView), how to detect change (diff strategy), and what to do when change is detected (`on_change` handler).

**Deduplication by DataView + parameters.** If 500 clients connect to the same view with the same parameters, one poll loop serves all 500. Poll execution cost is proportional to unique parameter sets, not connection count.

**StorageEngine is required.** Polling requires persistent prev state storage between ticks. StorageEngine must be explicitly configured. If polling is declared and StorageEngine is not configured, the server fails at startup.

---

## 2. Mental Model

```
Client connects to SSE or WebSocket view with polling configured
    │
    ▼
Rivers looks up {view_name}:{parameter_hash} in poll loop registry
    │
    ├─ Loop exists → client joins existing loop's broadcast group
    │
    └─ Loop does not exist → create new poll loop for this parameter set
            │
            ▼
        Poll loop runs at tick_interval_ms
            │
            ├─ Execute DataView → current result
            │
            ├─ Load prev result from StorageEngine
            │
            ├─ Run diff strategy
            │       hash         → SHA(prev) != SHA(current)
            │       null         → current != null
            │       change_detect → CodeComponent(prev, current) → bool
            │
            ├─ No change → store current as prev → wait for next tick
            │
            └─ Change detected
                    │
                    ├─ Store current as new prev in StorageEngine
                    │
                    ├─ Execute on_change(current) CodeComponent
                    │
                    └─ Broadcast result to all connected clients on this loop

Last client on a loop disconnects → loop stopped → StorageEngine entry retained (TTL-based cleanup)
```

---

## 3. Poll Loop Lifecycle

### 3.1 Loop creation

A poll loop is created when the first client connects to a polling view with a given parameter set. Loop key: `poll:{view_name}:{sha256(canonical_params)}`.

### 3.2 Client join

Subsequent clients connecting with the same parameters join the existing loop's broadcast group. They receive the next change push — they do not receive the previous state on connect unless `emit_on_connect = true` is configured (see §10).

### 3.3 Loop stop

When the last client on a loop disconnects, the loop stops. The StorageEngine entry for `prev` state is **not deleted** — it is retained until TTL expiry. This means if a new client connects shortly after, the next diff runs against the last known state rather than treating everything as new.

### 3.4 Tick execution

Each tick:
1. DataView executes with the loop's parameter set
2. Prev result loaded from StorageEngine (null on first tick)
3. Diff strategy runs
4. If changed: store current as prev, execute `on_change`, broadcast to clients
5. If unchanged: store current as prev (hash/null strategies store lightweight), wait for next tick

On first tick, prev is null. Behavior depends on diff strategy:
- `hash` — null prev SHA != current SHA → change detected, `on_change` fires
- `null` — if current is non-null → change detected, `on_change` fires; if current is also null → no change
- `change_detect` — `change_detect(null, current)` called; handler decides

### 3.5 Error handling

If the DataView execution fails on a tick, the tick is skipped — no diff runs, no state update. The error is logged and a `PollTickFailed` internal event is emitted. The loop continues at the next tick interval.

If `on_change` throws, the error is logged and a `OnChangeFailed` internal event is emitted. The prev state update still occurs — the next tick will not re-fire `on_change` for the same data.

If `change_detect` throws, the tick is treated as no-change. The error is logged. This is conservative — a broken diff handler does not cause runaway `on_change` fires.

---

## 4. Diff Strategies

### 4.1 `hash`

Rivers serializes the DataView result to canonical JSON, computes `SHA-256`, and compares against the stored hash of the previous result.

```
sha256(serialize(prev)) != sha256(serialize(current))  →  change
```

Prev state stored in StorageEngine: the full serialized result (not just the hash) — required so `on_change` receives `current` and the next tick has a full prev for `change_detect` if strategy changes.

Best for: any structured result where order is stable. Simple, fast, no user code.

### 4.2 `null`

Rivers checks whether the current result is non-null (non-empty array, non-null object).

```
current != null && current != []  →  change
```

Semantics: fires `on_change` every tick that produces a non-empty result. Does **not** compare against previous — this is a presence check, not a delta check. Useful for alert-style patterns: "fire when this query returns anything."

Prev state stored: not used for comparison. Current stored as prev for continuity.

### 4.3 `change_detect`

User-supplied CodeComponent that receives `(prev, current)` and returns `bool`. Runs in ProcessPool.

```typescript
// true  = change occurred → fire on_change
// false = no change → skip
function detectChange(prev: any, current: any): boolean {
    // user logic
}
```

`prev` is the full materialized previous result (or `null` on first tick). `current` is the full current result. The function is synchronous — it must return `bool` directly, not `Promise<bool>`. This keeps diff execution lightweight.

If the function is async (returns `Promise`), Rivers rejects it at deploy time with `change_detect handler must be synchronous`.

Runs in ProcessPool using `task_timeout_ms` (not `stream_timeout_ms`) — diff is expected to be fast. If it exceeds timeout, the tick is treated as no-change.

---

## 5. Handlers

### 5.1 `change_detect(prev, current) → bool`

**Purpose:** Determine whether a change occurred.  
**Receives:** Previous result (full value or null) + current result (full value).  
**Returns:** `bool` — `true` means change detected.  
**Execution:** ProcessPool, synchronous, `task_timeout_ms`.  
**Present when:** `diff_strategy = "change_detect"` only.

```typescript
// change_detect contract
function detectChange(prev: any, current: any): boolean { }
```

### 5.2 `on_change(current)`

**Purpose:** React to a detected change.  
**Receives:** Current result only.  
**Returns:** `Rivers.Response | void` — for SSE/WebSocket views the return value is broadcast to connected clients. If void or null, nothing is pushed and the raw DataView result is broadcast instead.  
**Execution:** ProcessPool, async, `task_timeout_ms`.  
**Present when:** Always required when polling is configured.

```typescript
// on_change contract
async function onChange(
    req: Rivers.Request   // req.body = current DataView result
): Promise<Rivers.Response | void> { }
```

`req.body` contains the current DataView result. The handler can reshape, enrich, or filter the result before it reaches clients. If the handler returns void or null, Rivers broadcasts the raw DataView result.

---

## 6. Poll Loop Deduplication

### 6.1 Deduplication key

```
poll:{view_name}:{sha256(canonical_json(sorted_params))}
```

Parameters are sorted by key before hashing to ensure `{a:1, b:2}` and `{b:2, a:1}` produce the same key.

### 6.2 Examples

| View | Parameters | Result |
|---|---|---|
| `/sse/prices` | `{ symbol: "AAPL" }` | 1 loop for all AAPL watchers |
| `/sse/prices` | `{ symbol: "GOOG" }` | Separate loop — different params |
| `/sse/prices` | `{}` | 1 loop for all parameterless watchers |
| `/sse/dashboard` | `{}` | Separate loop — different view |

500 clients watching `AAPL` → 1 DataView execution per tick.  
500 clients each watching a different symbol → 500 DataView executions per tick.  
Operators should be aware of this when designing high-cardinality polling views.

### 6.3 Parameter source

For SSE views, parameters come from query string: `/sse/prices?symbol=AAPL`. For WebSocket views, parameters come from path params or connection init message. For REST polling views, parameters come from path params and query string as normal.

---

## 7. StorageEngine Integration

### 7.1 Requirement

Polling requires StorageEngine to be configured. If `polling` is declared on any view and `base.storage_engine` is not configured, the server fails at startup:

```
RiversError::Validation: polling views require storage_engine to be configured
```

### 7.2 Storage schema

| Key pattern | Value | TTL |
|---|---|---|
| `poll:{view}:{param_hash}:prev` | Full serialized DataView result | `poll_state_ttl_s` (default: 3600) |
| `poll:{view}:{param_hash}:meta` | `{ last_tick_ms, loop_active, client_count }` | Same TTL |

### 7.3 TTL behavior

When the last client disconnects, the poll loop stops but the StorageEngine entry is retained until TTL. This serves two purposes:

1. If a new client connects within the TTL window, the next diff runs against the last known state rather than treating everything as new (prevents spurious `on_change` fires on reconnect)
2. Operational visibility — operators can inspect last known poll state without needing an active connection

`poll_state_ttl_s` is configurable per view (see §10). Default is 3600 seconds (1 hour).

### 7.4 Cluster behavior

StorageEngine backend determines cluster behavior:

- `sqlite` / `in_memory` — node-local prev state. Different nodes serving the same view may have divergent prev state. Acceptable for single-node deployments.
- `redis` — shared prev state across all cluster nodes. Required for multi-node deployments where clients may connect to any node.

Operators running polling views in a multi-node cluster must configure Redis-backed StorageEngine.

---

## 8. View Type Behavior

### 8.1 SSE views with polling

The natural fit. On change detected → result broadcast as SSE event to all connected clients on the loop. `on_change` return value is the event data. If `on_change` returns void, the raw DataView result is the event data.

```
event: change\n
data: {<on_change return value or raw DataView result>}\n\n
```

The SSE connection is the loop membership. Client connects → joins loop. Client disconnects → leaves loop.

### 8.2 WebSocket views with polling

Same model as SSE. On change detected → result broadcast as text frame to all connected clients on the loop. Bidirectional capability of WebSocket is orthogonal to polling — clients can still send messages via `on_stream` handler.

### 8.3 REST views with polling

Polling on a REST view is valid configuration but produces a side-effect-only machine. There is no persistent connection, so `on_change` fires but there is no client to push to. The return value of `on_change` is discarded.

This is useful for: write-behind patterns (poll a source, write changes to another datasource), webhook relay (poll a source, call an HTTP datasource on change), or internal state synchronization.

Rivers does not warn on this configuration — it is intentional. Poll loops on REST views behave identically to SSE/WebSocket loops except the broadcast step is a no-op.

---

## 9. Validation Rules

Enforced at config load time.

| Rule | Error message |
|---|---|
| `polling` declared, StorageEngine not configured | `polling views require storage_engine to be configured` |
| `diff_strategy = "change_detect"` without `change_detect` handler | `diff_strategy change_detect requires a change_detect handler` |
| `change_detect` handler declared with `diff_strategy != "change_detect"` | `change_detect handler requires diff_strategy = change_detect` |
| `change_detect` handler is async | `change_detect handler must be synchronous` |
| `on_change` handler not declared | `polling requires an on_change handler` |
| `tick_interval_ms = 0` | `tick_interval_ms must be greater than 0` |
| `poll_state_ttl_s = 0` | `poll_state_ttl_s must be greater than 0` |

---

## 10. Configuration Reference

### 10.1 Polling config block

Added to `ApiViewConfig` as an optional section:

```toml
[api.views.<name>.polling]
tick_interval_ms   = 5000          # how often to execute the DataView (required)
diff_strategy      = "hash"        # "hash" | "null" | "change_detect" (default: "hash")
poll_state_ttl_s   = 3600          # how long to retain prev state after last client disconnects
emit_on_connect    = false         # push current state immediately on client connect (default: false)

[api.views.<name>.polling.on_change]
module     = "handlers/prices.ts"
entrypoint = "onPriceChange"

# Only present when diff_strategy = "change_detect"
[api.views.<name>.polling.change_detect]
module     = "handlers/prices.ts"
entrypoint = "detectPriceChange"
```

### 10.2 `emit_on_connect`

When `true`, the current DataView result is immediately pushed to a newly connected client without waiting for the next tick or a change. The `on_change` handler is **not** invoked for this initial push — the raw DataView result is sent directly.

This prevents the client from seeing stale UI while waiting for the first tick. Default is `false`.

---

## 11. Examples

### 11.1 Live price feed — hash strategy

```toml
[api.views.price_feed]
path      = "/sse/prices"
method    = "GET"
view_type = "ServerSentEvents"

[api.views.price_feed.handler]
type     = "dataview"
dataview = "get_price"          # SELECT price, bid, ask FROM quotes WHERE symbol = $1

[api.views.price_feed.polling]
tick_interval_ms = 1000
diff_strategy    = "hash"
emit_on_connect  = true

[api.views.price_feed.polling.on_change]
module     = "handlers/prices.ts"
entrypoint = "onPriceChange"
```

```typescript
// handlers/prices.ts
async function onPriceChange(
    req: Rivers.Request
): Promise<Rivers.Response> {
    const quote = req.body;
    return {
        status: 200,
        body: {
            symbol:    quote.symbol,
            price:     quote.price,
            change_pct: computeChangePct(quote),
            timestamp: Date.now()
        }
    };
}
```

500 clients watching AAPL → 1 DataView execution per second.  
Client connects → immediately receives current quote (emit_on_connect).  
Price changes → hash differs → `onPriceChange` fires → all 500 clients receive update.  
Price unchanged → hash matches → nothing sent.

---

### 11.2 Alert pattern — null strategy

```toml
[api.views.fraud_alerts]
path      = "/sse/alerts"
method    = "GET"
view_type = "ServerSentEvents"

[api.views.fraud_alerts.handler]
type     = "dataview"
dataview = "get_pending_alerts"   # SELECT * FROM alerts WHERE status = 'pending' AND severity = 'high'

[api.views.fraud_alerts.polling]
tick_interval_ms = 10000
diff_strategy    = "null"         # fire whenever query returns results

[api.views.fraud_alerts.polling.on_change]
module     = "handlers/alerts.ts"
entrypoint = "onAlertDetected"
```

Query returns rows → null strategy fires → `onAlertDetected` pushes to connected clients.  
Query returns empty → no push.  
Same rows on next tick → fires again (null strategy does not compare content, only presence).

---

### 11.3 Custom diff — change_detect strategy

```toml
[api.views.order_status]
path      = "/ws/orders/{order_id}"
method    = "GET"
view_type = "Websocket"

[api.views.order_status.handler]
type     = "dataview"
dataview = "get_order"

[api.views.order_status.polling]
tick_interval_ms = 2000
diff_strategy    = "change_detect"
emit_on_connect  = true

[api.views.order_status.polling.change_detect]
module     = "handlers/orders.ts"
entrypoint = "detectOrderChange"

[api.views.order_status.polling.on_change]
module     = "handlers/orders.ts"
entrypoint = "onOrderChange"
```

```typescript
// handlers/orders.ts

// Synchronous — must not be async
function detectOrderChange(prev: any, current: any): boolean {
    if (prev === null) return true;
    // Only push if status or fulfillment changed — ignore timestamp noise
    return prev.status !== current.status ||
           prev.fulfillment_status !== current.fulfillment_status;
}

async function onOrderChange(
    req: Rivers.Request
): Promise<Rivers.Response> {
    const order = req.body;
    return {
        status: 200,
        body: {
            order_id:           order.id,
            status:             order.status,
            fulfillment_status: order.fulfillment_status,
            updated_at:         order.updated_at
        }
    };
}
```

Each connected client watching a different `order_id` → separate poll loop per order.  
Timestamp or metadata changes → `detectOrderChange` returns false → no push.  
Status changes → returns true → `onOrderChange` fires → client receives update.

---

### 11.4 REST polling — side-effect machine

```toml
[api.views.sync_inventory]
path      = "/api/inventory/sync"
method    = "GET"
view_type = "Rest"

[api.views.sync_inventory.handler]
type     = "dataview"
dataview = "get_inventory_snapshot"

[api.views.sync_inventory.polling]
tick_interval_ms = 30000
diff_strategy    = "hash"

[api.views.sync_inventory.polling.on_change]
module     = "handlers/inventory.ts"
entrypoint = "onInventoryChange"
```

```typescript
// on_change return value is discarded — REST view has no persistent connection
// use this for write-behind or webhook relay
async function onInventoryChange(
    req: Rivers.Request
): Promise<void> {
    const snapshot = req.body;
    // Write delta to warehouse system via HTTP datasource
    await Rivers.http.post(
        Rivers.resources.WarehouseAPI,
        "/api/inventory/update",
        { body: snapshot }
    );
    Rivers.log.info("inventory synced to warehouse", {
        item_count: snapshot.length
    });
}
```
