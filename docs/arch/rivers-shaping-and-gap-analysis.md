# Rivers — Shaping & Gap Analysis

**Purpose:** Single source of truth for all spec conflicts, ambiguity resolutions, spec deviations, code rework, and remaining gap tasks. This document supersedes `gutter.md` and `rivers-spec-conflicts-and-ambiguities.md`.

**Scope:** Epics 1–37 (landed and in-progress code). RPS v2 excluded.

**Baseline:** 707 tests passing, 42 commits, 11 crates (4 core + 7 plugins), 37 epics attempted. ~160/210 tasks complete (76%).

---

## Part 1: Shaping Decisions (All Locked)

### Severity Key

| Tag | Meaning |
|-----|---------|
| **CONFLICT** | Two specs directly contradict |
| **AMBIGUITY** | Spec vague enough that two engineers diverge |

---

### SHAPE-1: Circuit Breaker — Windowed Everywhere

**Severity:** CONFLICT
**Specs:** Data Layer §5.2 (consecutive) vs HTTP Driver §9.2 (rolling window)
**Decision:** ACCEPTED

Windowed is the canonical algorithm everywhere. Add `window_ms` to data layer `CircuitBreakerConfig`. Rename `open_duration_ms` → `open_timeout_ms` for consistency.

```rust
pub struct CircuitBreakerConfig {
    pub enabled: bool,
    pub failure_threshold: u32,     // failures within window before OPEN
    pub window_ms: u64,             // rolling failure window (default: 60_000)
    pub open_timeout_ms: u64,       // time in OPEN before HALF_OPEN
    pub half_open_max_trials: u32,  // trial calls allowed in HALF_OPEN
}
```

---

### SHAPE-2: Error Envelope — Epic 37 Canonical

**Severity:** CONFLICT
**Specs:** HTTPD §18 vs Streaming REST vs Epic 37
**Decision:** ACCEPTED

Non-streaming errors use Epic 37's `ErrorResponse`:

```json
{
  "code": 500,
  "message": "human-readable error message",
  "details": "optional diagnostic info",
  "trace_id": "abc-123"
}
```

Streaming poison chunks remain wire-protocol-specific:

```json
{"error": "<message>", "error_type": "HandlerError", "stream_terminated": true}
```

---

### SHAPE-3: Cache Key — Canonical JSON Defined

**Severity:** CONFLICT
**Specs:** Data Layer §7, Polling Views, StorageEngine §5
**Decision:** ACCEPTED

One function, one algorithm:

1. Parameters into `BTreeMap<String, serde_json::Value>`
2. `serde_json::to_string()`
3. SHA-256 → hex-encode

| Consumer | Key format |
|----------|-----------|
| DataView L1/L2 cache | `cache:views:{view_name}:{param_hash}` |
| Polling loop | `poll:{view_name}:{param_hash}` |
| StorageEngine L2 | uses DataView cache key directly |

---

### SHAPE-4: Credential Redaction — Dropped

**Severity:** CONFLICT
**Specs:** Logging §8 vs Data Layer §6
**Decision:** DROPPED

Remove all string-scanning redaction. LockBox + capability model is the security boundary. Driver authors own their own error hygiene.

---

### SHAPE-5: LockBox Secrets — Never in Memory

**Severity:** CONFLICT
**Specs:** LockBox §8.4 vs Data Layer §5.3
**Decision:** MODIFIED

1. In-memory name/alias → entry index for O(1) lookup (no secret values)
2. Secret values read from disk, decrypted, used, zeroized on every access
3. `CredentialRotated` event eliminated entirely
4. `rivers lockbox rotate` writes to disk; next connection picks it up naturally
5. Pool drain/rebuild unnecessary — connections cycle via `max_lifetime` or health check failure
6. No restart required for credential rotation

---

### SHAPE-6: DriverError — Split Unsupported/NotImplemented

**Severity:** CONFLICT
**Specs:** Data Layer §2 vs Driver §3-4
**Decision:** ACCEPTED

```rust
pub enum DriverError {
    Unsupported(String),    // driver exists, operation not applicable
    NotImplemented(String), // stub placeholder, driver not yet wired
    // ... existing variants unchanged
}
```

All honest stubs switch to `NotImplemented`.

---

### SHAPE-7: Operation Inference — Defined Algorithm

**Severity:** CONFLICT
**Specs:** Data Layer §6 + Driver §2
**Decision:** ACCEPTED

1. Trim leading whitespace
2. Strip leading SQL comments (`--` to newline, `/* ... */`)
3. First whitespace-delimited token, lowercased
4. Map: `select|get|find|search` → Read, `insert|create|add|set|put` → Write, `update|patch|replace` → Write, `delete|remove|del` → Delete, else → Read
5. Explicit `operation` on DataView/Query wins over inference
6. Driver can override for its own syntax

