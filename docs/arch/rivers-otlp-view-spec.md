# Rivers OTLP View Specification

**Document Type:** New spec
**Scope:** `view_type = "OTLP"` — declarative OpenTelemetry OTLP/HTTP endpoint with framework-owned wire-format handling
**Status:** Design / Pre-Implementation
**Patches:** `rivers-view-layer-spec.md`, `rivers-feature-inventory.md`, `rivers-httpd-spec.md`
**Source ask:** CB OTLP feature request bundle (`cb-rivers-otlp-feature-request.zip`, filed 2026-05-11)
**Depends on:** P1.6 protobuf transcoder (shipped — [otlp_transcoder.rs](../../crates/riversd/src/otlp_transcoder.rs)). Bearer-token auth uses the P1.10 `guard_view` pattern (see §8) — there is no dependency on a `auth = "bearer"` mode.

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [Mental Model](#2-mental-model)
3. [View Declaration](#3-view-declaration)
4. [Wire Protocol Handling](#4-wire-protocol-handling)
5. [Path Routing](#5-path-routing)
6. [Handler Dispatch Envelope](#6-handler-dispatch-envelope)
7. [Response Shape](#7-response-shape)
8. [Auth](#8-auth)
9. [Validation Rules](#9-validation-rules)
10. [Configuration Reference](#10-configuration-reference)
11. [Observability](#11-observability)
12. [Examples](#12-examples)
13. [Non-goals (v1)](#13-non-goals-v1)
14. [Implementation Notes](#14-implementation-notes)

---

## 1. Design Rationale

### 1.1 The problem

OpenTelemetry's OTLP/HTTP protocol is the industry standard for telemetry ingestion (metrics + logs + traces). Today, Rivers operators who want to receive OTLP have to stand up three near-identical `view_type = "Rest"` views — one per signal type — and hand-decode the OTLP envelope inside a codecomponent handler. The hand-rolled path works for `application/json` with no compression but fails the moment a real OTLP client appears with `application/x-protobuf` or `Content-Encoding: gzip` — i.e., production defaults for OTel SDKs and the OTel Collector.

A native `view_type = "OTLP"` collapses the workaround into one declarative block, surfaces existing capabilities the framework already has, and adds the few missing wire-format pieces (gzip, partial-success response).

### 1.2 What's already there — the implementation lever

Rivers v0.61.1 already ships an OTLP protobuf transcoder ([crates/riversd/src/otlp_transcoder.rs](../../crates/riversd/src/otlp_transcoder.rs), 77 lines), wired into [view_dispatch.rs](../../crates/riversd/src/server/view_dispatch.rs) at the body-extraction stage. When a REST request arrives with `Content-Type: application/x-protobuf` and a path ending in `/v1/{traces,metrics,logs}`, the framework decodes the prost message and re-encodes as JSON before handing off to the handler.

This means the asked feature is **not** a new wire-format parser. It is:

- a declarative opt-in (`view_type = "OTLP"`) to the existing transcoder,
- gzip/deflate decompression (new, but ~30 lines via `flate2`),
- path-based per-signal dispatch (small router on top of existing handler config),
- partial-success response envelope (output transform, ~20 lines).

The cost calculus favors implementing it cleanly rather than continuing to ask every handler to reinvent the same boilerplate.

### 1.3 Ownership boundary

- **Framework MUST** — content-type negotiation between JSON and protobuf, gzip/deflate decompression, path-based dispatch to per-signal handlers, partial-success response wrapping, body-size enforcement.
- **Developer MUST** — per-signal handler bodies (or one handler with a `kind` discriminator), declaring `auth = "none"` (the only accepted value — bearer-style auth uses `guard_view`, see §8), declaring resources.
- **Framework MUST NOT** — interpret payload semantics beyond decoding the envelope, persist telemetry on the developer's behalf, transform individual data points.

### 1.4 Precedent in the codebase

Same architectural level as `view_type = "Mcp"` (JSON-RPC envelope + tool dispatch), `view_type = "ServerSentEvents"` (SSE framing + tick scheduler), and `view_type = "Cron"` (tick scheduler + no client). The framework owns the protocol; the developer owns the per-row business logic.

### 1.5 Key decisions

| Decision | Choice | Rationale |
|---|---|---|
| Wire formats | JSON + protobuf (transcoded to JSON before dispatch) | Reuses P1.6 transcoder; handler stays JSON-uniform |
| Compression | gzip + deflate (auto-decompressed before transcode) | Matches OTel SDK defaults |
| Per-signal dispatch | `handlers.{metrics,logs,traces}` table OR single `handler` with `ctx.otel.kind` discriminator | First is more declarative; second is simpler for one-handler bundles |
| Response on success | `200 {}` | OTLP spec — empty success envelope |
| Response on partial success | `200 {partialSuccess: {rejectedDataPoints, errorMessage}}` | OTLP spec — partial success is still a 200 |
| Response on hard failure | `4xx`/`5xx` with `application/json` `{error: "..."}` | Matches existing REST error responses |
| Streaming | Forbidden | OTLP/HTTP is unary; rejected at validation (X-OTLP-4) |
| Path | Operator declares root prefix (e.g., `path = "otel"`), framework mounts `/v1/{metrics,logs,traces}` underneath | Avoids fragile per-signal `path` repetition |

---

## 2. Mental Model

```
Inbound POST /otel/v1/metrics
    │
    ▼
┌─────────────────────────────────────────────┐
│ OTLP view dispatcher (framework-owned)      │
│                                             │
│  1. Body size check vs max_body_mb          │
│  2. Content-Encoding: gunzip / inflate      │
│  3. Content-Type:                            │
│       application/x-protobuf → P1.6         │
│         transcoder → JSON                   │
│       application/json → pass through       │
│  4. Path tail (/v1/metrics, /v1/logs,       │
│     /v1/traces) → pick handler              │
│  5. Build ctx.otel = {kind, payload, ...}   │
└─────────────────────────────────────────────┘
    │
    ▼
  Dispatch handler (codecomponent)
    │
  Same capability propagation as REST/MCP/Cron
    │
    ▼
  Handler returns; framework reads
  ctx.otel.rejected + ctx.otel.errorMessage
    │
    ▼
  Framework emits:
    200 {}                                  (success)
    200 {partialSuccess: {...}}             (partial)
    4xx/5xx {error: "..."}                  (hard fail)
```

---

## 3. View Declaration

### 3.1 Multi-handler form (recommended)

```toml
[api.views.otel_ingest]
path         = "otel"            # framework mounts /otel/v1/{metrics,logs,traces}
view_type    = "OTLP"
auth         = "none"            # the only accepted value (see §8)
# guard_view = "bearer_check"    # optional — bearer-token preflight
max_body_mb  = 4                 # OTLP spec default; optional, defaults below

[api.views.otel_ingest.handlers.metrics]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestMetrics"
resources  = ["cb_db"]

[api.views.otel_ingest.handlers.logs]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestLogs"
resources  = ["cb_db"]

[api.views.otel_ingest.handlers.traces]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestTraces"
resources  = ["cb_db"]
```

Partial declarations are allowed: an operator may declare only `handlers.metrics` to expose `/v1/metrics` and have `/v1/logs` and `/v1/traces` return `404`. The validator does **not** require all three signals — only that at least one is declared (X-OTLP-2).

### 3.2 Single-handler form

```toml
[api.views.otel_ingest]
path         = "otel"
view_type    = "OTLP"
auth         = "none"

[api.views.otel_ingest.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestAny"
resources  = ["cb_db"]
```

The same handler receives all three signal types with `ctx.otel.kind ∈ {"metrics", "logs", "traces"}` set by the framework. This is simpler when the per-signal logic is small or shares most code.

`handler` and `handlers.*` are mutually exclusive — the validator rejects both at once (X-OTLP-5).

### 3.3 Required fields

| Field | Required? | Default | Notes |
|---|---|---|---|
| `view_type` | yes | — | Must be `"OTLP"` |
| `path` | yes | — | Root prefix; framework mounts `/v1/{metrics,logs,traces}` underneath |
| `handlers.metrics` ∨ `handlers.logs` ∨ `handlers.traces` ∨ `handler` | exactly one form, at least one signal | — | Per-signal table or single discriminator handler |
| `auth` | optional | `"none"` | Must be `"none"` — bearer-style auth uses `guard_view` (see §8) |
| `guard_view` | optional | — | Name of another view in the same app whose codecomponent runs as a preflight (P1.10); the canonical way to do bearer/HMAC auth on OTLP views |
| `max_body_mb` | optional | `4` | Per the OTLP/HTTP spec recommendation |

### 3.4 Forbidden fields

| Field | Reason | Code |
|---|---|---|
| `method` | OTLP/HTTP is POST-only — framework hard-codes | X-OTLP-6 |
| `streaming` / `streaming_format` / `stream_timeout_ms` | OTLP/HTTP is unary | X-OTLP-4 |
| `polling`, `tick_interval_seconds` | No client subscription model | X-OTLP-6 |
| `websocket_mode`, `max_connections` | Not a WS view | X-OTLP-6 |
| `tools`, `resources`, `prompts`, `instructions` | MCP-only fields | X-OTLP-6 |
| `schedule`, `interval_seconds`, `overlap_policy` | Cron-only fields | X-OTLP-6 |
| `auth = "session"` | OTLP clients are stateless | X-OTLP-3 |

---

## 4. Wire Protocol Handling

### 4.1 Content-Type negotiation

| Inbound `Content-Type` | Framework behavior |
|---|---|
| `application/json` (or `application/json; charset=utf-8`) | Body parsed as JSON; passed through unchanged |
| `application/x-protobuf` | Body fed to existing P1.6 `transcode_otlp_protobuf` → JSON |
| Other / missing | `415 Unsupported Media Type`, body `{error: "OTLP requires application/json or application/x-protobuf"}` |

The transcoder already lives in `crates/riversd/src/otlp_transcoder.rs` and matches on the path tail (`/v1/traces|metrics|logs`). The OTLP view reuses it directly — no new prost types are introduced.

### 4.2 Content-Encoding (compression)

Inspected before content-type processing:

| Inbound `Content-Encoding` | Framework behavior |
|---|---|
| absent / `identity` | Body used as-is |
| `gzip` | Decoded via `flate2::read::GzDecoder` |
| `deflate` | Decoded via `flate2::read::DeflateDecoder` |
| Other (e.g., `br`, `zstd`) | `415`, body `{error: "OTLP Content-Encoding '<x>' not supported"}` |

Decompression is bounded: the decoded body is capped at `max_body_mb * 1.5` to prevent zip-bomb amplification. Exceeding the cap returns `413 Payload Too Large`.

### 4.3 Body size enforcement

Pre-decompression: reject `> max_body_mb` with `413`. Post-decompression: reject `> max_body_mb * 1.5` with `413` (amplification guard). Default `max_body_mb = 4` matches the OTLP/HTTP recommendation.

### 4.4 Order of operations

```
Inbound bytes
    │
    ▼
Size pre-check (max_body_mb)        → 413 on fail
    │
    ▼
Decompress (gzip/deflate)           → 415 on unknown encoding
    │
    ▼
Decompressed-size check (×1.5 cap)  → 413 on fail
    │
    ▼
Decode by Content-Type:
    application/x-protobuf → P1.6 transcoder → 415 on prost decode error
    application/json       → serde_json::from_slice → 400 on parse error
    other                  → 415
    │
    ▼
JSON value → dispatcher
```

---

## 5. Path Routing

The view's declared `path` is the root prefix. The framework mounts the three OTLP signal endpoints underneath:

| View `path` | Mounted endpoints |
|---|---|
| `"otel"` | `/otel/v1/metrics`, `/otel/v1/logs`, `/otel/v1/traces` |
| `"telemetry/otel"` | `/telemetry/otel/v1/metrics`, etc. |
| `"/"` (root) | `/v1/metrics`, `/v1/logs`, `/v1/traces` |

For each signal:

- If the corresponding `handlers.<signal>` block is declared, the framework dispatches to it.
- If only the single `handler` form is declared, the framework dispatches there with `ctx.otel.kind = "<signal>"`.
- If neither matches (e.g., `handlers.metrics` and `handlers.logs` are declared but a request hits `/v1/traces`), respond `404` with `{error: "OTLP signal 'traces' not configured on this view"}`.

The validator rejects (`X-OTLP-1`) any `path` that itself ends in `/v1/metrics`, `/v1/logs`, or `/v1/traces` — that pattern implies the operator is trying to mount one OTLP path under a non-OTLP view type, which is exactly what `view_type = "OTLP"` exists to replace.

---

## 6. Handler Dispatch Envelope

### 6.1 Inbound context

Handlers receive the standard `ctx` shape plus an `otel` field:

```json
{
  "request": {
    "method":  "POST",
    "path":    "/otel/v1/metrics",
    "headers": { "content-type": "...", "...": "..." },
    "body":    { "...": "..." },
    "path_params": {},
    "query":   {}
  },
  "session": null,
  "otel": {
    "kind":     "metrics",
    "payload":  { "resourceMetrics": [ /* decoded shape */ ] },
    "encoding": "json"
  }
}
```

- `ctx.otel.kind` — one of `"metrics"`, `"logs"`, `"traces"`. Set by the framework from the matched signal path.
- `ctx.otel.payload` — the decoded OTLP envelope. Always JSON-shaped (protobuf inputs are transcoded first). For metrics it's `ExportMetricsServiceRequest`, for logs `ExportLogsServiceRequest`, for traces `ExportTraceServiceRequest`. Field naming matches the prost-derived JSON (camelCase keys per the canonical OTLP JSON encoding).
- `ctx.otel.encoding` — `"json"` if the inbound `Content-Type` was JSON; `"protobuf"` if it was transcoded. Useful for diagnostics and metrics; handlers usually ignore it.
- `ctx.request.body` is **also** set to `ctx.otel.payload` for handler-code compatibility with REST views.
- `ctx.session` is `null` for OTLP views — they don't carry sessions. A `guard_view` codecomponent (see §8) can populate principal info on `ctx.request.headers` or via the named-guard envelope before the OTLP handler dispatches.

### 6.2 Outbound context (what the handler can set)

Handlers may set the following on `ctx.otel` before returning:

| Field | Type | Default | Effect |
|---|---|---|---|
| `rejected` | integer ≥ 0 | `0` | Count of rejected points/spans/log records |
| `errorMessage` | string | `""` | Human-readable reason for the rejection |

If `rejected > 0` the framework emits a partial-success body (§7.2). If `rejected == 0` (or the field is absent) the framework emits an empty success body (§7.1). Handlers that throw or return an error response trigger a hard failure (§7.3) — `partialSuccess` is only meaningful when the request itself was well-formed.

The handler does **not** set HTTP status, response headers, or response body. The framework owns those for OTLP per spec compliance.

---

## 7. Response Shape

### 7.1 Success

```
HTTP/1.1 200 OK
Content-Type: application/json

{}
```

Per OTLP/HTTP spec: empty success envelope. The framework emits the same response for JSON and protobuf requests — protobuf clients receive JSON responses by default in v1. (See §13 non-goals for protobuf-response negotiation.)

### 7.2 Partial success

When `ctx.otel.rejected > 0`:

```
HTTP/1.1 200 OK
Content-Type: application/json

{
  "partialSuccess": {
    "rejectedDataPoints": 3,
    "errorMessage": "decision_id missing on tool_use_id 'abc'"
  }
}
```

The field name varies by signal kind:

| Signal | Field name |
|---|---|
| metrics | `rejectedDataPoints` |
| logs | `rejectedLogRecords` |
| traces | `rejectedSpans` |

Framework selects the right field from `ctx.otel.kind`.

### 7.3 Hard failure

For framework-level rejections (size, decode, content-type) and uncaught handler errors:

```
HTTP/1.1 <status> <reason>
Content-Type: application/json

{"error": "<message>"}
```

| Status | Cause |
|---|---|
| `400` | JSON parse error (well-formed protobuf is never a `400` — it's a `415`) |
| `401` / `403` | Auth failure (when `auth = "bearer"` is enabled and the token is missing/invalid) |
| `413` | Body exceeds `max_body_mb` (pre- or post-decompression) |
| `415` | Unsupported `Content-Type` or `Content-Encoding`, or protobuf decode failure |
| `500` | Handler threw an uncaught exception |

---

## 8. Auth

OTLP views accept **only** `auth = "none"` at the view level. Session auth is rejected at validation (`[X-OTLP-3]`) — OTLP clients are stateless and do not carry cookies. The structural validator also rejects `auth = "bearer"` with `[X-OTLP-3]` and points the operator at `guard_view` (see below) — there is no first-class `auth = "bearer"` mode on any view type.

### 8.1 Bearer-token auth via `guard_view`

Rivers does not implement bearer-token authentication as an `auth` mode; the project's canonical pattern for bearer auth is a per-view named guard (CB-P1.10, closed-as-superseded for CB-P1.12). The operator declares a `guard_view` whose codecomponent reads `Authorization: Bearer <token>` from `ctx.request.headers`, validates it, and returns `{ allow: true }` to admit the request or any other value to reject with `401`.

Spec reference: `docs/arch/rivers-auth-session-spec.md` §11.5 ("Bearer-token authentication via a named guard") — full recipe with TOML config, TypeScript handler, and operational notes.

```toml
[api.views.otel_ingest]
path        = "/otel"
view_type   = "OTLP"
auth        = "none"          # required for OTLP views
guard_view  = "bearer_check"  # per-view preflight reads Authorization

[api.views.otel_ingest.handlers.metrics]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestMetrics"
resources  = ["telemetry_db"]

# … handlers.logs and handlers.traces …

# The guard view — a regular REST view whose codecomponent runs as a
# preflight before any OTLP request reaches the per-signal handlers.
[api.views.bearer_check]
path      = "/_internal/bearer-check"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.bearer_check.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/guards/bearer.ts"
entrypoint = "checkBearer"
resources  = ["auth_db"]
```

### 8.2 Why not `auth = "bearer"`

CB-P1.12 originally asked for a first-class `auth = "bearer"` view mode. The team closed it as superseded by CB-P1.10 (named guards) — every config knob a `bearer` mode would have needed (token table, hash column, hash algorithm, claims projection, last-used update, audit fields) is a one-liner inside the guard codecomponent. The recipe gives operators more flexibility than a frozen framework primitive would, and stays the single canonical answer across MCP, OTLP, and any future protocol-specific view types. Decision recorded as CB-OTLP-D2 in `todo/changedecisionlog.md`.

---

## 9. Validation Rules

Validation runs at the `validate_structural` and `validate_crossref` layers in the existing 4-layer pipeline ([rivers-bundle-validation-spec.md](rivers-bundle-validation-spec.md)).

| Code | Layer | Condition | Severity |
|---|---|---|---|
| X-OTLP-1 | structural | `path` ends with `/v1/metrics`, `/v1/logs`, or `/v1/traces` — operator is mounting OTLP under a non-OTLP view type | error |
| X-OTLP-2 | structural | Neither `handlers.{metrics,logs,traces}` nor a single `handler` block declared | error |
| X-OTLP-3 | structural | `auth = "session"` declared — OTLP is stateless | error |
| X-OTLP-4 | structural | `streaming = true` declared — OTLP/HTTP is unary | error |
| X-OTLP-5 | structural | Both `handler` and `handlers.*` declared — choose one | error |
| X-OTLP-6 | structural | Any forbidden field declared (see §3.4) | error |
| X-OTLP-7 | existence (L2) | A declared `handlers.{signal}` handler's `module` does not resolve to a file in the bundle. Emitted as `E001` with `[X-OTLP-7]` in the `referenced_by` path. | error |
| X-OTLP-8 | syntax (L4) | A declared `handlers.{signal}` handler's `entrypoint` is not exported from `module`. Emitted as the existing `C002` code (same as single-`handler` form), available when the engine dylib is loaded. | error |
| X003 | crossref (L3) | A declared `handlers.{signal}` handler's `resources` entry is not declared in `resources.toml`. Reuses the existing `X003` code with `handlers.<signal>` in the message. | error |
| W-OTLP-1 | structural | `max_body_mb > 16` — likely a misconfiguration; OTLP recommends 4 | warning |

---

## 10. Configuration Reference

```toml
[api.views.<name>]
view_type    = "OTLP"                # required
path         = "<prefix>"            # required; framework appends /v1/<signal>
auth         = "none"                # optional, default "none" (only accepted value)
guard_view   = "<view-name>"         # optional — bearer-style auth preflight (P1.10)
max_body_mb  = <integer>             # optional, default 4

# Multi-handler form (any subset of the three signals)
[api.views.<name>.handlers.metrics]
type       = "codecomponent"
language   = "typescript" | "javascript"
module     = "<path>"
entrypoint = "<exported-fn>"
resources  = [ "<resource>", ... ]

[api.views.<name>.handlers.logs]
# … same shape …

[api.views.<name>.handlers.traces]
# … same shape …

# OR — single-handler form (mutually exclusive with handlers.*)
[api.views.<name>.handler]
type       = "codecomponent"
language   = "typescript" | "javascript"
module     = "<path>"
entrypoint = "<exported-fn>"   # receives ctx.otel.kind discriminator
resources  = [ "<resource>", ... ]
```

---

## 11. Observability

Metrics (Prometheus, when `[metrics] enabled = true`):

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `otlp_requests_total` | counter | `view`, `signal`, `encoding`, `status` | One per request |
| `otlp_decode_failures_total` | counter | `view`, `signal`, `reason` | `reason ∈ {protobuf, json, gzip, deflate, size_pre, size_post}` |
| `otlp_partial_success_total` | counter | `view`, `signal` | One per request with `rejected > 0` |
| `otlp_rejected_points_total` | counter | `view`, `signal` | Sum of `rejected` across requests |
| `otlp_request_bytes` | histogram | `view`, `signal`, `encoding` | Pre-decompression body size |
| `otlp_decoded_bytes` | histogram | `view`, `signal` | Post-decompression, pre-handler |
| `otlp_dispatch_duration_ms` | histogram | `view`, `signal` | Handler execution time |

Logs (per-app log file via `Rivers.log` / AppLogRouter):

- INFO at request start with view, signal, encoding, size, trace_id.
- WARN on partial success with rejected count and errorMessage.
- ERROR on handler exception with stack trace.
- ERROR on framework-level reject (415, 413, 400) with reason.

Trace ID propagation: the framework generates a `trace_id` per request (existing pattern from `view_dispatch.rs:277`) and surfaces it on `ctx.trace_id`. Handlers can read it; the framework does **not** read `traceparent` from OTLP payloads (that's the customer's domain). Future enhancement (non-goal v1): respect inbound `traceparent` headers.

---

## 12. Examples

### 12.1 Three signals, one handler module

```toml
[api.views.otel_ingest]
path         = "otel"
view_type    = "OTLP"
auth         = "none"
max_body_mb  = 4

[api.views.otel_ingest.handlers.metrics]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestMetrics"
resources  = ["cb_db"]

[api.views.otel_ingest.handlers.logs]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestLogs"
resources  = ["cb_db"]

[api.views.otel_ingest.handlers.traces]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestTraces"
resources  = ["cb_db"]
```

```typescript
// libraries/handlers/otel.ts
import { Ctx } from './_lib.ts';

export async function ingestMetrics(ctx: Ctx): Promise<void> {
    const payload = ctx.otel.payload;
    let rejected = 0;
    let reason = '';
    for (const rm of payload.resourceMetrics ?? []) {
        for (const sm of rm.scopeMetrics ?? []) {
            for (const m of sm.metrics ?? []) {
                try {
                    await Rivers.db.cb_db.run(
                        'INSERT INTO telemetry_events (name, payload) VALUES ($1, $2)',
                        [m.name, JSON.stringify(m)]
                    );
                } catch (e) {
                    rejected += 1;
                    reason = (e as Error).message;
                }
            }
        }
    }
    if (rejected > 0) {
        ctx.otel.rejected = rejected;
        ctx.otel.errorMessage = reason;
    }
}

export async function ingestLogs(ctx: Ctx): Promise<void> { /* ... */ }
export async function ingestTraces(ctx: Ctx): Promise<void> { /* ... */ }
```

### 12.2 Single handler with discriminator

```toml
[api.views.otel_ingest]
path      = "otel"
view_type = "OTLP"

[api.views.otel_ingest.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestAny"
resources  = ["cb_db"]
```

```typescript
export async function ingestAny(ctx: Ctx): Promise<void> {
    switch (ctx.otel.kind) {
        case 'metrics': /* ... */ break;
        case 'logs':    /* ... */ break;
        case 'traces':  /* ... */ break;
    }
}
```

### 12.3 Metrics-only ingest (other signals 404)

```toml
[api.views.metrics_only]
path      = "otel"
view_type = "OTLP"

[api.views.metrics_only.handlers.metrics]
type       = "codecomponent"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestMetrics"
resources  = ["cb_db"]
```

Requests to `/otel/v1/logs` and `/otel/v1/traces` return `404 {error: "OTLP signal '<x>' not configured on this view"}`.

---

## 13. Non-goals (v1)

- **Protobuf response encoding.** Responses are always `application/json` in v1, regardless of inbound encoding. Real OTel clients accept JSON responses; protobuf-out can be added later if a client surfaces a need.
- **OTLP/gRPC.** This spec covers OTLP/**HTTP** only. gRPC support would be a separate `view_type` and depends on Rivers landing a gRPC server primitive.
- **Inbound `traceparent` correlation.** The framework generates its own `trace_id`; it does not parse W3C trace context from OTLP payloads or request headers in v1.
- **Other compression algorithms.** Only `gzip` and `deflate`. `br` and `zstd` rejected with `415`.
- **Schema validation of OTLP payload internals.** The framework decodes the envelope and hands `ctx.otel.payload` to the handler. Field-level validation (e.g., enforcing that every span has a `traceId`) is the handler's job.
- **Resource attribute extraction.** No automatic flattening of `resource.attributes` onto `ctx`. Handlers walk the envelope.
- **Rate limiting per OTLP client.** Standard per-view `rate_limit_per_minute` (IP-based) is available; per-bearer-token rate limiting is out of scope for v1.

---

## 14. Implementation Notes

### 14.1 Code reuse

| Existing piece | Reuse |
|---|---|
| `crates/riversd/src/otlp_transcoder.rs` | Used as-is for protobuf → JSON |
| `crates/riversd/src/server/view_dispatch.rs` `match view_type` switch | Add `"OTLP" => execute_otlp_view(...)` branch |
| REST codecomponent dispatch path | OTLP handlers go through the same `process_pool` dispatch with `TaskKind::Rest` (or a new `TaskKind::Otlp` if discriminator-level metrics are wanted) |
| Per-app log routing, metrics fabric, rate limiter | All reused unchanged |
| `validate_structural` pipeline | Add `validate_otlp_view` step emitting X-OTLP-N codes |

### 14.2 New code surface

| New piece | Estimated size | Notes |
|---|---|---|
| `crates/riversd/src/server/otlp_view.rs` | ~200 lines | Body extraction, decompression, transcode, dispatch, response wrap |
| `crates/rivers-runtime/src/bundle_loader/validate_otlp.rs` | ~120 lines | X-OTLP-1..8, W-OTLP-1..2 |
| `OtelContext` shape in handler envelope (`engine-sdk` SerializedTaskContext) | ~30 lines | New `otel` field on the dispatch envelope, optional |
| `flate2` dependency | already present transitively via tonic; verify | If not present, add to `riversd` crate's Cargo.toml |
| Bundle-validation amendments doc entry | ~10 lines | Document the new X-OTLP-N codes |
| Feature inventory entry (§2.6c or new §2.6) | ~10 lines | Link to this spec |

### 14.3 Sequencing relative to current sprint

**Shipped in v0.62.0** (commits `51f524d`..`4d7b01f` on `main`). Sequenced as five tracks, each independently shippable:

1. **O1** — Validator + feature-inventory entry (PR #116). Closes the "Rivers gives no actionable error for OTLP misconfiguration" gap on its own.
2. **O2** — Dispatcher (`otlp_view.rs`) with multi-handler form, V8 `ctx.otel` injection, body-extraction → decompression → transcode → routing → response shaping (PR #117 commit `997c267`).
3. **O3** — Single-handler discriminator coverage + `pick_handler` unit tests (PR #117 commit `d7c3bd3`).
4. **O5** — Prometheus metrics, tutorial, `v0.62.0` minor bump (PR #117 commit `26dfa1c`).
5. **O5.6 + O1.3** — End-to-end smoke fixture (`tests/fixtures/otlp-probe/`) that caught 5 dispatcher bugs; per-signal `handlers.*` validation across L2/L3/L4 (PR #117 commits `9e56f37` + `e011663`).

No `auth = "bearer"` sequencing — bearer auth was resolved via `guard_view` per CB-OTLP-D2 (CB-P1.12 closed-as-superseded). See §8.
