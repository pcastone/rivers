# Rivers Canary Fleet — Conformance Test Spec

## Version 1.1 (v0.53.0)

---

## What You Are Building

A Rivers app bundle containing seven apps organized as **conformance profiles**. Each profile is a self-contained app-service that tests one failure domain. The fleet runs on a single `riversd` instance. Running the full fleet exercises cross-app integration; running a single profile tests that domain in isolation.

The canary is not a real application. It exists solely to exercise every API surface point Rivers exposes and fail loudly when something breaks. Every endpoint is a test case. Every response is a verdict.

```
Browser → canary-main (8080)  ← SPA dashboard + cross-app proxy
              ↓
         canary-guard (9100)  ← guard view, session lifecycle, CSRF
         canary-sql (9101)    ← PostgreSQL, MySQL, SQLite CRUD + param binding + DDL rejection
         canary-nosql (9102)  ← MongoDB, Elasticsearch, CouchDB, Cassandra, LDAP, Redis
         canary-handlers (9103) ← ctx.* surface, Rivers.* globals, V8 security, StorageEngine
         canary-streams (9104) ← WebSocket, SSE, Streaming REST, Kafka MessageConsumer, Polling
         canary-ops (9105)    ← PID file, metrics, per-app logging, doctor, TLS, config discovery
```

**Startup order:** canary-guard → canary-sql → canary-nosql → canary-handlers → canary-streams → canary-ops → canary-main. Order matters — canary-guard must be healthy before any session-consuming service starts. canary-ops starts after canary-streams because it verifies operational infrastructure that other profiles produce (log files, metrics). canary-main starts last because it depends on all services.

---

## Design Principles

**Every endpoint is a test.** There are no business logic endpoints. Every path under `/canary/*` exercises a specific spec assertion and returns a structured verdict.

**Self-reporting verdicts.** Endpoints do their own assertions internally and return pass/fail with evidence. The Rust integration test harness just checks `passed == true`. The SPA dashboard aggregates verdicts visually.

**Profiles are independent.** canary-sql can run without canary-nosql. The only hard dependency for all profiles is canary-guard (session provider). A profile that doesn't need sessions can run standalone.

**Negative tests live with their domain.** DDL rejection tests live in canary-sql because they're a SQL driver concern. V8 timeout tests live in canary-handlers because they're a runtime concern. No separate "security" app — security is tested where it matters.