---

### SHAPE-8: Multi-Node Detection — Redis Sentinel Key

**Severity:** CONFLICT
**Specs:** Auth/Session, Application, StorageEngine
**Decision:** MODIFIED

Without RPS, Rivers is single-node only. On startup with Redis backend:

1. Check for existing `rivers:node:*` keys
2. If found → hard failure: `"Another Rivers node detected on this Redis instance. Multi-node requires RPS."`
3. Write `rivers:node:{node_id}` with TTL heartbeat
4. Key expires naturally on crash/shutdown

---

### SHAPE-9: ProcessPool — Isolate Reuse with Application Context

**Severity:** AMBIGUITY
**Specs:** ProcessPool §Open Questions #2, Streaming REST §4
**Decision:** MODIFIED

Isolates pooled and reused. Application context is per-request:

- V8/WASM instances stay warm in pool
- Context created fresh per request (capability tokens, request data, trace ID)
- Context destroyed and zeroized after task completes
- Security boundary is the context, not the isolate
- Streaming handlers: long-lived context for stream duration
- Worker: pick up task → bind context → execute → unbind → return to pool

---

### SHAPE-10: V8/WASM API — Four-Scope Injection, No Snapshots

**Severity:** AMBIGUITY
**Specs:** ProcessPool §3
**Decision:** MODIFIED

No V8 snapshots. Pure injection model:

1. **Application (permanent):** `Rivers.*` APIs, app config, shared constants
2. **Session:** Per-user session variables, persist across requests for same session
3. **Connection:** Per WS/SSE connection, persist for connection lifetime
4. **Request:** Capability tokens, request data, caller context — ephemeral

Narrower scope shadows broader on name collision. REST: application + session + request. WS/SSE: all four.

---

### SHAPE-11: SSRF Prevention — Dropped

**Severity:** AMBIGUITY
**Specs:** ProcessPool §3
**Decision:** DROPPED

Capability model prevents SSRF by design. `Rivers.http.fetch` is token-gated to configured datasources. No runtime IP validation needed.

---

### SHAPE-12: Parallel Pipeline Stages — Dropped

**Severity:** AMBIGUITY
**Specs:** View Layer §4
**Decision:** DROPPED

Stages run sequentially in declaration order, always. `parallel = true` removed.

---

### SHAPE-13: WebSocket Binary Frames — Rate-Limited Logging

**Severity:** AMBIGUITY
**Specs:** View Layer §6
**Decision:** ACCEPTED

First binary frame per connection logged as WARN. Subsequent frames increment counter. Summary every 60s if count > 0. Reset after summary.

---

### SHAPE-14: Polling `emit_on_connect` — Iceboxed

**Severity:** AMBIGUITY
**Specs:** Polling Views §3, §10.2
**Decision:** ICEBOXED

Removed from v1. Validation rejects config key if present. Parked for future.

---

### SHAPE-15: `stream_terminated` Field Collision — Runtime Validation

**Severity:** AMBIGUITY
**Specs:** Streaming REST §5
**Decision:** ACCEPTED

If yielded JSON has top-level `stream_terminated` key: chunk blocked, poison chunk emitted, generator terminated, stream closed. Handler must strip/rename the key.

---

### SHAPE-16: Retry-After — Declared Per-Datasource

**Severity:** AMBIGUITY
**Specs:** HTTP Driver §9.1
**Decision:** MODIFIED

```toml
[data.datasources.openai.retry]
retry_after_format = "seconds"   # "seconds" | "http_date"
```

Parse only declared format. Failure → ignore, WARN log, normal backoff. Capped at `max_delay_ms`.

---

### SHAPE-17: EventBus Topic Validation — Dropped

**Severity:** AMBIGUITY
**Specs:** View Layer §11
**Decision:** DROPPED

EventBus is a dumb pipe. Any topic string, no registry validation.

---

### SHAPE-18: StorageEngine Queue Operations — Dropped

**Severity:** AMBIGUITY
**Specs:** StorageEngine §4
**Decision:** DROPPED

StorageEngine is pure KV: sessions, poll state, CSRF tokens, DataView L2 cache. `enqueue`/`dequeue`/`ack` removed. BrokerConsumerBridge goes broker → EventBus directly.

---

### SHAPE-19: Port Conflict Detection — Dropped

**Severity:** AMBIGUITY
**Specs:** Application §12
**Decision:** DROPPED

OS handles port conflicts at bind time. Preflight is for resource detection only.

---

### SHAPE-20: Polling `change_detect` Timeout — Diagnostic Events

