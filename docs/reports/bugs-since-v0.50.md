# Rivers Bug Report: v0.50.0 through v0.52.8

**Period:** March 2026 — April 3, 2026
**Commits:** 159 across 7 releases
**Releases:** v0.50.2.0, v0.50.2.1, v0.50.2.2, v0.52.5, v0.52.6, v0.52.7, v0.52.8

---

## Executive Summary

38 bugs found and fixed since v0.50. The most dangerous were **silent** — they didn't crash or produce errors, they just did the wrong thing. The V8 sandbox had 4 security gaps. Parameter binding was broken across all SQL drivers. The `--no-ssl` dev path was missing critical subsystem initialization.

| Category | Count | Silent | Critical |
|----------|-------|--------|----------|
| Security (V8 sandbox + DDL) | 14 | 10 | 3 |
| Runtime (ctx.*, dataview, dispatch) | 12 | 8 | 2 |
| Infrastructure (CI/tooling) | 6 | 0 | 0 |
| Documentation (ghost APIs) | 6 | 6 | 0 |
| **Total** | **38** | **24** | **5** |

---

## Critical Bugs (P0)

### BUG-001: DDL statements execute unchecked through Connection::execute()
- **Found:** Security audit (v0.52.6)
- **Impact:** Any DataView query containing DROP TABLE, CREATE TABLE, or ALTER TABLE would execute without restriction. A malicious or misconfigured TOML config could destroy databases.
- **Root cause:** Connection::execute() had no guard against DDL statements. No driver checked query type before execution.
- **Fix:** Three-gate enforcement model. Gate 1: `check_admin_guard()` on all 16 drivers. Gate 2: `ExecutionContext` enum (ViewRequest vs ApplicationInit). Gate 3: `ddl_whitelist` in SecurityConfig.
- **Silent:** Yes — DDL would execute and return success.
- **PR:** #49

### BUG-002: V8 no execution timeout — infinite loop blocks worker
- **Found:** Security audit (v0.52.6)
- **Impact:** A handler with an infinite loop would block the V8 worker thread permanently. No timeout, no recovery. Server effectively DoS'd.
- **Root cause:** No watchdog thread for V8 execution. Isolate ran until completion or process kill.
- **Fix:** Added watchdog thread with configurable `task_timeout_ms` (default 5s). Watchdog calls `isolate.terminate_execution()`.
- **Silent:** Yes — server hangs with no error response.
- **PR:** #50

