# Bug Report — 2026-04-07

## Summary
V8 timeout test (`RT-V8-TIMEOUT`) crashes the entire riversd process, killing all subsequent canary tests.

## Symptoms
- `GET /canary-fleet/handlers/canary/rt/v8/timeout` returns HTTP 500
- Immediately after, the riversd process exits (no graceful error recovery)
- All subsequent test requests get "Connection refused"
- The canary test runner reports TIMEOUT for every endpoint after v8-timeout

## Environment
Rivers v0.53.0, macOS aarch64, dynamic build with V8 engine dylib (`librivers_engine_v8.dylib`).

## Handler
`canary-handlers/libraries/handlers/v8-security.ts` lines 40-61:
```javascript
// Deliberately runs while(true){} to test the watchdog mechanism
// Expects V8 termination to throw a catchable error
```

## Root Cause
Race condition between multiple termination paths. When the V8 timeout fires, up to three independent mechanisms can call `isolate.terminate_execution()` simultaneously:

1. **Inline watchdog thread** in `crates/rivers-engine-v8/src/execution.rs:56-61` — spawns a thread that sleeps then calls `terminate_execution()`
2. **Pool-level watchdog** in `crates/riversd/src/process_pool/mod.rs` — monitors active tasks and calls `terminate_execution()` via `TaskTerminator::V8(handle)`
3. **Heap limit callback** in `crates/riversd/src/process_pool/v8_engine/init.rs:97` — called from V8's GC thread, also calls `terminate_execution()`

While `terminate_execution()` is thread-safe by design, calling it simultaneously from multiple threads during certain V8 internal states (GC, scope cleanup, internal allocation) can cause V8 to abort the process rather than throw a catchable exception.

Additionally, the TryCatch scope in execution.rs checks `has_terminated()` after compilation and top-level execution, but if termination occurs during **global scope injection** (ctx object, Rivers globals) before the handler function is called, those injection steps don't have TryCatch protection.

## Fix Applied

### V8 Timeout (engine dylib — `crates/rivers-engine-v8/src/execution.rs`)
- Replaced fire-and-forget watchdog thread with **cancellable watchdog** using `AtomicBool` flag
- Watchdog checks cancel flag every 50ms instead of sleeping the full timeout
- After handler completes, flag is set and watchdog thread is joined before touching the isolate
- `has_terminated()` checked after `func.call()` — terminated isolates are dropped, never recycled

### V8 Timeout (ProcessPool — `crates/riversd/src/process_pool/v8_engine/execution.rs`)
- Deregister from pool watchdog **BEFORE** touching the isolate (prevents race with watchdog thread)
- On timeout/error: explicitly `drop(isolate)` instead of recycling

### V8 Heap OOM (engine dylib — `crates/rivers-engine-v8/src/v8_runtime.rs`)
- `near_heap_limit_callback` no longer calls `terminate_execution()` directly from V8's GC thread
- Instead: sets `HEAP_OOM_TRIGGERED` flag, spawns thread that terminates after 1ms delay
- Grants 64MB headroom (was 0MB — returning `current_heap_limit` with no growth)
- Flag reset after each handler execution

### V8 Heap OOM (ProcessPool — `crates/riversd/src/process_pool/v8_engine/init.rs`)
- `HeapCallbackData` struct with `oom_triggered` flag + `IsolateHandle`
- Same deferred-thread approach as engine dylib
- Execution path checks `oom_hit` flag — OOM-tainted isolates are dropped, never recycled

## Impact
The v8-timeout test crashes the server, making it impossible to run the full canary test suite in sequence. All tests after v8-timeout fail with TIMEOUT.

## Occurrence Log
| Date | Context | Notes |
|------|---------|-------|
| 2026-04-07 | Canary fleet test run from release/canary deploy | Process exits after v8-timeout, 49 subsequent tests fail |
| 2026-04-07 | Fixed — cancellable watchdog + deferred termination | Server survives both v8-timeout and v8-heap OOM tests |
