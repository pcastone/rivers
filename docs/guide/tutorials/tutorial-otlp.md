# Tutorial — OTLP Views (OpenTelemetry Ingest)

`view_type = "OTLP"` is a first-class view type for receiving OpenTelemetry
OTLP/HTTP traffic. Rivers owns the wire format (Content-Type negotiation,
gzip/deflate decompression, per-signal routing, partial-success response
shape); your handler keeps the per-row insert logic.

**Spec:** [`rivers-otlp-view-spec.md`](../../arch/rivers-otlp-view-spec.md)
**Requires:** Rivers v0.62+ (Track O2 ships the dispatcher).

---

## When to use

Reach for an OTLP view when:

- You're receiving telemetry from an OpenTelemetry SDK or the OTel Collector.
- You want JSON and protobuf clients to work without per-handler boilerplate.
- You want gzip'd or deflate'd request bodies decompressed for you.
- You want OTLP-spec-compliant partial-success responses on rejection.

Don't use an OTLP view when:

- The endpoint isn't OTLP/HTTP — use `view_type = "Rest"` instead.
- You need OTLP/gRPC — that's a separate transport (not in v1).
- You need streaming response framing — OTLP/HTTP is unary.

---

## Minimal example — three signals, three handlers

```toml
# app.toml

[api.views.otel_ingest]
path      = "/otel"          # framework mounts /v1/{metrics,logs,traces}
view_type = "OTLP"
auth      = "none"           # "none" is the only accepted value (see Auth below)
# max_body_mb = 4            # optional, default 4 (OTLP spec recommendation)

[api.views.otel_ingest.handlers.metrics]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestMetrics"
resources  = ["telemetry_db"]

[api.views.otel_ingest.handlers.logs]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestLogs"
resources  = ["telemetry_db"]

[api.views.otel_ingest.handlers.traces]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestTraces"
resources  = ["telemetry_db"]
```

The framework registers three POST routes:

- `POST /otel/v1/metrics`
- `POST /otel/v1/logs`
- `POST /otel/v1/traces`

Each request flows through the same pipeline: size check → decompress (if
`Content-Encoding: gzip` or `deflate`) → decode (`application/json` or
`application/x-protobuf`) → dispatch to the per-signal handler.

```typescript
// libraries/handlers/otel.ts

export async function ingestMetrics(ctx) {
    // ctx.otel.payload is already decoded (JSON), already gunzipped if
    // needed, already protobuf-transcoded if the client sent protobuf.
    // ctx.otel.kind === "metrics" here.
    let rejected = 0;
    let lastError = "";
    for (const rm of ctx.otel.payload.resourceMetrics ?? []) {
        for (const sm of rm.scopeMetrics ?? []) {
            for (const m of sm.metrics ?? []) {
                try {
                    await Rivers.db.run(
                        "telemetry_db",
                        "INSERT INTO metrics (name, payload) VALUES ($1, $2)",
                        [m.name, JSON.stringify(m)]
                    );
                } catch (e) {
                    rejected += 1;
                    lastError = e.message;
                }
            }
        }
    }
    // Return partial-success signals to the framework.
    return { rejected, errorMessage: lastError };
}

export async function ingestLogs(ctx)  { /* same shape, payload.resourceLogs  */ }
export async function ingestTraces(ctx){ /* same shape, payload.resourceSpans */ }
```

Framework wraps the handler's return value into the OTLP response:

- `{}` when `rejected == 0` or the handler returns nothing.
- `{"partialSuccess": {"rejectedDataPoints": N, "errorMessage": "..."}}` for
  metrics; `rejectedLogRecords` for logs; `rejectedSpans` for traces.

Both responses are `200 OK` per the OTLP spec — partial success is still
a 200.

---

## Alternative — single handler with `ctx.otel.kind` discriminator

When all three signals share most code, declare one handler. The framework
sets `ctx.otel.kind ∈ {"metrics","logs","traces"}` so the handler can
branch:

```toml
[api.views.otel_ingest]
path      = "/otel"
view_type = "OTLP"

[api.views.otel_ingest.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestAny"
resources  = ["telemetry_db"]
```

```typescript
export async function ingestAny(ctx) {
    switch (ctx.otel.kind) {
        case "metrics": return ingestMetrics(ctx);
        case "logs":    return ingestLogs(ctx);
        case "traces":  return ingestTraces(ctx);
    }
}
```

`handler` and `handlers.*` are mutually exclusive — declaring both fails
validation with `[X-OTLP-5]`.

---

## Partial signals

A view can declare only the signals it cares about:

```toml
[api.views.metrics_only]
path      = "/otel"
view_type = "OTLP"

[api.views.metrics_only.handlers.metrics]
type       = "codecomponent"
module     = "libraries/handlers/otel.ts"
entrypoint = "ingestMetrics"
```

Requests to `/otel/v1/logs` or `/otel/v1/traces` get `404 Not Found`. No
silent dispatch to a bogus handler.

---

## Configuration reference

| Field | Required? | Default | Notes |
|---|---|---|---|
| `view_type` | yes | — | Must be `"OTLP"` |
| `path` | yes | — | Root prefix; framework appends `/v1/<signal>` |
| `handlers.metrics` ∨ `handlers.logs` ∨ `handlers.traces` ∨ `handler` | yes (one form) | — | Per-signal table or single discriminator handler |
| `auth` | optional | `"none"` | Only `"none"` is accepted — see Auth below |
| `max_body_mb` | optional | `4` | OTLP/HTTP recommendation |

