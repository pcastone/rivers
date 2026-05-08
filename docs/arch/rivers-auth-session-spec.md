# Rivers Authentication & Session Specification

**Document Type:** Spec Addition  
**Scope:** Guard view, session lifecycle, per-view session validation, StorageEngine integration  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-view-layer-spec.md`, `rivers-httpd-spec.md`, `rivers-storage-engine-spec.md`  
**Depends On:** Epic 5 (LockBox), Epic 10 (DataView Engine), Epic 13 (View Layer), StorageEngine

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [Mental Model](#2-mental-model)
3. [Guard View](#3-guard-view)
4. [Session Lifecycle](#4-session-lifecycle)
5. [Per-View Session Validation](#5-per-view-session-validation)
6. [Session Handlers](#6-session-handlers)
7. [StorageEngine Integration](#7-storageengine-integration)
8. [Token Delivery](#8-token-delivery)
9. [Validation Rules](#9-validation-rules)
10. [Configuration Reference](#10-configuration-reference)
11. [Examples](#11-examples)

---

## 1. Design Rationale

### 1.1 The Problem

Auth is an application concern, not a protocol concern. Rivers does not know whether a given application uses JWT, OAuth2, SAML, API keys, magic links, or a custom scheme. Attempting to natively validate all of these inside the framework produces a brittle, incomplete abstraction that breaks every time a new mechanism appears.

The correct boundary is: **Rivers owns session lifecycle, the application owns credential validation.**

### 1.2 Design Principles

**One guard, one place.** A single guard view is the sole entry point for credential validation. All credential-specific logic lives in its CodeComponent. Rivers sees a pass/fail signal, not the credential itself.

**Rivers owns the session.** Token generation, signing, storage, delivery, and expiry are all Rivers concerns. The guard CodeComponent returns identity claims. Rivers turns them into a session.

**All other views are session consumers.** They declare what happens when a session is valid or invalid. Rivers handles validation automatically. No boilerplate.

**Single guard point.** One guard view per server for v1.

---

## 2. Mental Model

```
                    ┌─────────────────────────────┐
                    │         Guard View           │
                    │  guard = true                │
                    │  CodeComponent validates      │
                    │  any credential mechanism     │
                    └──────────────┬──────────────┘
                                   │
               ┌───────────────────┼───────────────────┐
               │                   │                   │
          Credential          Valid session        Auth attempt
          valid (new)           exists               failed
               │                   │                   │
        on_valid_session    on_valid_session        on_failed
        redirect to         redirect to             (stays on
        valid_session_url   valid_session_url        guard or
        Rivers sets         Rivers sets              custom redirect)
        session cookie      session cookie
               │
               ▼
     ┌─────────────────────┐
     │    Protected Views  │
     │                     │
     │  Rivers validates   │
     │  session on every   │
     │  request            │
     │                     │
     │  Valid session ──── on_session_valid ──── primary execution
     │                                           (DataView/CodeComponent)
     │                     │
     │  Invalid session ── on_invalid_session ── auto-redirect to
     │                                           guard invalid_session_url
     └─────────────────────┘
```

---

## 3. Guard View

### 3.1 Declaration

A view becomes the guard by declaring `guard = true`. Only one guard view may exist per server. Config validation rejects a second guard declaration.

```toml
[api.views.auth]
path      = "/auth"
method    = "POST"
view_type = "Rest"
guard     = true

