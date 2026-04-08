# Bug: Init handler `ctx.ddl()` is not implemented — DDL silently no-ops

**Severity:** Blocker — any app relying on init handler DDL (the only permitted DDL path in 0.53.0) cannot create tables  
**Rivers version:** 0.53.0 (source at `dist/rivers-0.53.0/`)  
**Date filed:** 2026-04-07  
**Date fixed:** 2026-04-07  
**Fixed in:** 0.53.5  
**Status:** Fixed  
**Reporter:** IPAM team  

---

## Summary

The DDL security spec (Gate 1/2/3) correctly blocks DDL from view handlers via `execute()`. The init handler dispatches correctly and runs JavaScript. But `ctx.ddl()` in the init handler JS context is **not wired to any host callback** — calls resolve silently without executing any SQL. Tables are never created, and the init handler reports success.

---

## Root Cause — 3 missing pieces

### 1. `dispatch_init_handler()` doesn't use the DataViewExecutor or whitelist

**File:** `crates/riversd/src/bundle_loader/load.rs`, lines 542-548

```rust
async fn dispatch_init_handler(
    pool: &ProcessPoolManager,
    app: &rivers_runtime::LoadedApp,
    init_config: &InitHandlerConfig,
    _executor: &Arc<DataViewExecutor>,    // ← UNUSED (underscore prefix)
    _ddl_whitelist: &[String],            // ← UNUSED (underscore prefix)
    timeout_s: u64,
) -> Result<(), String> {
```

The executor and whitelist are passed in from the caller (line 384) but never used. The init handler's TaskContext has no database dispatch capability.

### 2. No `ctx.ddl()` host callback in V8 engine

**File:** `crates/rivers-engine-v8/` — no `host_ddl_execute` or equivalent exists

The DDL security spec (§8.1) defines the JS API:

```typescript
interface InitContext {
    ddl(datasource: string, statement: string): Promise<QueryResult>;
    admin(datasource: string, operation: string, params?: Record<string, any>): Promise<QueryResult>;
    query(datasource: string, statement: string, params?: Record<string, any>): Promise<QueryResult>;
}
```

None of these are implemented as V8 host callbacks. When JS calls `await ctx.ddl(...)`, it resolves to `undefined` without throwing — so the init handler completes "successfully."

### 3. No path from host callback → `Connection::ddl_execute()`

Even if a host callback existed, there's no code path that calls `ddl_execute()` on the SQLite `Connection`. The existing `host_dataview_execute` callback only calls `execute()`, which has the DDL guard (Gate 1) that rejects DDL statements.

---

## What works vs what doesn't

| Component | Status (0.53.0) | Status (0.53.5) |
|-----------|-----------------|-----------------|
| `Connection::execute()` — Gate 1 DDL guard | Working | Working |
| `Connection::ddl_execute()` — SQLite impl | Working | Working |
| `[init]` manifest parsing | Working | Working |
| Init handler JS dispatch via ProcessPool | Working | Working |
| `[security].ddl_whitelist` parsing | Untested | **Working** |
| `ctx.ddl()` JS → host callback → `ddl_execute()` | **NOT IMPLEMENTED** | **Working** |
| `ctx.admin()` JS → host callback | **NOT IMPLEMENTED** | Not yet implemented |
| `ctx.query()` JS → host callback | **NOT IMPLEMENTED** | Not yet implemented |

---

## How to reproduce

**manifest.toml:**
```toml
[init]
module     = "handlers/bootstrap.js"
entrypoint = "initialize"
```

**riversd.toml:**
```toml
[security]
ddl_whitelist = [
    "mydb@a1b2c3d4-1111-2222-3333-444455556666",
]
```

**handlers/bootstrap.js:**
```javascript
async function initialize(ctx) {
    await ctx.ddl("my_db", "CREATE TABLE test (id INTEGER PRIMARY KEY)");
    Rivers.log.info("init done");  // This logs — init "succeeds"
}
```

**Expected:** Table `test` created in SQLite database on disk.

**Actual:** Init handler logs success. Table does not exist. All subsequent view handlers fail with `sqlite prepare: no such table: test`.

**Evidence from logs:**
```
INFO  init handler started   app_id=a1b2c3d4... module=handlers/bootstrap.js entrypoint=initialize
INFO  schema_bootstrap_start
INFO  init handler completed app_id=a1b2c3d4... duration_ms=54
```
No errors, no DDL execution logs, no `DdlExecuted` events. DB file on disk is 4096 bytes (empty SQLite header, no tables).

---

## What needs to be built

1. **V8 host callback for `ctx.ddl()`** — register `host_ddl_execute` (or similar) that:
   - Receives datasource name + SQL statement from JS
   - Looks up the datasource connection via DriverFactory/DataViewExecutor
   - Checks `ddl_whitelist` for `"{database}@{appId}"` (Gate 3)
   - Calls `Connection::ddl_execute()` on the resolved connection
   - Returns result to JS