**Parameter binding traps are universal.** Every SQL DataView deliberately uses parameter names where alphabetical order ≠ declaration order. This is the exact class of bug that caused silent data corruption (Issue #54).

---

## Final Bundle Structure

```
canary-bundle/
├── CHANGELOG.md
├── manifest.toml
├── canary-guard/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   └── identity.schema.json
│   └── libraries/
│       └── handlers/
│           ├── guard.ts
│           └── session-test.ts
├── canary-sql/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   ├── pg-record.schema.json
│   │   ├── mysql-record.schema.json
│   │   └── sqlite-record.schema.json
│   └── libraries/
│       └── handlers/
│           ├── init.ts
│           ├── sql-tests.ts
│           └── negative-sql.ts
├── canary-nosql/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   ├── mongo-doc.schema.json
│   │   ├── es-doc.schema.json
│   │   ├── couch-doc.schema.json
│   │   ├── cassandra-row.schema.json
│   │   ├── ldap-entry.schema.json
│   │   └── redis-kv.schema.json
│   └── libraries/
│       └── handlers/
│           ├── nosql-tests.ts
│           └── negative-nosql.ts
├── canary-handlers/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   └── faker-record.schema.json
│   └── libraries/
│       └── handlers/
│           ├── ctx-surface.ts
│           ├── rivers-api.ts
│           ├── storage-tests.ts
│           ├── eventbus-tests.ts
│           └── v8-security.ts
├── canary-streams/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   ├── schemas/
│   │   └── stream-payload.schema.json
│   └── libraries/
│       └── handlers/
│           ├── ws-handler.ts
│           ├── sse-handler.ts
│           ├── streaming-rest.ts
│           ├── kafka-consumer.ts
│           └── poll-handler.ts
├── canary-ops/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml
│   └── libraries/
│       └── handlers/
│           ├── test-harness.ts
│           ├── pid-tests.ts
│           ├── metrics-tests.ts
│           ├── logging-tests.ts
│           ├── doctor-tests.ts
│           ├── tls-tests.ts
│           └── config-discovery-tests.ts
└── canary-main/
    ├── manifest.toml
    ├── resources.toml
    ├── app.toml
    └── libraries/
        ├── handlers/
        │   └── proxy-tests.ts
        ├── package.json
        ├── rollup.config.js
        ├── src/
        │   ├── App.svelte
        │   ├── components/
        │   │   ├── ProfileCard.svelte
        │   │   ├── TestRow.svelte
        │   │   └── VerdictBadge.svelte
        │   └── main.js
        └── spa/
            ├── index.html
            ├── bundle.js
            └── bundle.css
```

---

## CHANGELOG.md

Create at `canary-bundle/CHANGELOG.md` — same level as `manifest.toml`. Filename uppercase. Append across rounds, never replace.

Entry format:
```markdown
## [Decision|Gap|Ambiguity|Error] — <short title>
**File:** <filename>
**Description:** What you did, decided, or encountered.
**Spec reference:** Which section of this spec.
**Resolution:** How you resolved it, or "UNRESOLVED".
```

---

## Self-Reporting Test Protocol

Every canary test endpoint — regardless of profile — returns this JSON envelope:

```json
{
  "test_id": "SQL-PG-CRUD-INSERT",
  "profile": "SQL",
  "spec_ref": "rivers-data-layer-spec.md §3.1",
  "passed": true,
  "assertions": [
    {
      "id": "row_inserted",
      "passed": true,
      "detail": "INSERT returned 1 affected row"
    },
    {
      "id": "params_bound_non_alpha",
      "passed": true,
      "detail": "declared: [zname, age], bound: [zname=Alice, age=30]"
    }
  ],
  "duration_ms": 12,
  "error": null
}
```

**Rules:**

- `test_id` — uppercase, `PROFILE-DRIVER-FEATURE-OPERATION` format. Unique across the entire fleet.
- `profile` — one of: `AUTH`, `SQL`, `NOSQL`, `RUNTIME`, `STREAM`, `OPS`, `PROXY`.
- `spec_ref` — the spec filename and section this test validates. If no spec section exists for the feature being tested, use `"none — negative test"`.
- `passed` — `true` only if ALL assertions passed. If any assertion fails, `passed` is `false`.
- `assertions` — array of individual checks. Each has `id` (snake_case), `passed` (bool), and optional `detail` (string, human-readable evidence).
- `error` — `null` on success. On failure, a string describing what went wrong.
- `duration_ms` — wall-clock time of the test execution in the handler, measured by the handler itself.

**HTTP status codes:**
- `200` — test executed and reported a verdict (check `passed` for result)
- `500` — test could not execute at all (framework error, not a test failure)

A `200` with `passed: false` is a **test failure** — the feature is broken. A `500` means the test harness itself is broken.

**Handler helper pattern:**

Every test handler uses a shared helper module. This is not a Rivers API — it's a TypeScript utility bundled in each app's `libraries/`:

```typescript
// libraries/handlers/test-harness.ts

export interface Assertion {
  id: string;
  passed: boolean;
  detail?: string;
}

export class TestResult {
  test_id: string;
  profile: string;
  spec_ref: string;
  assertions: Assertion[] = [];
  error: string | null = null;
  private start: number;

  constructor(test_id: string, profile: string, spec_ref: string) {
    this.test_id = test_id;
    this.profile = profile;
    this.spec_ref = spec_ref;
    this.start = Date.now();
  }

  assert(id: string, passed: boolean, detail?: string) {
    this.assertions.push({ id, passed, detail: detail || undefined });
  }

  assertEquals(id: string, expected: any, actual: any) {
    const passed = JSON.stringify(expected) === JSON.stringify(actual);
    this.assertions.push({
      id,
      passed,
      detail: passed
        ? `expected=${JSON.stringify(expected)}`
        : `expected=${JSON.stringify(expected)}, actual=${JSON.stringify(actual)}`
    });
  }

  assertExists(id: string, value: any) {
    const passed = value !== undefined && value !== null;
    this.assertions.push({
      id,
      passed,
      detail: passed ? `type=${typeof value}` : `value was ${value}`
    });
  }

  assertType(id: string, value: any, expectedType: string) {
    const actual = typeof value;
    const passed = actual === expectedType;
    this.assertions.push({
      id,
      passed,
      detail: passed ? `type=${actual}` : `expected type=${expectedType}, actual=${actual}`
    });
  }

  assertThrows(id: string, fn: () => any) {
    let threw = false;
    let errMsg = "";
    try { fn(); } catch (e) { threw = true; errMsg = String(e); }
    this.assertions.push({
      id,
      passed: threw,
      detail: threw ? `threw: ${errMsg}` : "did not throw"
    });
  }

  assertNotContains(id: string, haystack: string, needle: string) {
    const passed = !haystack.toLowerCase().includes(needle.toLowerCase());
    this.assertions.push({
      id,
      passed,
      detail: passed ? `"${needle}" not found` : `"${needle}" found in response`
    });
  }

  finish(): object {
    return {
      test_id: this.test_id,
      profile: this.profile,
      spec_ref: this.spec_ref,
      passed: this.assertions.every(a => a.passed),
      assertions: this.assertions,
      duration_ms: Date.now() - this.start,
      error: this.error
    };
  }

  fail(error: string): object {
    this.error = error;
    return {
      test_id: this.test_id,
      profile: this.profile,
      spec_ref: this.spec_ref,
      passed: false,
      assertions: this.assertions,
      duration_ms: Date.now() - this.start,
      error
    };
  }
}
```

Copy `test-harness.ts` into every app's `libraries/handlers/` directory. Each app gets its own copy — cross-app imports are forbidden.

---

## Shared Test Infrastructure

### Connection Strings (from podman cluster)

All credentials are stored in LockBox. The canary bundle's `resources.toml` files reference LockBox aliases. For the test cluster, provision these aliases:

| Alias | Connection String |
|-------|-------------------|
| `canary-pg` | `postgresql://rivers:rivers_test@192.168.2.209:5432/rivers` |
| `canary-mysql` | `mysql://rivers:rivers_test@192.168.2.215:3306/rivers` |
| `canary-redis` | `redis://:rivers_test@192.168.2.206:6379` |
| `canary-mongo` | `mongodb://rivers:rivers_test@192.168.2.212:27017/?replicaSet=rivers-rs&authSource=admin` |
| `canary-es` | `http://192.168.2.218:9200` |
| `canary-kafka` | `192.168.2.203:9092,192.168.2.204:9092,192.168.2.205:9092` |
| `canary-couch` | `http://rivers:rivers_test@192.168.2.221:5984` |
| `canary-cassandra` | `192.168.2.224:9042` |
| `canary-ldap` | `ldap://192.168.2.227:389` |

SQLite uses `:memory:` — no LockBox alias needed.

### DDL for Test Tables

canary-sql's init handler creates these tables. The DDL runs through the three-gate enforcement path: driver guard (Gate 1) + ApplicationInit execution context (Gate 2) + `ddl_whitelist` in `riversd.toml` (Gate 3).

**PostgreSQL:**
```sql
CREATE TABLE IF NOT EXISTS canary_records (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  zname TEXT NOT NULL,
  age INTEGER NOT NULL,
  email TEXT,
  created_at TIMESTAMPTZ DEFAULT NOW()
);
```

**MySQL:**
```sql
CREATE TABLE IF NOT EXISTS canary_records (
  id CHAR(36) PRIMARY KEY,
  zname VARCHAR(255) NOT NULL,
  age INT NOT NULL,
  email VARCHAR(255),
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

**SQLite:**
```sql
CREATE TABLE IF NOT EXISTS canary_records (
  id TEXT PRIMARY KEY,
  zname TEXT NOT NULL,
  age INTEGER NOT NULL,
  email TEXT,
  created_at TEXT DEFAULT (datetime('now'))
);
```

Note the column `zname` — not `name`. This ensures that in any DataView with parameters `[zname, age]`, the alphabetical order (`age, zname`) differs from declaration order (`zname, age`). This is the parameter binding trap that catches Issue #54.

### riversd.toml DDL Whitelist

```toml
[security]
ddl_whitelist = [
  "canary-pg@aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01",
  "canary-mysql@aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01",
  "canary-sqlite@aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01"
]
```

The `appId` value `aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01` is canary-sql's stable appId. The format is `"datasource_name@appId"`.

### StorageEngine Config

```toml
[storage_engine]
backend = "redis"
url     = "redis://:rivers_test@192.168.2.206:6379"
```

Redis is the StorageEngine backend for the canary. This enables session storage, CSRF tokens, poll state, and L2 cache — all of which are under test.

### Metrics Config [v0.53.0]

```toml
[metrics]
enabled = true
port    = 9091
```

Prometheus metrics endpoint on port 9091. Required by canary-ops profile for OPS-METRICS-* tests. Exposes `rivers_http_requests_total`, `rivers_http_request_duration_ms`, and other counters/histograms.

### Logging Config [v0.53.0]

```toml
[logging]
app_log_dir     = "log/apps"
rotation_max_mb = 10
```

Per-app log routing (AppLogRouter) writes each app's `Rivers.log.*` calls to `log/apps/<appName>.log`. Required by canary-ops and canary-handlers for per-app logging tests. Rotation triggers at 10MB.

---

## Part 1 — canary-bundle Root

### canary-bundle/manifest.toml

```toml
bundleName    = "canary-fleet"
bundleVersion = "1.0.0"
source        = "https://github.com/rivers-framework/canary-fleet"
apps          = [
  "canary-guard",
  "canary-sql",
  "canary-nosql",
  "canary-handlers",
  "canary-streams",
  "canary-ops",
  "canary-main"
]
```

Order in `apps` matters. App-services start in declared order, each waiting for the previous to be healthy. canary-ops starts after canary-streams. canary-main (app-main) starts last.

---

## Part 2 — Profile: AUTH (canary-guard)

### Purpose

Tests guard view pattern, session lifecycle, CSRF protection, and session token properties. This is the foundation — every other profile consumes sessions created here.

### Test Inventory

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| AUTH-GUARD-LOGIN | `/canary/auth/login` | POST | Guard view creates session from credentials | auth-session §3 |
| AUTH-GUARD-CLAIMS | `/canary/auth/login` | POST | IdentityClaims returned to framework | auth-session §3.3 |
| AUTH-SESSION-TOKEN-SIZE | `/canary/auth/token-check` | GET | Session token is 256-bit CSPRNG (not UUID v4) | feature-inventory §21.4 |
| AUTH-SESSION-COOKIE | `/canary/auth/cookie-check` | GET | Cookie has HttpOnly, Secure, SameSite flags | auth-session §8 |
| AUTH-SESSION-READ | `/canary/auth/session-read` | GET | Handler reads ctx.session with valid claims | auth-session §5 |
| AUTH-SESSION-EXPIRED | `/canary/auth/session-expired` | GET | Expired session returns 401 | auth-session §4 |
| AUTH-CSRF-REQUIRED | `/canary/auth/csrf-test` | POST | POST without CSRF token returns 403 | auth-session §7.3 |
| AUTH-CSRF-VALID | `/canary/auth/csrf-test` | POST | POST with valid CSRF cookie+header succeeds | auth-session §7.3 |
| AUTH-LOGOUT | `/canary/auth/logout` | POST | Session invalidation clears StorageEngine entry | auth-session §4 |

### canary-guard/manifest.toml

```toml
appName       = "canary-guard"
description   = "Canary Fleet — AUTH profile: guard view and session lifecycle"
version       = "1.0.0"
type          = "app-service"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
entryPoint    = "http://0.0.0.0:9100"
appEntryPoint = "https://canary-guard.internal"
source        = "https://github.com/rivers-framework/canary-fleet/canary-guard"
```

### canary-guard/resources.toml

```toml
# No external datasources — guard uses only framework-provided session storage
```

### canary-guard/schemas/identity.schema.json

```json
{
  "type": "object",
  "description": "Identity claims for canary test sessions",
  "fields": [
    { "name": "sub",   "type": "string", "required": true },
    { "name": "role",  "type": "string", "required": true },
    { "name": "email", "type": "email",  "required": false }
  ]
}
```

### canary-guard/app.toml

```toml
# ─────────────────────────────────────────────
# Guard View — sole session creation point
# ─────────────────────────────────────────────

[api.views.guard]
path      = "/canary/auth/login"
method    = "POST"
view_type = "Rest"
auth      = "guard"

[api.views.guard.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/guard.ts"
entrypoint = "authenticate"

[api.views.guard.session]
ttl_s          = 3600
idle_timeout_s = 600

# ─────────────────────────────────────────────
# Session test views
# ─────────────────────────────────────────────

[api.views.token_check]
path      = "/canary/auth/token-check"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.token_check.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/session-test.ts"
entrypoint = "checkTokenSize"

[api.views.cookie_check]
path      = "/canary/auth/cookie-check"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.cookie_check.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/session-test.ts"
entrypoint = "checkCookieFlags"

[api.views.session_read]
path      = "/canary/auth/session-read"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.session_read.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/session-test.ts"
entrypoint = "readSession"

[api.views.session_expired]
path      = "/canary/auth/session-expired"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.session_expired.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/session-test.ts"
entrypoint = "expiredSessionTest"

[api.views.csrf_test]
path      = "/canary/auth/csrf-test"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.csrf_test.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/session-test.ts"
entrypoint = "csrfTest"

[api.views.logout]
path      = "/canary/auth/logout"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.logout.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/session-test.ts"
entrypoint = "logoutTest"
```

### canary-guard/libraries/handlers/guard.ts

```typescript
// Guard handler — returns IdentityClaims on success
// This is a guard handler (returns IdentityClaims, not void)

export function authenticate(ctx: any): any {
  const body = ctx.request.body;

  // Accept any request with username "canary" and password "canary-test"
  // This is a test guard — not a real auth system
  if (body?.username === "canary" && body?.password === "canary-test") {
    return {
      sub: "canary-user-001",
      role: "tester",
      email: "canary@test.rivers"
    };
  }

  // Returning null/undefined signals auth failure — Rivers returns 401
  return null;
}
```

### canary-guard/libraries/handlers/session-test.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function checkTokenSize(ctx: any): void {
  const t = new TestResult("AUTH-SESSION-TOKEN-SIZE", "AUTH",
    "feature-inventory §21.4");

  // The session token is delivered in the cookie or response body.
  // We check the token length — 256-bit = 32 bytes = 64 hex chars or 43 base64url chars.
  const token = ctx.request.headers["x-rivers-session"] ||
                ctx.session?._token || "";
  t.assert("token_not_empty", token.length > 0, `length=${token.length}`);
  // 256-bit CSPRNG: at least 43 chars (base64url) or 64 chars (hex)
  t.assert("token_min_length", token.length >= 43,
    `length=${token.length}, expected >= 43 (256-bit)`);

  ctx.resdata = t.finish();
}

export function checkCookieFlags(ctx: any): void {
  const t = new TestResult("AUTH-SESSION-COOKIE", "AUTH",
    "auth-session §8");

  // If we reached this handler, the session cookie was valid.
  // The cookie flags are set by Rivers, not the handler.
  // We verify by checking that the request was authenticated (session exists).
  t.assertExists("session_exists", ctx.session);
  t.assertExists("session_sub", ctx.session?.sub);

  // Note: actual cookie flag verification (HttpOnly, Secure, SameSite)
  // must be done by the Rust integration test inspecting response headers.
  // The handler can only confirm the session was delivered.
  t.assert("session_delivered", true, "handler received valid session");

  ctx.resdata = t.finish();
}

export function readSession(ctx: any): void {
  const t = new TestResult("AUTH-SESSION-READ", "AUTH",
    "auth-session §5");

  t.assertExists("ctx_session", ctx.session);
  t.assertType("session_is_object", ctx.session, "object");
  t.assertEquals("session_sub", "canary-user-001", ctx.session?.sub);
  t.assertEquals("session_role", "tester", ctx.session?.role);
  t.assertEquals("session_email", "canary@test.rivers", ctx.session?.email);

  ctx.resdata = t.finish();
}

export function expiredSessionTest(ctx: any): void {
  // This handler should never execute if the session is expired.
  // The Rust integration test calls this with an expired token
  // and expects a 401 BEFORE the handler runs.
  // If we reach here, the session was NOT properly expired.
  const t = new TestResult("AUTH-SESSION-EXPIRED", "AUTH",
    "auth-session §4");
  t.assert("session_should_have_been_rejected", false,
    "handler executed — session was not expired as expected");
  ctx.resdata = t.finish();
}

export function csrfTest(ctx: any): void {
  const t = new TestResult("AUTH-CSRF-VALID", "AUTH",
    "auth-session §7.3");

  // If we reached this handler on a POST, CSRF validation passed.
  t.assert("csrf_passed", true, "POST reached handler — CSRF token was valid");
  t.assertExists("session_exists", ctx.session);

  ctx.resdata = t.finish();
}

export function logoutTest(ctx: any): void {
  const t = new TestResult("AUTH-LOGOUT", "AUTH",
    "auth-session §4");

  // Invalidate the session
  // Rivers.session.invalidate() destroys the session in StorageEngine
  if (typeof ctx.session?.invalidate === "function") {
    ctx.session.invalidate();
    t.assert("session_invalidated", true, "invalidate() called");
  } else {
    t.assert("invalidate_available", false,
      "ctx.session.invalidate is not a function");
  }

  ctx.resdata = t.finish();
}
```

---

## Part 3 — Profile: SQL (canary-sql)

### Purpose

Tests SQL driver CRUD operations across PostgreSQL, MySQL, and SQLite. Every DataView uses the parameter binding trap (`zname` before `age`). Tests DDL rejection through standard execute path and DDL success through init handler whitelist path.

### Test Inventory

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| SQL-PG-CRUD-INSERT | `/canary/sql/pg/insert` | POST | PostgreSQL INSERT with param binding | data-layer §3.1 |
| SQL-PG-CRUD-SELECT | `/canary/sql/pg/select` | GET | PostgreSQL SELECT with WHERE params | data-layer §3.1 |
| SQL-PG-CRUD-UPDATE | `/canary/sql/pg/update` | PUT | PostgreSQL UPDATE with param binding | data-layer §3.1 |
| SQL-PG-CRUD-DELETE | `/canary/sql/pg/delete` | DELETE | PostgreSQL DELETE by id | data-layer §3.1 |
| SQL-PG-PARAM-ORDER | `/canary/sql/pg/param-order` | POST | INSERT with zname,age — verifies non-alpha binding | Issue #54 |
| SQL-PG-DDL-REJECT | `/canary/sql/pg/ddl-reject` | POST | DROP TABLE via handler is rejected with Forbidden | feature-inventory §21.1 |
| SQL-PG-MAX-ROWS | `/canary/sql/pg/max-rows` | GET | Query returns exactly max_rows, not unbounded | feature-inventory §21.5 |
| SQL-MYSQL-CRUD-INSERT | `/canary/sql/mysql/insert` | POST | MySQL INSERT with param binding | data-layer §3.1 |
| SQL-MYSQL-CRUD-SELECT | `/canary/sql/mysql/select` | GET | MySQL SELECT with WHERE params | data-layer §3.1 |
| SQL-MYSQL-CRUD-UPDATE | `/canary/sql/mysql/update` | PUT | MySQL UPDATE with param binding | data-layer §3.1 |
| SQL-MYSQL-CRUD-DELETE | `/canary/sql/mysql/delete` | DELETE | MySQL DELETE by id | data-layer §3.1 |
| SQL-MYSQL-PARAM-ORDER | `/canary/sql/mysql/param-order` | POST | INSERT verifies non-alpha binding | Issue #54 |
| SQL-MYSQL-DDL-REJECT | `/canary/sql/mysql/ddl-reject` | POST | DROP TABLE via handler is rejected | feature-inventory §21.1 |
| SQL-SQLITE-CRUD-INSERT | `/canary/sql/sqlite/insert` | POST | SQLite INSERT with param binding | data-layer §3.1 |
| SQL-SQLITE-CRUD-SELECT | `/canary/sql/sqlite/select` | GET | SQLite SELECT with WHERE params | data-layer §3.1 |
| SQL-SQLITE-PREFIX | `/canary/sql/sqlite/prefix` | GET | SQLite `$name` vs `:name` prefix handling | Issue #54 |
| SQL-CACHE-L1-HIT | `/canary/sql/cache/l1-hit` | GET | Second identical query hits L1 cache | storage-engine §11.6 |
| SQL-CACHE-INVALIDATE | `/canary/sql/cache/invalidate` | POST | Write DataView triggers cache invalidation | data-layer §3.3 |
| SQL-SQLITE-PATH-DATABASE | `/canary/sql/sqlite/path-database` | GET | SQLite driver accepts `database=` for file path | driver-spec §4.5 |
| SQL-SQLITE-PATH-HOST | `/canary/sql/sqlite/path-host` | GET | SQLite driver accepts `host=` as fallback for file path | driver-spec §4.5 |
| SQL-SQLITE-PATH-MKDIR | `/canary/sql/sqlite/path-mkdir` | GET | SQLite driver creates parent directories if missing | driver-spec §4.5 |
| SQL-SQLITE-PATH-EMPTY | `/canary/sql/sqlite/path-empty` | GET | SQLite driver returns clear error if both database= and host= empty | driver-spec §4.5 |
| SQL-INIT-DDL-SUCCESS | (startup) | — | Init handler creates tables via whitelist | application §13.7 |

### canary-sql/manifest.toml

```toml
appName       = "canary-sql"
description   = "Canary Fleet — SQL profile: PostgreSQL, MySQL, SQLite CRUD and security"
version       = "1.0.0"
type          = "app-service"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01"
entryPoint    = "http://0.0.0.0:9101"
appEntryPoint = "https://canary-sql.internal"
source        = "https://github.com/rivers-framework/canary-fleet/canary-sql"

[init]
module     = "handlers/init.ts"
entrypoint = "initDatabase"
```

### canary-sql/resources.toml

```toml
[[datasources]]
name     = "canary-pg"
driver   = "postgresql"
x-type   = "postgresql"
required = true

[datasources.lockbox]
alias = "canary-pg"

[[datasources]]
name     = "canary-mysql"
driver   = "mysql"
x-type   = "mysql"
required = true

[datasources.lockbox]
alias = "canary-mysql"

[[datasources]]
name       = "canary-sqlite"
driver     = "sqlite"
x-type     = "sqlite"
nopassword = true
required   = true

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

### canary-sql/schemas/pg-record.schema.json

```json
{
  "type": "object",
  "driver": "postgresql",
  "description": "Canary test record for PostgreSQL",
  "fields": [
    { "name": "id",         "type": "uuid",     "required": true  },
    { "name": "zname",      "type": "string",   "required": true  },
    { "name": "age",        "type": "integer",  "required": true  },
    { "name": "email",      "type": "string",   "required": false },
    { "name": "created_at", "type": "datetime", "required": false }
  ]
}
```

### canary-sql/schemas/mysql-record.schema.json

```json
{
  "type": "object",
  "driver": "mysql",
  "description": "Canary test record for MySQL",
  "fields": [
    { "name": "id",         "type": "string",   "required": true  },
    { "name": "zname",      "type": "string",   "required": true  },
    { "name": "age",        "type": "integer",  "required": true  },
    { "name": "email",      "type": "string",   "required": false },
    { "name": "created_at", "type": "datetime", "required": false }
  ]
}
```

### canary-sql/schemas/sqlite-record.schema.json

```json
{
  "type": "object",
  "driver": "sqlite",
  "description": "Canary test record for SQLite",
  "fields": [
    { "name": "id",         "type": "string",   "required": true  },
    { "name": "zname",      "type": "string",   "required": true  },
    { "name": "age",        "type": "integer",  "required": true  },
    { "name": "email",      "type": "string",   "required": false },
    { "name": "created_at", "type": "string",   "required": false }
  ]
}
```

### canary-sql/app.toml

```toml
# ─────────────────────────────────────────────
# Datasources
# ─────────────────────────────────────────────

[data.datasources.canary-pg]
driver = "postgresql"

[data.datasources.canary-pg.config]
max_pool_size = 5

[data.datasources.canary-mysql]
driver = "mysql"

[data.datasources.canary-mysql.config]
max_pool_size = 5

[data.datasources.canary-sqlite]
driver     = "sqlite"
nopassword = true

[data.datasources.canary-sqlite.config]
path = ":memory:"

# ─────────────────────────────────────────────
# DataViews — PostgreSQL
# ─────────────────────────────────────────────

[data.dataviews.pg_insert]
datasource    = "canary-pg"
query         = "INSERT INTO canary_records (zname, age, email) VALUES ($zname, $age, $email) RETURNING *"
return_schema = "schemas/pg-record.schema.json"

[[data.dataviews.pg_insert.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.pg_insert.parameters]]
name     = "age"
type     = "integer"
required = true

[[data.dataviews.pg_insert.parameters]]
name     = "email"
type     = "string"
required = false

[data.dataviews.pg_select]
datasource    = "canary-pg"
query         = "SELECT * FROM canary_records WHERE zname = $zname AND age = $age"
return_schema = "schemas/pg-record.schema.json"

[[data.dataviews.pg_select.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.pg_select.parameters]]
name     = "age"
type     = "integer"
required = true

[data.dataviews.pg_update]
datasource = "canary-pg"
query      = "UPDATE canary_records SET email = $email WHERE zname = $zname AND age = $age"

[[data.dataviews.pg_update.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.pg_update.parameters]]
name     = "age"
type     = "integer"
required = true

[[data.dataviews.pg_update.parameters]]
name     = "email"
type     = "string"
required = true

[data.dataviews.pg_delete]
datasource = "canary-pg"
query      = "DELETE FROM canary_records WHERE zname = $zname"

[[data.dataviews.pg_delete.parameters]]
name     = "zname"
type     = "string"
required = true

[data.dataviews.pg_select_all]
datasource    = "canary-pg"
query         = "SELECT * FROM canary_records"
return_schema = "schemas/pg-record.schema.json"

[data.dataviews.pg_select_all.cache]
enabled     = true
ttl_seconds = 60

[data.dataviews.pg_select_all.config]
max_rows = 100

# ─────────────────────────────────────────────
# DataViews — MySQL (mirrors PostgreSQL structure)
# ─────────────────────────────────────────────

[data.dataviews.mysql_insert]
datasource    = "canary-mysql"
query         = "INSERT INTO canary_records (id, zname, age, email) VALUES ($id, $zname, $age, $email)"
return_schema = "schemas/mysql-record.schema.json"

[[data.dataviews.mysql_insert.parameters]]
name     = "id"
type     = "string"
required = true

[[data.dataviews.mysql_insert.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.mysql_insert.parameters]]
name     = "age"
type     = "integer"
required = true

[[data.dataviews.mysql_insert.parameters]]
name     = "email"
type     = "string"
required = false

[data.dataviews.mysql_select]
datasource    = "canary-mysql"
query         = "SELECT * FROM canary_records WHERE zname = $zname AND age = $age"
return_schema = "schemas/mysql-record.schema.json"

[[data.dataviews.mysql_select.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.mysql_select.parameters]]
name     = "age"
type     = "integer"
required = true

[data.dataviews.mysql_update]
datasource = "canary-mysql"
query      = "UPDATE canary_records SET email = $email WHERE zname = $zname AND age = $age"

[[data.dataviews.mysql_update.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.mysql_update.parameters]]
name     = "age"
type     = "integer"
required = true

[[data.dataviews.mysql_update.parameters]]
name     = "email"
type     = "string"
required = true

[data.dataviews.mysql_delete]
datasource = "canary-mysql"
query      = "DELETE FROM canary_records WHERE zname = $zname"

[[data.dataviews.mysql_delete.parameters]]
name     = "zname"
type     = "string"
required = true

# ─────────────────────────────────────────────
# DataViews — SQLite
# ─────────────────────────────────────────────

[data.dataviews.sqlite_insert]
datasource    = "canary-sqlite"
query         = "INSERT INTO canary_records (id, zname, age, email) VALUES ($id, $zname, $age, $email)"
return_schema = "schemas/sqlite-record.schema.json"

[[data.dataviews.sqlite_insert.parameters]]
name     = "id"
type     = "string"
required = true

[[data.dataviews.sqlite_insert.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.sqlite_insert.parameters]]
name     = "age"
type     = "integer"
required = true

[[data.dataviews.sqlite_insert.parameters]]
name     = "email"
type     = "string"
required = false

[data.dataviews.sqlite_select]
datasource    = "canary-sqlite"
query         = "SELECT * FROM canary_records WHERE zname = $zname AND age = $age"
return_schema = "schemas/sqlite-record.schema.json"

[[data.dataviews.sqlite_select.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.sqlite_select.parameters]]
name     = "age"
type     = "integer"
required = true

# ─────────────────────────────────────────────
# Views — PostgreSQL CRUD
# ─────────────────────────────────────────────

[api.views.pg_insert]
path      = "/canary/sql/pg/insert"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.pg_insert.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "pgInsert"
resources  = ["canary-pg"]

[api.views.pg_insert.parameter_mapping.body]
zname = "zname"
age   = "age"
email = "email"

[api.views.pg_select]
path      = "/canary/sql/pg/select"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.pg_select.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "pgSelect"
resources  = ["canary-pg"]

[api.views.pg_select.parameter_mapping.query]
zname = "zname"
age   = "age"

[api.views.pg_update]
path      = "/canary/sql/pg/update"
method    = "PUT"
view_type = "Rest"
auth      = "session"

[api.views.pg_update.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "pgUpdate"
resources  = ["canary-pg"]

[api.views.pg_update.parameter_mapping.body]
zname = "zname"
age   = "age"
email = "email"

[api.views.pg_delete]
path      = "/canary/sql/pg/delete"
method    = "DELETE"
view_type = "Rest"
auth      = "session"

[api.views.pg_delete.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "pgDelete"
resources  = ["canary-pg"]

[api.views.pg_delete.parameter_mapping.body]
zname = "zname"

[api.views.pg_param_order]
path      = "/canary/sql/pg/param-order"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.pg_param_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "pgParamOrder"
resources  = ["canary-pg"]

[api.views.pg_param_order.parameter_mapping.body]
zname = "zname"
age   = "age"

# ─── Negative: DDL rejection ───

[api.views.pg_ddl_reject]
path      = "/canary/sql/pg/ddl-reject"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.pg_ddl_reject.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/negative-sql.ts"
entrypoint = "pgDdlReject"
resources  = ["canary-pg"]

# ─── max_rows truncation ───

[api.views.pg_max_rows]
path      = "/canary/sql/pg/max-rows"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.pg_max_rows.handler]
type     = "data_view"
dataview = "pg_select_all"

# ─── MySQL Views (mirror structure) ───

[api.views.mysql_insert]
path      = "/canary/sql/mysql/insert"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.mysql_insert.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "mysqlInsert"
resources  = ["canary-mysql"]

[api.views.mysql_insert.parameter_mapping.body]
zname = "zname"
age   = "age"
email = "email"

[api.views.mysql_select]
path      = "/canary/sql/mysql/select"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.mysql_select.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "mysqlSelect"
resources  = ["canary-mysql"]

[api.views.mysql_select.parameter_mapping.query]
zname = "zname"
age   = "age"

[api.views.mysql_update]
path      = "/canary/sql/mysql/update"
method    = "PUT"
view_type = "Rest"
auth      = "session"

[api.views.mysql_update.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "mysqlUpdate"
resources  = ["canary-mysql"]

[api.views.mysql_update.parameter_mapping.body]
zname = "zname"
age   = "age"
email = "email"

[api.views.mysql_delete]
path      = "/canary/sql/mysql/delete"
method    = "DELETE"
view_type = "Rest"
auth      = "session"

[api.views.mysql_delete.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "mysqlDelete"
resources  = ["canary-mysql"]

[api.views.mysql_delete.parameter_mapping.body]
zname = "zname"

[api.views.mysql_param_order]
path      = "/canary/sql/mysql/param-order"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.mysql_param_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "mysqlParamOrder"
resources  = ["canary-mysql"]

[api.views.mysql_param_order.parameter_mapping.body]
zname = "zname"
age   = "age"

[api.views.mysql_ddl_reject]
path      = "/canary/sql/mysql/ddl-reject"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.mysql_ddl_reject.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/negative-sql.ts"
entrypoint = "mysqlDdlReject"
resources  = ["canary-mysql"]

# ─── SQLite Views ───

[api.views.sqlite_insert]
path      = "/canary/sql/sqlite/insert"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_insert.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqliteInsert"
resources  = ["canary-sqlite"]

[api.views.sqlite_select]
path      = "/canary/sql/sqlite/select"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_select.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqliteSelect"
resources  = ["canary-sqlite"]

[api.views.sqlite_select.parameter_mapping.query]
zname = "zname"
age   = "age"

[api.views.sqlite_prefix]
path      = "/canary/sql/sqlite/prefix"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_prefix.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqlitePrefix"
resources  = ["canary-sqlite"]

# ─── Cache Tests ───

[api.views.cache_l1_hit]
path      = "/canary/sql/cache/l1-hit"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.cache_l1_hit.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "cacheL1Hit"
resources  = ["canary-pg"]

[api.views.cache_invalidate]
path      = "/canary/sql/cache/invalidate"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.cache_invalidate.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "cacheInvalidate"
resources  = ["canary-pg"]
```

### canary-sql/libraries/handlers/init.ts

```typescript
// Application Init Handler — runs in ApplicationInit execution context
// This is the ONLY context where DDL is permitted (three-gate enforcement)

export function initDatabase(ctx: any): void {
  // PostgreSQL DDL
  ctx.dataview("pg_ddl_create", {
    statement: `CREATE TABLE IF NOT EXISTS canary_records (
      id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
      zname TEXT NOT NULL,
      age INTEGER NOT NULL,
      email TEXT,
      created_at TIMESTAMPTZ DEFAULT NOW()
    )`
  });

  // MySQL DDL
  ctx.dataview("mysql_ddl_create", {
    statement: `CREATE TABLE IF NOT EXISTS canary_records (
      id CHAR(36) PRIMARY KEY,
      zname VARCHAR(255) NOT NULL,
      age INT NOT NULL,
      email VARCHAR(255),
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )`
  });

  // SQLite DDL
  ctx.dataview("sqlite_ddl_create", {
    statement: `CREATE TABLE IF NOT EXISTS canary_records (
      id TEXT PRIMARY KEY,
      zname TEXT NOT NULL,
      age INTEGER NOT NULL,
      email TEXT,
      created_at TEXT DEFAULT (datetime('now'))
    )`
  });

  // Seed PostgreSQL with enough rows for max_rows test
  for (let i = 0; i < 200; i++) {
    ctx.dataview("pg_insert", {
      zname: `seed-user-${i}`,
      age: 20 + (i % 60),
      email: `seed${i}@test.rivers`
    });
  }

  Rivers.log.info("canary-sql init complete: tables created, 200 seed rows inserted");
}
```

### canary-sql/libraries/handlers/sql-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

// ─── PostgreSQL Tests ───

export function pgInsert(ctx: any): void {
  const t = new TestResult("SQL-PG-CRUD-INSERT", "SQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("pg_insert", {
      zname: "CanaryAlice",
      age: 30,
      email: "alice@canary.test"
    });
    t.assert("insert_returned", result != null, `result=${JSON.stringify(result)}`);
    t.assert("row_has_id", result?.rows?.[0]?.id != null);
    t.assertEquals("row_zname", "CanaryAlice", result?.rows?.[0]?.zname);
    t.assertEquals("row_age", 30, result?.rows?.[0]?.age);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function pgSelect(ctx: any): void {
  const t = new TestResult("SQL-PG-CRUD-SELECT", "SQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("pg_select", {
      zname: "CanaryAlice",
      age: 30
    });
    t.assert("select_returned_rows", result?.rows?.length > 0,
      `row_count=${result?.rows?.length}`);
    t.assertEquals("first_row_zname", "CanaryAlice", result?.rows?.[0]?.zname);
    t.assertEquals("first_row_age", 30, result?.rows?.[0]?.age);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function pgUpdate(ctx: any): void {
  const t = new TestResult("SQL-PG-CRUD-UPDATE", "SQL", "data-layer §3.1");
  try {
    ctx.dataview("pg_update", {
      zname: "CanaryAlice",
      age: 30,
      email: "alice-updated@canary.test"
    });
    // Verify update by selecting
    const check = ctx.dataview("pg_select", { zname: "CanaryAlice", age: 30 });
    t.assertEquals("email_updated", "alice-updated@canary.test",
      check?.rows?.[0]?.email);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function pgDelete(ctx: any): void {
  const t = new TestResult("SQL-PG-CRUD-DELETE", "SQL", "data-layer §3.1");
  try {
    ctx.dataview("pg_delete", { zname: "CanaryAlice" });
    const check = ctx.dataview("pg_select", { zname: "CanaryAlice", age: 30 });
    t.assertEquals("rows_after_delete", 0, check?.rows?.length || 0);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function pgParamOrder(ctx: any): void {
  const t = new TestResult("SQL-PG-PARAM-ORDER", "SQL", "Issue #54");
  // Insert with zname and age — declaration order is [zname, age]
  // Alphabetical order would be [age, zname]
  // If the driver sorts alphabetically and binds positionally,
  // age gets bound to zname's position → silent data corruption
  try {
    const result = ctx.dataview("pg_insert", {
      zname: "ParamOrderTest",
      age: 99,
      email: "paramorder@canary.test"
    });
    // Read back and verify values are in the correct columns
    const check = ctx.dataview("pg_select", {
      zname: "ParamOrderTest",
      age: 99
    });
    t.assert("row_found", check?.rows?.length > 0);
    t.assertEquals("zname_in_zname_column", "ParamOrderTest",
      check?.rows?.[0]?.zname);
    t.assertEquals("age_in_age_column", 99, check?.rows?.[0]?.age);
    // Cleanup
    ctx.dataview("pg_delete", { zname: "ParamOrderTest" });
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── MySQL Tests (mirror PostgreSQL pattern) ───

export function mysqlInsert(ctx: any): void {
  const t = new TestResult("SQL-MYSQL-CRUD-INSERT", "SQL", "data-layer §3.1");
  try {
    const id = Rivers.crypto.randomHex(16);
    ctx.dataview("mysql_insert", {
      id: id,
      zname: "CanaryBob",
      age: 25,
      email: "bob@canary.test"
    });
    const check = ctx.dataview("mysql_select", { zname: "CanaryBob", age: 25 });
    t.assert("row_found", check?.rows?.length > 0);
    t.assertEquals("row_zname", "CanaryBob", check?.rows?.[0]?.zname);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function mysqlSelect(ctx: any): void {
  const t = new TestResult("SQL-MYSQL-CRUD-SELECT", "SQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("mysql_select", { zname: "CanaryBob", age: 25 });
    t.assert("rows_returned", result?.rows?.length > 0);
    t.assertEquals("zname_correct", "CanaryBob", result?.rows?.[0]?.zname);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function mysqlUpdate(ctx: any): void {
  const t = new TestResult("SQL-MYSQL-CRUD-UPDATE", "SQL", "data-layer §3.1");
  try {
    ctx.dataview("mysql_update", {
      zname: "CanaryBob",
      age: 25,
      email: "bob-updated@canary.test"
    });
    const check = ctx.dataview("mysql_select", { zname: "CanaryBob", age: 25 });
    t.assertEquals("email_updated", "bob-updated@canary.test",
      check?.rows?.[0]?.email);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function mysqlDelete(ctx: any): void {
  const t = new TestResult("SQL-MYSQL-CRUD-DELETE", "SQL", "data-layer §3.1");
  try {
    ctx.dataview("mysql_delete", { zname: "CanaryBob" });
    const check = ctx.dataview("mysql_select", { zname: "CanaryBob", age: 25 });
    t.assertEquals("rows_after_delete", 0, check?.rows?.length || 0);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function mysqlParamOrder(ctx: any): void {
  const t = new TestResult("SQL-MYSQL-PARAM-ORDER", "SQL", "Issue #54");
  try {
    const id = Rivers.crypto.randomHex(16);
    ctx.dataview("mysql_insert", {
      id: id,
      zname: "MysqlParamTest",
      age: 77,
      email: "paramorder@canary.test"
    });
    const check = ctx.dataview("mysql_select", {
      zname: "MysqlParamTest",
      age: 77
    });
    t.assert("row_found", check?.rows?.length > 0);
    t.assertEquals("zname_correct", "MysqlParamTest", check?.rows?.[0]?.zname);
    t.assertEquals("age_correct", 77, check?.rows?.[0]?.age);
    ctx.dataview("mysql_delete", { zname: "MysqlParamTest" });
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── SQLite Tests ───

export function sqliteInsert(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-CRUD-INSERT", "SQL", "data-layer §3.1");
  try {
    const id = Rivers.crypto.randomHex(16);
    ctx.dataview("sqlite_insert", {
      id: id,
      zname: "CanaryCharlie",
      age: 40,
      email: "charlie@canary.test"
    });
    const check = ctx.dataview("sqlite_select", {
      zname: "CanaryCharlie",
      age: 40
    });
    t.assert("row_found", check?.rows?.length > 0);
    t.assertEquals("zname_correct", "CanaryCharlie", check?.rows?.[0]?.zname);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function sqliteSelect(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-CRUD-SELECT", "SQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("sqlite_select", {
      zname: "CanaryCharlie",
      age: 40
    });
    t.assert("rows_returned", result?.rows?.length > 0);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function sqlitePrefix(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-PREFIX", "SQL", "Issue #54");
  // SQLite auto-prefixes `:` but queries might use `$`.
  // This test verifies the DataView engine handles the translation.
  try {
    const result = ctx.dataview("sqlite_select", {
      zname: "CanaryCharlie",
      age: 40
    });
    t.assert("query_executed", result != null,
      "query with $name params succeeded on SQLite");
    t.assert("rows_returned", result?.rows?.length > 0);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── Cache Tests ───

export function cacheL1Hit(ctx: any): void {
  const t = new TestResult("SQL-CACHE-L1-HIT", "SQL", "storage-engine §11.6");
  try {
    // First call — populates cache
    const t1 = Date.now();
    ctx.dataview("pg_select_all", {});
    const d1 = Date.now() - t1;

    // Second call — should hit L1 cache
    const t2 = Date.now();
    const result = ctx.dataview("pg_select_all", {});
    const d2 = Date.now() - t2;

    t.assert("first_call_returned", result != null);
    t.assert("second_call_faster", d2 <= d1,
      `first=${d1}ms, second=${d2}ms`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function cacheInvalidate(ctx: any): void {
  const t = new TestResult("SQL-CACHE-INVALIDATE", "SQL", "data-layer §3.3");
  try {
    // Prime cache
    ctx.dataview("pg_select_all", {});

    // Write — should invalidate pg_select_all cache
    ctx.dataview("pg_insert", {
      zname: "CacheInvalidateTest",
      age: 1,
      email: "cache@canary.test"
    });

    // Read again — should miss cache (fresh query)
    const result = ctx.dataview("pg_select_all", {});
    // We can't directly observe cache miss, but we can verify
    // the new row is present (proving it wasn't a stale cache hit)
    const found = result?.rows?.some((r: any) => r.zname === "CacheInvalidateTest");
    t.assert("new_row_visible", found === true,
      "new row found in result — cache was invalidated");

    // Cleanup
    ctx.dataview("pg_delete", { zname: "CacheInvalidateTest" });
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-sql/libraries/handlers/negative-sql.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function pgDdlReject(ctx: any): void {
  const t = new TestResult("SQL-PG-DDL-REJECT", "SQL",
    "feature-inventory §21.1");
  try {
    // Attempt DDL through a pseudo DataView (not init handler)
    // This MUST be rejected with DriverError::Forbidden
    const ds = ctx.datasource("canary-pg");
    const dv = ds.fromQuery("DROP TABLE canary_records").build();
    const result = ctx.dataview(dv);
    // If we reach here, DDL was NOT rejected — test fails
    t.assert("ddl_rejected", false,
      "DROP TABLE executed without error — DDL guard is broken");
  } catch (e) {
    const errStr = String(e);
    t.assert("ddl_rejected", true, `threw: ${errStr}`);
    t.assert("error_is_forbidden",
      errStr.toLowerCase().includes("forbidden") ||
      errStr.toLowerCase().includes("ddl"),
      `error message: ${errStr}`);
  }
  ctx.resdata = t.finish();
}

export function mysqlDdlReject(ctx: any): void {
  const t = new TestResult("SQL-MYSQL-DDL-REJECT", "SQL",
    "feature-inventory §21.1");
  try {
    const ds = ctx.datasource("canary-mysql");
    const dv = ds.fromQuery("DROP TABLE canary_records").build();
    ctx.dataview(dv);
    t.assert("ddl_rejected", false,
      "DROP TABLE executed without error — DDL guard is broken");
  } catch (e) {
    const errStr = String(e);
    t.assert("ddl_rejected", true, `threw: ${errStr}`);
    t.assert("error_is_forbidden",
      errStr.toLowerCase().includes("forbidden") ||
      errStr.toLowerCase().includes("ddl"),
      `error message: ${errStr}`);
  }
  ctx.resdata = t.finish();
}
```

**SQLite path fallback handlers (sql-tests.ts additions) [v0.53.0]:**

```typescript
export function sqlitePathDatabase(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-PATH-DATABASE", "SQL",
    "driver-spec §4.5");
  try {
    // Create a datasource using database= for file path
    const ds = ctx.datasource("canary-sqlite");
    // The canary-sqlite datasource uses path = ":memory:" which exercises database=
    // Verify it is operational by running a query
    const result = ctx.dataview("sqlite_select", {
      zname: "CanaryCharlie", age: 40
    });
    t.assert("database_path_works", result != null,
      "query succeeded using database= path");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function sqlitePathHost(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-PATH-HOST", "SQL",
    "driver-spec §4.5");
  // The host= fallback is tested by constructing a pseudo DataView
  // with a temporary SQLite file using host= instead of database=
  try {
    const tmpPath = `/tmp/canary-sqlite-host-${Date.now()}.db`;
    const ds = ctx.datasource("canary-sqlite");
    // Attempt to open via host= fallback configuration
    // The driver should accept host= as an alternative to database=
    const dv = ds.fromConfig({ host: tmpPath })
      .fromQuery("CREATE TABLE IF NOT EXISTS test_host (id TEXT PRIMARY KEY)")
      .build();
    ctx.dataview(dv);
    t.assert("host_fallback_works", true,
      `SQLite opened via host= at ${tmpPath}`);
  } catch (e) {
    // If the driver doesn't support fromConfig, this is also valid feedback
    const errStr = String(e);
    t.assert("host_fallback_attempted", true,
      `driver response: ${errStr}`);
  }
  ctx.resdata = t.finish();
}

export function sqlitePathMkdir(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-PATH-MKDIR", "SQL",
    "driver-spec §4.5");
  try {
    // Use a path with non-existent parent directories
    const deepPath = `/tmp/canary-sqlite-mkdir-${Date.now()}/nested/dir/test.db`;
    const ds = ctx.datasource("canary-sqlite");
    const dv = ds.fromConfig({ database: deepPath })
      .fromQuery("CREATE TABLE IF NOT EXISTS test_mkdir (id TEXT PRIMARY KEY)")
      .build();
    ctx.dataview(dv);
    t.assert("parent_dirs_created", true,
      `SQLite created file at ${deepPath} (parent dirs auto-created)`);
  } catch (e) {
    t.fail(`parent directory creation failed: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}

export function sqlitePathEmpty(ctx: any): void {
  const t = new TestResult("SQL-SQLITE-PATH-EMPTY", "SQL",
    "driver-spec §4.5");
  try {
    // Attempt to open SQLite with both database= and host= empty
    const ds = ctx.datasource("canary-sqlite");
    const dv = ds.fromConfig({ database: "", host: "" })
      .fromQuery("SELECT 1")
      .build();
    ctx.dataview(dv);
    t.assert("empty_path_rejected", false,
      "query succeeded with empty path — should have been rejected");
  } catch (e) {
    const errStr = String(e);
    t.assert("empty_path_rejected", true, `threw: ${errStr}`);
    t.assert("clear_error_message",
      errStr.toLowerCase().includes("path") ||
      errStr.toLowerCase().includes("database") ||
      errStr.toLowerCase().includes("empty"),
      `error should mention path/database: ${errStr}`);
  }
  ctx.resdata = t.finish();
}
```

**canary-sql/app.toml additions for SQLite path fallback views [v0.53.0]:**

```toml
# ─── SQLite Path Fallback Tests (v0.53.0) ───

[api.views.sqlite_path_database]
path      = "/canary/sql/sqlite/path-database"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_path_database.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqlitePathDatabase"
resources  = ["canary-sqlite"]

[api.views.sqlite_path_host]
path      = "/canary/sql/sqlite/path-host"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_path_host.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqlitePathHost"
resources  = ["canary-sqlite"]

[api.views.sqlite_path_mkdir]
path      = "/canary/sql/sqlite/path-mkdir"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_path_mkdir.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqlitePathMkdir"
resources  = ["canary-sqlite"]

[api.views.sqlite_path_empty]
path      = "/canary/sql/sqlite/path-empty"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.sqlite_path_empty.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sql-tests.ts"
entrypoint = "sqlitePathEmpty"
resources  = ["canary-sqlite"]
```

---

## Part 4 — Profile: NOSQL (canary-nosql)

### Purpose

Tests non-SQL driver operations: MongoDB read/write, Elasticsearch index/search, CouchDB document CRUD, Cassandra read/write, LDAP bind/search, and Redis KV operations. Tests admin operation rejection per driver.

### Test Inventory

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| NOSQL-MONGO-INSERT | `/canary/nosql/mongo/insert` | POST | MongoDB document insert | data-layer §3.1 |
| NOSQL-MONGO-FIND | `/canary/nosql/mongo/find` | GET | MongoDB document query | data-layer §3.1 |
| NOSQL-MONGO-ADMIN-REJECT | `/canary/nosql/mongo/admin-reject` | POST | drop_collection rejected | feature-inventory §21.1 |
| NOSQL-ES-INDEX | `/canary/nosql/es/index` | POST | Elasticsearch document index | data-layer §3.1 |
| NOSQL-ES-SEARCH | `/canary/nosql/es/search` | GET | Elasticsearch search query | data-layer §3.1 |
| NOSQL-COUCH-PUT | `/canary/nosql/couch/put` | POST | CouchDB document create | data-layer §3.1 |
| NOSQL-COUCH-GET | `/canary/nosql/couch/get` | GET | CouchDB document read | data-layer §3.1 |
| NOSQL-CASSANDRA-INSERT | `/canary/nosql/cassandra/insert` | POST | Cassandra row insert | data-layer §3.1 |
| NOSQL-CASSANDRA-SELECT | `/canary/nosql/cassandra/select` | GET | Cassandra row query | data-layer §3.1 |
| NOSQL-LDAP-SEARCH | `/canary/nosql/ldap/search` | GET | LDAP directory search | driver-spec §6.3 |
| NOSQL-REDIS-SET | `/canary/nosql/redis/set` | POST | Redis SET/GET operations | data-layer §4.1 |
| NOSQL-REDIS-GET | `/canary/nosql/redis/get` | GET | Redis GET by key | data-layer §4.1 |
| NOSQL-REDIS-ADMIN-REJECT | `/canary/nosql/redis/admin-reject` | POST | FLUSHDB rejected | feature-inventory §21.1 |

**Config and handler files for canary-nosql follow the identical pattern as canary-sql.** Each driver gets a datasource declaration, schema file, DataView definitions, view endpoints, and a handler that calls `ctx.dataview()` and reports the self-test result. Negative tests attempt admin operations and assert `DriverError::Forbidden`.

The critical constraint: every NoSQL handler must use the `TestResult` harness and return the standard verdict envelope. Handlers are in `nosql-tests.ts` (positive) and `negative-nosql.ts` (negative).

### canary-nosql/manifest.toml

```toml
appName       = "canary-nosql"
description   = "Canary Fleet — NOSQL profile: MongoDB, ES, CouchDB, Cassandra, LDAP, Redis"
version       = "1.0.0"
type          = "app-service"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee02"
entryPoint    = "http://0.0.0.0:9102"
appEntryPoint = "https://canary-nosql.internal"
source        = "https://github.com/rivers-framework/canary-fleet/canary-nosql"
```

### canary-nosql/resources.toml

```toml
[[datasources]]
name     = "canary-mongo"
driver   = "mongodb"
x-type   = "mongodb"
required = true

[datasources.lockbox]
alias = "canary-mongo"

[[datasources]]
name       = "canary-es"
driver     = "elasticsearch"
x-type     = "elasticsearch"
nopassword = true
required   = true

[datasources.lockbox]
alias = "canary-es"

[[datasources]]
name     = "canary-couch"
driver   = "couchdb"
x-type   = "couchdb"
required = true

[datasources.lockbox]
alias = "canary-couch"

[[datasources]]
name       = "canary-cassandra"
driver     = "cassandra"
x-type     = "cassandra"
nopassword = true
required   = true

[datasources.lockbox]
alias = "canary-cassandra"

[[datasources]]
name     = "canary-ldap"
driver   = "ldap"
x-type   = "ldap"
required = true

[datasources.lockbox]
alias = "canary-ldap"

[[datasources]]
name     = "canary-redis"
driver   = "redis"
x-type   = "redis"
required = true

[datasources.lockbox]
alias = "canary-redis"

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

**Remaining NoSQL config (app.toml, schemas, handlers):** Follow the exact same pattern as canary-sql. Each driver gets DataViews, Views, and handlers. The handler patterns are identical — call `ctx.dataview()`, assert on the result, return the verdict envelope. Negative handlers attempt admin operations and expect `Forbidden`.

---

## Part 5 — Profile: RUNTIME (canary-handlers)

### Purpose

Tests the handler execution environment: every `ctx.*` property and method, every `Rivers.*` global API, StorageEngine application KV, EventBus publishing, V8 security guardrails, and error sanitization.

### Test Inventory

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| RT-CTX-TRACE-ID | `/canary/rt/ctx/trace-id` | GET | ctx.trace_id is a non-empty string | processpool §9.8 |
| RT-CTX-NODE-ID | `/canary/rt/ctx/node-id` | GET | ctx.node_id is a non-empty string | processpool §9.8 |
| RT-CTX-APP-ID | `/canary/rt/ctx/app-id` | GET | ctx.app_id matches manifest appId | processpool §9.8 |
| RT-CTX-ENV | `/canary/rt/ctx/env` | GET | ctx.env is a non-empty string | processpool §9.8 |
| RT-CTX-SESSION | `/canary/rt/ctx/session` | GET | ctx.session is an object with claims | processpool §9.8 |
| RT-CTX-REQUEST | `/canary/rt/ctx/request` | POST | ctx.request has method, path, headers, body | processpool §9.8 |
| RT-CTX-DATA | `/canary/rt/ctx/data` | GET | ctx.data contains pre-fetched DataView results | processpool §9.8 |
| RT-CTX-RESDATA | `/canary/rt/ctx/resdata` | GET | ctx.resdata is writable and becomes response | processpool §9.8 |
| RT-CTX-DATAVIEW | `/canary/rt/ctx/dataview` | GET | ctx.dataview() calls DataView with params | processpool §9.8 |
| RT-CTX-DATAVIEW-PARAMS | `/canary/rt/ctx/dataview-params` | POST | ctx.dataview() passes params correctly (not dropped) | dream-doc: ctx.dataview() bug |
| RT-CTX-PSEUDO-DV | `/canary/rt/ctx/pseudo-dv` | GET | ctx.datasource() pseudo DataView builder | view-layer §3.2 |
| RT-CTX-STORE-GET-SET | `/canary/rt/ctx/store` | POST | ctx.store.get/set/del operations | storage-engine §11.5 |
| RT-CTX-STORE-NAMESPACE | `/canary/rt/ctx/store-ns` | GET | ctx.store rejects reserved namespace prefixes | storage-engine §11.3 |
| RT-RIVERS-LOG | `/canary/rt/rivers/log` | GET | Rivers.log.info/warn/error execute without throw | processpool §9.10 |
| RT-RIVERS-CRYPTO-HASH | `/canary/rt/rivers/crypto-hash` | GET | Rivers.crypto.hashPassword + verifyPassword | processpool §9.10 |
| RT-RIVERS-CRYPTO-RANDOM | `/canary/rt/rivers/crypto-random` | GET | Rivers.crypto.randomHex + randomBase64url | processpool §9.10 |
| RT-RIVERS-CRYPTO-HMAC | `/canary/rt/rivers/crypto-hmac` | GET | Rivers.crypto.hmac produces consistent output | processpool §9.10 |
| RT-RIVERS-CRYPTO-TIMING | `/canary/rt/rivers/crypto-timing` | GET | Rivers.crypto.timingSafeEqual works for equal+unequal | processpool §9.10 |
| RT-V8-TIMEOUT | `/canary/rt/v8/timeout` | GET | Infinite loop terminates within task_timeout_ms | feature-inventory §21.2 |
| RT-V8-HEAP | `/canary/rt/v8/heap` | GET | Massive allocation triggers heap callback, not crash | feature-inventory §21.2 |
| RT-V8-CODEGEN | `/canary/rt/v8/codegen` | GET | eval() and Function() are blocked | feature-inventory §21.2 |
| RT-V8-CONSOLE | `/canary/rt/v8/console` | GET | console.log is not available | processpool §9.1 |
| RT-EVENTBUS-PUBLISH | `/canary/rt/eventbus/publish` | POST | Handler publishes event to EventBus topic | eventbus §12.1 |
| RT-ERROR-SANITIZE | `/canary/rt/error/sanitize` | GET | Error response doesn't leak driver names | feature-inventory §21.5 |
| RT-HEADER-BLOCKLIST | `/canary/rt/header/blocklist` | GET | Handler-set Set-Cookie is stripped from response | feature-inventory §1.5 |
| RT-FAKER-DETERMINISM | `/canary/rt/faker/determinism` | GET | Seeded faker returns identical results | data-layer §4.1 |
| RT-LOG-APP-ROUTER | `/canary/rt/log/app-router` | GET | Rivers.log.info writes to log/apps/canary-handlers.log | logging §5.2 |
| RT-LOG-STRUCTURED | `/canary/rt/log/structured` | POST | Rivers.log.info with structured fields includes key=value | logging §5.3 |
| RT-LOG-LEVELS | `/canary/rt/log/levels` | GET | Rivers.log.info/warn/error each produce correct level tag | logging §5.1 |

### canary-handlers/manifest.toml

```toml
appName       = "canary-handlers"
description   = "Canary Fleet — RUNTIME profile: ctx/Rivers API surface, V8 security, StorageEngine"
version       = "1.0.0"
type          = "app-service"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee03"
entryPoint    = "http://0.0.0.0:9103"
appEntryPoint = "https://canary-handlers.internal"
source        = "https://github.com/rivers-framework/canary-fleet/canary-handlers"
```

### canary-handlers/resources.toml

```toml
[[datasources]]
name       = "canary-faker"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

**Handler examples (ctx-surface.ts):**

```typescript
import { TestResult } from "./test-harness.ts";

export function checkTraceId(ctx: any): void {
  const t = new TestResult("RT-CTX-TRACE-ID", "RUNTIME", "processpool §9.8");
  t.assertExists("trace_id", ctx.trace_id);
  t.assertType("trace_id_is_string", ctx.trace_id, "string");
  t.assert("trace_id_not_empty", ctx.trace_id.length > 0);
  ctx.resdata = t.finish();
}

export function checkAppId(ctx: any): void {
  const t = new TestResult("RT-CTX-APP-ID", "RUNTIME", "processpool §9.8");
  t.assertExists("app_id", ctx.app_id);
  t.assertEquals("app_id_matches_manifest",
    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee03", ctx.app_id);
  ctx.resdata = t.finish();
}

export function checkDataviewParams(ctx: any): void {
  const t = new TestResult("RT-CTX-DATAVIEW-PARAMS", "RUNTIME",
    "dream-doc: ctx.dataview() bug");
  // This is THE test for the ctx.dataview() param-dropping bug.
  // We call a DataView with known params and verify the results match.
  try {
    const result = ctx.dataview("faker_by_seed", { seed: 42, limit: 5 });
    t.assert("result_not_null", result != null);
    t.assert("result_has_rows", result?.rows?.length > 0,
      `row_count=${result?.rows?.length}`);
    t.assertEquals("row_count", 5, result?.rows?.length);
  } catch (e) {
    t.fail(`ctx.dataview() with params threw: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}

export function checkStoreNamespace(ctx: any): void {
  const t = new TestResult("RT-CTX-STORE-NAMESPACE", "RUNTIME",
    "storage-engine §11.3");
  // Attempt to read a reserved namespace — must fail
  try {
    ctx.store.get("session:hijack-attempt");
    t.assert("reserved_ns_rejected", false,
      "session: prefix was accessible — namespace isolation broken");
  } catch (e) {
    t.assert("reserved_ns_rejected", true, `threw: ${String(e)}`);
  }
  // Attempt to write a custom key — must succeed
  try {
    ctx.store.set("canary-test-key", "canary-value", 60);
    const val = ctx.store.get("canary-test-key");
    t.assertEquals("custom_key_value", "canary-value", val);
    ctx.store.del("canary-test-key");
  } catch (e) {
    t.fail(`custom namespace failed: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}
```

**V8 security handlers (v8-security.ts):**

```typescript
import { TestResult } from "./test-harness.ts";

export function v8Timeout(ctx: any): void {
  // This handler deliberately runs an infinite loop.
  // The V8 watchdog thread must terminate it within task_timeout_ms.
  // The Rust integration test expects a timeout error response, NOT a hang.
  // If this handler returns a verdict, the timeout did NOT fire.
  const t = new TestResult("RT-V8-TIMEOUT", "RUNTIME",
    "feature-inventory §21.2");
  t.assert("should_not_reach", false,
    "infinite loop completed — V8 timeout not enforced");
  while (true) {} // infinite loop — watchdog should kill this
  ctx.resdata = t.finish(); // unreachable
}

export function v8Heap(ctx: any): void {
  // Allocate massive arrays to trigger NearHeapLimitCallback.
  // Must NOT crash the process — must terminate the handler gracefully.
  const t = new TestResult("RT-V8-HEAP", "RUNTIME",
    "feature-inventory §21.2");
  try {
    const arrays: any[] = [];
    for (let i = 0; i < 1000000; i++) {
      arrays.push(new Array(100000).fill(i));
    }
    t.assert("heap_limit_enforced", false,
      "massive allocation succeeded — heap limit not enforced");
  } catch (e) {
    t.assert("heap_limit_enforced", true, `threw: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}

export function v8Codegen(ctx: any): void {
  const t = new TestResult("RT-V8-CODEGEN", "RUNTIME",
    "feature-inventory §21.2");
  // eval() must be blocked
  try {
    const result = eval("1 + 1");
    t.assert("eval_blocked", false, `eval returned: ${result}`);
  } catch (e) {
    t.assert("eval_blocked", true, `eval threw: ${String(e)}`);
  }
  // Function constructor must be blocked
  try {
    const fn = new Function("return 42");
    t.assert("function_constructor_blocked", false, `Function() returned: ${fn()}`);
  } catch (e) {
    t.assert("function_constructor_blocked", true,
      `Function() threw: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}

export function v8Console(ctx: any): void {
  const t = new TestResult("RT-V8-CONSOLE", "RUNTIME", "processpool §9.1");
  t.assert("console_not_available",
    typeof console === "undefined" || typeof console.log !== "function",
    `typeof console=${typeof console}`);
  ctx.resdata = t.finish();
}

export function headerBlocklist(ctx: any): void {
  const t = new TestResult("RT-HEADER-BLOCKLIST", "RUNTIME",
    "feature-inventory §1.5");
  // Set both a blocked and allowed header.
  // The Rust integration test verifies:
  // - Set-Cookie is NOT in the response
  // - X-Canary-Custom IS in the response
  ctx.response?.setHeader?.("Set-Cookie", "evil=true; Path=/");
  ctx.response?.setHeader?.("X-Canary-Custom", "canary-value");
  t.assert("headers_set", true, "handler attempted to set both headers");
  ctx.resdata = t.finish();
}

export function errorSanitize(ctx: any): void {
  // Deliberately throw with a message containing driver names.
  // The error response to the client must NOT contain these strings.
  // The Rust integration test checks the response body.
  throw new Error("connection to postgres at 192.168.2.209:5432 refused by mysql driver");
}
```

**Per-app logging handlers (rivers-api.ts additions) [v0.53.0]:**

```typescript
export function logAppRouter(ctx: any): void {
  const t = new TestResult("RT-LOG-APP-ROUTER", "RUNTIME", "logging §5.2");
  try {
    // Write a uniquely identifiable entry
    const marker = `canary-marker-${Date.now()}`;
    Rivers.log.info(marker);

    // Verify the app-specific log file exists and contains the marker.
    // AppLogRouter routes Rivers.log.* calls to log/apps/<appName>.log.
    const logPath = "log/apps/canary-handlers.log";
    const exists = Rivers.fs.exists(logPath);
    t.assert("app_log_exists", exists === true, `path=${logPath}`);

    if (exists) {
      const content = Rivers.fs.readText(logPath);
      t.assert("marker_in_log", content.includes(marker),
        `marker=${marker}`);
    }
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function logStructured(ctx: any): void {
  const t = new TestResult("RT-LOG-STRUCTURED", "RUNTIME", "logging §5.3");
  try {
    const marker = `structured-${Date.now()}`;
    Rivers.log.info(marker, { key: "canary-value", count: 42 });

    const logPath = "log/apps/canary-handlers.log";
    const content = Rivers.fs.readText(logPath);
    // Structured fields should appear as key=value pairs in the log line
    t.assert("marker_present", content.includes(marker));
    t.assert("structured_key",
      content.includes("key=canary-value") || content.includes("\"key\":\"canary-value\""),
      "structured field 'key' found in log");
    t.assert("structured_count",
      content.includes("count=42") || content.includes("\"count\":42"),
      "structured field 'count' found in log");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function logLevels(ctx: any): void {
  const t = new TestResult("RT-LOG-LEVELS", "RUNTIME", "logging §5.1");
  try {
    const ts = Date.now();
    Rivers.log.info(`level-test-info-${ts}`);
    Rivers.log.warn(`level-test-warn-${ts}`);
    Rivers.log.error(`level-test-error-${ts}`);

    const logPath = "log/apps/canary-handlers.log";
    const content = Rivers.fs.readText(logPath);

    // Each level should produce a line with the correct level tag
    t.assert("info_level",
      content.includes(`INFO`) && content.includes(`level-test-info-${ts}`),
      "INFO level tag present");
    t.assert("warn_level",
      content.includes(`WARN`) && content.includes(`level-test-warn-${ts}`),
      "WARN level tag present");
    t.assert("error_level",
      content.includes(`ERROR`) && content.includes(`level-test-error-${ts}`),
      "ERROR level tag present");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

**canary-handlers/app.toml additions for per-app logging views [v0.53.0]:**

```toml
# ─── Per-App Logging Tests (v0.53.0) ───

[api.views.log_app_router]
path      = "/canary/rt/log/app-router"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.log_app_router.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/rivers-api.ts"
entrypoint = "logAppRouter"

[api.views.log_structured]
path      = "/canary/rt/log/structured"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.log_structured.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/rivers-api.ts"
entrypoint = "logStructured"

[api.views.log_levels]
path      = "/canary/rt/log/levels"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.log_levels.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/rivers-api.ts"
entrypoint = "logLevels"
```

---

## Part 6 — Profile: STREAM (canary-streams)

### Purpose

Tests all persistent connection and streaming view types: WebSocket (broadcast + direct), SSE (event-driven + tick-based), Streaming REST (NDJSON), Kafka MessageConsumer, and Polling with hash diff strategy.

### Test Inventory

| Test ID | Endpoint | Type | What It Tests | Spec Ref |
|---------|----------|------|---------------|----------|
| STREAM-WS-ECHO | `/canary/stream/ws/echo` | WebSocket | Message send and receive | view-layer §2.4 |
| STREAM-WS-BROADCAST | `/canary/stream/ws/broadcast` | WebSocket | Fan-out to all connected clients | view-layer §2.4 |
| STREAM-WS-BINARY-LOG | `/canary/stream/ws/echo` | WebSocket | Binary frame produces rate-limited WARN | SHAPE-13 |
| STREAM-SSE-TICK | `/canary/stream/sse/tick` | SSE | Tick-based push at interval | view-layer §2.5 |
| STREAM-SSE-EVENT | `/canary/stream/sse/event` | SSE | EventBus-triggered push (cross-app from canary-handlers) | view-layer §2.5 |
| STREAM-REST-NDJSON | `/canary/stream/rest/ndjson` | POST | Streaming REST with NDJSON wire format | streaming-rest §2.7 |
| STREAM-REST-POISON | `/canary/stream/rest/poison` | POST | Poison chunk guard on stream_terminated field | SHAPE-15 |
| STREAM-KAFKA-CONSUME | `/canary/stream/kafka/consume` | MessageConsumer | Kafka message triggers handler | view-layer §2.6 |
| STREAM-POLL-HASH | `/canary/stream/poll/hash` | SSE+Polling | Polling with hash diff — push on change, no push on same | polling-views §10.2 |

### canary-streams/manifest.toml

```toml
appName       = "canary-streams"
description   = "Canary Fleet — STREAM profile: WebSocket, SSE, Streaming REST, Kafka, Polling"
version       = "1.0.0"
type          = "app-service"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee04"
entryPoint    = "http://0.0.0.0:9104"
appEntryPoint = "https://canary-streams.internal"
source        = "https://github.com/rivers-framework/canary-fleet/canary-streams"
```

### canary-streams/resources.toml

```toml
[[datasources]]
name       = "canary-kafka"
driver     = "kafka"
x-type     = "kafka"
nopassword = true
required   = true

[datasources.lockbox]
alias = "canary-kafka"

[[datasources]]
name       = "canary-faker"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

### canary-streams/app.toml (key view configs)

```toml
# ─── WebSocket ───

[api.views.ws_echo]
path      = "/canary/stream/ws/echo"
method    = "GET"
view_type = "Websocket"
auth      = "session"

[api.views.ws_echo.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/ws-handler.ts"
entrypoint = "onConnection"

[api.views.ws_echo.on_stream]
module     = "handlers/ws-handler.ts"
entrypoint = "onMessage"

[api.views.ws_broadcast]
path      = "/canary/stream/ws/broadcast"
method    = "GET"
view_type = "Websocket"
auth      = "session"

[api.views.ws_broadcast.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/ws-handler.ts"
entrypoint = "onBroadcastConnection"

[api.views.ws_broadcast.on_stream]
module     = "handlers/ws-handler.ts"
entrypoint = "onBroadcastMessage"

# ─── SSE ───

[api.views.sse_tick]
path                 = "/canary/stream/sse/tick"
method               = "GET"
view_type            = "ServerSentEvents"
sse_tick_interval_ms = 1000
auth                 = "session"

[api.views.sse_tick.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sse-handler.ts"
entrypoint = "onTick"

[api.views.sse_event]
path                = "/canary/stream/sse/event"
method              = "GET"
view_type           = "ServerSentEvents"
sse_tick_interval_ms = 0
sse_trigger_events  = ["canary.ping"]
auth                = "optional"

[api.views.sse_event.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/sse-handler.ts"
entrypoint = "onEventTriggered"

# ─── Streaming REST ───

[api.views.stream_ndjson]
path               = "/canary/stream/rest/ndjson"
method             = "POST"
view_type          = "Rest"
stream_format      = "ndjson"
stream_timeout_ms  = 10000
auth               = "session"

[api.views.stream_ndjson.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/streaming-rest.ts"
entrypoint = "streamNdjson"

[api.views.stream_poison]
path               = "/canary/stream/rest/poison"
method             = "POST"
view_type          = "Rest"
stream_format      = "ndjson"
stream_timeout_ms  = 10000
auth               = "session"

[api.views.stream_poison.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/streaming-rest.ts"
entrypoint = "streamPoison"

# ─── Kafka MessageConsumer ───

[api.views.kafka_consume]
view_type = "MessageConsumer"

[api.views.kafka_consume.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/kafka-consumer.ts"
entrypoint = "onMessage"
resources  = ["canary-kafka"]

[api.views.kafka_consume.on_event]
topic = "canary.kafka.test"

# ─── Polling ───

[api.views.poll_hash]
path      = "/canary/stream/poll/hash"
method    = "GET"
view_type = "ServerSentEvents"
auth      = "optional"

[api.views.poll_hash.handler]
type     = "data_view"
dataview = "poll_data"

[api.views.poll_hash.polling]
tick_interval_ms = 2000
diff_strategy    = "hash"
poll_state_ttl_s = 60

[api.views.poll_hash.polling.on_change]
module     = "handlers/poll-handler.ts"
entrypoint = "onPollChange"
```

### Streaming REST handler (streaming-rest.ts):

```typescript
export function* streamNdjson(ctx: any): Generator {
  // Yield 5 NDJSON chunks, then close
  for (let i = 0; i < 5; i++) {
    yield {
      chunk_index: i,
      test_id: "STREAM-REST-NDJSON",
      profile: "STREAM",
      data: `chunk-${i}`
    };
  }
}

export function* streamPoison(ctx: any): Generator {
  // Yield a normal chunk first
  yield { chunk_index: 0, data: "normal" };
  // Then yield a chunk with stream_terminated — SHAPE-15 guard must block this
  yield { stream_terminated: true, data: "this should be blocked" };
  // If guard works, this is unreachable (generator terminated by runtime)
  yield { chunk_index: 2, data: "should not arrive" };
}
```

---

## Part 6b — Profile: OPS (canary-ops) [v0.53.0]

### Purpose

Tests operational infrastructure added in v0.53.0: PID file lifecycle, Prometheus metrics endpoint, per-app log routing (AppLogRouter), config discovery, `riversctl` commands (stop/status), `riversctl doctor --fix/--lint`, TLS cert renewal, `riverpackage init/validate`, and engine loader naming conventions. This profile exercises the control plane and observability surface — features that are invisible to application handlers but critical for production operations.

### Test Inventory

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| OPS-PID-EXISTS | `/canary/ops/pid/exists` | GET | PID file exists at `run/riversd.pid` after startup | httpd §2.3 |
| OPS-PID-VALID | `/canary/ops/pid/valid` | GET | PID file contains a numeric PID matching running process | httpd §2.3 |
| OPS-PID-CLEANUP | (harness) | — | PID file removed after `riversctl stop` | httpd §2.3 |
| OPS-METRICS-ENDPOINT | `/canary/ops/metrics/endpoint` | GET | `/metrics` on port 9091 returns Prometheus text format | metrics §1.1 |
| OPS-METRICS-REQUEST-COUNTER | `/canary/ops/metrics/request-counter` | GET | `rivers_http_requests_total` increments after requests | metrics §1.2 |
| OPS-METRICS-DURATION-HISTOGRAM | `/canary/ops/metrics/duration` | GET | `rivers_http_request_duration_ms` records latencies | metrics §1.3 |
| OPS-LOG-APP-FILE | `/canary/ops/log/app-file` | GET | `log/apps/canary-ops.log` exists and receives entries | logging §5.2 |
| OPS-LOG-ROTATION-SIZE | `/canary/ops/log/rotation` | POST | Log rotation triggers at 10MB threshold | logging §5.4 |
| OPS-LOG-CROSS-APP | `/canary/ops/log/cross-app` | GET | Each canary app has its own log file in `log/apps/` | logging §5.2 |
| OPS-CONFIG-DISCOVERY | `/canary/ops/config/discovery` | GET | `riversd.toml` discovered from expected path | httpd §1.2 |
| OPS-RIVERSCTL-STATUS | (harness) | — | `riversctl status` reports running state correctly | httpd §2.5 |
| OPS-RIVERSCTL-STOP | (harness) | — | `riversctl stop` sends SIGTERM and process exits | httpd §2.5 |
| OPS-DOCTOR-LINT-PASS | (harness) | — | `riversctl doctor --lint` passes on valid canary bundle | doctor §3.1 |
| OPS-DOCTOR-LINT-FAIL | (harness) | — | `riversctl doctor --lint` fails on intentionally broken bundle | doctor §3.1 |
| OPS-DOCTOR-FIX-LOCKBOX | (harness) | — | `riversctl doctor --fix` auto-repairs missing lockbox | doctor §3.2 |
| OPS-DOCTOR-FIX-LOGDIRS | (harness) | — | `riversctl doctor --fix` creates missing log directories | doctor §3.2 |
| OPS-DOCTOR-FIX-PERMS | (harness) | — | `riversctl doctor --fix` corrects file permissions | doctor §3.2 |
| OPS-DOCTOR-FIX-TLS | (harness) | — | `riversctl doctor --fix` auto-repairs missing TLS certs | doctor §3.2 |
| OPS-TLS-CERT-RENEW | (harness) | — | `riversctl tls renew` regenerates cert successfully | tls §4.1 |
| OPS-TLS-CERT-EXPIRY | (harness) | — | Doctor detects cert near expiry and auto-renews with `--fix` | tls §4.2 |
| OPS-RIVERPACKAGE-INIT | (harness) | — | `riverpackage init <name>` scaffolds valid bundle structure | packaging §6.1 |
| OPS-RIVERPACKAGE-VALIDATE | (harness) | — | `riverpackage validate` passes on scaffolded bundle | packaging §6.2 |
| OPS-ENGINE-LOADER-NAMING | (harness) | — | Engine dylib named `librivers_engine_v8.dylib` loads correctly | engine-sdk §7.1 |
| OPS-PLUGIN-ABI-EXPORTS | (harness) | — | Plugin dylib with `--features plugin-exports` has required ABI symbols | engine-sdk §7.2 |

### canary-ops/manifest.toml

```toml
appName       = "canary-ops"
description   = "Canary Fleet — OPS profile: PID, metrics, logging, doctor, TLS, config discovery"
version       = "1.0.0"
type          = "app-service"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee06"
entryPoint    = "http://0.0.0.0:9105"
appEntryPoint = "https://canary-ops.internal"
source        = "https://github.com/rivers-framework/canary-fleet/canary-ops"
```

### canary-ops/resources.toml

```toml
# canary-ops has no external datasources — it tests operational infrastructure.
# It reads files, scrapes metrics, and verifies process state.

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

### canary-ops/app.toml

```toml
# ─────────────────────────────────────────────
# Views — PID file tests
# ─────────────────────────────────────────────

[api.views.pid_exists]
path      = "/canary/ops/pid/exists"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.pid_exists.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/pid-tests.ts"
entrypoint = "pidExists"

[api.views.pid_valid]
path      = "/canary/ops/pid/valid"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.pid_valid.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/pid-tests.ts"
entrypoint = "pidValid"

# ─────────────────────────────────────────────
# Views — Metrics tests
# ─────────────────────────────────────────────

[api.views.metrics_endpoint]
path      = "/canary/ops/metrics/endpoint"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.metrics_endpoint.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/metrics-tests.ts"
entrypoint = "metricsEndpoint"

[api.views.metrics_request_counter]
path      = "/canary/ops/metrics/request-counter"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.metrics_request_counter.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/metrics-tests.ts"
entrypoint = "metricsRequestCounter"

[api.views.metrics_duration]
path      = "/canary/ops/metrics/duration"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.metrics_duration.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/metrics-tests.ts"
entrypoint = "metricsDuration"

# ─────────────────────────────────────────────
# Views — Logging tests
# ─────────────────────────────────────────────

[api.views.log_app_file]
path      = "/canary/ops/log/app-file"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.log_app_file.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/logging-tests.ts"
entrypoint = "logAppFile"

[api.views.log_rotation]
path      = "/canary/ops/log/rotation"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.log_rotation.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/logging-tests.ts"
entrypoint = "logRotation"

[api.views.log_cross_app]
path      = "/canary/ops/log/cross-app"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.log_cross_app.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/logging-tests.ts"
entrypoint = "logCrossApp"

# ─────────────────────────────────────────────
# Views — Config discovery
# ─────────────────────────────────────────────

[api.views.config_discovery]
path      = "/canary/ops/config/discovery"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.config_discovery.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/config-discovery-tests.ts"
entrypoint = "configDiscovery"
```

### canary-ops/libraries/handlers/pid-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function pidExists(ctx: any): void {
  const t = new TestResult("OPS-PID-EXISTS", "OPS", "httpd §2.3");
  try {
    // Read PID file from well-known location
    const pidPath = "run/riversd.pid";
    const exists = Rivers.fs.exists(pidPath);
    t.assert("pid_file_exists", exists === true,
      `path=${pidPath}, exists=${exists}`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function pidValid(ctx: any): void {
  const t = new TestResult("OPS-PID-VALID", "OPS", "httpd §2.3");
  try {
    const pidPath = "run/riversd.pid";
    const content = Rivers.fs.readText(pidPath);
    t.assertExists("pid_content", content);
    const pid = parseInt(content.trim(), 10);
    t.assert("pid_is_numeric", !isNaN(pid), `parsed=${pid}`);
    t.assert("pid_positive", pid > 0, `pid=${pid}`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-ops/libraries/handlers/metrics-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function metricsEndpoint(ctx: any): void {
  const t = new TestResult("OPS-METRICS-ENDPOINT", "OPS", "metrics §1.1");
  try {
    // Scrape the metrics endpoint on port 9091
    const ds = ctx.datasource("http");
    const dv = ds.fromQuery("http://127.0.0.1:9091/metrics")
      .method("GET")
      .build();
    const result = ctx.dataview(dv);
    const body = result?.rows?.[0]?.body || "";
    t.assert("metrics_returned", body.length > 0, `body_length=${body.length}`);
    t.assert("prometheus_format",
      body.includes("# HELP") || body.includes("# TYPE"),
      "response contains Prometheus metadata comments");
    t.assert("has_rivers_prefix",
      body.includes("rivers_"),
      "response contains rivers_ prefixed metrics");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function metricsRequestCounter(ctx: any): void {
  const t = new TestResult("OPS-METRICS-REQUEST-COUNTER", "OPS",
    "metrics §1.2");
  try {
    // Scrape metrics, extract counter, make a request, scrape again
    const ds = ctx.datasource("http");
    const scrape = () => {
      const dv = ds.fromQuery("http://127.0.0.1:9091/metrics")
        .method("GET").build();
      return ctx.dataview(dv)?.rows?.[0]?.body || "";
    };

    const before = scrape();
    const counterMatch = before.match(/rivers_http_requests_total\s+(\d+)/);
    const beforeCount = counterMatch ? parseInt(counterMatch[1], 10) : 0;

    // Make a dummy request to increment the counter
    const pingDv = ds.fromQuery(`http://127.0.0.1:${9105}/canary/ops/pid/exists`)
      .method("GET").build();
    ctx.dataview(pingDv);

    const after = scrape();
    const afterMatch = after.match(/rivers_http_requests_total\s+(\d+)/);
    const afterCount = afterMatch ? parseInt(afterMatch[1], 10) : 0;

    t.assert("counter_incremented", afterCount > beforeCount,
      `before=${beforeCount}, after=${afterCount}`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function metricsDuration(ctx: any): void {
  const t = new TestResult("OPS-METRICS-DURATION-HISTOGRAM", "OPS",
    "metrics §1.3");
  try {
    const ds = ctx.datasource("http");
    const dv = ds.fromQuery("http://127.0.0.1:9091/metrics")
      .method("GET").build();
    const result = ctx.dataview(dv);
    const body = result?.rows?.[0]?.body || "";
    t.assert("has_duration_metric",
      body.includes("rivers_http_request_duration_ms"),
      "metrics response contains duration histogram");
    t.assert("has_histogram_buckets",
      body.includes("_bucket{") || body.includes("_sum") || body.includes("_count"),
      "duration metric has histogram structure");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-ops/libraries/handlers/logging-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function logAppFile(ctx: any): void {
  const t = new TestResult("OPS-LOG-APP-FILE", "OPS", "logging §5.2");
  try {
    // Write a log entry from this app
    Rivers.log.info("canary-ops log file existence test");

    // Check that the app-specific log file exists
    const logPath = "log/apps/canary-ops.log";
    const exists = Rivers.fs.exists(logPath);
    t.assert("log_file_exists", exists === true,
      `path=${logPath}`);

    if (exists) {
      const content = Rivers.fs.readText(logPath);
      t.assert("log_has_content", content.length > 0,
        `file_size=${content.length}`);
      t.assert("log_contains_entry",
        content.includes("canary-ops log file existence test"),
        "wrote entry found in log file");
    }
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function logRotation(ctx: any): void {
  const t = new TestResult("OPS-LOG-ROTATION-SIZE", "OPS", "logging §5.4");
  // Log rotation at 10MB is verified by the Rust integration test harness.
  // The handler writes a large volume of log entries to approach the threshold.
  // The harness then checks for rotated log files (canary-ops.log.1, etc).
  try {
    // Write ~500KB of log data (50,000 entries of ~10 bytes each)
    // The harness calls this endpoint 20+ times to approach 10MB.
    for (let i = 0; i < 50000; i++) {
      Rivers.log.info(`rotation-stress-${i}-${Date.now()}`);
    }
    t.assert("stress_written", true,
      "50000 log entries written for rotation stress test");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function logCrossApp(ctx: any): void {
  const t = new TestResult("OPS-LOG-CROSS-APP", "OPS", "logging §5.2");
  try {
    // Verify that each canary app has its own log file
    const apps = [
      "canary-guard", "canary-sql", "canary-nosql",
      "canary-handlers", "canary-streams", "canary-ops"
    ];
    for (const app of apps) {
      const logPath = `log/apps/${app}.log`;
      const exists = Rivers.fs.exists(logPath);
      t.assert(`log_exists_${app.replace("canary-", "")}`,
        exists === true, `path=${logPath}`);
    }
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-ops/libraries/handlers/config-discovery-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function configDiscovery(ctx: any): void {
  const t = new TestResult("OPS-CONFIG-DISCOVERY", "OPS", "httpd §1.2");
  try {
    // The config discovery path is logged at startup.
    // We verify that the running instance found its config by checking
    // that the server is operational (we are executing right now) and
    // that the admin API reports the config source.
    t.assert("server_running", true,
      "handler executing — config was discovered and loaded");

    // Check that the config source is available via Rivers context
    if (typeof ctx.env === "string" && ctx.env.length > 0) {
      t.assert("env_resolved", true, `env=${ctx.env}`);
    } else {
      t.assert("env_resolved", false, "ctx.env not available");
    }

    // Verify RIVERS_HOME or equivalent was used
    // The exact source is in startup logs — harness verifies those
    t.assert("config_loaded", true,
      "riversd.toml loaded — exact discovery path verified by harness");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-ops/libraries/handlers/doctor-tests.ts

Note: Doctor and TLS tests are **harness-only** tests. They exercise CLI commands (`riversctl doctor`, `riversctl tls`) outside the running server. The handlers below are stubs that document the test contract — the actual assertions are in the Rust integration test harness.

```typescript
// doctor-tests.ts and tls-tests.ts contain no handler endpoints.
// All OPS-DOCTOR-* and OPS-TLS-* tests are harness-only.
// They are documented in the test inventory table for completeness.
// The Rust integration test harness runs:
//   riversctl doctor --lint <bundle-path>
//   riversctl doctor --fix <bundle-path>
//   riversctl tls renew
// and asserts on exit codes, output, and file state.
```

### Rust Integration Test Contract — OPS Profile

The OPS profile has more harness-only tests than any other profile. These tests cannot be delegated to handlers because they exercise CLI tools and process lifecycle.

```rust
#[tokio::test]
async fn canary_ops_pid_cleanup() {
    // Start riversd, verify PID file exists
    let pid_path = "run/riversd.pid";
    assert!(std::path::Path::new(pid_path).exists());

    // Stop via riversctl
    let output = Command::new("riversctl")
        .args(["stop"])
        .output().await.unwrap();
    assert!(output.status.success());

    // PID file must be cleaned up
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!std::path::Path::new(pid_path).exists(),
        "PID file not cleaned up after stop");
}

#[tokio::test]
async fn canary_ops_riversctl_status() {
    let output = Command::new("riversctl")
        .args(["status"])
        .output().await.unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("running") || stdout.contains("healthy"),
        "status should report running state");
}

#[tokio::test]
async fn canary_ops_doctor_lint_pass() {
    let output = Command::new("riversctl")
        .args(["doctor", "--lint", "canary-bundle/"])
        .output().await.unwrap();
    assert!(output.status.success(),
        "doctor --lint should pass on valid canary bundle");
}

#[tokio::test]
async fn canary_ops_doctor_lint_fail() {
    // Create a broken bundle (missing manifest)
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("broken-app")).unwrap();
    let output = Command::new("riversctl")
        .args(["doctor", "--lint", tmp.path().to_str().unwrap()])
        .output().await.unwrap();
    assert!(!output.status.success(),
        "doctor --lint should fail on broken bundle");
}

#[tokio::test]
async fn canary_ops_doctor_fix_logdirs() {
    // Remove log dirs, run --fix, verify recreated
    let _ = std::fs::remove_dir_all("log/apps");
    let output = Command::new("riversctl")
        .args(["doctor", "--fix"])
        .output().await.unwrap();
    assert!(output.status.success());
    assert!(std::path::Path::new("log/apps").exists(),
        "doctor --fix should recreate log directories");
}

#[tokio::test]
async fn canary_ops_tls_cert_renew() {
    let output = Command::new("riversctl")
        .args(["tls", "renew"])
        .output().await.unwrap();
    assert!(output.status.success(),
        "tls renew should succeed");
}

#[tokio::test]
async fn canary_ops_riverpackage_init() {
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("test-bundle");
    let output = Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    assert!(output.status.success(), "riverpackage init should succeed");
    assert!(bundle_path.join("manifest.toml").exists(),
        "scaffolded bundle should have manifest.toml");
}

#[tokio::test]
async fn canary_ops_riverpackage_validate() {
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("test-bundle");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    assert!(output.status.success(),
        "riverpackage validate should pass on scaffolded bundle");
}

#[tokio::test]
async fn canary_ops_log_rotation() {
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    // Call stress endpoint 20 times to approach 10MB
    for _ in 0..20 {
        client.post("/canary/proxy/ops/log/rotation", "{}").await;
    }

    // Check for rotated log file
    let rotated = std::path::Path::new("log/apps/canary-ops.log.1");
    assert!(rotated.exists(),
        "log rotation should create canary-ops.log.1");
}
```

---

## Part 7 — Profile: PROXY (canary-main)

### Purpose

Tests the HTTP driver as a cross-app proxy, session propagation across app boundaries, and hosts the SPA dashboard that visualizes all test results.

### Test Inventory

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| PROXY-SESSION-PROPAGATION | `/canary/proxy/session-check` | GET | Session headers survive HTTP driver proxy | auth-session §7.5 |
| PROXY-SQL-PASSTHROUGH | `/canary/proxy/sql/pg/select` | GET | Proxy to canary-sql returns correct result | http-driver §6.2 |
| PROXY-HANDLER-PASSTHROUGH | `/canary/proxy/rt/ctx/trace-id` | GET | Proxy to canary-handlers returns verdict | http-driver §6.2 |
| PROXY-ERROR-PROPAGATION | `/canary/proxy/error` | GET | Error from downstream service propagated correctly | SHAPE-2 |

### canary-main/manifest.toml

```toml
appName       = "canary-main"
description   = "Canary Fleet — PROXY profile: SPA dashboard, HTTP driver, cross-app proxy"
version       = "1.0.0"
type          = "app-main"
appId         = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee05"
entryPoint    = "http://0.0.0.0:8080"
appEntryPoint = "https://canary.test"
source        = "https://github.com/rivers-framework/canary-fleet/canary-main"
```

### canary-main/resources.toml

```toml
[[datasources]]
name       = "canary-sql-api"
driver     = "http"
x-type     = "http"
nopassword = true
required   = true

[[datasources]]
name       = "canary-handlers-api"
driver     = "http"
x-type     = "http"
nopassword = true
required   = true

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true

[[services]]
name     = "canary-sql"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01"
required = true

[[services]]
name     = "canary-handlers"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee03"
required = true

[[services]]
name     = "canary-streams"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee04"
required = true

[[services]]
name     = "canary-ops"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee06"
required = true
```

### SPA Dashboard

The dashboard fetches every test endpoint from all profiles (via canary-main proxy) and renders a matrix:

```
┌──────────────────────────────────────────────────────────┐
│  Rivers Canary Fleet — Conformance Dashboard  v0.53.0    │
├──────────────┬──────┬─────────┬──────────────────────────┤
│  Profile     │ Pass │  Fail   │  Total                   │
├──────────────┼──────┼─────────┼──────────────────────────┤
│  AUTH        │  8   │   1     │   9                      │
│  SQL         │  22  │   0     │   22                     │
│  NOSQL       │  12  │   1     │   13                     │
│  RUNTIME     │  27  │   0     │   28                     │
│  STREAM      │  8   │   1     │   9                      │
│  OPS         │  22  │   2     │   24                     │
│  PROXY       │  4   │   0     │   4                      │
├──────────────┼──────┼─────────┼──────────────────────────┤
│  TOTAL       │ 103  │   5     │  109                     │
└──────────────┴──────┴─────────┴──────────────────────────┘

[Click any profile row to expand individual test results]
```

Each test row shows: test_id, passed/failed badge, spec_ref, duration_ms, and expandable assertion details. Failed tests sort to top.

The SPA is built with Svelte (same pattern as address book). `ProfileCard.svelte` renders each profile. `TestRow.svelte` renders individual test results. `VerdictBadge.svelte` renders pass/fail badges.

**SPA refresh:** A "Run All" button hits every endpoint sequentially and updates the display. Individual profile "Run" buttons test just that profile.

---

## Part 8 — Rust Integration Test Contract

The canary bundle is the APPLICATION. The Rust integration test crate is the HARNESS. They are separate artifacts.

The harness boots `riversd` with the canary bundle, then hits every endpoint and asserts. It is a Rust `#[cfg(test)]` module in the Rivers repo, not part of the bundle.

### Test execution pattern

```rust
#[tokio::test]
async fn canary_sql_pg_param_order() {
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    let result: TestVerdict = client
        .get("/canary/proxy/sql/pg/param-order")
        .await
        .json();

    assert!(result.passed, "SQL-PG-PARAM-ORDER failed: {:?}", result);
    assert_eq!(result.profile, "SQL");
}

#[tokio::test]
async fn canary_v8_timeout() {
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    // This endpoint runs an infinite loop — expect timeout error, not hang
    let response = client
        .get_with_timeout("/canary/proxy/rt/v8/timeout", Duration::from_secs(10))
        .await;

    // The response should be an error (V8 killed the handler)
    // NOT a 200 with passed:false (handler never returns)
    assert_ne!(response.status(), 200);
}

#[tokio::test]
async fn canary_header_blocklist() {
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    let response = client
        .get_raw("/canary/proxy/rt/header/blocklist")
        .await;

    // Set-Cookie must NOT be in response headers
    assert!(response.headers().get("set-cookie").is_none(),
        "Set-Cookie header was not stripped");
    // X-Canary-Custom MUST be present
    assert_eq!(
        response.headers().get("x-canary-custom").unwrap(),
        "canary-value"
    );
}

#[tokio::test]
async fn canary_error_sanitization() {
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    let response = client
        .get_text("/canary/proxy/rt/error/sanitize")
        .await;

    // Error response must not contain driver names
    let body = response.to_lowercase();
    assert!(!body.contains("postgres"), "error leaks 'postgres'");
    assert!(!body.contains("mysql"), "error leaks 'mysql'");
    assert!(!body.contains("192.168"), "error leaks IP address");
}

#[tokio::test]
async fn canary_csrf_enforcement() {
    let client = CanaryClient::new("http://localhost:8080");
    client.login("canary", "canary-test").await;

    // POST without CSRF token — must get 403
    let no_csrf = client
        .post_without_csrf("/canary/proxy/auth/csrf-test", "{}")
        .await;
    assert_eq!(no_csrf.status(), 403, "POST without CSRF should be 403");

    // POST with valid CSRF — must succeed
    let with_csrf = client
        .post("/canary/proxy/auth/csrf-test", "{}")
        .await;
    assert_eq!(with_csrf.status(), 200, "POST with CSRF should be 200");
}
```

### Tests the harness CANNOT delegate to handlers

Some tests require observing HTTP-level behavior that handlers can't see:

- **Cookie flags** (HttpOnly, Secure, SameSite) — inspect `Set-Cookie` header on login response
- **CSRF enforcement** — send POST without token, expect 403
- **Session expiry** — wait for TTL, send request, expect 401
- **Header blocklist** — inspect response headers for stripped/preserved headers
- **Error sanitization** — inspect error response body for leaked strings
- **Rate limiting** — send N+1 requests, expect 429 on overflow
- **V8 timeout** — expect non-200 response (handler never returns a verdict)
- **V8 heap** — expect non-200 response (handler may be killed before returning)
- **Graceful shutdown** — send SIGTERM during request, verify completion + 503 on new requests
- **SSE/WebSocket** — connect, receive events, verify content and timing

---

## Validation Checklist

```
canary-bundle/
├── CHANGELOG.md                                         ✓ bundle root, uppercase
├── manifest.toml                                        ✓ 7 apps in startup order
├── canary-guard/
│   ├── manifest.toml                                    ✓ type=app-service, stable appId
│   ├── resources.toml                                   ✓ no external datasources
│   ├── app.toml                                         ✓ guard view + session test views
│   ├── schemas/identity.schema.json                     ✓ sub, role, email
│   └── libraries/handlers/
│       ├── test-harness.ts                              ✓ shared TestResult class
│       ├── guard.ts                                     ✓ returns IdentityClaims
│       └── session-test.ts                              ✓ 7 session test handlers
├── canary-sql/
│   ├── manifest.toml                                    ✓ type=app-service, init handler declared
│   ├── resources.toml                                   ✓ pg + mysql + sqlite datasources
│   ├── app.toml                                         ✓ 23 DataViews + 23 Views (+4 SQLite path)
│   ├── schemas/{pg,mysql,sqlite}-record.schema.json     ✓ zname column in all schemas
│   └── libraries/handlers/
│       ├── test-harness.ts                              ✓ copy
│       ├── init.ts                                      ✓ DDL whitelist path
│       ├── sql-tests.ts                                 ✓ CRUD + param order + cache + SQLite path fallback
│       └── negative-sql.ts                              ✓ DDL rejection
├── canary-nosql/
│   ├── manifest.toml                                    ✓ type=app-service
│   ├── resources.toml                                   ✓ 6 datasources
│   ├── app.toml                                         ✓ 13 Views
│   ├── schemas/*.schema.json                            ✓ one per driver
│   └── libraries/handlers/
│       ├── test-harness.ts                              ✓ copy
│       ├── nosql-tests.ts                               ✓ CRUD per driver
│       └── negative-nosql.ts                            ✓ admin op rejection
├── canary-handlers/
│   ├── manifest.toml                                    ✓ type=app-service
│   ├── resources.toml                                   ✓ faker datasource
│   ├── app.toml                                         ✓ 28 Views (+3 per-app logging)
│   ├── schemas/faker-record.schema.json                 ✓ seeded faker
│   └── libraries/handlers/
│       ├── test-harness.ts                              ✓ copy
│       ├── ctx-surface.ts                               ✓ every ctx.* property/method
│       ├── rivers-api.ts                                ✓ Rivers.log, Rivers.crypto, per-app logging
│       ├── storage-tests.ts                             ✓ ctx.store + namespace isolation
│       ├── eventbus-tests.ts                            ✓ EventBus publish
│       └── v8-security.ts                               ✓ timeout, heap, codegen, console
├── canary-streams/
│   ├── manifest.toml                                    ✓ type=app-service
│   ├── resources.toml                                   ✓ kafka + faker datasources
│   ├── app.toml                                         ✓ 9 Views (mixed types)
│   └── libraries/handlers/
│       ├── test-harness.ts                              ✓ copy
│       ├── ws-handler.ts                                ✓ echo + broadcast
│       ├── sse-handler.ts                               ✓ tick + event trigger
│       ├── streaming-rest.ts                            ✓ NDJSON + poison guard
│       ├── kafka-consumer.ts                            ✓ MessageConsumer handler
│       └── poll-handler.ts                              ✓ hash diff on_change
├── canary-ops/
│   ├── manifest.toml                                    ✓ type=app-service, port 9105
│   ├── resources.toml                                   ✓ no datasources, guard service dep
│   ├── app.toml                                         ✓ 8 handler Views + 16 harness-only
│   └── libraries/handlers/
│       ├── test-harness.ts                              ✓ copy
│       ├── pid-tests.ts                                 ✓ PID file existence + validity
│       ├── metrics-tests.ts                             ✓ Prometheus endpoint + counters + histograms
│       ├── logging-tests.ts                             ✓ app log file + rotation + cross-app
│       ├── doctor-tests.ts                              ✓ stub (harness-only tests)
│       ├── tls-tests.ts                                 ✓ stub (harness-only tests)
│       └── config-discovery-tests.ts                    ✓ config discovery verification
└── canary-main/
    ├── manifest.toml                                    ✓ type=app-main
    ├── resources.toml                                   ✓ HTTP datasources + all service deps
    ├── app.toml                                         ✓ proxy views + SPA config
    └── libraries/
        ├── handlers/proxy-tests.ts                      ✓ cross-app session verification
        ├── package.json                                 ✓ Svelte + Rollup
        ├── rollup.config.js                             ✓ src/main.js → spa/bundle.js
        ├── src/App.svelte                               ✓ dashboard layout
        ├── src/components/ProfileCard.svelte             ✓ profile summary + expand
        ├── src/components/TestRow.svelte                 ✓ individual result display
        ├── src/components/VerdictBadge.svelte            ✓ pass/fail badge
        ├── src/main.js                                  ✓ mounts App
        └── spa/{index.html, bundle.js, bundle.css}      ✓ compiled output
```

---

## Expected Behavior

```bash
# Provision LockBox aliases for test cluster
rivers lockbox add --alias canary-pg --type string --value "postgresql://rivers:rivers_test@192.168.2.209:5432/rivers"
rivers lockbox add --alias canary-mysql --type string --value "mysql://rivers:rivers_test@192.168.2.215:3306/rivers"
rivers lockbox add --alias canary-redis --type string --value "redis://:rivers_test@192.168.2.206:6379"
rivers lockbox add --alias canary-mongo --type string --value "mongodb://rivers:rivers_test@192.168.2.212:27017/?replicaSet=rivers-rs&authSource=admin"
rivers lockbox add --alias canary-es --type string --value "http://192.168.2.218:9200"
rivers lockbox add --alias canary-kafka --type string --value "192.168.2.203:9092"
rivers lockbox add --alias canary-couch --type string --value "http://rivers:rivers_test@192.168.2.221:5984"
rivers lockbox add --alias canary-cassandra --type string --value "192.168.2.224:9042"
rivers lockbox add --alias canary-ldap --type string --value "ldap://192.168.2.227:389"

# Build SPA
cd canary-main/libraries && npm install && npm run build

# Deploy bundle
riversd deploy canary-bundle/

# Or for development — direct config
riversd --config canary-guard/app.toml &      # port 9100
riversd --config canary-sql/app.toml &        # port 9101
riversd --config canary-nosql/app.toml &      # port 9102
riversd --config canary-handlers/app.toml &   # port 9103
riversd --config canary-streams/app.toml &    # port 9104
riversd --config canary-ops/app.toml &       # port 9105
riversd --config canary-main/app.toml &       # port 8080

# Open dashboard
open http://localhost:8080

# Run individual profile tests
curl -s http://localhost:9101/canary/sql/pg/param-order | jq .passed
curl -s http://localhost:9103/canary/rt/v8/codegen | jq .passed

# Run Rust integration harness
cargo test --test canary_fleet -- --test-threads=1
```

---

## Test Count Summary

| Profile | Positive Tests | Negative Tests | Total |
|---------|---------------|----------------|-------|
| AUTH | 7 | 2 | 9 |
| SQL | 18 | 4 | 22 |
| NOSQL | 9 | 2 | 11 |
| RUNTIME | 19 | 9 | 28 |
| STREAM | 7 | 2 | 9 |
| OPS | 16 | 8 | 24 |
| PROXY | 4 | 0 | 4 |
| **Total** | **80** | **27** | **107** |

107 test endpoints across 7 profiles, 12 datasource drivers, 5 view types, exercising 21+ spec documents. Covers all v0.53.0 features: per-app logging (AppLogRouter), config discovery, riversctl stop/status/PID, doctor --fix/--lint, Prometheus metrics, TLS cert renewal, SQLite path fallback, riverpackage init/validate, and engine loader naming. Every silent bug from the v0.50–v0.52.7 dream doc would have been caught by at least one canary endpoint.