**Severity:** AMBIGUITY
**Specs:** Polling Views §3
**Decision:** ACCEPTED

| Condition | Client | Diagnostic |
|-----------|--------|------------|
| returns `true` | Push | — |
| returns `false` | No push | — |
| throws | No push | `OnChangeFailed` event |
| times out | No push | `PollChangeDetectTimeout` event with `consecutive_timeouts` |

---

### SHAPE-21: TLS Mandatory — New `[base.tls]` Structure

**Severity:** CONFLICT
**Specs:** HTTPD §5 (TLS optional, under `[base.http2]`) vs. design session 2026-03-18
**Decision:** ACCEPTED

TLS is mandatory. `[base.tls]` absent at startup is a hard error. There is no plain HTTP main server path. TLS certificate paths move out of `Http2Config` and into a dedicated `TlsConfig` struct. `Http2Config` becomes a protocol-only toggle.

New config structure:
```toml
[base.tls]
cert          = "..."      # omit → auto-gen self-signed
key           = "..."
# redirect = false         # uncomment to disable HTTP :80 → HTTPS

[base.tls.x509]            # for auto-gen and riversctl tls gen/request
common_name = "localhost"
san         = ["localhost", "127.0.0.1"]
days        = 365

[base.tls.engine]          # cipher + min TLS version
min_version = "tls12"
ciphers     = []           # empty = rustls defaults
```

`Http2Config` retains only `enabled`, `initial_window_size`, `max_concurrent_streams`.

---

### SHAPE-22: HTTP Redirect Server — Port 80, Default On

**Severity:** AMBIGUITY (feature was missing entirely)
**Specs:** HTTPD §5 (no redirect server described)
**Decision:** ACCEPTED

Rivers spawns a second Axum listener on port 80 by default when `[base.tls]` is configured. All HTTP traffic → `301 Location: https://host:[base.port]{path}`. Port is always 80, not configurable. `redirect = false` in `[base.tls]` disables it. Port 80 bind failure → warning, not error. Internal services set `redirect = false` — they must not compete for port 80.

---

### SHAPE-23: CORS Config → Application Init Handler

**Severity:** CONFLICT
**Specs:** HTTPD §9 (`[security]` CORS fields, per-view `cors_enabled`) vs. Technology Path §10 (`app.cors()` in init handler)
**Decision:** Technology Path wins — ACCEPTED

CORS is an application concern configured in the init handler via `app.cors()`. Not per-view. Not in `[security]` server config. `SecurityConfig` does not contain CORS fields. See `rivers-httpd-spec.md §9` (updated) and `rivers-technology-path-spec.md §10`.

---

### SHAPE-24: Rate Limit Config → `[app.rate_limit]`

**Severity:** CONFLICT
**Specs:** HTTPD §10 (`[security]` global rate limit) vs. Technology Path §17 (`[app.rate_limit]` per-app)
**Decision:** Technology Path wins — ACCEPTED

Rate limiting is per-app, not per-server. Config lives in `[app.rate_limit]` in `app.toml`. Field names: `per_minute`, `burst_size`, `strategy`. Strategies: `ip` | `header` | `session` (not `Ip` | `CustomHeader`). `SecurityConfig` does not contain rate limit fields. Per-view override fields (`rate_limit_per_minute`, `rate_limit_burst_size`) remain on `ApiViewConfig` but reference `[app.rate_limit]` defaults.

---

## Part 2: Feature Deviation Register

Every spec line that now contradicts a shaping decision. Grouped by spec file for efficient amendment.

### `rivers-data-layer-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §5.2 `CircuitBreakerConfig` | `failure_threshold: u32, // consecutive failures before OPEN` | SHAPE-1 | Add `window_ms: u64`, change comment to "failures within window" |
| §5.2 state machine | `CLOSED →(failure_threshold consecutive failures)→ OPEN` | SHAPE-1 | Replace "consecutive" with "within window_ms" |
| §5.3 pool lifecycle | `On CredentialRotated event, the affected pool is drained...` | SHAPE-5 | Remove entire CredentialRotated paragraph |
| §6 error redaction | `strings containing "password", "token", "secret"... replaced with "sensitive details redacted"` | SHAPE-4 | Remove redaction paragraph |
| §2 DriverError | 6 variants, `Unsupported` serves dual role | SHAPE-6 | Add `NotImplemented(String)` variant |
| §6 operation inference | "Operation inferred from first token of statement (lowercased)" | SHAPE-7 | Replace with full algorithm (comment stripping, token map, driver override) |
| §7 cache key | `SHA-256(view_name + ":" + canonical_json_params)` | SHAPE-3 | Reference shared canonical JSON appendix, use `cache:views:{view_name}:{param_hash}` format |
| §10 BrokerConsumerBridge | `StorageEngine.enqueue("eventbus:{event_name}", payload)` | SHAPE-18 | Remove StorageEngine buffering, bridge goes broker → EventBus directly |
| §10 bridge drain | References dequeued messages and StorageEngine queue | SHAPE-18 | Remove StorageEngine queue references from drain logic |