2. **Wire `_executor` and `_ddl_whitelist`** in `dispatch_init_handler()` — pass them through `task_enrichment::enrich()` so the ProcessPool task has access to database connections and whitelist

3. **Same for `ctx.admin()` and `ctx.query()`** — the init handler needs its own set of host callbacks distinct from view handler callbacks (no `ctx.request`, `ctx.session`, etc.)

---

## Also found: parameter type enforcement change (separate issue)

Rivers 0.53.0 now rejects `null` for typed DataView parameters:

```
ParameterTypeMismatch { name: "cursor", expected: "string", actual: "null" }
```

Previously `null` was accepted for optional string parameters. We've fixed this on our side by omitting the parameter entirely instead of passing `null`, but this is a breaking change that should be documented in the 0.53.0 release notes.

---

## Impact

Any application on 0.53.0–0.53.4 that needs schema initialization at startup is blocked. The old pattern (DDL via view handlers) is now forbidden by Gate 1, and the new pattern (DDL via init handler) was not yet functional. **There was no working DDL path in 0.53.0–0.53.4.**

## Workaround

Upgrade to 0.53.5. No workaround needed — `ctx.ddl()` is fully functional.

Previous workaround (0.53.0–0.53.4): create tables externally via `sqlite3` CLI before starting riversd, bypassing the init handler entirely.

---

## Resolution (0.53.5)

Fixed across versions 0.53.3–0.53.5. The full `ctx.ddl()` path required fixes in three layers:

### Version timeline

| Version | What was done |
|---------|---------------|
| 0.53.0 | Gate 1 blocks DDL on `execute()`. `ctx.ddl()` not implemented (no-op) |
| 0.53.3 | `host_ddl_execute` Rust callback implemented, but not in HostCallbacks ABI struct |
| 0.53.4 | HostCallbacks ABI wired, `ctx.ddl()` reaches Rust, but HOST_CONTEXT not set at init time |
| 0.53.5 | **Full fix** — 3 bugs resolved (see below) |

### Fix 1: HOST_CONTEXT initialization order

`set_host_context()` was called in `lifecycle.rs` **after** `load_and_wire_bundle()`, but init handlers run **inside** `load_and_wire_bundle()` (Phase 1.5). Moved `set_host_context()`, `set_ddl_whitelist()`, and `set_app_id_map()` into `load_and_wire_bundle()` right after DataViewExecutor and DriverFactory are created — before the init handler dispatch loop. The `OnceLock` ensures subsequent calls in `lifecycle.rs` are harmless no-ops.

**File:** `crates/riversd/src/bundle_loader/load.rs`

### Fix 2: DDL whitelist checked datasource name instead of database path

The whitelist format is `{database}@{appId}`, but `host_ddl_execute` was passing the JS-level datasource name (e.g. `"test_db"`) to `is_ddl_permitted()`. The resolved `ConnectionParams.database` (e.g. `"ddl-test-service/data/test.db"`) is the correct value. Moved the Gate 3 whitelist check into the async block **after** resolving ConnectionParams from the DataViewExecutor.

**File:** `crates/riversd/src/engine_loader/host_callbacks.rs`

### Fix 3: Whitelist used entry_point name instead of manifest UUID

The ProcessPool sets `TASK_APP_ID` to the entry_point name (e.g. `"service"`), but the whitelist expects the manifest `appId` UUID (e.g. `"deadbeef-0000-0000-0000-000000000001"`). Added an `APP_ID_MAP` OnceLock that maps entry_point names to manifest UUIDs, populated during bundle loading. `host_ddl_execute` resolves the entry_point to UUID before the whitelist check.

**Files:** `crates/riversd/src/engine_loader/host_context.rs`, `crates/riversd/src/engine_loader/mod.rs`, `crates/riversd/src/bundle_loader/load.rs`

### Files changed

| File | Change |
|------|--------|
| `crates/riversd/src/bundle_loader/load.rs` | Wire HOST_CONTEXT, DDL_WHITELIST, APP_ID_MAP before Phase 1.5 |
| `crates/riversd/src/engine_loader/host_callbacks.rs` | Resolve database path + UUID before whitelist check |
| `crates/riversd/src/engine_loader/host_context.rs` | Add APP_ID_MAP OnceLock + setter |
| `crates/riversd/src/engine_loader/mod.rs` | Export `set_app_id_map` |
| `crates/riversd/src/server/lifecycle.rs` | Simplify (no-op after bundle load wiring) |

### Verified with

IPAM team's `ddl-test-bundle` (`dist/ddl-test-bundle.tar.gz`):
- Init handler runs `ctx.ddl("test_db", "CREATE TABLE IF NOT EXISTS items ...")`
- Gate 3 whitelist passes (`ddl-test-service/data/test.db@deadbeef-...`)
- SQLite table created on disk
- POST `/api/items` inserts rows via `ctx.dataview("insert_item", ...)`
- GET `/api/items` returns rows via `ctx.dataview("list_items", ...)`

### Remaining work

- `ctx.admin()` — not yet implemented
- `ctx.query()` — not yet implemented
