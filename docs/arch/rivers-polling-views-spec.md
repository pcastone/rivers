# Rivers Polling Views Specification

**Document Type:** Spec Addition / Patch  
**Scope:** Polling configuration for SSE and WebSocket views — diff strategies, change detection, on_change handler, poll loop lifecycle  
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

A poll loop is created when the first client connects to a polling view with a given parameter set. Loop key: `poll:{view_name}:{sha256(canonical_params)}`. <!-- SHAPE-3 amendment: SHA-256 of canonical JSON (BTreeMap ordering, serde_json::to_string). See appendix-canonical-json-key-derivation.md for the shared algorithm. -->

### 3.2 Client join

Subsequent clients connecting with the same parameters join the existing loop's broadcast group. They receive the next change push — they do not receive the previous state on connect. <!-- SHAPE-14 amendment: emit_on_connect removed, iceboxed for v1 -->

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

If `change_detect` throws, the tick is treated as no-change. The error is logged and an `OnChangeFailed` internal event is emitted. This is conservative — a broken diff handler does not cause runaway `on_change` fires.

<!-- SHAPE-20 amendment: diagnostic events for polling views -->
If `change_detect` times out (exceeds `task_timeout_ms`), the tick is treated as no-change and a `PollChangeDetectTimeout` diagnostic event is emitted with `consecutive_timeouts` count. This helps operators identify slow or stuck diff handlers.

| Condition | Client effect | Diagnostic event |
|---|---|---|
| `change_detect` returns `true` | Push to clients | -- |
| `change_detect` returns `false` | No push | -- |
| `change_detect` throws | No push | `OnChangeFailed` |
| `change_detect` times out | No push | `PollChangeDetectTimeout` with `consecutive_timeouts` |

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
// Synchronous — for pure in-memory comparisons
function detectChange(prev: any, current: any): boolean {
    // user logic
}

// Async — for comparisons requiring datasource lookups
async function detectChange(prev: any, current: any): Promise<boolean> {
    // user logic
}
```

`prev` is the full materialized previous result (or `null` on first tick). `current` is the full current result. The function may be synchronous or async — Rivers awaits the return value either way.

Async `change_detect` is appropriate when meaningful diff requires a datasource lookup (e.g., checking notification preferences before deciding whether a change is significant). Keep diff logic focused — a `change_detect` handler that makes multiple datasource round-trips on every tick will become a performance bottleneck at high poll frequency.

Runs in ProcessPool under `task_timeout_ms` — same timeout as non-streaming CodeComponent handlers. If it exceeds timeout, the tick is treated as no-change.

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

<!-- SHAPE-3 amendment: canonical JSON key derivation -->
Parameters are collected into a `BTreeMap<String, serde_json::Value>` (which provides deterministic key ordering), serialized via `serde_json::to_string()`, then SHA-256 hashed and hex-encoded. See `appendix-canonical-json-key-derivation.md` for the shared algorithm used by cache keys and polling state keys.

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

For SSE views, parameters come from query string: `/sse/prices?symbol=AAPL`. For WebSocket views, parameters come from path params or connection init message.

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

### 8.3 Session revalidation on polling connections

SSE and WebSocket connections persist for potentially hours. Session validation happens at connection time (the HTTP GET/upgrade request). Session revalidation on persistent connections is app-configurable per-view via `session_revalidation_interval_s`.

```toml
[api.views.price_feed]
view_type                       = "ServerSentEvents"
session_revalidation_interval_s = 300    # default: 0 (validate at connect only)
```

`session_revalidation_interval_s = 0` — session validated at connection time only. Connection lifetime is trusted independently of session expiry (default).

`session_revalidation_interval_s > 0` — Rivers rechecks the session in StorageEngine at the configured interval on each poll tick where the interval has elapsed. If the session has expired:

- **SSE** — Rivers sends a terminal event before closing:
  ```
  event: session_expired
  data: {"code": 4401, "reason": "session expired"}

  ```
  Then closes the connection.
- **WebSocket** — Rivers sends close frame with code `4401` and closes the connection.

The client is responsible for handling the close/event and redirecting to the guard view to re-authenticate.

---

## 9. Validation Rules

Enforced at config load time.

| Rule | Error message |
|---|---|
| `polling` declared, StorageEngine not configured | `polling views require storage_engine to be configured` |
| `diff_strategy = "change_detect"` without `change_detect` handler | `diff_strategy change_detect requires a change_detect handler` |
| `change_detect` handler declared with `diff_strategy != "change_detect"` | `change_detect handler requires diff_strategy = change_detect` |
| `on_change` handler not declared | `polling requires an on_change handler` |
| `tick_interval_ms = 0` | `tick_interval_ms must be greater than 0` |
| `poll_state_ttl_s = 0` | `poll_state_ttl_s must be greater than 0` |
| `polling` declared on `view_type = Rest` | `polling is only valid for ServerSentEvents and Websocket views` |
| `session_revalidation_interval_s` declared on `view_type = Rest` | `session_revalidation_interval_s is only valid for ServerSentEvents and Websocket views` |

---

## 10. Configuration Reference

### 10.1 Polling config block

Added to `ApiViewConfig` as an optional section:

```toml
[api.views.<name>.polling]
tick_interval_ms   = 5000          # how often to execute the DataView (required)
diff_strategy      = "hash"        # "hash" | "null" | "change_detect" (default: "hash")
poll_state_ttl_s   = 3600          # how long to retain prev state after last client disconnects
# emit_on_connect — removed, iceboxed for v1 <!-- SHAPE-14 amendment -->