### `rivers-http-driver-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §9.2 CircuitBreaker | `open_duration_ms = 30000` | SHAPE-1 | Rename to `open_timeout_ms` |
| §9.2 config comment | `failure_threshold = 5 # failures in window before opening` | SHAPE-1 | Already correct — just ensure field names match data layer |
| §9.1 Retry-After | "Rivers checks for Retry-After header and honors it if present" | SHAPE-16 | Add `retry_after_format` config attribute ("seconds" \| "http_date") |
| §14 examples | `parallel = true` on on_request stages | SHAPE-12 | Remove `parallel = true` from all examples |

### `rivers-httpd-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §18 error format | `{"error": "human-readable error message"}` | SHAPE-2 | Replace with Epic 37 envelope: `{code, message, details, trace_id}` |
| §5 inline errors | `Json(json!({"error": "server is shutting down"}))` | SHAPE-2 | Update inline error examples to use ErrorResponse envelope |
| §4 rate limit error | `{"error": "rate limit exceeded"}` | SHAPE-2 | Update to ErrorResponse envelope |
| §4 backpressure error | `{"error": "server overloaded; retry later"}` | SHAPE-2 | Update to ErrorResponse envelope |
| §15 topic registry step | `configure_topic_registry` in startup sequence | SHAPE-17 | Remove or demote to optional internal bookkeeping |
| §5 (entire) | TLS optional, under `[base.http2]` | SHAPE-21 | **DONE** — rewritten: `[base.tls]` mandatory, x509, engine, auto-gen, redirect server |
| §6 `Http2Config` | `tls_cert: Option<String>`, `tls_key: Option<String>` | SHAPE-21 | **DONE** — fields removed from struct |
| §2 startup | `validate_http2_runtime` step | SHAPE-21 | **DONE** — replaced with `validate_tls_config` + `maybe_autogen_tls_cert` |
| §9 (entire) | CORS in `[security]`, per-view `cors_enabled` | SHAPE-23 | **DONE** — rewritten: init handler only, `SecurityConfig` contains `admin_ip_allowlist` only |
| §10 (rate limit location) | `[security]` global rate limit fields | SHAPE-24 | **DONE** — updated: config at `[app.rate_limit]`, strategies renamed |
| §19.1 | `[base.http2]` with TLS cert fields | SHAPE-21 | **DONE** — config reference updated to `[base.tls]` + `[base.tls.x509]` + `[base.tls.engine]` |
| §19 (missing) | No `response_format` field documented | GAP-5 | Add `response_format = "envelope" \| "raw"` to view config reference |

### `rivers-streaming-rest-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §4.5 isolate reuse | "follow the same isolate reuse policy (Open Question #2)" | SHAPE-9 | Replace with: isolates reused, fresh context per request; streaming gets long-lived context |
| §5 stream_terminated | Behavior undefined | SHAPE-15 | Add: runtime validation, chunk blocked, poison emitted, generator killed |
| Poison chunk format | No note about relationship to standard error envelope | SHAPE-2 | Add note: poison chunks are wire-format-only, not standard ErrorResponse |

### `rivers-storage-engine-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §1 overview | "Queue — durable-enough message queue with dequeue-and-ack semantics" | SHAPE-18 | Remove queue from overview; StorageEngine is pure KV |
| §2 trait | `enqueue()`, `dequeue()`, `ack()` methods | SHAPE-18 | Remove all three methods from trait |
| §2.3 StoredMessage | `StoredMessage` struct definition | SHAPE-18 | Remove struct |
| §2.4 dequeue semantics | Entire subsection on dequeue/pending/ack | SHAPE-18 | Remove |
| §3.1 InMemory | `queues: Arc<Mutex<HashMap<String, VecDeque<StoredMessage>>>>` | SHAPE-18 | Remove queue field |
| §3.2 SQLite | `queue_store` table and indices | SHAPE-18 | Remove queue table/indices |
| §3.3 Redis | `XADD`/`XREADGROUP`/`XACK` queue ops | SHAPE-18 | Remove Streams queue ops |
| §5 cache key | Redundant cache key description | SHAPE-3 | Reference shared canonical JSON appendix |
| (new) | — | SHAPE-8 | Add sentinel key mechanism for single-node enforcement |

