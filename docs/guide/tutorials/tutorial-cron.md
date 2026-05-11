# Tutorial — Cron Views (Scheduled Tasks)

Cron views fire on a schedule with no client connected. Use them for
periodic recompute, idle aging, scheduled rollups, and any background
work you'd otherwise reach for OS cron to drive.

**Spec:** [`rivers-cron-view-spec.md`](../../arch/rivers-cron-view-spec.md)
**Requires:** Rivers v0.61.0+, `[storage_engine]` configured.

---

## When to use

Reach for a Cron view when:

- The work is time-driven, not event-driven. ("Recompute signals every
  5 minutes" — yes. "Recompute when a telemetry event lands" — that's a
  MessageConsumer or REST handler.)
- The work should run regardless of whether anyone is watching.
- You need exactly-once-ish semantics across a multi-node cluster.

Don't use a Cron view when:

- A client connection drives the cadence — that's polling on an SSE or
  WebSocket view.
- You need exact, replayable semantics across node failures — Cron in v1
  has no catch-up replay, missed ticks are gone.

---

## Minimal example — every 5 minutes

```toml
# app.toml

[api.views.recompute_signals]
view_type        = "Cron"
schedule         = "0 */5 * * * *"   # 6-field: sec min hour dom month dow
overlap_policy   = "skip"            # default — drop tick if previous still running

[api.views.recompute_signals.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/signals.ts"
entrypoint = "recomputeAllProjects"
resources  = ["app_db"]
```

```typescript
// libraries/handlers/signals.ts

export async function recomputeAllProjects(): Promise<void> {
    const projects = await Rivers.db.query(
        "app_db",
        "SELECT id FROM projects WHERE archived = false",
        []
    );
    for (const p of projects.rows) {
        await Rivers.db.execute(
            "app_db",
            "UPDATE signals SET evaluated_at = strftime('%s','now') WHERE project_id = $1",
            [p.id]
        );
    }
}
```

Schedule fires every 5 minutes. Handler executes in the same environment
as REST handlers — `Rivers.db.*` capability propagation works through.

---

## Configuration reference

| Field | Required? | Notes |
|---|---|---|
| `view_type` | yes | `"Cron"` |
| `handler` | yes | `type = "codecomponent"` (only) |
| `schedule` *or* `interval_seconds` | exactly one | Mutually exclusive |
| `overlap_policy` | optional | `"skip"` (default) \| `"queue"` \| `"allow"` |
| `max_concurrent` | optional | Bound for `overlap_policy = "queue"` (default 16) |

**Forbidden on Cron views** (rejected with `S005` at validation):
`path`, `method`, `auth`, `guard_view`, `response_headers`, `polling`,
all MCP fields, all WS/SSE-specific fields. Cron views have no caller —
none of those have semantic meaning.

---

## Schedule formats

### 6-field cron expression

`schedule = "sec min hour day-of-month month day-of-week"`. Always six
fields; the leading seconds field is required. Five-field POSIX cron is
rejected at validation.

| Expression | Meaning |
|---|---|
| `0 */5 * * * *` | Every 5 minutes (at second 0) |
| `*/30 * * * * *` | Every 30 seconds |
| `0 0 */1 * * *` | Every hour, on the hour |
| `0 0 9 * * MON-FRI` | 9:00 AM weekdays |
| `0 0 0 1 * *` | Midnight on the 1st of every month |

All schedules are UTC.

### Fixed interval

`interval_seconds = N` for a plain integer interval. Computed from the
loop's start time, not the wall clock.

```toml
[api.views.heartbeat]
view_type        = "Cron"
interval_seconds = 60
```

---

## Overlap policies

### `skip` (default)

If the previous tick is still running when the next fires, the new tick
is dropped. Best for handlers that occasionally exceed their interval —
the next normal interval recovers automatically.

### `queue`

Bounded `mpsc` queue (capacity = `max_concurrent`, default 16). Pushes
ticks for sequential dispatch. When the queue is full, drops + metrics.

```toml
overlap_policy = "queue"
max_concurrent = 4
```

### `allow`

Spawn unconditionally — concurrent ticks may execute simultaneously.
Caller's responsibility to be safe.

---

## Multi-instance dedupe

When you run riversd on multiple nodes against the same StorageEngine
(typically Redis), only **one node** fires each tick:

- Each tick computes a key `cron:{app}:{view}:{tick_epoch}`.
- The first node to write the key (`set_if_absent`) wins.
- Other nodes get "key already exists" and skip — metric: `rivers_cron_skipped_dedupe_total`.

For this to work, configure a shared StorageEngine backend:

```toml
# riversd.toml
[storage_engine]
backend = "redis"
url     = "redis://redis.internal:6379"
```

`in_memory` and `sqlite` backends are node-local — running cron views on
multiple nodes against those backends will fire every tick on every node.
The validator emits a startup warning when this configuration is detected.

---

## Observability

### Metrics (with the `metrics` feature)

| Metric | Type | Labels |
|---|---|---|
| `rivers_cron_runs_total` | counter | `app`, `view` |
| `rivers_cron_failures_total` | counter | `app`, `view` |
| `rivers_cron_skipped_overlap_total` | counter | `app`, `view` |
| `rivers_cron_skipped_dedupe_total` | counter | `app`, `view` |
| `rivers_cron_dropped_queue_full_total` | counter | `app`, `view` |
| `rivers_cron_storage_errors_total` | counter | `app`, `view` |
| `rivers_cron_duration_ms` | histogram | `app`, `view` |

### Logs

- Tick fired: `debug` level, target `rivers.cron`, includes `dispatch_latency_ms`.
- Tick skipped (dedupe / overlap): `debug`.
- Handler error: `error`, includes the error message and duration.
- Schedule parse error at startup: `error`, the bad view's loop does not
  start (other Cron views unaffected).

Per-app log routing follows the existing `AppLogRouter` rules — handler
output goes to `log/apps/{app}.log`.

---

## Failure handling

- A handler that throws or returns non-OK is logged at `error` and bumps
  `rivers_cron_failures_total`. **No automatic retry in v1.** If you need
  retry, do it inside the handler.
- A storage error during dedupe (`set_if_absent` failed) treats the tick
  as skipped (fail closed). `rivers_cron_storage_errors_total` increments.
- Schedule parse failure at startup: that view's loop does not start.
  Others continue.

---

## What v1 does NOT include

- **Catch-up.** Missed ticks are gone — no replay queue.
- **Retry / dead-letter.** Handler retries are caller's responsibility.
- **Timezones.** All schedules are UTC.
- **Per-tick parameters.** Every tick gets the same empty envelope.
- **Manual trigger.** No "fire this Cron view now" admin endpoint.
- **5-field POSIX cron** — always include the seconds field.
- **`@hourly`, `@daily` aliases** — use the explicit 6-field form.

These are tracked for future iterations.

---

## Related

- [`rivers-cron-view-spec.md`](../../arch/rivers-cron-view-spec.md) — full spec
- [`rivers-polling-views-spec.md`](../../arch/rivers-polling-views-spec.md) — for client-driven tick loops
- [`rivers-storage-engine-spec.md`](../../arch/rivers-storage-engine-spec.md) — the StorageEngine backing dedupe
