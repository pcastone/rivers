# Gutter — Deferred Work

## Previous canary-fleet gap remediation tasks (completed v0.52.8)
- Moved from tasks.md on 2026-04-03. All P0/P1/P2 items completed.
- Remaining: P3.2 (optional import refactor), P3.3 (validate command)

---

## S1.3 — V8 InitContext JS API (deferred 2026-04-02)

**Context:** DDL three-gate enforcement is in place (Gate 1-3 all working), but the JS handler API for init handlers (`ctx.ddl()`, `ctx.admin()`, `ctx.query()`) is not yet wired in the V8 engine.

**Why deferred:** Requires either:
1. Adding new slots to `HostCallbacks` struct in `rivers-engine-sdk` — this is a C-ABI struct, changing it requires bumping `ENGINE_ABI_VERSION` and rebuilding all cdylib engines
2. Or: routing through the existing `dataview_execute` callback with a `"ddl": true` flag — needs design for how the host side distinguishes DDL from DML dispatch

**What works now:**
- DDL is blocked on all `Connection::execute()` calls (Gate 1)
- `ddl_execute()` exists on the trait and works for Rust-level callers
- `DataViewEngine::execute_ddl()` checks the whitelist (Gate 3)
- Init handler dispatches with full capabilities (storage, dataview, lockbox, keystore)

**What doesn't work:**
- JS init handlers cannot call `ctx.ddl("db", "CREATE TABLE ...")` — the V8 callback doesn't exist yet
- JS init handlers CAN call `ctx.dataview("name")` for DML queries (that works)

**Suggested approach:** Add a `"mode": "ddl"` field to the `dataview_execute` callback input JSON. The host side checks this flag and routes to `execute_ddl()` instead of `execute()`. No ABI version bump needed.

**Files:**
- `crates/rivers-engine-v8/src/execution.rs` — add `ctx.ddl()` callback
- `crates/riversd/src/engine_loader/host_callbacks.rs` — check mode flag, route to execute_ddl
- `crates/rivers-engine-sdk/src/lib.rs` — no changes needed if using existing callback

---

## Bundle validation warnings (deferred)

- Warn if app has `init` declared but no matching `ddl_whitelist` entry
- Low priority — init handlers work without DDL, the warning is just advisory