### `rivers-logging-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §8.1 | Error message redaction (DataViewEngine) — 4-keyword list | SHAPE-4 | Remove entire subsection |
| §8.2 | General text redaction (V8/WASM) — 15-keyword list | SHAPE-4 | Remove entire subsection |
| Event level table | `CredentialRotated — Warn` | SHAPE-5 | Remove row |

### `rivers-lockbox-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §3 overview | "riversd decrypts the keystore into host memory" | SHAPE-5 | Rewrite: index loaded to memory, secret values read from disk per-access |
| §5 resolution | "in-memory resolved keystore, O(1) via hash map" | SHAPE-5 | Rewrite: in-memory name/alias → entry index only; values fetched from disk |
| §7 startup | "build in-memory name+alias → value map" | SHAPE-5 | Change to "build in-memory name+alias → entry index" |
| §8.4 | "no hot-reload in v1, restart required" | SHAPE-5 | Rewrite: rotation writes to disk, no restart needed; next connection reads fresh value |
| §8.4 | "Plaintext TOML and Age-decrypted bytes zeroized after in-memory map is built" | SHAPE-5 | Rewrite: no plaintext map built; values decrypted per-access and immediately zeroized |

### `rivers-driver-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §2 DriverError | 6 variants | SHAPE-6 | Add `NotImplemented(String)` |
| §2 inference | "Operation inference from statement first token if not explicit" | SHAPE-7 | Add driver-override hook |
| §5 RPS driver | "On CredentialRotated events... pool drains and reconnects" | SHAPE-5 | Remove CredentialRotated reference |

### `rivers-application-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §12 preflight | "If two apps declare same port, second deploy fails with port conflict error" | SHAPE-19 | Remove port conflict from preflight |
| (new) | — | SHAPE-8 | Add startup gate: single-node enforcement without RPS |

### `rivers-processpool-runtime-spec-v2.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §1 workers | "No state crosses between worker executions. Isolate is reset or recycled via clean snapshot" | SHAPE-9, 10 | Rewrite: isolates reused, context is per-request; four-scope injection model |
| §3 V8 worker | "isolate is created from a clean snapshot... snapshot contains only River standard library stubs" | SHAPE-10 | Remove snapshot references; describe injection at isolate creation |
| §3 heap reset | "heap usage after reset exceeds max_heap_mb * 0.8, isolate destroyed and recreated from snapshot" | SHAPE-10 | Remove snapshot recreation; describe isolate recycling without snapshots |
| §5 startup | "Load V8 isolate snapshot (shared across workers)" | SHAPE-10 | Remove snapshot loading step |
| §3 SSRF | "post-DNS RFC 1918 validation" and SSRF references | SHAPE-11 | Remove SSRF validation; note capability model handles it |
| Open Question #1 | V8 snapshot content | SHAPE-10 | Close: no snapshots, injection model |
| Open Question #2 | Isolate-per-request vs reuse | SHAPE-9 | Close: reuse with fresh context |
| Open Question #4 | Shared V8 Heap Snapshot | SHAPE-10 | Close: no snapshots |
| Security table | "No SSRF from handlers... post-DNS RFC 1918 validation" | SHAPE-11 | Rewrite: capability model prevents SSRF; no IP validation |
| (new) | — | SHAPE-10 | Add four-scope variable model (application, session, connection, request) |

### `rivers-view-layer-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §4 parallel stages | "parallel = true... collected and executed via join_all" | SHAPE-12 | Remove parallel option; stages sequential only |
| §4 parallel example | pricing/inventory parallel stages | SHAPE-12 | Rewrite as sequential stages |
| §6 binary frames | "logged as warning, discarded" | SHAPE-13 | Replace with rate-limited logging rule |
| §11 topic validation | "Referenced topic must exist in TopicRegistry" | SHAPE-17 | Remove validation rule |
| §11.1 TopicRegistry | "Topics are registered at server startup from config" | SHAPE-17 | Demote to optional internal bookkeeping |
| §13 validation table | "on_event.topic not registered in TopicRegistry → unknown topic" | SHAPE-17 | Remove row |
| §14 examples | `parallel = true` in order detail example | SHAPE-12 | Remove `parallel = true` |

### `rivers-polling-views-spec.md`