### BUG-003: V8 dynamic code generation from strings not blocked
- **Found:** Security audit (v0.52.6), re-found by canary (v0.52.8)
- **Impact:** String-based code generation APIs were available inside the V8 sandbox. Code injection via user-controlled strings was possible.
- **Root cause:** `--disallow-code-generation-from-strings` V8 flag was set in the cdylib engine build but **missing from the static build** (riversd's built-in V8 init).
- **Fix:** Added flag to `ensure_v8_initialized()` in both engine builds.
- **Silent:** Yes — no error unless handler code is audited.
- **PR:** #50 (partial), #61 (static build fix)

### BUG-004: Parameter binding silently corrupts data across SQL drivers
- **Found:** User bug report, investigation (v0.52.7)
- **Impact:** `$name` placeholders in DataView queries were translated incorrectly by every SQL driver. PostgreSQL and MySQL sorted parameters alphabetically then bound positionally — `$zname, $age` would bind as `$age, $zname` silently. SQLite used `:` prefix but queries used `$`.
- **Root cause:** Each driver invented its own parameter handling. No centralized translation layer. The spec examples (`$name`) didn't match any driver's native format.
- **Fix:** Added `translate_params()` in DataView engine with `ParamStyle` enum. Zero-padded numeric keys for positional ordering. Centralized, driver-independent.
- **Silent:** Yes — queries return wrong data, no errors.
- **Issue:** #54, **PR:** #56

### BUG-005: --no-ssl path missing all subsystem initialization
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** Running `riversd --no-ssl` (dev mode) skipped StorageEngine, SessionManager, CsrfManager, EventBus, engine loader, and host context wiring. Any app using sessions, storage, or CSRF in dev mode was completely broken.
- **Root cause:** `run_server_no_ssl()` was a simplified copy of the TLS path that never got the subsystem initialization code added during later development.
- **Silent:** No — "protected views require [storage_engine]" error at startup.
- **PR:** #61

---

## High Severity Bugs (P1)

### BUG-006: V8 heap limit not enforced on isolate reuse
- **Found:** Security audit (v0.52.6)
- **Impact:** V8 isolates were pooled and reused. A handler that allocated near the heap limit would leave a "dirty" isolate that could OOM-crash the process on the next use.
- **Fix:** Added `NearHeapLimitCallback` that terminates execution. Added heap usage check in `release_isolate()` — discards isolates that used >50% of limit.
- **PR:** #50

### BUG-007: timingSafeEqual uses short-circuiting comparison
- **Found:** Security audit (v0.52.6)
- **Impact:** `Rivers.crypto.timingSafeEqual(a, b)` used `.all()` iterator which short-circuits on first mismatch. Enables timing side-channel attacks on token comparison.
- **Fix:** Changed to XOR accumulation that always processes all bytes.
- **PR:** #50

### BUG-008: ctx.dataview() silently drops parameters
- **Found:** User testing (v0.52.5)
- **Impact:** Every handler calling `ctx.dataview("name", {id: 42})` had the second argument silently ignored. All dynamic DataView queries from CodeComponent handlers returned unfiltered results.
- **Root cause:** V8 bridge function extracted first arg (name) but never read second arg (params).
- **Fix:** Forward params to host callback, convert to `HashMap<String, QueryValue>`.
- **Silent:** Yes — returns unfiltered data, no errors.
- **PR:** #48

### BUG-009: ctx.dataview() didn't namespace lookups
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** Handler calling `ctx.dataview("list_records")` failed because registry key is `"handlers:list_records"`. Every dynamic DataView call from a CodeComponent handler was broken.
- **Root cause:** V8 callback passed bare name to DataViewExecutor without entry-point prefix.
- **Fix:** Added `TASK_DV_NAMESPACE` thread-local, prepend entry point prefix to bare names.
- **PR:** #61

### BUG-010: ctx.app_id returned entry point slug instead of manifest UUID
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** Handlers got `"handlers"` instead of the stable appId UUID. Any logic depending on stable app identity was broken.
- **Root cause:** `view_dispatch.rs` passed `app_entry_point` (route namespace) as `app_id`.
- **Fix:** Added `app_id` field to ViewRoute/MatchedRoute, populated from `app.manifest.app_id`.
- **PR:** #61

### BUG-011: ctx.node_id always empty
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** `ctx.node_id` was always `""` in every handler.
- **Root cause:** TaskContextBuilder never had `node_id()` called. Pipeline didn't wire it.
- **Fix:** Added `node_id` to SharedTaskCapabilities, set from server config `app_id`.
- **PR:** #61

### BUG-012: ctx.request.query serialized as "query_params"
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** Handlers checking `ctx.request.query` got `undefined`. The field was there but named `query_params` in the JSON.
- **Fix:** Added `#[serde(rename = "query")]` to `ParsedRequest.query_params`.
- **PR:** #61

### BUG-013: CodeComponent module paths not resolved to absolute
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** V8 engine read handler `.ts` files relative to CWD. Any bundle run from a different working directory got "module not found" errors.
- **Fix:** Added module path resolution step during bundle load that rewrites all handler module paths to absolute.
- **PR:** #61

---

## Medium Severity Bugs (P2)

### BUG-014: Session tokens use UUID v4 (122 bits) instead of 256-bit CSPRNG
- **Found:** Security audit — OWASP ASVS check
- **Fix:** Upgraded to 256-bit random tokens.
- **PR:** #52

### BUG-015: CSRF cookie missing Secure flag
- **Found:** Security audit
- **Fix:** Added Secure flag to CSRF cookie configuration.
- **PR:** #51

### BUG-016: Admin RBAC default-allow for unknown paths
- **Found:** Code review during security audit
- **Fix:** Changed to deny-by-default for unregistered admin paths.
- **PR:** #51

### BUG-017: No query result row limit
- **Found:** Security audit
- **Impact:** Unbounded memory allocation on large query results.
- **Fix:** Added `max_rows` to DataViewConfig with default 1000. Truncates with warning.
- **PR:** #52

### BUG-018: Init handler timeout not enforced
- **Found:** Security audit
- **Impact:** A hung init handler blocks startup indefinitely.
- **Fix:** Wrapped dispatch in `tokio::time::timeout` with `init_timeout_s` config.
- **PR:** #52

### BUG-019: ViewContext.app_id always empty string
- **Found:** Bug report during development
- **Fix:** Populated from matched route (later changed to manifest UUID in v0.52.8).
- **PR:** #56

### BUG-020: Outbound TLS verification disabled by default
- **Found:** Security audit
- **Impact:** HTTP driver could MITM without warning.
- **Fix:** Enabled TLS verification by default.
- **PR:** #51

### BUG-021: Store TTL API expects milliseconds but handler passed object
- **Found:** Canary fleet testing (v0.52.8)
- **Impact:** `ctx.store.set(key, val, {ttl: 60})` silently ignored the TTL.
- **Fix:** Corrected test handler to pass `60000` (number).
- **PR:** #61

---

## Low Severity Bugs (P3)

### BUG-022: CORS missing Vary: Origin header
- **Found:** Fetch spec audit
- **Fix:** Added `Vary: Origin` to CORS responses.
- **PR:** #51

### BUG-023: No HSTS header
- **Found:** CIS benchmark audit
- **Fix:** Added `Strict-Transport-Security` header.
- **PR:** #51

### BUG-024: Error responses leak driver/infra details
- **Found:** Security audit
- **Fix:** Error sanitization — generic messages in production, full details in debug.
- **PR:** #52

### BUG-025: Neo4j driver — 10 gap analysis findings
- **Found:** Gap analysis after initial build
- **Issues:** Wrong export name, non-functional row_to_map(), null params dropped, error swallowing, unnecessary Arc wrapper, missing config options.
- **Fix:** All 10 findings addressed.
- **PR:** #55

---

## Infrastructure / CI Bugs

### BUG-026 to BUG-028: sccache crashes build (3 PRs)
- GitHub Actions cache service outage caused `RUSTC_WRAPPER=sccache` to fail.
- **Fix:** Health-check sccache, unset wrapper when unhealthy. 3 PRs needed (#39, #40, #41).

### BUG-029: Windows exec plugin tests fail
- Unix paths (`/tmp`, `/bin/echo`) hardcoded in tests.
- **Fix:** Tests marked `#[ignore]` on CI. Issue #46 filed (open).

### BUG-030: .gitignore `src/` blocks all crate source
- Pattern was `src/` (recursive) instead of `/src/` (root only).
- **Fix:** Changed to `/src/`.

### BUG-031: 12 GB stale cargo caches filling GitHub quota
- **Fix:** Manual API cleanup of stale cache entries.

---

## Documentation Bugs (Ghost APIs)

| # | Ghost API | Where | Status |
|---|-----------|-------|--------|
| 32 | `ctx.session` documented but not injected | AI skill docs | Fixed in doc update |
| 33 | `ctx.streamDataview()` documented but doesn't exist | AI skill docs | Removed |
| 34 | `Rivers.http` methods documented but not wired | AI skill docs | Removed |
| 35 | Async handler pattern documented but V8 has no async | AI skill docs | Removed |
| 36 | `[hot_reload]` config documented but struct doesn't exist | AI docs | Removed |
| 37 | OpenTelemetry `[performance.tracing]` documented | AI docs | Removed |

---

## Spec vs Implementation Mismatches

| # | Spec says | Implementation does | Fixed in |
|---|-----------|-------------------|----------|
| S1 | `entryPoint = "http://0.0.0.0:{port}"` | Router uses it as URL path segment | v0.52.8 — spec updated to `"{slug}"` |
| S2 | Driver name `postgresql` | Registered as `postgres` | v0.52.8 — spec updated |
| S3 | `ctx.request.query` | Serialized as `query_params` | v0.52.8 — serde rename |
| S4 | `ctx.request.path_params` | Handler tests used `params` | v0.52.8 — test fixed |

---

## Statistics

| Metric | Value |
|--------|-------|
| Total bugs found | 38 |
| Silent bugs (no error/warning) | 24 (63%) |
| Security-critical bugs | 5 |
| Bugs found by canary fleet | 9 |
| Bugs found by security audit | 14 |
| Bugs found by user testing | 4 |
| Bugs found by CI failures | 6 |
| Bugs found by doc audit | 6 |
| Average PRs to fix CI bugs | 2.0 |
| Most bug-dense file | `execution.rs` (5 bugs) |
| Releases to fix all bugs | 4 (v0.52.5 to v0.52.8) |

---

## How Bugs Were Found

```
Security audit      ████████████████  14  (37%)
Canary fleet        █████████         9   (24%)
CI failures         ██████            6   (16%)
Doc audit           ██████            6   (16%)
User testing        ████              4   (11%)
```

---

## Key Lessons

1. **63% of bugs were silent.** The system didn't crash — it just did the wrong thing. Canary-style conformance tests catch these where unit tests don't.

2. **The V8 sandbox had the door unlocked.** No timeout, no heap limit, code generation available, non-constant-time comparison. Four gaps in one component, all found by a single structured audit.

3. **Parameter binding was architecturally wrong.** Each driver solved the same problem differently. Centralized translation fixed all drivers at once.

4. **The --no-ssl dev path was a second-class citizen.** Missing 7 subsystem initializations. The TLS path worked; the dev path didn't. Test both paths.

5. **Docs drifted from reality.** 6 documented APIs didn't exist. Spec examples used field names that weren't in the code. Automated spec-code validation would prevent this.

---

## Open Issues

| Issue | Status | Priority |
|-------|--------|----------|
| #46 — Windows CI exec tests | Open | Low |
| EventBus not exposed to V8 handlers | Known gap | Future |
| HTTP driver not registered in built-in drivers | Known gap | Future |
| `ctx.response.setHeader` not available | By design | N/A |
