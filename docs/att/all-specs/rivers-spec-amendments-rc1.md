# Rivers Spec Amendments — Review Cycle 1

**Document Type:** Amendments / Corrections  
**Patches:** rivers-streaming-rest-spec.md, rivers-polling-views-spec.md, rivers-auth-session-spec.md, rivers-http-driver-spec.md, rivers-processpool-runtime-spec-v2.md  
**Status:** Authoritative — supersedes conflicting content in patched documents  

---

## Table of Contents

1. [AMD-1 — Rivers.http Escape Hatch with Audit Logging](#amd-1)
2. [AMD-2 — Remove Rivers.lockbox.get()](#amd-2)
3. [AMD-3 — Define Rivers.crypto API Surface](#amd-3)
4. [AMD-4 — Remove REST Polling](#amd-4)
5. [AMD-5 — Guard View Auto-Detects Browser vs API Mode](#amd-5)
6. [AMD-6 — CSRF Auto-Generation and Validation](#amd-6)
7. [AMD-7 — Rivers.view.stream() for Upstream Streaming Consumption](#amd-7)
8. [AMD-8 — change_detect Handler May Be Async](#amd-8)

---

## AMD-1

### Rivers.http — Escape Hatch with Audit Logging

**Patches:** `rivers-streaming-rest-spec.md` (§4.4), `rivers-processpool-runtime-spec-v2.md` (§8)

#### Problem

The streaming spec used `Rivers.http` to call the Anthropic API — a known upstream with a static URL. This is the wrong pattern. Known upstreams belong in the HTTP driver as declared datasources with pooling, LockBox credentials, retry, and circuit breaker. The example implied `Rivers.http` was the standard outbound HTTP mechanism, which conflicts with the HTTP driver spec entirely.

#### Resolution

`Rivers.http` is retained as an **escape hatch for runtime-dynamic URLs only** — URLs that are genuinely unknown at config time (user-supplied webhook URLs, dynamic callback endpoints, runtime-constructed API paths). It is not a replacement for the HTTP driver datasource.

#### Framing

The ProcessPool spec §8 (Rivers API surface) adds the following to the `Rivers.http` entry:

> **`Rivers.http` is the escape hatch, not the standard.** Use the HTTP driver datasource for any upstream whose base URL is known at config time. Use `Rivers.http` only when the full URL is determined at runtime — user-supplied webhook targets, dynamic callback URLs, or runtime-constructed endpoints.

#### Audit logging — two levels

**Level 1 — Startup warning.** When any ProcessPool has `allow_outbound_http = true`, Rivers emits at startup:

```
WARN rivers::processpool: ad-hoc HTTP enabled on pool '{pool_name}' — 
     outbound calls to undeclared URLs will be logged [ad_hoc_http_enabled]
```

**Level 2 — Call-time warning.** Every `Rivers.http` invocation emits a structured log entry:

```json
{
    "level":    "WARN",
    "message":  "ad_hoc_http_call",
    "host":     "webhooks.customer.example.com",
    "method":   "POST",
    "view":     "notify_webhook",
    "module":   "handlers/notify.ts",
    "trace_id": "trace-abc-123"
}
```

Host only — not the full URL, which may contain sensitive query parameters. Method, view, module, and trace ID provide full attribution. Filterable by the fixed key `"ad_hoc_http_call"`.

#### Corrected streaming spec examples

All streaming spec examples that used `Rivers.http` to call known upstreams (Anthropic, OpenAI) are replaced with DataView calls:

```typescript
// REMOVED — wrong pattern for known upstream
const stream = await Rivers.http.post("https://api.anthropic.com/v1/messages", {
    headers: { "x-api-key": await Rivers.lockbox.get("anthropic_key") },
    body: { ... },
    stream: true
});

// CORRECT — HTTP driver datasource declared in config
const result = await Rivers.view.stream("generate_completion", {
    prompt: req.body.prompt,
    model:  req.body.model,
});
```

#### Legitimate Rivers.http usage example

```typescript
// Webhook relay — URL is user-supplied at runtime, cannot be a datasource
export async function* relayWebhook(
    req: Rivers.Request
): AsyncGenerator<any> {
    const { webhook_url, payload } = req.body;

    // Rivers.http appropriate here — URL is runtime-dynamic
    // WARN rivers::handler: ad_hoc_http_call will appear in logs
    const response = await Rivers.http.post(webhook_url, { body: payload });

    yield { delivered: true, status: response.status };
}
```

---

## AMD-2

### Remove Rivers.lockbox.get()

**Patches:** `rivers-streaming-rest-spec.md`, `rivers-processpool-runtime-spec-v2.md` (§8)

#### Problem

The streaming spec example called `Rivers.lockbox.get("anthropic_key")` to retrieve a raw credential inside a CodeComponent handler. The ProcessPool spec explicitly states the raw credential never enters the isolate — the lockbox is resolved host-side via opaque resource tokens. `Rivers.lockbox.get()` returning a plaintext credential into an isolate directly contradicts the security model.

#### Resolution

`Rivers.lockbox.get()` is **removed from the Rivers API surface entirely**. It does not exist.

Credentials are a datasource concern, not a handler concern. The HTTP driver datasource holds the LockBox reference at the config level — the CodeComponent never sees the credential. The lockbox resource token model (opaque token → host resolves → result returned) is already specified in the ProcessPool spec and is the only credential access path.

The one legitimate use of lockbox access from a handler is the RPS secret broker pattern — already specified as `Rivers.lockbox.fetchEncrypted()`, which returns an encrypted envelope, not the raw credential. That API is retained. Raw credential retrieval via any handler API is not permitted.

#### ProcessPool spec amendment

Section 8 (Rivers API surface) removes `Rivers.lockbox.get()` and adds:

> **Credentials are never available in handler code.** LockBox references are declared at the datasource config level. The CodeComponent receives opaque resource tokens for datasource access. Raw credentials do not enter the isolate. `Rivers.lockbox.fetchEncrypted()` (RPS secret broker only) returns an encrypted envelope — not a plaintext credential.

---

## AMD-3

### Define Rivers.crypto API Surface

**Patches:** `rivers-processpool-runtime-spec-v2.md` (§8)

#### Problem

The auth spec example called `Rivers.crypto.verifyPassword()` — an API with no definition in the ProcessPool spec. An agent implementing the auth example would call an API that doesn't exist.

#### Resolution

`Rivers.crypto` is added to the Rivers API surface in the ProcessPool spec §8:

```typescript
Rivers.crypto = {
    // Password hashing — bcrypt with configurable cost factor
    async hashPassword(
        password: string,
        cost?: number           // default: 12
    ): Promise<string>,         // returns hash string

    // Password verification — constant-time comparison
    async verifyPassword(
        password: string,
        hash: string
    ): Promise<boolean>,

    // Cryptographically secure random bytes as hex string
    randomHex(bytes: number): string,

    // Cryptographically secure random bytes as base64url string
    randomBase64url(bytes: number): string,

    // HMAC-SHA256 — key is a LockBox alias, resolved host-side
    async hmac(
        data:        string,
        lockboxAlias: string    // key never enters isolate
    ): Promise<string>,         // returns hex digest

    // Constant-time string comparison — prevents timing attacks
    timingSafeEqual(a: string, b: string): boolean,
}
```

#### Security notes

- `hashPassword` and `verifyPassword` use bcrypt. Cost factor defaults to 12. Minimum enforced: 10. Values below 10 rejected at call time.
- `hmac` resolves the signing key via LockBox resource token — the raw key never enters the isolate.
- `timingSafeEqual` is provided explicitly to prevent timing oracle vulnerabilities in auth code. Handlers that compare secrets (API keys, tokens) must use this, not `===`.
- `Rivers.crypto` is available in all CodeComponents regardless of pool config — no capability declaration required.

---

## AMD-4

### Remove REST Polling

**Patches:** `rivers-polling-views-spec.md`

#### Problem

Polling on REST views was spec'd as valid but described as producing a "side-effect-only machine" where the `on_change` return value is discarded. The loop lifecycle was undefined — SSE/WebSocket loops are client-driven (start on first connect, stop on last disconnect), but REST has no persistent connection. There is no client event to drive loop start or stop, no defined parameter source for the deduplication key, and no defined lifetime.

REST polling is structurally a background job, not a view behavior. Speccing it as a view property creates a logical contradiction.

#### Resolution

**Polling is only valid on `view_type = ServerSentEvents` and `view_type = Websocket`.** Declaring `polling` on a REST view is a config validation error.

```
RiversError::Validation: polling is only valid for ServerSentEvents and Websocket views
```

#### Polling spec amendments

The following are removed from `rivers-polling-views-spec.md`:

- §8.3 "REST views with polling"
- §11.4 "REST polling — side-effect machine" example
- All references to `REST polling` throughout

The scope line is updated:

```
Scope: Polling configuration for SSE and WebSocket views — diff strategies, 
       change detection, on_change handler, poll loop lifecycle
```

The validation rules table adds:

| Rule | Error message |
|---|---|
| `polling` declared on `view_type = Rest` | `polling is only valid for ServerSentEvents and Websocket views` |

---

## AMD-5

### Guard View Auto-Detects Browser vs API Mode

**Patches:** `rivers-auth-session-spec.md`

#### Problem

The guard view model was browser-centric by default — redirect-driven, cookie-based, with `include_token_in_body` buried as an optional flag. A mobile app or REST API client cannot follow browser redirects for auth. The spec never framed API/mobile as a first-class mode, making it easy for an agent building a mobile backend to conclude Rivers doesn't support it.

#### Resolution

Rivers detects client mode automatically from the guard handler's return value. No `mode` field in config. The handler decides by what it returns.

#### Auto-detection rules

| Handler returns | Rivers behavior |
|---|---|
| `IdentityClaims` object (no `_response` key) | **Browser mode** — Rivers sets cookie, redirects to `valid_session_url` |
| `{ _response: Rivers.Response, claims: IdentityClaims }` | **API mode** — Rivers sets cookie AND returns the `_response` body to client. No redirect. |

The `_response` key is the signal. Rivers strips it before storing claims — it never appears in the session payload.

#### Browser mode (default)

```typescript
// Return claims only → browser mode
async function authenticate(
    req: Rivers.Request
): Promise<IdentityClaims> {
    const user = await validateCredentials(req.body);
    return {
        subject: user.id,
        email:   user.email,
        role:    user.role,
    };
    // Rivers: sets cookie, redirects to valid_session_url
}
```

#### API / mobile mode

```typescript
// Return _response + claims → API mode
async function authenticate(
    req: Rivers.Request
): Promise<{ _response: Rivers.Response; claims: IdentityClaims }> {
    const user = await validateCredentials(req.body);

    const claims: IdentityClaims = {
        subject: user.id,
        email:   user.email,
        role:    user.role,
    };

    return {
        claims,
        _response: {
            status: 200,
            body: {
                user_id: user.id,
                email:   user.email,
                // session token included automatically — see below
            }
        }
    };
    // Rivers: sets cookie AND returns _response body
    // No redirect
}
```

#### Session token in API mode

In API mode, Rivers automatically injects the session token into the response body under `_token` key before sending. The CodeComponent does not construct the token — Rivers owns that. The client receives:

```json
{
    "user_id": "42",
    "email": "user@example.com",
    "_token": "sess_abc123xyz"
}
```

The `_token` key is reserved. If the CodeComponent response body already contains `_token`, config validation rejects it at deploy time.

API clients pass the token on subsequent requests as `Authorization: Bearer sess_abc123xyz`. Rivers accepts session tokens from both cookie and Authorization Bearer header — cookie takes precedence if both present.

#### on_valid_session / on_invalid_session in API mode

`on_valid_session` in API mode should return a `Rivers.Response` (not void/redirect) to serve the intended resource directly. `on_invalid_session` in API mode should return a `Rivers.Response` with `status: 401` — not a redirect. The auto-detection logic applies to these handlers as well:

- Return void → Rivers applies default behavior (redirect in browser mode, 401 JSON in API mode)
- Return `Rivers.Response` → Rivers uses it directly regardless of mode

Rivers infers the session mode from the guard handler's initial return shape and applies it consistently to all subsequent session handler behaviors on that request.

---

## AMD-6

### CSRF Auto-Generation and Validation

**Patches:** `rivers-auth-session-spec.md`, `rivers-httpd-spec.md`

#### Problem

The auth spec configured `SameSite=Lax` cookies. For a SPA making POST/PUT/DELETE AJAX requests, `SameSite=Lax` only protects top-level navigations — not cross-site AJAX. Rivers targets SPA applications. The CSRF attack surface was unaddressed.

#### Resolution

Rivers auto-generates and validates CSRF tokens for browser-mode sessions. API/mobile mode sessions (Authorization Bearer header) are exempt — CSRF is a browser cookie attack vector.

#### Token generation

When a browser-mode session is created, Rivers generates a CSRF token alongside the session:

```
csrf_token = Rivers.crypto.randomBase64url(32)
```

Stored in StorageEngine under `csrf:{session_id}` with the same TTL as the session.

#### Token delivery

The CSRF token is delivered to the browser via a **non-HttpOnly cookie** — intentionally JS-readable so the SPA can read and send it:

```
Set-Cookie: rivers_csrf=<token>; SameSite=Strict; Secure; Path=/
```

Note: `rivers_csrf` is `SameSite=Strict` and does NOT have `HttpOnly`. This is the double-submit cookie pattern — the SPA reads the cookie value via JS and sends it as a header. The session cookie remains `HttpOnly`.

#### Token validation

Rivers validates the CSRF token on all state-mutating requests (POST, PUT, PATCH, DELETE) for browser-mode sessions. Validation checks:

1. `X-CSRF-Token` request header present
2. Header value matches StorageEngine value for `csrf:{session_id}`
3. Constant-time comparison via `timingSafeEqual`

If validation fails: `403 Forbidden`, request rejected before the view executes.

#### Exemptions

CSRF validation is skipped when:
- Request uses `Authorization: Bearer` header (API/mobile mode — not a cookie-based session)
- View declares `auth = "none"` (public view)
- Request method is GET, HEAD, or OPTIONS (safe methods — no state mutation)

#### SPA integration

```javascript
// SPA reads CSRF token from cookie (JS-readable — no HttpOnly)
function getCsrfToken() {
    return document.cookie
        .split('; ')
        .find(row => row.startsWith('rivers_csrf='))
        ?.split('=')[1];
}

// Include on all state-mutating requests
fetch('/api/orders', {
    method: 'POST',
    headers: {
        'Content-Type':  'application/json',
        'X-CSRF-Token':  getCsrfToken(),
    },
    body: JSON.stringify(order),
});
```

#### Configuration

CSRF protection is on by default for browser-mode sessions. Operators may disable it (not recommended):

```toml
[security.session]
csrf_protection = true     # default: true
csrf_header     = "X-CSRF-Token"    # header name (default: "X-CSRF-Token")
```

CSRF token is rotated on each session renewal (when `last_seen` is updated and the session is within `idle_timeout_s`). Rotation frequency is bounded — Rivers rotates at most once per `csrf_rotation_interval_s` (default: 300) to avoid token churn on high-frequency polling views.

---

## AMD-7

### Rivers.view.stream() for Upstream Streaming Consumption

**Patches:** `rivers-processpool-runtime-spec-v2.md` (§8), `rivers-http-driver-spec.md`

#### Problem

The streaming spec example called `Rivers.http.post("...", { stream: true })` with `.events()` — both undefined APIs. The HTTP driver spec defines how Rivers consumes upstream SSE/WebSocket at the bridge level, but provided no API for a CodeComponent to consume a streaming HTTP DataView response. An agent building an LLM token proxy had no defined path.

#### Resolution

`Rivers.view.stream()` is added to the Rivers API surface. It executes an HTTP DataView that is configured for streaming response consumption and returns an `AsyncIterable` of chunks.

#### API definition

Added to ProcessPool spec §8:

```typescript
Rivers.view = {
    // Existing — non-streaming DataView execution
    async query(
        dataview: string,
        params?: Record<string, any>
    ): Promise<any[]>,

    // NEW — streaming DataView execution
    // Returns AsyncIterable — only valid for HTTP driver DataViews
    // configured with streaming_response = true
    stream(
        dataview: string,
        params?: Record<string, any>
    ): AsyncIterable<any>,
}
```

`Rivers.view.stream()` is synchronous in construction (returns `AsyncIterable` immediately) and async in consumption (each `for await` iteration awaits the next chunk from the upstream). The upstream connection is established on first iteration, not at call time.

#### HTTP DataView streaming config

An HTTP DataView declares `streaming_response = true` to indicate the upstream returns a streaming response (SSE or ndjson):

```toml
[data.dataviews.generate_completion]
datasource         = "anthropic"
method             = "POST"
path               = "/v1/messages"
streaming_response = true           # enables Rivers.view.stream()
response_format    = "sse"          # "sse" | "ndjson" — how to parse upstream chunks

[data.dataviews.generate_completion.body_template]
model      = "claude-sonnet-4-20250514"
max_tokens = 1024
stream     = true
messages   = [{role = "user", content = "{prompt}"}]

[[data.dataviews.generate_completion.parameters]]
name     = "prompt"
location = "body"
required = true
```

`Rivers.view.query()` on a `streaming_response = true` DataView is a validation error at deploy time — streaming DataViews must use `Rivers.view.stream()`.

#### Chunk shape

Each iteration yields a parsed chunk from the upstream. For `response_format = "sse"`, the chunk is:

```typescript
interface StreamChunk {
    event?: string;     // SSE event type (if present)
    data:   any;        // parsed JSON data payload
    id?:    string;     // SSE id field (if present)
}
```

For `response_format = "ndjson"`, the chunk is the parsed JSON object directly.

#### Corrected LLM proxy example

Replaces the broken `Rivers.http.post(..., { stream: true })` example in the streaming spec:

```typescript
// handlers/llm.ts
export async function* generate(
    req: Rivers.Request
): AsyncGenerator<any> {
    const { prompt, model } = req.body as { prompt: string; model: string };

    Rivers.log.info("starting generation", { model, trace_id: req.trace_id });

    let total_tokens = 0;

    // Rivers.view.stream() — clean API, no raw HTTP, no undefined methods
    for await (const chunk of Rivers.view.stream("generate_completion", { prompt })) {
        if (chunk.data?.type === "content_block_delta") {
            yield { token: chunk.data.delta.text };
            total_tokens++;
        }
        if (chunk.data?.type === "message_stop") {
            break;
        }
    }

    yield { done: true, total_tokens };
}
```

#### Error handling

If the upstream stream terminates with an error mid-iteration, the `AsyncIterable` throws on the next `for await`. The generator's try/catch handles it normally:

```typescript
try {
    for await (const chunk of Rivers.view.stream("generate_completion", { prompt })) {
        yield { token: chunk.data.delta?.text };
    }
} catch (err) {
    // Upstream stream error — handler threw after possible yields
    // Rivers emits poison chunk automatically
    throw err;
}
```

---

## AMD-8

### change_detect Handler May Be Async

**Patches:** `rivers-polling-views-spec.md` (§4.3, §9)

#### Problem

The `change_detect` handler was required to be synchronous (`bool`, not `Promise<bool>`). The stated rationale was keeping diff lightweight. In practice, meaningful change detection often requires a database lookup — checking whether a price change is significant requires knowing the user's watchlist, checking whether an order status change matters requires knowing the user's notification preferences. The sync constraint forces these cases into an awkward workaround: put the real logic in `on_change` and treat `change_detect` as a no-op.

#### Resolution

`change_detect` may be synchronous or async. Rivers awaits the result either way.

#### Updated contract

```typescript
// Both forms are valid

// Synchronous — for pure in-memory comparisons
function detectChange(prev: any, current: any): boolean { }

// Async — for comparisons requiring datasource lookups
async function detectChange(prev: any, current: any): Promise<boolean> { }
```

#### Timeout

`change_detect` runs in ProcessPool under `task_timeout_ms` — the same timeout as non-streaming CodeComponent handlers. The tick is treated as no-change if `change_detect` exceeds the timeout. This is the existing behavior — the only change is removing the synchronous requirement.

#### Amended validation rule

The following validation rule is **removed**:

| ~~Rule~~ | ~~Error message~~ |
|---|---|
| ~~`change_detect` handler is async~~ | ~~`change_detect handler must be synchronous`~~ |

#### Amended section §4.3

The last paragraph of §4.3 is updated from:

> The function is synchronous — it must return `bool` directly, not `Promise<bool>`. ...If the function is async (returns `Promise`), Rivers rejects it at deploy time with `change_detect handler must be synchronous`.

To:

> The function may be synchronous or async. Rivers awaits the return value. If async, it runs under `task_timeout_ms` — keep diff logic focused. A `change_detect` handler that makes multiple datasource round-trips on every tick will become a performance bottleneck at high poll frequency.

---

## Amendment Summary

| ID | Issue | Action | Specs Affected |
|---|---|---|---|
| AMD-1 | `Rivers.http` conflict with HTTP driver | Retained as escape hatch, audit logging at startup + call time | Streaming, ProcessPool |
| AMD-2 | `Rivers.lockbox.get()` violates security model | Removed from API surface | Streaming, ProcessPool |
| AMD-3 | `Rivers.crypto` undeclared | Defined: hashPassword, verifyPassword, randomHex, randomBase64url, hmac, timingSafeEqual | ProcessPool |
| AMD-4 | REST polling undefined lifecycle | Removed — polling SSE/WebSocket only | Polling |
| AMD-5 | Guard view browser-only by default | Auto-detect from handler return value — `_response` key signals API mode | Auth |
| AMD-6 | CSRF unaddressed | Auto-generate and validate — double-submit cookie pattern, on by default | Auth, HTTPD |
| AMD-7 | Streaming upstream consumption undefined | `Rivers.view.stream()` returning AsyncIterable, `streaming_response = true` on HTTP DataViews | Streaming, HTTP Driver, ProcessPool |
| AMD-8 | `change_detect` sync constraint too restrictive | Dropped — async allowed, runs under task_timeout_ms | Polling |