| Line/Section | Current Text | Shaping | Required Change |
|---|---|---|---|
| §3.2 client join | "unless emit_on_connect = true is configured" | SHAPE-14 | Remove emit_on_connect reference |
| §10.1 config | `emit_on_connect = false` | SHAPE-14 | Remove config option |
| §10.2 | Entire `emit_on_connect` subsection | SHAPE-14 | Remove; add to icebox/future section |
| §11.1 example | `emit_on_connect = true` in price feed | SHAPE-14 | Remove line from example |
| §11.3 example | `emit_on_connect = true` in order status | SHAPE-14 | Remove line from example |
| §3 cache key | `poll:{view_name}:{sha256(canonical_json(sorted_params))}` | SHAPE-3 | Reference shared canonical JSON appendix |
| §3.5 error handling | No timeout diagnostic distinction | SHAPE-20 | Add `PollChangeDetectTimeout` event with `consecutive_timeouts` |

### New Shared Content

| Item | Shaping | Description |
|---|---|---|
| Appendix: Canonical JSON & Key Derivation | SHAPE-3 | BTreeMap ordering, serde_json serialization, SHA-256, hex-encode |

---

## Part 3: Code Rework Register

Completed tasks that need modification due to shaping decisions.

### Rework: Code Removal

| Epic.Task | Description | Shaping | Action |
|---|---|---|---|
| 5.8 | Redaction — 15-keyword `redact_sensitive_text` + 4-keyword DataView redaction | SHAPE-4 | Remove all redaction code |
| 6.7 | LockBox startup — in-memory credential resolution map | SHAPE-5 | Rework: index only in memory, values read from disk per-access |
| 14.6 | Pool Manager — `CredentialRotated` event handler (drain + rebuild) | SHAPE-5 | Remove event handler entirely |
| 30.7 | Polling — `emit_on_connect` flag in PollLoopState | SHAPE-14 | Remove flag and related broadcast-on-connect logic |
| 18.x | `Http2Config` — remove `tls_cert`, `tls_key` fields | SHAPE-21 | Remove TLS fields from `Http2Config`; TLS lives in `TlsConfig` |
| config.rs | `SecurityConfig` — remove all `cors_*` and `rate_limit_*` fields | SHAPE-23/24 | Keep only `admin_ip_allowlist`; CORS → init handler; rate limit → `AppRateLimitConfig` |

### Rework: Code Change

| Epic.Task | Description | Shaping | Action |
|---|---|---|---|
| 14.2 | CircuitBreakerConfig — consecutive failure counter | SHAPE-1 | Add `window_ms`, implement rolling window counter |
| 17.4 | DataView cache key — FNV-1a hash | SHAPE-3 | Replace with SHA-256 (sha2 crate already available from Epic 21) |
| 7.1 | DriverError enum — 6 variants | SHAPE-6 | Add `NotImplemented(String)` variant |
| 7.3 | Query operation inference — first token only | SHAPE-7 | Implement comment stripping, token mapping, safe default |
| 10.x | All built-in honest stubs | SHAPE-6 | Change `Unsupported` → `NotImplemented` |
| 11.x | All plugin honest stubs | SHAPE-6 | Change `Unsupported` → `NotImplemented` |
| 18.x | HTTPD inline error responses | SHAPE-2 | Update `json!({"error": "..."})` to use ErrorResponse envelope |
| 32.9 | Preflight checks — port conflict detection | SHAPE-19 | Remove port conflict check from preflight |
| bundle.rs | `AppStaticFilesConfig.root` → `root_path` | GAP-1 | Rename field; add `max_age: Option<u64>`; update conversion to `StaticFilesConfig` |
| app.toml | `address-book-main` static files | GAP-1 | Change `root = "libraries/spa"` → `root_path = "libraries/spa"` |

### Rework: New Code

| Area | Description | Shaping |
|---|---|---|
| StorageEngine Redis startup | Sentinel key `rivers:node:{node_id}` write + check for existing nodes | SHAPE-8 |
| StorageEngine trait | Remove `enqueue`/`dequeue`/`ack` from trait definition | SHAPE-18 |
| InMemoryStorageEngine | Remove `queues` field and queue implementation | SHAPE-18 |
| EventBus | Remove topic existence validation from `publish()` | SHAPE-17 |
| View validation | Remove "topic must exist in TopicRegistry" rule | SHAPE-17 |
| Streaming REST validation | Add runtime `stream_terminated` key check on yielded chunks | SHAPE-15 |
| WebSocket handler | Add per-connection binary frame counter + 60s summary logging | SHAPE-13 |

---

## Part 4: Gap Analysis — Open Tasks

Updated from gutter.md with shaping impacts applied. Struck tasks are eliminated by shaping decisions.

### Category 1: External Crate Dependencies

