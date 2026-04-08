# Bug: Init handler `ctx.ddl()` is not implemented — DDL silently no-ops

**Severity:** Blocker — any app relying on init handler DDL (the only permitted DDL path in 0.53.0) cannot create tables  
**Rivers version:** 0.53.0 (source at `dist/rivers-0.53.0/`)  
**Date filed:** 2026-04-07  
**Status:** Fixed (0.53.5)  
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

| Component | Status |
|-----------|--------|
| `Connection::execute()` — Gate 1 DDL guard | Working — correctly rejects DDL |
| `Connection::ddl_execute()` — SQLite impl | Working — code is correct, tests pass (`sqlite.rs:712-853`) |
| `[init]` manifest parsing | Working — init config parsed, handler dispatched |
| Init handler JS dispatch via ProcessPool | Working — JS executes, logs emitted |
| `[security].ddl_whitelist` parsing | Untested — `_ddl_whitelist` param is unused |
| `ctx.ddl()` JS → host callback → `ddl_execute()` | **NOT IMPLEMENTED** |
| `ctx.admin()` JS → host callback | **NOT IMPLEMENTED** |
| `ctx.query()` JS → host callback | **NOT IMPLEMENTED** |

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

Any application on 0.53.0 that needs schema initialization at startup is blocked. The old pattern (DDL via view handlers) is now forbidden by Gate 1, and the new pattern (DDL via init handler) is not yet functional. **There is no working DDL path in 0.53.0.**

## Workaround

None within Rivers. The only option is to create tables externally via `sqlite3` CLI before starting riversd, bypassing the init handler entirely.
