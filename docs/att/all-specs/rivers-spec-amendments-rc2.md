# Rivers Spec Amendments — Review Cycle 2

**Document Type:** Amendments / Corrections  
**Patches:** rivers-auth-session-spec.md, rivers-httpd-spec.md, rivers-view-layer-spec.md, rivers-processpool-runtime-spec-v2.md, rivers-http-driver-spec.md, rivers-polling-views-spec.md  
**Status:** Authoritative — supersedes conflicting content in patched documents  

---

## Table of Contents

1. [AMD-9 — StorageEngine Reserved Key Prefixes](#amd-9)
2. [AMD-10 — Session Storage Canonical Source](#amd-10)
3. [AMD-11 — Remove Session Auto-Creation](#amd-11)
4. [AMD-12 — Cookie Name Standardization](#amd-12)
5. [AMD-13 — on_session_valid Pipeline Position](#amd-13)
6. [AMD-14 — Persistent Connection Session Revalidation](#amd-14)
7. [AMD-15 — MessageConsumer Session Exemption](#amd-15)
8. [AMD-16 — Rivers.db.query Name vs Token Clarification](#amd-16)
9. [AMD-17 — Rivers.view.stream() Resources Declaration](#amd-17)
10. [AMD-18 — CSRF on Streaming POST Views](#amd-18)

---

## AMD-9

### StorageEngine Reserved Key Prefixes

**Patches:** `rivers-storage-engine-spec.md`, `rivers-auth-session-spec.md`

#### Problem

Sessions, CSRF tokens, and poll state all share the same StorageEngine namespace as application data. A buggy or malicious handler could read another user's session claims by constructing a `session:{other_id}` key directly.

#### Resolution

Reserved key prefixes are enforced at the host layer. No CodeComponent can read or write keys with reserved prefixes. The Rivers API surface never exposes raw StorageEngine access to handler code — handlers access session data only through `Rivers.session.current`, never through a StorageEngine key lookup.

Enforcement is at the host-side token resolution layer, the same model as credential isolation. If a handler somehow constructs a `session:` prefixed key in a StorageEngine call, the host returns `CapabilityError` without touching the store.

#### Reserved prefix table

Added to `rivers-storage-engine-spec.md` as a new section:

| Prefix | Owner | Handler access |
|---|---|---|
| `session:` | Rivers core — session store | None |
| `csrf:` | Rivers core — CSRF token store | None |
| `poll:` | Rivers core — poll loop state | None |
| `rivers:` | Rivers core — internal use | None |
| All other keys | Application | Read/write per declared capability |

Application StorageEngine access uses the `Rivers.store` API (defined in ProcessPool spec §8). Calls with reserved-prefix keys return `CapabilityError` immediately. The restriction is not configurable.

#### Rivers.store API addition

Added to ProcessPool spec §8 alongside `Rivers.db`, `Rivers.view`, etc.:

```typescript
Rivers.store = {
    async get(key: string): Promise<any | null>,
    async set(key: string, value: any, ttl_s?: number): Promise<void>,
    async delete(key: string): Promise<void>,
    async exists(key: string): Promise<boolean>,
}
```

Keys beginning with `session:`, `csrf:`, `poll:`, or `rivers:` throw `CapabilityError` at call time. All other keys are application-owned.

---

## AMD-10

### Session Storage Canonical Source

**Patches:** `rivers-httpd-spec.md` §12, `rivers-auth-session-spec.md` §7

#### Problem

`rivers-httpd-spec.md` §12.2 states sessions are distributed via gossip (`GossipPayload::SessionUpserted`). The new auth spec says sessions live in StorageEngine. These are two different mechanisms and cannot both be authoritative.

#### Resolution

**StorageEngine is the canonical session store.** Gossip-based session propagation is removed.

**Rationale:** Gossip is eventually consistent — a session created on node A may not be visible on node B for several gossip rounds. For session validation this is unacceptable — a client whose request is load-balanced to a different node after login would fail validation. StorageEngine with a Redis backend provides immediate consistency across all nodes. StorageEngine with in-memory or SQLite backend is node-local, which is acceptable for single-node deployments.

#### httpd spec §12.2 replacement

**Removed:**
```
Session creates/updates are broadcast via GossipPayload::SessionUpserted { session_id }.
Session deletes via GossipPayload::SessionDeleted { session_id }.
All nodes in the cluster maintain a consistent session store.
```

**Replaced with:**
```
Sessions are stored in and read from StorageEngine. In multi-node deployments, 
StorageEngine must use a Redis backend to provide consistent session access across 
all nodes. In single-node deployments, SQLite or in-memory backends are sufficient.

GossipPayload::SessionUpserted and GossipPayload::SessionDeleted are removed 
from the gossip protocol — session state is not gossiped.
```

#### SessionConfig replacement

**Removed:**
```rust
pub struct SessionConfig {
    pub enabled: bool,
    pub cookie_name: String,
}
```

**Replaced with:**
```rust
pub struct SessionConfig {
    pub cookie_name:         String,      // default: "rivers_session"
    pub ttl_s:               u64,         // default: 3600
    pub idle_timeout_s:      u64,         // default: 1800
    pub csrf_protection:     bool,        // default: true
    pub csrf_header:         String,      // default: "X-CSRF-Token"
    pub csrf_rotation_interval_s: u64,   // default: 300
}
```

---

## AMD-11

### Remove Session Auto-Creation

**Patches:** `rivers-httpd-spec.md` §12.1

#### Problem

The existing httpd spec §12.1 auto-creates an anonymous session for any request that arrives without a session cookie:

```
if no cookie and session not required:
    generate new session_id
    SessionManager::create_session(id)
```

The new auth spec says sessions are only created by the guard CodeComponent returning `IdentityClaims`. Anonymous auto-created sessions would satisfy the session check on protected views — bypassing the guard entirely.

#### Resolution

Session auto-creation is **removed**. Sessions are only created by the guard view after successful CodeComponent execution. No other code path creates sessions.

#### Updated §12.1 session middleware flow

```
request arrives
    │
    ├─ parse cookie: rivers_session (configured cookie name)
    │
    ├─ if cookie present:
    │       StorageEngine.get("session:{id}")
    │       → not found or expired: clear_cookie = true, session = None
    │       → valid: inject Session into request extensions, update last_seen
    │
    ├─ if no cookie:
    │       session = None
    │       (no auto-creation)
    │
    ├─ route to view handler
    │       → guard view: CodeComponent runs, creates session on success
    │       → protected view: session = None → invalid session path
    │       → auth = "none" view: proceeds regardless
    │
    └─ after handler:
            set_cookie → Set-Cookie: rivers_session={id}; HttpOnly; SameSite=Lax; Secure; Path=/
            clear_cookie → Set-Cookie: rivers_session=; Max-Age=0; ...
```

---

## AMD-12

### Cookie Name Standardization

**Patches:** `rivers-httpd-spec.md`, `rivers-auth-session-spec.md`

#### Problem

`rivers-httpd-spec.md` used `rivers_session_id` as the default cookie name. The new auth spec used `rivers_session`. Both documents will be read together. Inconsistency causes integration bugs.

#### Resolution

**`rivers_session` is the canonical default cookie name** across all documents.

All references to `rivers_session_id` in `rivers-httpd-spec.md` are replaced with `rivers_session`. This affects:
- §12.1 session middleware flow
- §12.3 `SessionConfig.cookie_name` default value  
- §15 config reference example

---

## AMD-13

### on_session_valid Pipeline Position and Configurability

**Patches:** `rivers-view-layer-spec.md` §4, `rivers-auth-session-spec.md` §6

#### Problem

`on_session_valid` was introduced in the auth spec but never placed in the view layer pipeline diagram. Its position relative to `pre_process` was unspecified. Whether pipeline stages could see session data was unanswered.

#### Resolution

`on_session_valid` runs **after middleware, before `pre_process`** by default. Its position is explicit in the pipeline diagram. Session data is available to all pipeline stages including `pre_process` observers.

The position is app-configurable per-view via `session_stage`.

#### Updated view layer pipeline diagram

```
Incoming request
    │
    ▼
Router  (path + method → ApiViewConfig)
    │
    ▼
Middleware stack  (rate limit, CORS, backpressure, trace, session parse)
    │
    ▼
Session validation
    │
    ├─ Invalid session → on_invalid_session → redirect / 401
    │
    └─ Valid session
            │
            ▼
        [on_session_valid]   ← configurable position (default: here)
            │
            ├─ redirect returned → stop, send redirect
            │
            └─ continue
                    │
                    ▼
                [pre_process]     — observer, fire-and-forget
                [on_request]      — accumulator, deposits into ctx.sources
                 Primary execution (DataView or CodeComponent)
                [transform]       — chained pipeline
                [on_response]     — accumulator
                [post_process]    — observer, fire-and-forget
                    │
                    ▼
                Response
```

`ctx.session` (and `Rivers.session.current` in TypeScript) is populated after session validation and is available in all stages including `pre_process`, `on_request`, `transform`, `on_response`, and `post_process`.

#### session_stage config

```toml
[api.views.dashboard]
session_stage = "before_pipeline"   # default — runs before pre_process
                                    # "pre_process" — runs as first pre_process stage
                                    # "on_request"  — runs as first on_request stage
```

| `session_stage` | Position | Use case |
|---|---|---|
| `before_pipeline` | Before `pre_process` | Default. Session identity available everywhere. |
| `pre_process` | First `pre_process` stage | When session handler should be an observer alongside other observers. |
| `on_request` | First `on_request` stage | When session handler needs to deposit into `ctx.sources` as an accumulator. |

The `on_session_valid` handler contract changes slightly per stage:

- `before_pipeline` and `pre_process` — handler returns `OnSessionValidResult | void`
- `on_request` — handler returns `OnRequestResult | void` (same contract as other `on_request` handlers, deposits into `ctx.sources[key]`)

---

## AMD-14

### Persistent Connection Session Revalidation

**Patches:** `rivers-view-layer-spec.md` §6, §7, `rivers-polling-views-spec.md` §3

#### Problem

WebSocket connections and SSE polling connections persist for potentially hours. Session validation happens at connection time (the HTTP GET/upgrade request). If the session expires mid-connection, nothing addressed what happens — the connection could remain open indefinitely with an expired session.

#### Resolution

Session revalidation on persistent connections is **app-configurable per-view**. The default is validate at connection time only.

#### Config

```toml
# WebSocket view
[api.views.order_updates]
view_type = "Websocket"
session_revalidation_interval_s = 300    # default: 0 (disabled — validate at connect only)

# SSE polling view
[api.views.price_feed]
view_type            = "ServerSentEvents"
session_revalidation_interval_s = 300    # default: 0
```

`session_revalidation_interval_s = 0` — session validated at connection time only. Connection lifetime is trusted independently of session expiry.

`session_revalidation_interval_s > 0` — Rivers rechecks the session in StorageEngine at the configured interval. If the session has expired:

- **WebSocket** — Rivers sends a close frame with code `4401` (application-defined: session expired) and closes the connection.
- **SSE** — Rivers sends a terminal event before closing:
  ```
  event: session_expired\n
  data: {"code": 4401, "reason": "session expired"}\n\n
  ```
  Then closes the connection.

The client is responsible for handling the close/event and redirecting to the guard view to re-authenticate.

#### Polling view revalidation

For SSE polling views, session revalidation runs on the poll tick — Rivers checks session validity before executing the DataView on each tick where `session_revalidation_interval_s` has elapsed since the last check. If expired, the terminal event is emitted and the poll loop for that client's connection stops.

#### No revalidation on non-persistent connections

REST views validate session on every request by definition — each HTTP request carries the cookie. Revalidation config is only valid on `Websocket` and `ServerSentEvents` views. Config validation rejects `session_revalidation_interval_s` on REST views.

---

## AMD-15

### MessageConsumer Session Exemption

**Patches:** `rivers-auth-session-spec.md` §5

#### Problem

`MessageConsumer` views have no HTTP request — they are driven by EventBus events from the broker bridge. There is no client, no cookie, no session. The auth spec said "all views are protected by default," which is incorrect for `MessageConsumer`.

#### Resolution

`MessageConsumer` views are **auto-exempt from session validation** by default. No config required. The auth middleware does not run for EventBus-driven views.

This is app-configurable — a `MessageConsumer` can opt into session validation if the broker message carries a session token:

```toml
[api.views.process_order]
view_type = "MessageConsumer"
auth      = "session"           # default: "none" for MessageConsumer
```

When `auth = "session"` is declared on a `MessageConsumer` view, Rivers expects the EventBus event payload to contain a `_session_id` field. Rivers validates it against StorageEngine before the CodeComponent executes. If absent or invalid, the event is rejected — the CodeComponent does not execute, and a `MessageConsumerAuthFailed` internal event is emitted.

```typescript
// Event payload with session token
{
    "_session_id": "sess_abc123",   // Rivers validates this
    "order_id":    "ord_456",
    "amount":      99.99
}
```

`_session_id` is a reserved field in EventBus message payloads. Application payloads must not use this key for other purposes.

#### Updated auth spec §5.1

The "protected vs public" section is updated:

| View type | Default auth | Override |
|---|---|---|
| `Rest` | `session` (protected) | `auth = "none"` |
| `Websocket` | `session` (protected) | `auth = "none"` |
| `ServerSentEvents` | `session` (protected) | `auth = "none"` |
| `MessageConsumer` | `none` (exempt) | `auth = "session"` |
| Guard view | `none` (implicit) | Not configurable |

---

## AMD-16

### Rivers.db.query — Name vs Token Clarification

**Patches:** `rivers-processpool-runtime-spec-v2.md` §8, §10

#### Problem

The ProcessPool spec described the first argument to `Rivers.db.query()` as an "opaque token" and showed internal representations like `"tok:mysql-1"`. The code example in §10.3 used `"primary-db"` with the comment `"datasource token (string alias)"`. These look different. An agent generating handler code would not know whether to pass `"primary-db"` or `"tok:mysql-1"`.

#### Resolution

**The handler always uses the declared datasource name as a plain string.** `"primary-db"` is correct. The `tok:` prefix is an internal host-side implementation detail that never appears in handler code.

The resolution path is:
```
handler passes "primary-db" as string
    → host receives string
    → host looks up "primary-db" in the view's declared resources
    → host resolves to internal connection pool token
    → host executes operation
    → result returned to handler
```

If the handler passes a name not declared in `resources`, the host returns `CapabilityError` — same as before. The name is the capability declaration. The token is invisible.

#### Updated §8 Rivers API surface

All occurrences of `token` in the `Rivers.db` API description are replaced with `datasource_name`:

```typescript
Rivers.db = {
    // datasource_name must be declared in the view's resources array
    async query(
        datasource_name: string,
        sql:             string,
        params?:         any[]
    ): Promise<QueryRow[]>,

    async execute(
        datasource_name: string,
        sql:             string,
        params?:         any[]
    ): Promise<ExecuteResult>,
}
```

#### Updated §10.3 example comment

```typescript
const rows = await Rivers.db.query(
    "primary-db",    // declared datasource name — must be in view's resources array
    "SELECT id, name, email FROM users WHERE id = $1",
    [req.params.id]
);
```

All references to `tok:`, "opaque token" (in the context of handler-visible strings), and "datasource token" in examples are updated to "datasource name."

The internal token model (host-side resolution) is documented separately in the ProcessPool implementation notes as an internal detail — not part of the handler API contract.

---

## AMD-17

### Rivers.view.stream() Resources Declaration

**Patches:** `rivers-spec-amendments-rc1.md` AMD-7

#### Problem

AMD-7 defined `Rivers.view.stream()` for consuming streaming HTTP DataView responses but did not specify whether the streaming DataView's datasource must be declared in the CodeComponent's `resources` array. The same rule applies to `Rivers.view.query()` — the referenced DataView's datasource must be in `resources`.

#### Resolution

**The same `resources` rule applies to `Rivers.view.stream()`.** The datasource referenced by the streaming DataView must be declared in the CodeComponent's `resources` array. If not declared, the host returns `CapabilityError` at the first iteration of the `AsyncIterable`.

#### Config example

```toml
[api.views.generate]
path      = "/api/generate"
method    = "POST"
view_type = "Rest"
streaming = true

[api.views.generate.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/llm.ts"
entrypoint = "generate"
resources  = ["anthropic"]      # must include the HTTP datasource used by the streaming DataView
```

```toml
[data.datasources.anthropic]
driver   = "http"
base_url = "https://api.anthropic.com"
auth     = "bearer"
credentials = "lockbox://anthropic/api_key"

[data.dataviews.generate_completion]
datasource         = "anthropic"    # "anthropic" must be in handler's resources
streaming_response = true
```

#### Validation

At deploy time, Rivers validates that for every `Rivers.view.stream("dataview_name", ...)` call reachable in the CodeComponent, the DataView's `datasource` is listed in the view's `resources`. This is static analysis at deploy time — same as existing DataView resource validation.

If the DataView is not statically determinable (e.g., the dataview name is a runtime variable), the `CapabilityError` is enforced at runtime on first iteration.

---

## AMD-18

### CSRF on Streaming POST Views

**Patches:** `rivers-spec-amendments-rc1.md` AMD-6

#### Problem

AMD-6 specified CSRF validation on POST, PUT, PATCH, DELETE requests for browser-mode sessions. Streaming REST views that use POST (the LLM generation endpoint being the primary example) were not explicitly addressed. An agent building a browser SPA that calls a streaming POST endpoint would not know to include the CSRF header.

#### Resolution

**CSRF validation applies to streaming POST views identically to non-streaming POST views.** No special case. The rule is method-based, not response-type-based.

For browser-mode sessions (cookie-based), any POST to a streaming view requires `X-CSRF-Token`. For API/mobile mode sessions (Authorization Bearer), CSRF is exempt — same as non-streaming.

#### Explicit statement added to AMD-6 exemptions

The exemptions list in AMD-6 §"Exemptions" adds:

> **Streaming REST views** — CSRF validation applies normally. A streaming POST from a browser SPA requires `X-CSRF-Token`. A streaming POST from an API/mobile client using `Authorization: Bearer` is exempt. The streaming nature of the response does not affect CSRF validation of the request.

#### SPA streaming example — complete headers

```javascript
// Browser SPA calling a streaming POST endpoint
const response = await fetch('/api/generate', {
    method: 'POST',
    headers: {
        'Content-Type':  'application/json',
        'X-CSRF-Token':  getCsrfToken(),    // required for browser session
    },
    body: JSON.stringify({ prompt: "..." }),
});

// Read ndjson stream
const reader = response.body.getReader();
const decoder = new TextDecoder();

while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    const lines = decoder.decode(value).split('\n').filter(Boolean);
    for (const line of lines) {
        const chunk = JSON.parse(line);
        if (chunk.stream_terminated) {
            console.error('Stream error:', chunk.error);
            break;
        }
        if (chunk.token) {
            appendToken(chunk.token);
        }
    }
}
```

This example also serves as the canonical browser SPA streaming consumption pattern — the first complete end-to-end example covering CSRF + streaming in a single code block.

---

## Amendment Summary

| ID | Issue | Action | Specs Affected |
|---|---|---|---|
| AMD-9 | Session isolation — cross-session data access | Reserved key prefixes enforced host-side. `Rivers.store` API defined. | StorageEngine, ProcessPool |
| AMD-10 | Session storage conflict — gossip vs StorageEngine | StorageEngine canonical. Gossip session propagation removed. | HTTPD, Auth |
| AMD-11 | Session auto-creation conflicts with guard model | Auto-creation removed. Sessions created by guard only. | HTTPD |
| AMD-12 | Cookie name mismatch across specs | `rivers_session` canonical everywhere. | HTTPD, Auth |
| AMD-13 | `on_session_valid` missing from pipeline diagram | Added before `pre_process`. Position app-configurable via `session_stage`. | View Layer, Auth |
| AMD-14 | Session expiry on persistent connections unspecified | App-configurable `session_revalidation_interval_s`. Default: 0 (connect-time only). | View Layer, Polling |
| AMD-15 | MessageConsumer session exemption | Auto-exempt by default. Opt-in `auth = "session"` via `_session_id` in payload. | Auth |
| AMD-16 | `Rivers.db.query` token vs name confusion | Handler always uses datasource name. `tok:` is internal. | ProcessPool |
| AMD-17 | `Rivers.view.stream()` resources not specified | Same `resources` rule as `Rivers.view.query()`. | AMD-7, HTTP Driver |
| AMD-18 | CSRF on streaming POST not explicit | CSRF applies to streaming POST identically. Canonical SPA example added. | AMD-6, Streaming |