| Epic | Task | Blocker Crate | Description | Shaping Impact |
|------|------|---------------|-------------|----------------|
| 4.4 | SQLite StorageEngine | `sqlx` | WAL-mode SQLite backend — KV only | SHAPE-18: no queue tables |
| 4.5 | Redis StorageEngine | `redis` | Redis SET EX for KV | SHAPE-18: no Streams queue; SHAPE-8: add sentinel key |
| 12.10 | reqwest HTTP execution | `reqwest` | Live HTTP driver execution | — |
| 18.4 | TLS via rustls | `rustls`, `tokio-rustls` | TLS termination via `tokio_rustls::TlsAcceptor` | SHAPE-21 |
| 18.NEW | TLS cert auto-gen | `rcgen` | Generate self-signed cert at startup when `cert`/`key` absent in `[base.tls]` | SHAPE-21 |
| 24.3 | V8 worker | `v8` / `rusty_v8` | V8 isolate pool | SHAPE-9: reuse model; SHAPE-10: injection not snapshot |
| 24.4 | V8 context reset | `v8` | Reset V8 context between executions | SHAPE-9: context unbind, not isolate reset |
| 24.5 | Wasmtime worker | `wasmtime` | WASM instance pool | SHAPE-9: same reuse model |
| 24.6 | V8 preemption | `v8` | CPU time limits | — |
| 24.7 | Wasmtime preemption | `wasmtime` | Fuel-based limits | — |
| 24.11 | TypeScript via swc | `swc` | TS → JS compilation | — |
| 31.9 | Ed25519 admin auth | `ed25519-dalek` | Signature verification | — |
| 34.1 | async-graphql integration | `async-graphql` | Axum router integration | — |
| 35.2 | File watcher | `notify` | Config file change detection | — |

**Total: 14 tasks** (added 18.NEW rcgen)

---

### Category 2: Requires CodeComponent / ProcessPool Engine

| Epic | Task | Description | Shaping Impact |
|------|------|-------------|----------------|
| 23.2 | Guard CodeComponent contract | Guard view credential validation | — |
| 23.5 | Guard on_failed handler | Guard failure dispatch | — |
| 24.9 | Worker crash recovery | Restart crashed workers | — |
| 24.12 | Rivers.crypto API | Crypto in CodeComponent | SHAPE-10: part of application scope |
| ~~25.7~~ | ~~Parallel pipeline stages~~ | ~~join_all for parallel = true~~ | **SHAPE-12: REMOVED** |
| 25.8 | on_error / on_timeout stages | Error/timeout observers | — |
| 25.10 | on_session_valid stage | Session validation positioning | — |
| 26.4 | WS on_stream handler | Inbound WS message handling | — |
| 26.7 | WS lag handling | Live WS lag detection | — |
| 27.3 | SSE hybrid push loop | tokio::select! tick + EventBus | — |
| 28.2 | MessageConsumer EventBus sub | EventBus subscription | — |
| 28.3 | MessageConsumer handler dispatch | Event → CodeComponent | — |
| 29.8 | Streaming generator loop | Drive async generator | SHAPE-9: long-lived context |
| 29.9 | Client disconnect detection | Dropped HTTP connections | — |
| 29.10 | Rivers.view.stream() | ProcessPool streaming API | — |
| 30.5 | change_detect diff strategy | CodeComponent diff | SHAPE-20: add diagnostic events |
| 34.4 | GraphQL mutation resolvers | Mutations via CodeComponent | — |

**Total: 16 tasks** (was 17 — SHAPE-12 removed 25.7)

---

### Category 3: Wiring / Integration (no new deps)