### Forbidden on OTLP views

These fail validation with a specific `[X-OTLP-N]` code:

| Code | What it catches |
|---|---|
| `[X-OTLP-1]` | `path` ends in `/v1/{metrics,logs,traces}` (the framework mounts those itself — don't repeat the suffix) |
| `[X-OTLP-2]` | Neither `handler` nor `handlers.*` declared |
| `[X-OTLP-3]` | `auth` is anything other than `"none"` |
| `[X-OTLP-4]` | `streaming = true` (OTLP/HTTP is unary) |
| `[X-OTLP-5]` | Both `handler` and `handlers.*` declared |
| `[X-OTLP-6]` | A field from another view type's domain (e.g., `websocket_mode`, `polling`, `schedule`) |
| `[W-OTLP-1]` | (warning) `max_body_mb > 16` — unusually large; accepted but flagged |

Run `riverpackage validate <bundle>` to surface these before deploy.

---

## Wire format details

### Content-Type negotiation

| Inbound `Content-Type` | Behavior |
|---|---|
| `application/json` (or `application/json; charset=utf-8`) | Body parsed as JSON, passed through unchanged |
| `application/x-protobuf` | Body fed through the existing P1.6 transcoder (decodes prost types and re-encodes as JSON) before dispatch — handler sees JSON |
| Other / missing | `415 Unsupported Media Type` |

### Content-Encoding (compression)

| Inbound `Content-Encoding` | Behavior |
|---|---|
| absent / `identity` | Body used as-is |
| `gzip` | Decoded via `flate2::read::GzDecoder` |
| `deflate` | Decoded via `flate2::read::DeflateDecoder` |
| `br`, `zstd`, etc. | `415` |

Decompression is bounded: the decoded body is capped at `max_body_mb × 1.5`
to prevent zip-bomb amplification. Exceeding the cap returns `413 Payload
Too Large`.

### Body size

- Pre-decompression: reject `> max_body_mb` with `413`.
- Post-decompression: reject `> max_body_mb × 1.5` with `413`.

---

## Authentication

OTLP views accept only `auth = "none"`. The OTLP spec describes bearer
tokens via the `Authorization` header; Rivers handles bearer-style auth
through the existing `guard_view` pattern rather than a dedicated
`auth = "bearer"` mode. To require a bearer token, declare a `guard_view`
that checks the header:

```toml
[api.views.otel_ingest]
path        = "/otel"
view_type   = "OTLP"
guard_view  = "bearer_check"
# … handlers …

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
```

The guard runs as a preflight before the OTLP dispatcher — returning
`{ allow: false }` rejects the request with `401`.

---

## Observability

When riversd is built with the `metrics` feature, OTLP views emit seven
Prometheus metrics on the metrics endpoint (port 9091 by default):

| Metric | Type | Labels |
|---|---|---|
| `rivers_otlp_requests_total` | counter | `view`, `signal`, `encoding`, `status` |
| `rivers_otlp_decode_failures_total` | counter | `view`, `signal`, `reason` |
| `rivers_otlp_partial_success_total` | counter | `view`, `signal` |
| `rivers_otlp_rejected_points_total` | counter | `view`, `signal` |
| `rivers_otlp_request_bytes` | histogram | `view`, `signal`, `encoding` |
| `rivers_otlp_decoded_bytes` | histogram | `view`, `signal` |
| `rivers_otlp_dispatch_duration_ms` | histogram | `view`, `signal` |

`reason` on `decode_failures_total` is one of: `size_pre`, `encoding`,
`content_type`, `decompress`, `json`, `protobuf`, `signal`. Build alerts
on specific failure classes by filtering on this label.

Each request also writes an INFO line to the per-app log file with the
trace_id, signal, encoding, body size, decoded size, and dispatch duration.

---

## Failure semantics (handler errors vs partial success)

Two different concerns:

- **Handler threw / panicked**: the framework returns `500 Internal Server
  Error` with `{"error": "..."}`. Nothing was ingested; the client should
  retry per its own policy.
- **Handler completed but rejected some points**: handler returns
  `{rejected: N, errorMessage: "..."}`; framework returns `200` with the
  `partialSuccess` body. Most clients won't retry — they'll log and move on.

Don't conflate the two. If the entire batch is unusable (schema mismatch,
auth-style reject), let the handler throw — the client gets a clear `500`.
If individual records failed but most succeeded, set `rejected` — the
batch is accounted for.

---

## v1 non-goals

- **OTLP/gRPC** — separate transport, separate view type when it ships.
- **Protobuf responses** — Rivers always returns JSON, even to protobuf
  clients. The OTel ecosystem's clients all accept JSON responses.
- **W3C `traceparent` correlation** — the framework generates its own
  trace_id; it does not parse traceparent from OTLP payloads or headers.
- **Other compression algorithms** — only gzip and deflate. `br` and
  `zstd` are rejected with `415`.
- **Per-signal validation inside the payload** — the framework decodes the
  envelope and hands you `ctx.otel.payload`. Field-level rules (every
  span has a `traceId`, etc.) are the handler's job.