[api.views.<name>.polling.on_change]
module     = "handlers/prices.ts"
entrypoint = "onPriceChange"

# Only present when diff_strategy = "change_detect"
[api.views.<name>.polling.change_detect]
module     = "handlers/prices.ts"
entrypoint = "detectPriceChange"
```

### 10.2 `emit_on_connect` — Iceboxed

<!-- SHAPE-14 amendment: emit_on_connect iceboxed for v1 -->
`emit_on_connect` has been removed from v1. If present in config, validation rejects it:

```
RiversError::Validation: emit_on_connect is not supported in v1 — remove from polling config
```

This feature is parked for future implementation when polling views are wired end-to-end.

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
# emit_on_connect removed — iceboxed for v1 <!-- SHAPE-14 amendment -->

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
Client connects → waits for next tick to receive first update.
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
# emit_on_connect removed — iceboxed for v1 <!-- SHAPE-14 amendment -->

[api.views.order_status.polling.change_detect]
module     = "handlers/orders.ts"
entrypoint = "detectOrderChange"

[api.views.order_status.polling.on_change]
module     = "handlers/orders.ts"
entrypoint = "onOrderChange"
```

```typescript
// handlers/orders.ts

// May be synchronous or async
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

## Shaping Amendments

The following changes were applied to this spec per decisions in `rivers-shaping-and-gap-analysis.md`:

### SHAPE-3: SHA-256 Cache Keys

- **S3.1** — Poll loop key uses SHA-256 of canonical JSON (BTreeMap ordering, `serde_json::to_string()`). Format: `poll:{view_name}:{hex_sha256}`. References `appendix-canonical-json-key-derivation.md` for the shared algorithm.
- **S6.1** — Deduplication key uses the same canonical JSON key derivation.

### SHAPE-14: `emit_on_connect` Iceboxed

- **S3.2** — Removed `emit_on_connect` reference from client join behavior. Clients receive the next change push, not the previous state on connect.
- **S10.1** — `emit_on_connect` config option removed from polling config block.
- **S10.2** — Entire `emit_on_connect` subsection replaced with icebox notice. Validation rejects config key if present.
- **S11.1** — `emit_on_connect = true` removed from price feed example.
- **S11.3** — `emit_on_connect = true` removed from order status example.

### SHAPE-20: Diagnostic Events for `change_detect`

- **S3.5** — Added `PollChangeDetectTimeout` diagnostic event with `consecutive_timeouts` count. Added diagnostic event table distinguishing `change_detect` outcomes: returns true (push), returns false (no push), throws (`OnChangeFailed`), times out (`PollChangeDetectTimeout`).
