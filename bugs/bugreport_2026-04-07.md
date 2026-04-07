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
None — requires architectural fix:
1. Single termination authority (remove duplicate watchdog, use pool-level only)
2. TryCatch protection around all V8 scope operations, not just compilation/execution
3. Consider `std::panic::catch_unwind` around the V8 execution thread to prevent process abort

## Impact
The v8-timeout test crashes the server, making it impossible to run the full canary test suite in sequence. All tests after v8-timeout fail with TIMEOUT.

## Occurrence Log
| Date | Context | Notes |
|------|---------|-------|
| 2026-04-07 | Canary fleet test run from release/canary deploy | Process exits after v8-timeout, 49 subsequent tests fail |
| 2026-04-07 | Fixed — cancellable watchdog + deferred termination | Server survives both v8-timeout and v8-heap OOM tests |
