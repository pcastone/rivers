# Bug Report — 2026-04-09

## Summary

cdylib driver plugins (MongoDB, Elasticsearch, CouchDB, Cassandra, LDAP) crash the entire riversd process with SIGABRT when their async `connect()` is called through the host's DataViewExecutor. The panic message is "there is no reactor running, must be called from the context of a Tokio 1.x runtime" and Rust reports "cannot catch foreign exceptions, aborting."

## Symptoms

- Server starts normally, all 12 plugins load successfully
- Guard login works, Redis (built-in driver) works
- First request to any cdylib-backed NoSQL datasource (e.g., `ctx.dataview("mongo_ping")`) kills the process
- Exit code 134 (SIGABRT)
- Server log shows:
  ```
  thread '<unnamed>' panicked at mongodb-3.5.2/src/client.rs:176:9:
  there is no reactor running, must be called from the context of a Tokio 1.x runtime
  
  fatal runtime error: Rust cannot catch foreign exceptions, aborting
  ```
- `host_dataview_execute: channel recv failed` — the spawned task died before sending a result

## Environment

Rivers v0.53.5, macOS aarch64, dynamic build (`cargo deploy`). All 12 cdylib plugins loaded from `dist/rivers-0.53.6/plugins/`. Test cluster at 192.168.2.x (all containers running). Lockbox credentials resolved for all datasources.

## Root Cause

**Three layers of failure:**

1. **Tokio ABI mismatch:** Each cdylib plugin is compiled with its own statically-linked tokio dependency. The host process (riversd) has its own tokio runtime. When the host spawns an async task via `rt_handle.spawn()` and that task calls `factory.connect("mongodb", &params)`, the MongoDB driver's `connect()` internally calls `tokio::runtime::Handle::current()` — but it looks up the handle in ITS OWN tokio's thread-local storage, not the host's. That thread-local is empty because no runtime was created for the cdylib's tokio instance.

2. **Panic crosses FFI boundary:** The `mongodb` crate panics with "no reactor running." Normally `catch_unwind` would catch this, but the panic originates in code compiled into the cdylib (a separate compilation unit with its own panic infrastructure). When the panic tries to propagate back across the C-ABI boundary, Rust detects it as a "foreign exception" and calls `abort()` instead of unwinding.

3. **Wrong fix target:** The initial fix (spawn_blocking + Runtime::new + catch_unwind in `host_datasource_build`) targeted the wrong callback. The NoSQL canary handlers use `ctx.dataview("mongo_ping")` which goes through `host_dataview_execute` → `DataViewExecutor::execute()` → `DriverFactory::connect()`. The connect call happens deep inside the DataViewExecutor's async pipeline, not in `host_datasource_build`.

**The fix cannot be at the host callback level.** It must be at the **DriverFactory.connect()** level — the single point where all driver connections are created, regardless of which host callback initiated the request.

## Attempted Fixes (did not resolve)

### Attempt 1: catch_unwind in host_datasource_build
Wrapped `factory.connect()` + `conn.execute()` in `spawn_blocking` + `Runtime::new()` + `catch_unwind`. This correctly isolates cdylib calls for the datasource_build callback, but the crash happens through `host_dataview_execute` → `DataViewExecutor` → `DriverFactory::connect()`, which is a different code path.

### Attempt 2: catch_unwind in host_ddl_execute  
Same pattern applied to DDL. Same issue — DDL already uses SQLite (built-in), the crash is from NoSQL drivers called through DataView execution.

## Fix Required

The fix must wrap `DriverFactory::connect()` itself — the function in `crates/rivers-core/src/driver_factory.rs` that dispatches to the appropriate driver's `connect()` method.

**Option A: Isolated runtime per driver connect**
In `DriverFactory::connect()`, detect if the driver is a cdylib plugin (vs built-in), and if so, spawn a dedicated `tokio::runtime::Runtime` for the connection. This gives the cdylib's tokio the reactor it expects.

**Option B: Process isolation**
Run cdylib driver calls in a child process. Communicates via IPC. Completely prevents any crash from affecting the host. Heavy-weight but bulletproof.

**Option C: Shared tokio via dylib**
Compile tokio as a shared library that both the host and plugins link against. Eliminates the ABI mismatch entirely. Requires significant build system changes.

**Recommended: Option A** — it's the same pattern we used in the host callbacks, just applied at the right level. The DriverFactory already knows which drivers are from plugins (it tracks this during registration). The isolated runtime adds ~1ms overhead per connect but prevents the fatal crash.

## Related Bugs

- `bugreport_2026-04-07_3.md` — Static/dynamic plugin conflict (same tokio reactor root cause, different symptoms). Fixed with `rt_handle.spawn()` in host callbacks. That fix works for the host callback thread context but doesn't help when the DataViewExecutor calls the driver from within a spawned async task.

### Update — 2026-04-09 (attempt 3)

**catch_unwind at DriverFactory::connect() also fails.** The `Runtime::new()` + `spawn_blocking` + `catch_unwind` wrapper in `DriverFactory::connect()` creates a new runtime, but the MongoDB driver spawns internal background threads that don't inherit this runtime's context. The panic originates from those internal threads and results in `abort()` before any catch_unwind can intercept it.

**Confirmed: this is not fixable at the Rust code level.** The fundamental issue is that each cdylib plugin has its own statically-linked tokio. `otool -L` confirms the MongoDB plugin has zero external dependencies on shared tokio or rivers-runtime — it's fully self-contained.

**The real fix is at the build system level:** plugins must link against the shared `librivers_runtime.dylib` (which includes tokio) instead of statically linking their own copy. The `build-dynamic` mode in the Justfile already compiles `rivers-runtime` as a dylib — the plugin build flags just need to be updated to link against it dynamically instead of statically including tokio.

## Occurrence Log

| Date | Context | Notes |
|------|---------|-------|
| 2026-04-09 | Canary NoSQL profile with cdylib plugins loaded | Exit 134 (SIGABRT) on first MongoDB/ES/CouchDB request |
| 2026-04-09 | Attempt: catch_unwind in host_datasource_build | Wrong callback — crash is through host_dataview_execute |
| 2026-04-09 | Attempt: catch_unwind in DriverFactory::connect | Runtime::new doesn't help — mongodb spawns internal threads that miss the reactor |
| 2026-04-09 | Root cause confirmed | Plugins statically link own tokio. Fix requires shared dylib linkage at build time |