[api.views.auth.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/auth.ts"
entrypoint = "authenticate"
resources  = ["users_db"]

[api.views.auth.guard]
valid_session_url   = "/app/dashboard"
invalid_session_url = "/auth/login"

[api.views.auth.guard.on_valid_session]
module     = "handlers/auth.ts"
entrypoint = "onValidSession"

[api.views.auth.guard.on_invalid_session]
module     = "handlers/auth.ts"
entrypoint = "onInvalidSession"

[api.views.auth.guard.on_failed]
module     = "handlers/auth.ts"
entrypoint = "onAuthFailed"
```

### 3.2 Guard CodeComponent contract

The guard CodeComponent is responsible for all credential validation. It receives the full request and returns either an identity claims object (browser mode) or a response envelope (API/mobile mode). Rivers detects the mode automatically from the return value — no config field required.

```typescript
interface IdentityClaims {
    subject:    string;                    // required — unique user/principal identifier
    [key: string]: any;                    // any additional claims (roles, tenant, email, etc.)
}

interface ApiAuthResult {
    _response: Rivers.Response;            // response sent to client (must include token)
    [key: string]: any;                    // claims stored in session (minus _response key)
}

// Guard CodeComponent — two valid return shapes
async function authenticate(
    req: Rivers.Request
): Promise<IdentityClaims | ApiAuthResult> {
    // return IdentityClaims → browser mode (Rivers sets cookie, redirects)
    // return { _response, ...claims } → API mode (Rivers sends _response, sets cookie)
    // throw → Rivers calls on_failed, no session created
}
```

**Browser mode** — handler returns plain `IdentityClaims`. Rivers creates a session, stores it in StorageEngine, sets the `rivers_session` cookie, and redirects to `valid_session_url`. Used for SPAs and traditional browser flows.

**API/mobile mode** — handler returns an object with a `_response` key. Rivers creates a session, sets the cookie, and sends the `_response` value directly to the client — no redirect. The `_response` key is consumed by the framework and does not appear in stored session claims. Suitable for mobile apps and REST API clients that cannot follow browser redirects.

```typescript
// Browser mode — SPA / traditional web
async function authenticate(req: Rivers.Request): Promise<IdentityClaims> {
    const user = await validateCredentials(req.body);
    return { subject: user.id, roles: user.roles, email: user.email };
    // Rivers → set cookie → redirect to valid_session_url
}

// API/mobile mode — _response key signals the difference
async function authenticate(req: Rivers.Request): Promise<ApiAuthResult> {
    const user = await validateCredentials(req.body);
    return {
        _response: { status: 200, body: { expires_in: 3600 } },
        subject:   user.id,
        roles:     user.roles,
    };
    // Rivers → set cookie → send _response → no redirect
}
```

**On throw** — Rivers calls `on_failed`. No session is created. No cookie is set.

Rivers does not inspect the claims fields — it stores them verbatim as the session payload. The `subject` field is required for session keying. All other fields are the application's concern.

### 3.3 Guard behavior on existing valid session

When a request arrives at the guard view and a valid session already exists (session cookie present and valid), Rivers skips the CodeComponent entirely — no re-authentication. It calls `on_valid_session` directly and redirects to `valid_session_url`.

This is the standard "already logged in" path — hitting the login page when already authenticated.

### 3.4 Guard behavior on existing invalid session

When a request arrives at the guard view with a session cookie that is expired or invalid, Rivers clears the cookie, calls `on_invalid_session`, and redirects to `invalid_session_url`.

### 3.5 Guard handlers

All three guard handlers are optional. If not declared, Rivers applies the default behavior for each case.

#### `on_valid_session`

Fires when a valid session exists or when a new session has just been created after successful authentication.

```typescript
async function onValidSession(
    req: Rivers.Request,
    session: Rivers.Session
): Promise<Rivers.Response | void> { }
```

`session` contains the stored `IdentityClaims` and session metadata. Return value can:
- Override the redirect destination by returning `{ redirect: "/custom/path" }`
- Return a full `Rivers.Response` to skip redirect entirely (e.g., return JSON for SPA auth flows)
- Return void → Rivers uses `valid_session_url`

#### `on_invalid_session`

Fires when a session cookie is present but invalid or expired.

```typescript
async function onInvalidSession(
    req: Rivers.Request
): Promise<Rivers.Response | void> { }
```

Return value can override `invalid_session_url` or return a custom response. Return void → Rivers uses `invalid_session_url`.

#### `on_failed`

Fires when the guard CodeComponent throws.

```typescript
interface AuthFailedContext {
    error:      string;
    error_type: string;
    request:    Rivers.Request;
}

async function onAuthFailed(
    ctx: AuthFailedContext
): Promise<Rivers.Response | void> { }
```

Return value can return a custom error response or redirect. Return void → Rivers returns 401 with default error body.

---

## 4. Session Lifecycle

### 4.1 Session creation

After successful guard CodeComponent execution:

1. Rivers generates a cryptographically random session ID (`session_id`)
2. Session payload constructed:
```rust
pub struct Session {
    pub session_id:  String,
    pub subject:     String,               // from IdentityClaims.subject
    pub claims:      serde_json::Value,    // full IdentityClaims object
    pub created_at:  DateTime<Utc>,
    pub expires_at:  DateTime<Utc>,
    pub last_seen:   DateTime<Utc>,
}
```
3. Session stored in StorageEngine under `session:{session_id}`
4. Session cookie set on response (see §8)
5. `on_valid_session` fires
6. Redirect to `valid_session_url`

### 4.2 Session validation

On every non-guard request, Rivers validates the session before the view executes:

1. Extract session cookie from request
2. If absent → invalid session path
3. Look up `session:{session_id}` in StorageEngine
4. If not found → invalid session path
5. If `expires_at < now()` → expired, delete from StorageEngine → invalid session path
6. Update `last_seen` in StorageEngine
7. Attach session to request context → `on_session_valid` fires → primary execution

### 4.3 Session expiry

```toml
[security.session]
ttl_s          = 3600      # session lifetime from last activity (default: 3600)
idle_timeout_s = 1800      # expire if idle for this long (default: 1800)
```

Two independent expiry mechanisms:
- `ttl_s` — absolute lifetime from `created_at`. Session expires regardless of activity.
- `idle_timeout_s` — inactivity timeout. `last_seen` updated on each valid request. If `now() - last_seen > idle_timeout_s` → expired.

Whichever fires first expires the session.

### 4.4 Session termination

Logout is a standard REST view that calls `Rivers.session.destroy()`:

```typescript
async function logout(req: Rivers.Request): Promise<Rivers.Response> {
    await Rivers.session.destroy(req.session.session_id);
    return { redirect: "/auth/login" };
}
```

`Rivers.session.destroy()` deletes the StorageEngine entry and instructs Rivers to clear the session cookie on the response. If the session is cluster-shared (Redis backend), deletion propagates immediately across all nodes.

### 4.5 Rivers.session API

Available in all CodeComponents on protected views:

```typescript
Rivers.session = {
    // Current session — populated after validation
    current: {
        session_id: string,
        subject:    string,
        claims:     any,            // full IdentityClaims
        created_at: string,         // ISO 8601
        expires_at: string,
        last_seen:  string,
    },

    // Destroy the current session (logout)
    destroy(session_id: string): Promise<void>,
}
```

---

## 5. Per-View Session Validation

### 5.1 Protected vs public views

All views are **protected by default** — Rivers validates the session before execution. A view is made public by declaring `auth = "none"`:

```toml
[api.views.health]
path   = "/health"
method = "GET"
auth   = "none"          # public — no session validation
```

The guard view itself is implicitly public — it is the auth entry point.

### 5.2 Automatic invalid session handling

When a session is invalid on a protected view and no `on_invalid_session` handler is declared, Rivers automatically redirects to the guard's `invalid_session_url`. The view's primary handler never executes.

This is the zero-config path — declare a guard, protect views automatically, no boilerplate.

### 5.3 Session context in ViewContext

After successful session validation, the session is attached to `ViewContext`:

```rust
pub struct ViewContext {
    pub request:    ParsedRequest,
    pub sources:    HashMap<String, serde_json::Value>,
    pub meta:       HashMap<String, serde_json::Value>,
    pub trace_id:   String,
    pub session:    Option<Session>,    // None only on auth = "none" views
}
```

Available in all pipeline handlers and CodeComponents as `ctx.session` (Rust) or `req.session` / `Rivers.session.current` (TypeScript).

### 5.4 MessageConsumer session exemption

MessageConsumer views are **auto-exempt from session validation** by default. Broker-delivered messages arrive outside any HTTP request context — there is no cookie, no Authorization header, and no client connection to validate against.

```toml
[api.views.order_events]
view_type = "MessageConsumer"
# no auth declaration needed — auto-exempt
```

**Opt-in session validation:** A MessageConsumer view can declare `auth = "session"` to validate sessions against a `_session_id` field in the message payload. Rivers looks up `session:{_session_id}` in StorageEngine and attaches it to `ViewContext` if valid. If the session is missing or expired, the message is rejected (handler does not execute) and the rejection is logged.

```toml
[api.views.user_notifications]
view_type = "MessageConsumer"
auth      = "session"     # expects _session_id in message payload
```

Message payload shape required for `auth = "session"`:

```json
{ "_session_id": "sess_abc123", "...": "..." }
```

The `_session_id` field is consumed by the framework and does not appear in `ctx.sources["primary"]`.

---

## 6. Session Handlers

Per-view session handlers are optional. They run after session validation and before primary execution.

### 6.1 `on_session_valid`

Fires when session validation succeeds. Can deposit into `ctx.sources`, modify context, or redirect.

```typescript
interface OnSessionValidResult {
    key:      string;
    data:     any;
    redirect?: string;     // if set, redirect immediately — primary execution skipped
}

async function onSessionValid(
    req: Rivers.Request,
    session: Rivers.Session
): Promise<OnSessionValidResult | void> { }
```

Typical use: load user profile, resolve tenant, inject RBAC roles into `ctx.sources["identity"]` for downstream handlers. If `redirect` is returned, the primary handler does not execute.

### 6.2 `on_invalid_session`

Fires when session validation fails on a protected view. Can return a custom response or redirect.

```typescript
async function onInvalidSession(
    req: Rivers.Request
): Promise<Rivers.Response | void> { }
```

Return void → Rivers redirects to guard's `invalid_session_url`.  
Return `{ redirect: "/custom" }` → override redirect destination.  
Return full `Rivers.Response` → custom response (e.g., 401 JSON for API clients).

---

## 7. StorageEngine Integration

### 7.1 Requirement

Session management requires StorageEngine. If any protected view exists (or a guard is declared) and StorageEngine is not configured, the server fails at startup:

```
RiversError::Validation: session management requires storage_engine to be configured
```

### 7.2 Storage schema

| Key | Value | TTL |
|---|---|---|
| `session:{session_id}` | Full `Session` struct as JSON | `ttl_s` |

TTL is set at write time to `ttl_s`. Idle timeout is enforced at read time — on session lookup, Rivers checks `last_seen` against `idle_timeout_s` and treats expired-by-idle sessions the same as not-found.

### 7.3 Cluster behavior

| StorageEngine backend | Session behavior |
|---|---|
| `in_memory` | Node-local. Sessions created on one node are not visible to others. Single-node only. |
| `sqlite` | Node-local. Same limitation. |
| `redis` | Shared across all cluster nodes. Required for multi-node deployments. |

Operators running multi-node clusters must configure Redis-backed StorageEngine for sessions to work correctly across nodes.

---

## 8. Token Delivery

Rivers owns session token generation and delivery entirely. The guard CodeComponent never sees the session ID.

### 8.1 Cookie attributes

```toml
[security.session.cookie]
name      = "rivers_session"    # cookie name (default: "rivers_session")
http_only = true                # default: true — not accessible via JS
secure    = true                # default: true — HTTPS only
same_site = "Lax"               # "Strict" | "Lax" | "None" (default: "Lax")
path      = "/"                 # default: "/"
domain    = ""                  # default: not set (current domain only)
```

`http_only = true` is enforced — this is not configurable to false. Session cookies must not be accessible via JavaScript. Config validation rejects `http_only = false`.

`secure = true` is the default. Acceptable to set `secure = false` for local development only — emits a `tracing::warn!` at startup if set to false.

### 8.2 SPA / API clients

For single-page applications or API clients that cannot use cookies (e.g., mobile apps), the session token can optionally be returned in the response body in addition to the cookie. Configured per guard:

```toml
[api.views.auth.guard]
include_token_in_body = true    # default: false
token_body_key        = "token" # key in response JSON (default: "token")
```

When `include_token_in_body = true`, the guard response body includes the session ID under `token_body_key`. API clients can then pass it as `Authorization: Bearer <token>` on subsequent requests. Rivers accepts session tokens from both cookie and Authorization Bearer header — cookie takes precedence if both present.

---

## 9. CSRF Protection

### 9.1 Overview

Rivers auto-generates and validates CSRF tokens for browser-mode sessions (cookie-based). API/mobile sessions using `Authorization: Bearer` are exempt — Bearer tokens are not sent by browsers automatically, so CSRF does not apply.

CSRF protection uses the **double-submit cookie pattern**: Rivers sets a separate `rivers_csrf` cookie (readable by JavaScript, no `HttpOnly`) containing a random token. The SPA reads this cookie and includes the value in `X-CSRF-Token` on every state-mutating request. Rivers validates that the header value matches the stored token.

### 9.2 Token lifecycle

- Token is generated at session creation time and stored in StorageEngine under `csrf:{session_id}`
- Token TTL matches session TTL
- Token is rotated at most once per `csrf_rotation_interval_s` (default: 300s) — prevents constant rotation on high-frequency SSE/polling views from invalidating in-flight SPA requests
- On session destroy, the `csrf:{session_id}` key is deleted

### 9.3 Validation rules

| Request condition | CSRF required |
|---|---|
| Browser session (cookie), state-mutating method (POST/PUT/PATCH/DELETE) | Yes — `X-CSRF-Token` header must match |
| Browser session (cookie), safe method (GET/HEAD/OPTIONS) | No |
| API/mobile session (`Authorization: Bearer`) | No — exempt |
| `auth = "none"` view | No |
| MessageConsumer view | No — exempt by default (see §5.4) |
| Streaming POST, browser session | Yes — same as non-streaming |

### 9.4 Failure response

CSRF validation failure returns `403 Forbidden`:

```json
{ "error": "csrf_invalid" }
```

No redirect. The SPA handles 403 by re-reading the `rivers_csrf` cookie — Rivers sets a fresh token on the next successful GET.

### 9.5 Configuration

```toml
[security.csrf]
enabled                  = true            # default: true
csrf_rotation_interval_s = 300             # min seconds between rotations (default: 300)
cookie_name              = "rivers_csrf"   # default
header_name              = "X-CSRF-Token" # default
```

`enabled = false` is valid only for API-only deployments (all views use Bearer auth). Disabling CSRF when any cookie-session views are declared emits `tracing::warn!` at startup.

---

## 10. Validation Rules

| Rule | Error message |
|---|---|
| More than one `guard = true` view declared | `only one guard view is allowed per server` |
| Guard view without `valid_session_url` | `guard requires valid_session_url` |
| Guard view without `invalid_session_url` | `guard requires invalid_session_url` |
| Guard view handler is not `CodeComponent` | `guard view requires a codecomponent handler` |
| Protected view with no guard declared | `protected views require a guard view to be declared` |
| StorageEngine not configured with sessions active | `session management requires storage_engine to be configured` |
| `session.cookie.http_only = false` | `http_only must be true — session cookies must not be JS-accessible` |
| `auth = "none"` on guard view | `guard view cannot declare auth = none` |

---

## 10. Configuration Reference

### 10.1 Guard view

```toml
[api.views.auth]
path      = "/auth"
method    = "POST"
view_type = "Rest"
guard     = true

[api.views.auth.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/auth.ts"
entrypoint = "authenticate"
resources  = ["users_db"]

[api.views.auth.guard]
valid_session_url     = "/app/dashboard"
invalid_session_url   = "/auth/login"
include_token_in_body = false

[api.views.auth.guard.on_valid_session]
module     = "handlers/auth.ts"
entrypoint = "onValidSession"

[api.views.auth.guard.on_invalid_session]
module     = "handlers/auth.ts"
entrypoint = "onInvalidSession"

[api.views.auth.guard.on_failed]
module     = "handlers/auth.ts"
entrypoint = "onAuthFailed"
```

### 10.2 Session config

```toml
[security.session]
ttl_s          = 3600
idle_timeout_s = 1800

[security.session.cookie]
name      = "rivers_session"
http_only = true
secure    = true
same_site = "Lax"
path      = "/"
```

### 10.3 Protected view with session handlers

```toml
[api.views.dashboard]
path      = "/app/dashboard"
method    = "GET"
view_type = "Rest"

[api.views.dashboard.handler]
type     = "dataview"
dataview = "get_dashboard_data"

[api.views.dashboard.on_session_valid]
module     = "handlers/session.ts"
entrypoint = "loadUserContext"

[api.views.dashboard.on_invalid_session]
module     = "handlers/session.ts"
entrypoint = "handleInvalidSession"
```

### 10.4 Public view

```toml
[api.views.health]
path      = "/health"
method    = "GET"
view_type = "Rest"
auth      = "none"

[api.views.health.handler]
type     = "dataview"
dataview = "health_check"
```

---

## 11. Examples

### 11.1 Username/password authentication

```typescript
// handlers/auth.ts

async function authenticate(
    req: Rivers.Request
): Promise<IdentityClaims> {
    const { username, password } = req.body as {
        username: string;
        password: string;
    };

    if (!username || !password) {
        throw new Error("username and password are required");
    }

    const rows = await Rivers.db.query(
        Rivers.resources.users_db,
        "SELECT id, email, password_hash, role FROM users WHERE username = $1",
        [username]
    );

    if (rows.length === 0) {
        throw new Error("invalid credentials");
    }

    const user = rows[0];
    const valid = await Rivers.crypto.verifyPassword(password, user.password_hash);

    if (!valid) {
        throw new Error("invalid credentials");
    }

    // Return claims — Rivers creates the session
    return {
        subject: user.id,
        email:   user.email,
        role:    user.role,
    };
}

async function onValidSession(
    req: Rivers.Request,
    session: Rivers.Session
): Promise<void> {
    Rivers.log.info("user authenticated", {
        subject: session.claims.subject,
        email:   session.claims.email,
    });
    // void → Rivers redirects to valid_session_url
}

async function onAuthFailed(
    ctx: AuthFailedContext
): Promise<Rivers.Response> {
    Rivers.log.warn("auth failed", { error: ctx.error });
    return {
        status: 401,
        body: { error: "invalid credentials" }
    };
}
```

### 11.2 OAuth2 / SSO guard

```typescript
// Guard handles OAuth2 callback — credential validation is the token exchange
async function authenticate(
    req: Rivers.Request
): Promise<IdentityClaims> {
    const { code, state } = req.query as { code: string; state: string };

    // Exchange code for token via HTTP datasource
    const tokenResponse = await Rivers.view.query("exchange_oauth_code", {
        code,
        redirect_uri: "https://myapp.example.com/auth"
    });

    // Fetch user profile
    const profile = await Rivers.view.query("get_oauth_profile", {
        access_token: tokenResponse.access_token
    });

    return {
        subject:   profile.sub,
        email:     profile.email,
        name:      profile.name,
        provider:  "google",
    };
}
```

OAuth2 token exchange is just an HTTP DataView call. The guard CodeComponent is agnostic to the auth mechanism.

### 11.3 Per-view session handler — RBAC injection

```typescript
// handlers/session.ts

// on_session_valid on a protected view
// Loads full user profile and resolves roles into ctx.sources
async function loadUserContext(
    req: Rivers.Request,
    session: Rivers.Session
): Promise<OnSessionValidResult> {
    const profile = await Rivers.view.query("get_user_profile", {
        user_id: session.claims.subject
    });

    return {
        key:  "identity",
        data: {
            user_id:     profile.id,
            email:       profile.email,
            role:        profile.role,
            permissions: profile.permissions,
            tenant_id:   profile.tenant_id,
        }
    };
    // ctx.sources["identity"] now available to all downstream handlers
}

// on_invalid_session — API clients get JSON, browsers get redirect
async function handleInvalidSession(
    req: Rivers.Request
): Promise<Rivers.Response | void> {
    const acceptsJson = req.headers["accept"]?.includes("application/json");

    if (acceptsJson) {
        return {
            status: 401,
            body: { error: "session expired", code: "SESSION_EXPIRED" }
        };
    }

    // void → Rivers redirects to guard's invalid_session_url
}
```

### 11.4 Logout view

```toml
[api.views.logout]
path      = "/auth/logout"
method    = "POST"
view_type = "Rest"

[api.views.logout.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/auth.ts"
entrypoint = "logout"
resources  = []
```

```typescript
async function logout(req: Rivers.Request): Promise<Rivers.Response> {
    await Rivers.session.destroy(Rivers.session.current.session_id);
    return {
        status:   302,
        headers:  { location: "/auth/login" },
        body:     null
    };
}
```

`Rivers.session.destroy()` deletes from StorageEngine and instructs Rivers to clear the session cookie on the response. Client is redirected to the login page with no active session.

### 11.5 Bearer-token authentication via a named guard (CB-P1.10 / closes P1.12)

When a route needs `Authorization: Bearer <token>` validation rather
than the cookie-session model — typical for MCP routes, machine-to-machine
APIs, and CLI clients — the sanctioned shape is a small codecomponent
attached as a per-view named guard (see §3 + `rivers-mcp-view-spec.md`
§13.5). This recipe replaces the previously-considered first-class
`auth = "bearer"` mode (CB-P1.12): the named-guard primitive already
provides the same enforcement boundary with no new framework concept.

```toml
# The bearer-validating guard view.
[api.views.api_key_guard]
view_type = "Rest"
path      = "/internal/api-key-guard"
method    = "POST"
auth      = "none"

[api.views.api_key_guard.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/auth.ts"
entrypoint = "validate_api_key"
resources  = ["app_db"]

# The protected MCP route — references the guard by name.
[api.views.mcp_advisor]
view_type  = "Mcp"
path       = "/mcp/advisor"
method     = "POST"
guard_view = "api_key_guard"
```

```typescript
import { sha256 } from "rivers/crypto";

interface ApiKeyClaims {
    user_id: string;
    role: string;
    project_id: string | null;
}

export async function validate_api_key(req: Rivers.Request) {
    const auth = (req.headers["authorization"] ?? "").trim();
    const prefix = "Bearer ";
    if (!auth.startsWith(prefix)) {
        return { allow: false };
    }
    const token = auth.slice(prefix.length).trim();
    if (token.length === 0) {
        return { allow: false };
    }

    // Match against the hash, never the raw token.
    const key_hash = sha256(token);
    const rows = Rivers.db.query("app_db", `
        SELECT created_by AS user_id, role, project_id
        FROM api_keys
        WHERE key_hash = ?
          AND revoked_at IS NULL
        LIMIT 1
    `, [key_hash]);

    if (rows.length === 0) {
        return { allow: false };
    }
    const claims: ApiKeyClaims = rows[0] as ApiKeyClaims;

    // Optional audit: stamp last-used. Best-effort — don't fail auth if
    // the audit write errors.
    try {
        Rivers.db.execute("app_db",
            "UPDATE api_keys SET last_used_at = CURRENT_TIMESTAMP WHERE key_hash = ?",
            [key_hash]);
    } catch (_e) { /* swallow — audit is non-load-bearing */ }

    return { allow: true, session_claims: claims };
}
```

**Why this design rather than a first-class `auth = "bearer"` mode:**

| Concern | Named-guard recipe | Hypothetical `auth = "bearer"` |
|---|---|---|
| Lookup table | bundle's own SQL | framework hard-codes `api_keys` schema |
| Hash algorithm | bundle's choice | framework freezes one |
| Identity claims | bundle's choice (any SQL projection) | framework's fixed shape |
| Audit fields (`last_used_at`, etc.) | bundle's choice | framework would need config knobs |
| Multi-role enforcement | `WHERE role = ?` in the SQL | needs additional config knob |
| Cross-project bindings | composable in SQL | needs configurable predicate |

Every dimension a hypothetical `auth = "bearer"` would expose as config
(table name, hash column, hash algorithm, where-clause, claims
projection, audit update) is already a one-liner inside the
codecomponent. The named guard is the lower-config, higher-flexibility
shape.

**Operational notes:**

- The guard codecomponent runs synchronously in
  `view_dispatch_handler` before any view-type dispatch — applies
  uniformly to REST, streaming REST, MCP, WebSocket, and SSE. Runs
  after rate limiting and before the existing session pipeline. Keep
  the handler fast — it is on the hot path.
- The framework rejects with HTTP 401 + trace ID on `allow: false` or
  any dispatcher error. Auth fails closed.
- Per `rivers-view-layer-spec.md` §14, the body is *not* yet parsed
  when the guard runs. The guard cannot inspect tool arguments or
  request bodies — design for authentication-shape decisions only.
- Validator footguns (caught at `riverpackage validate`): chains are
  forbidden in v1 (`X014`); guard target with `auth = "session"`
  warns (`W009`); double auth-gate config warns (`W010`).

---

## Appendix — superseded asks

- **CB-P1.12 — `auth = "bearer"` mode (closed 2026-05-08).** Subsumed
  by CB-P1.10 named guards (§11.5 above). No framework change planned;
  the recipe is the recommended shape for bearer enforcement.