| Epic | Task | Description | Shaping Impact |
|------|------|-------------|----------------|
| 5.5 | Trace ID propagation | Request extensions → response headers → EventBus | — |
| 5.6 | Local file logging | Optional append-mode file output | — |
| 6.8 | LockBox CLI subcommands | init/add/list/show/alias/rotate/remove/rekey/validate | SHAPE-5: rotate writes disk only |
| 9.7 | EventBus driver events | DriverRegistered / PluginLoadFailed | — |
| 10.7 | EventBusDriver | Built-in driver via EventBus | SHAPE-17: no topic validation needed |
| 23.3 | Guard valid session behavior | Redirect on valid session in guard view | — |
| 23.4 | Guard invalid session behavior | Redirect on invalid session | — |
| 23.7 | Auto invalid session redirect | Automatic redirect on session failure | — |
| 25.9 | Null datasource pattern | `datasource = "none"` | — |
| 30.2 | Poll tick execution | DataView → diff → broadcast loop | SHAPE-14: no emit_on_connect |
| 30.8 | StorageEngine poll persistence | Previous poll state storage | — |
| 31.2 | Admin localhost enforcement | Bind admin to localhost when no public_key | — |
| 32.6 | Health check backoff | Exponential backoff during deployment | — |
| 32.7 | Zero-downtime redeployment | Live server orchestration | — |
| 32.8 | Auth scope carry-over | Inter-service auth | — |
| 33.5 | Health endpoint wiring | Wire types into server handlers | — |
| 36.3 | CLI → main.rs wiring | Config loading, log level, dispatch | — |
| 37.4 | CORS on error responses | CORS headers on ErrorResponse | SHAPE-2: use new envelope |
| 18.5 | TLS config structs | Add `TlsConfig`, `TlsX509Config`, `TlsEngineConfig` to `config.rs`; remove TLS fields from `Http2Config`; remove CORS/rate-limit fields from `SecurityConfig` | SHAPE-21/23/24 |
| 18.6 | TLS startup (`maybe_autogen_tls_cert`) | On startup: check `[base.tls]` for `cert`/`key`; if absent call `rcgen` to generate self-signed cert using `[base.tls.x509]` fields; write to disk | SHAPE-21 |
| 18.7 | HTTP redirect server (`maybe_spawn_http_redirect_server`) | Bind plain HTTP listener on port 80; issue 301 → `https://{host}:[base.port]{path}`; skip if `redirect = false` | SHAPE-22 |
| 36.6 | `riversctl tls` subcommands | Implement `gen`, `request`, `import`, `show`, `list`, `expire` — `show` prints cert details + "X days left"; `expire` purges only | SHAPE-21 |
| AB.1 | Address-book bundle TLS config | Update both `manifest.toml` `entryPoint` to `https://`; add `[base.tls]` + `[base.tls.x509]` to both `app.toml` files; add `skip_verify = true` to `address-book-main` datasource; set `redirect = false` on `address-book-service` | SHAPE-21/22 |

**Total: 23 tasks** (added 18.5, 18.6, 18.7, 36.6, AB.1)

---

### Category 4: Separate Binaries (v2)

| Epic | Task | Description |
|------|------|-------------|
| 36.4 | `riversctl` admin client | Ed25519 request signing |
| 36.5 | `riverpackage` bundle validator | Bundle validation tool |

**Total: 2 tasks**

---

### Category 5: Deferred Epics (v2+)

| Epic | Description |
|------|-------------|
| D1 | RPS — Rivers Provisioning Service |
| D2 | Clustering — Raft, Gossip, Multi-Node |

---

### Icebox

| Item | Origin | Notes |
|------|--------|-------|
| `emit_on_connect` | SHAPE-14 | Concept sound, not ready for v1. Revisit when polling views are wired end-to-end. |

---

## Part 5: Summary

### Task Counts

| Category | Open | Shaping Removed | Net |
|----------|------|-----------------|-----|
| 1. External crate deps | 14 | 0 | 14 |
| 2. Blocked on CodeComponent | 17 | 1 (25.7) | 16 |
| 3. Wiring / integration | 23 | 0 | 23 |
| 4. Separate binaries | 2 | 0 | 2 |
| **Subtotal open tasks** | **56** | **1** | **55** |
| Code rework (Part 3) | — | — | **23** |
| Spec amendments (Part 2) | — | — | **~65 line items across 12 specs** |

### Recommended Execution Order

**Phase A — Shaping compliance (do first, unblocks everything)**

1. Apply code rework (Part 3) — remove redaction, remove queue ops, fix circuit breaker, fix cache keys, split DriverError, update error envelope
2. Apply spec amendments (Part 2) — one spec at a time using the deviation register

**Phase B — Wiring (Category 3, no new deps)**

1. 36.3 CLI → main.rs wiring (makes `riversd serve` functional)
2. 5.5 + 5.6 trace ID propagation + file logging
3. 33.5 health endpoint wiring
4. 37.4 CORS on error responses (now using SHAPE-2 envelope)

**Phase C — External deps (Category 1, highest impact first)**

1. `rustls` + `tokio-rustls` + `rcgen` → TLS termination + cert auto-gen (18.4, 18.NEW)
2. `reqwest` → HTTP driver execution (12.10)
3. `notify` → hot reload file watcher (35.2)
4. `sqlx` + `redis` → StorageEngine backends (4.4, 4.5) — simplified by SHAPE-18
5. ProcessPool engine (v8/wasmtime) → unblocks all 16 Category 2 tasks

**Phase D — TLS wiring (requires Phase C rustls/rcgen)**

1. 18.5 TLS config structs — add `TlsConfig`, `TlsX509Config`, `TlsEngineConfig`; remove stale fields
2. 18.6 `maybe_autogen_tls_cert` — startup auto-gen using `rcgen`
3. 18.7 `maybe_spawn_http_redirect_server` — port 80 → 301 redirect
4. 36.6 `riversctl tls` subcommands — gen / request / import / show / list / expire
5. AB.1 Address-book bundle — update manifests + app.toml TLS config + `skip_verify`
