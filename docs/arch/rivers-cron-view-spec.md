# Rivers Cron View Specification

**Document Type:** New spec
**Scope:** `view_type = "Cron"` — fire-and-forget scheduled tasks driven by time, not by clients or events
**Status:** Implementation (Sprint 2026-05-09 Track 3)
**Patches:** `rivers-view-layer-spec.md`, `rivers-storage-engine-spec.md`, `rivers-feature-inventory.md`
**Source ask:** CB-P1.14 (`case-rivers-scheduled-task-primitive.md`, filed 2026-05-09)

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [Mental Model](#2-mental-model)
3. [Configuration](#3-configuration)
4. [Tick Lifecycle](#4-tick-lifecycle)
5. [Multi-instance Deduplication](#5-multi-instance-deduplication)
6. [Overlap Policy](#6-overlap-policy)
7. [StorageEngine Integration](#7-storageengine-integration)
8. [Validation Rules](#8-validation-rules)
9. [Observability](#9-observability)
10. [Failure Semantics](#10-failure-semantics)
11. [Examples](#11-examples)
12. [Non-goals (v1)](#12-non-goals-v1)

---

## 1. Design Rationale

### 1.1 The problem

Rivers' existing view types are all client-driven or event-driven:

- `Rest`, `Mcp` — fire on inbound HTTP request.
- `Websocket`, `ServerSentEvents` — fire on inbound connection (and tick under polling, but only when at least one client is connected).
- `MessageConsumer` — fire on EventBus event.

There is no way to declare "run handler X every N seconds, regardless of client activity." Background recompute, periodic cleanup, idle aging, and scheduled report rebuilds have no native shape — operators reach for OS cron hitting an admin REST endpoint, which breaks the single-binary deployment story.

### 1.2 Design principles

**Time is the trigger.** Cron views fire on schedule with no client present. They are not addressable by URL — there is no `path` or `method`.

**Same execution environment as REST/MCP.** A cron handler is a CodeComponent with the same `Rivers.db.*`, `Rivers.crypto.*`, `Rivers.log.*`, `Rivers.keystore` surfaces. Capability propagation works exactly like a `view = "..."` MCP tool dispatch.

**One node fires per tick, even in a multi-node cluster.** StorageEngine `set_if_absent` with a per-tick key gives first-writer-wins semantics. No coordinator process; no leader election. Same pattern polling views already use for loop dedupe (per `rivers-polling-views-spec.md` §6).

**Reuse over reinvention.** Rivers already owns the codecomponent dispatch path, the StorageEngine, the metrics + logging fabric, and the per-app capability model. Cron views compose existing primitives — they do not introduce a new runtime subsystem beyond a tick scheduler.

---

## 2. Mental Model

```
riversd starts
    │
    ▼
For each [api.views.<name>] with view_type = "Cron":
    │
    ▼
  Spawn one tokio::task running a Cron loop
    │
    ▼
  Cron loop computes next_tick_at (cron expression OR fixed interval)
    │
    ▼
  Sleep until next_tick_at
    │
    ▼
  Attempt StorageEngine.set_if_absent("cron:{app}:{view}:{tick_epoch}", node_id, ttl)
    │
    ├─ Conflict (another node won)  →  metrics::cron_skipped_dedupe++ → loop
    │
    └─ Won the tick
            │
            ├─ Check overlap_policy:
            │      skip   — if previous tick still running, skip + metric
            │      queue  — push tick into bounded queue (default cap 16)
            │      allow  — spawn unconditionally
            │
            ▼
      Dispatch codecomponent via process_pool with TaskKind::Cron
            │
      Same task_enrichment::wire_datasources call as REST → handler sees Rivers.db.*
            │
            ▼
      Handler runs. Result.value is logged at debug; non-OK results increment cron_failures_total.
            │
            ▼
  Loop
```

---

## 3. Configuration

### 3.1 Cron view shape

```toml
[api.views.recompute_signals]
view_type        = "Cron"
schedule         = "*/5 * * * *"          # cron expression OR
# interval_seconds = 300                   # plain integer (mutually exclusive)
overlap_policy   = "skip"                  # "skip" (default) | "queue" | "allow"
max_concurrent   = 16                      # only meaningful for overlap_policy = "queue" (default 16)

[api.views.recompute_signals.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/signals.ts"
entrypoint = "recomputeAllProjects"
resources  = ["cb_db"]
```

### 3.2 Required fields for `view_type = "Cron"`

| Field | Required? | Notes |
|---|---|---|
| `view_type` | yes | Must be `"Cron"` |
| `handler` | yes | `type = "codecomponent"` |
| `schedule` **or** `interval_seconds` | exactly one | Mutually exclusive |
| `overlap_policy` | optional | Default `"skip"` |
| `max_concurrent` | optional | Default 16; only honored for `overlap_policy = "queue"` |

### 3.3 Forbidden fields for `view_type = "Cron"`

| Field | Reason |
|---|---|
| `path` | Cron views aren't HTTP-addressable |
| `method` | Same |
| `auth` | No caller — no auth boundary |
| `guard_view` | Same — no caller |
| `polling` | Cron is the scheduler; polling drives client subscriptions |
| `response_headers` | No HTTP response |
| `tools`, `resources`, `prompts`, `instructions` | MCP-only fields |
| `federation` | MCP-only |
| `websocket_mode`, `max_connections`, etc. | WS/SSE-only |
| `streaming`, `streaming_format`, `stream_timeout_ms` | Streaming REST only |

The validator emits `S005` for any of these on a Cron view.

### 3.4 Schedule formats

**`schedule = "..."`** — cron expression with **6 fields**: `sec min hour day-of-month month day-of-week`. Matches the `cron` crate's parser. Examples:

| Expression | Meaning |
|---|---|
| `0 */5 * * * *` | every 5 minutes (at second 0) |
| `*/30 * * * * *` | every 30 seconds |
| `0 0 */1 * * *` | every hour, on the hour |
| `0 0 9 * * MON-FRI` | 9am weekdays |
| `0 0 0 1 * *` | midnight on the 1st of every month |

The 5-field POSIX shorthand (`*/5 * * * *`) is **not** accepted — always include the leading seconds field. The validator rejects 5-field expressions with `S005`.

**`interval_seconds = N`** — plain integer interval, computed from the loop's start time. `N >= 1`. The validator emits `W009`-style warning for `N < 5` (very high tick rate; warn but allow).

The two forms are mutually exclusive at validation. If both appear, `S005`.

### 3.5 Overlap policies

| Policy | Behavior |
|---|---|
| `skip` (default) | If previous tick still running, drop this tick. Increments `cron_skipped_overlap_total`. |
| `queue` | Push tick into a bounded `tokio::sync::mpsc` (cap = `max_concurrent`, default 16). When queue is full, drop with metric. |
| `allow` | Fire concurrently every tick. Caller's responsibility to be safe. |

`max_concurrent` only meaningful for `queue` — has no effect on `skip` or `allow`.

---

## 4. Tick Lifecycle

### 4.1 Loop startup

When `riversd` starts, for each Cron view in each app:

1. Parse the schedule (cron expression) or interval into a `NextTick` strategy.
2. Spawn one `tokio::task` per view: the **cron loop**.
3. Each loop computes `next_tick_at` (UTC instant) and sleeps via `tokio::time::sleep_until`.

### 4.2 Each tick

On wake-up:

1. Compute `tick_epoch = next_tick_at.timestamp()` (i64 seconds since epoch).
2. Compute dedupe key: `cron:{app_id}:{view_name}:{tick_epoch}`.
3. Call `StorageEngine.set_if_absent(namespace="cron", key=..., value=node_id, ttl=tick_dedup_ttl)`.
   - If `Ok(false)` (key already existed) — another node won. Record `cron_skipped_dedupe` and recompute next tick.
   - If `Ok(true)` — this node owns the tick.
4. Apply `overlap_policy`:
   - `skip`: if a previous task for this view is still in flight (atomic flag), record `cron_skipped_overlap` and skip.
   - `queue`: push tick onto the per-view queue; reject + metric if full.
   - `allow`: spawn unconditionally.
5. Dispatch handler. Record `cron_runs_total` on dispatch start; `cron_failures_total` on non-OK; `cron_duration_ms` histogram on completion.
6. Compute `next_tick_at` and sleep.

### 4.3 `tick_dedup_ttl`

The TTL on the dedupe key. Set to `max(tick_interval, 60s)` — one full interval plus a margin to ensure other nodes don't fire the same tick if their clocks are skewed. Capped at `3600s` to avoid leaking stale keys for very long intervals.

### 4.4 Synthetic dispatch envelope

The handler receives the same shape as a REST codecomponent invocation:

```json
{
  "request": {
    "headers":     {},
    "body":        null,
    "path_params": {},
    "query":       {}
  },
  "session":     null,
  "path_params": {}
}
```

Specifically: `request.body` is `null`, all params are empty, `session` is `null`. Handlers that read `request.headers["authorization"]` or `path_params.x` see empty values — the cron loop has no caller.

A `ctx.cron` field carries scheduler context:

```json
{
  "cron": {
    "view_name":   "recompute_signals",
    "tick_epoch":  1715299200,
    "scheduled":   "2026-05-09T22:00:00Z",
    "fired":       "2026-05-09T22:00:00.073Z",
    "node_id":     "<node_id>"
  }
}
```

`scheduled` is when the tick was supposed to fire; `fired` is when this node actually started the dispatch — the difference is the dispatch latency, useful for diagnosing slow tick handling.

---

## 5. Multi-instance Deduplication

### 5.1 Requirement

A Rivers cluster running N nodes with the same bundle must have **exactly one** tick fire per scheduled time, not N. Otherwise scheduled work runs N× and cron becomes unusable for "exactly once" recompute.

### 5.2 Approach

StorageEngine `set_if_absent`:

```
namespace = "cron"
key       = "{app_id}:{view_name}:{tick_epoch}"
value     = node_id (for diagnostic only)
ttl       = tick_dedup_ttl
```

First-writer-wins. Other nodes get `Ok(false)` and skip. Identical pattern to polling-view loop dedupe (`rivers-polling-views-spec.md` §6).

### 5.3 Backend implications

- **`memory` StorageEngine** — node-local. Multi-instance dedupe **does not work**; every node fires every tick. Acceptable for single-node dev only. `riversd` emits `W011` at startup if Cron views are declared with this backend.
- **`sqlite`** — node-local file. Same caveat as `memory` unless the SQLite file is on a shared filesystem (uncommon, fragile). Same `W011`.
- **`redis`** — shared across cluster. Required for multi-instance Cron deployments.

The backend string values match the riversd config: `[storage_engine] backend ∈ {"memory", "sqlite", "redis"}`.

### 5.4 Clock skew tolerance

`tick_dedup_ttl = max(tick_interval, 60s)` (capped 3600s). A node with clock skewed up to one interval behind/ahead of the cluster will still see the dedupe key for the previous tick and skip.

For very long intervals (e.g., daily), the 3600s cap means clock skew > 1 hour could allow double-fires. Operators with that level of skew have larger problems; not addressed in v1.

---

## 6. Overlap Policy

### 6.1 `skip` (default)

Per-view atomic flag (`AtomicBool`). On tick, `compare_exchange(false, true)`:

- Won: dispatch; reset flag in dispatch's `finally` (whether OK or error).
- Lost: another tick is still running. Increment `cron_skipped_overlap_total{view=...}`. Skip dispatch.

Works well for handlers that occasionally exceed their interval (slow DB query, transient lag). The next tick simply runs at the next interval.

### 6.2 `queue`

Per-view `tokio::sync::mpsc` channel of capacity `max_concurrent` (default 16). On tick: `try_send`. A consumer task pulls and dispatches sequentially.

- Send succeeded: queued.
- Send failed (channel full): increment `cron_dropped_queue_full_total`, log at warn.

Useful for handlers where every tick matters and you accept that bursts will be processed sequentially with bounded backlog.

### 6.3 `allow`

Just `tokio::spawn` per tick. No coordination. Concurrent ticks may execute simultaneously.

Useful for stateless or idempotent handlers where serialization gives no benefit.

---

## 7. StorageEngine Integration

### 7.1 Requirement

Cron views require StorageEngine to be configured. If any Cron view is declared and `[storage_engine]` is absent, server fails at startup with:

```
RiversError::Validation: Cron views require storage_engine to be configured
```

(Reuses the existing polling-view validation pattern.)

### 7.2 Storage schema

| Key pattern | Value | TTL |
|---|---|---|
| `cron:{app}:{view}:{tick_epoch}` | `node_id` (string) | `tick_dedup_ttl` |

Reads via `flush_expired` clean up automatically. No other state is persisted between ticks.

### 7.3 What is **not** persisted

- Tick history. If a node is offline during a tick, that tick is gone — no "catch-up" semantics in v1.
- Last-run timestamp. Operators read it from logs/metrics, not StorageEngine.
- Handler state. Cron handlers are stateless by design; if they need state, they read/write it themselves via `Rivers.db.*` or `Rivers.keystore`.

---

## 8. Validation Rules

Enforced at config load time (Layer 1 + Layer 3).

### 8.1 Structural (Layer 1, `S005`)

| Rule | Severity | Code |
|---|---|---|
| `view_type = "Cron"` with `schedule` and `interval_seconds` both set | error | `S005` |
| `view_type = "Cron"` with neither `schedule` nor `interval_seconds` | error | `S005` |
| `interval_seconds < 1` | error | `S005` |
| `interval_seconds < 5` | warning | `W011` |
| `schedule` not parseable by `cron` crate | error | `S005` |
| `overlap_policy` outside `{skip, queue, allow}` | error | `S005` |
| `path` set on Cron view | error | `S005` |
| `method` set on Cron view | error | `S005` |
| `auth` set on Cron view | error | `S005` |
| `guard_view` set on Cron view | error | `S005` |
| `response_headers` set on Cron view | error | `S005` |

### 8.2 Cross-references (Layer 3)

| Rule | Severity | Code |
|---|---|---|
| Cron view with `handler.type != "codecomponent"` | error | `X-CRON-1` |
| Cron view with `handler.resources` referencing unknown datasource | error | `X-CRON-2` (reuses existing handler-resource cross-ref) |

### 8.3 Cluster (startup)

| Rule | Severity | Surface |
|---|---|---|
| Cron views declared, `[storage_engine]` not configured | error | `ServerError::Config` returned from `load_and_wire_bundle` — server refuses to start |
| Cron views declared, storage backend `∈ {memory, sqlite}` | warning | `tracing::warn!` at startup with `code = "W011"` — multi-instance dedupe broken |

---

## 9. Observability

### 9.1 Metrics (gated on `metrics` feature)

| Name | Type | Labels |
|---|---|---|
| `rivers_cron_runs_total` | counter | `app`, `view` |
| `rivers_cron_failures_total` | counter | `app`, `view` |
| `rivers_cron_skipped_overlap_total` | counter | `app`, `view` |
| `rivers_cron_skipped_dedupe_total` | counter | `app`, `view` |
| `rivers_cron_dropped_queue_full_total` | counter | `app`, `view` |
| `rivers_cron_duration_ms` | histogram | `app`, `view` |

### 9.2 Logging

| Event | Level | Fields |
|---|---|---|
| Tick fired (this node won) | `debug` | `app`, `view`, `tick_epoch`, `dispatch_latency_ms` |
| Tick skipped (dedupe lost) | `debug` | `app`, `view`, `tick_epoch` |
| Tick skipped (overlap policy=skip) | `debug` | `app`, `view`, `tick_epoch` |
| Handler error | `error` | `app`, `view`, `tick_epoch`, `error`, `duration_ms` |
| Schedule parse error at startup | `error` | `app`, `view`, `schedule` |

Per-app logging routes to `log/apps/{app}.log` via `AppLogRouter` (same as REST/MCP).

---

## 10. Failure Semantics

### 10.1 Handler errors

A handler that throws or returns `{ status: 500 }` is logged at error level and increments `cron_failures_total`. **No automatic retry in v1.** If the handler wants retry, it retries internally — that gives the handler full control over backoff strategy and bounded attempts.

### 10.2 Dispatch errors

If `process_pool.dispatch` itself fails (engine missing, capability-wire failure, etc.), same as handler error: log, increment, move on. The cron loop is resilient — one failed tick does not stop the loop.

### 10.3 Schedule parse errors at startup

If a `schedule` expression doesn't parse, the cron loop for that view does not start. The error is logged with full context; other Cron views are unaffected. A startup error is fatal only if **every** Cron view fails to parse — that would imply a fundamentally broken bundle.

### 10.4 StorageEngine unreachable

If `set_if_absent` returns an error (not `Ok(false)`, but `Err(StorageError::...)`), we err on the side of skipping the tick — fail closed. Increment a `cron_storage_errors_total` counter, log at warn, continue.

---

## 11. Examples

### 11.1 Stale-signal recompute (CB's primary use case)

```toml
[api.views.recompute_signals]
view_type        = "Cron"
schedule         = "0 */5 * * * *"   # every 5 minutes
overlap_policy   = "skip"

[api.views.recompute_signals.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/signals.ts"
entrypoint = "recomputeAllProjects"
resources  = ["cb_db"]
```

```typescript
// libraries/handlers/signals.ts
export async function recomputeAllProjects(): Promise<void> {
    const projects = await Rivers.db.query("cb_db",
        "SELECT id FROM projects WHERE archived = false", []);
    for (const p of projects.rows) {
        await Rivers.db.execute("cb_db",
            "UPDATE signals SET evaluated_at = strftime('%s','now') WHERE project_id = $1",
            [p.id]);
    }
}
```

Active project: signals get fresh updates from inline event-driven recompute.
Quiet project: signals get refreshed every 5 minutes regardless. Catches deadline breaches and idle goals that no event would surface.

### 11.2 Hourly rollup

```toml
[api.views.rebuild_otel_rollups]
view_type        = "Cron"
interval_seconds = 3600
overlap_policy   = "skip"

[api.views.rebuild_otel_rollups.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/rollups.ts"
entrypoint = "rebuildHourly"
resources  = ["cb_db"]
```

### 11.3 Sprint completion sweep

```toml
[api.views.advance_completed_sprints]
view_type        = "Cron"
schedule         = "0 0 * * * *"     # every hour, on the hour
overlap_policy   = "skip"

[api.views.advance_completed_sprints.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/sprints.ts"
entrypoint = "advanceCompleted"
resources  = ["cb_db"]
```

### 11.4 Bursty work with bounded queue

```toml
[api.views.email_digest]
view_type        = "Cron"
interval_seconds = 60
overlap_policy   = "queue"
max_concurrent   = 4

[api.views.email_digest.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/digest.ts"
entrypoint = "sendDigests"
resources  = ["cb_db", "smtp_relay"]
```

If the digest job sometimes takes 3 minutes and the interval is 1 minute, queue-with-cap=4 absorbs bursts without unbounded backlog.

---

## 12. Non-goals (v1)

The following are **explicitly out of scope** for the v1 ship:

- **Catch-up semantics.** Missed ticks (node down, transient storage error) are gone. No replay queue.
- **Retry / dead-letter.** Handlers retry themselves if they want it.
- **`@hourly`/`@daily`-style cron aliases.** Use the explicit 6-field form.
- **Timezones.** All schedules are UTC. Operators wanting "9am Pacific" compute the UTC offset themselves.
- **Per-tick parameters.** The handler receives the same envelope every tick (empty body). If parameterization is needed, encode it in the handler logic or split into multiple Cron views.
- **Manual trigger.** No "fire this Cron view now" admin endpoint. Use a regular REST view that calls the same handler module if you need on-demand.
- **Cluster leadership.** No leader election; the per-tick `set_if_absent` race is the only coordination primitive. Adequate for the stated use cases.

These are tracked for future iterations; none block CB's v1 use case.
