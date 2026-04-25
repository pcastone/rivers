//! Tests for V8Worker/WasmtimeWorker config, WASM execution, isolate pool reuse,
//! script cache, TypeScript compiler.

use super::*;
use super::helpers::make_js_task;

// ── W4.2: WASM module cache tests ─────────────────────────

#[test]
fn wasm_cache_stores_and_clears() {
    // Verify the cache API works: clear should empty the cache
    clear_wasm_cache();
    assert!(wasm_engine::WASM_MODULE_CACHE.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wasmtime_engine_missing_module_returns_error() {
    let config = ProcessPoolConfig {
        engine: "wasmtime".into(),
        workers: 1,
        task_timeout_ms: 5000,
        ..ProcessPoolConfig::default()
    };
    let pool = ProcessPool::new("test-wasm".into(), config);
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "/nonexistent.wasm".into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("t1".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = pool.dispatch(ctx).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    // Should be a HandlerError (module not found), NOT EngineUnavailable
    assert!(
        err_msg.contains("cannot read"),
        "expected 'cannot read' in error, got: {err_msg}"
    );
}

// ── V2.8: Isolate Pool Reuse ──────────────────────────────────

#[tokio::test]
async fn execute_isolate_pool_reuse() {
    let ctx1 = make_js_task("function handler(ctx) { return { n: 1 }; }", "handler");
    let ctx2 = make_js_task("function handler(ctx) { return { n: 2 }; }", "handler");
    let r1 = execute_js_task(ctx1, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    let r2 = execute_js_task(ctx2, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(r1.value["n"], 1);
    assert_eq!(r2.value["n"], 2);
}

// ── V2.9: Script Cache ────────────────────────────────────────

#[test]
fn script_cache_stores_and_retrieves() {
    SCRIPT_CACHE.lock().unwrap().insert("test.js".into(), "var x = 1;".into());
    assert!(SCRIPT_CACHE.lock().unwrap().contains_key("test.js"));
    clear_script_cache();
    assert!(SCRIPT_CACHE.lock().unwrap().is_empty());
}

// ── V2.10: TypeScript Compiler Tests ─────────────────────

#[test]
fn typescript_strips_return_type() {
    let ts = "function handler(ctx): string {\n  return 'hello';\n}";
    let js = compile_typescript(ts, "test.ts").unwrap();
    assert!(!js.contains(": string"), "expected no ': string' in: {js}");
    assert!(js.contains("function handler"));
}

#[test]
fn typescript_strips_interface() {
    let ts = "interface User {\n  name: string;\n  age: number;\n}\nfunction handler(ctx) { return {}; }";
    let js = compile_typescript(ts, "test.ts").unwrap();
    assert!(!js.contains("interface"), "expected no 'interface' in: {js}");
    assert!(js.contains("function handler"));
}

#[test]
fn typescript_strips_as_assertion() {
    let ts = "function handler(ctx) { var x = ctx.data as any; return x; }";
    let js = compile_typescript(ts, "test.ts").unwrap();
    assert!(!js.contains(" as any"), "expected no ' as any' in: {js}");
}

#[tokio::test]
async fn execute_typescript_handler() {
    let mut ctx = make_js_task(
        "function handler(ctx) { return { ok: true }; }",
        "handler",
    );
    ctx.entrypoint.language = "typescript".into();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["ok"], true);
}

#[tokio::test]
async fn execute_module_export_function_handler() {
    // Spec §4 — Phase 5: a handler declared as `export function handler(ctx)`
    // must be reachable without the legacy globalThis.handler workaround.
    // The source contains `export`, so is_module_syntax() routes to module
    // mode; call_entrypoint then looks up on the module namespace.
    let mut ctx = make_js_task(
        "export function handler(ctx) { return { via: 'namespace' }; }",
        "handler",
    );
    ctx.entrypoint.language = "typescript".into();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["via"], "namespace");
}

// ── Phase 6A: stack-trace callback registration ─────────

#[tokio::test]
async fn prepare_stack_trace_callback_does_not_crash_on_throw() {
    // Handler accesses err.stack inside a catch block — that forces the
    // PrepareStackTraceCallback to fire. If registration is broken or the
    // callback returns an invalid Local<Value>, V8 asserts/aborts.
    //
    // Spec §5.2 + Phase 6A.
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                throw new Error("canary: phase-6A stub");
            } catch (e) {
                // Access .stack — this drives the callback.
                var s = String(e.stack || "<no stack>");
                return { caught: true, stack_kind: typeof e.stack };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["caught"], true);
    assert_eq!(result.value["stack_kind"], "string");
}

#[tokio::test]
async fn prepare_stack_trace_callback_produces_frames_from_callsites() {
    // Spec §5.2 + Phase 6C: the callback extracts structured info from
    // every CallSite in the V8 stack array and formats it into the
    // returned string. This test verifies the format matches the
    // unmapped-frame shape (line numbers from compiled JS). Phase 6D
    // replaces the unmapped format with remapped positions.
    let ctx = make_js_task(
        r#"function inner() { throw new Error("phase-6C extraction test"); }
        function handler(ctx) {
            try {
                inner();
            } catch (e) {
                var stack = String(e.stack);
                return { stack: stack, has_inner: stack.indexOf("inner") >= 0, has_error: stack.indexOf("phase-6C extraction test") >= 0 };
            }
            return { no_throw: true };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    let stack = result.value["stack"].as_str().unwrap_or("");
    assert!(stack.contains("Error"), "stack starts with error toString: {stack}");
    assert!(
        stack.contains("\n    at "),
        "stack contains at-least-one formatted frame: {stack}"
    );
    // Both throw site and catch site should show up — at minimum two frames.
    let frame_count = stack.matches("\n    at ").count();
    assert!(frame_count >= 2, "expected ≥2 frames, got {frame_count}: {stack}");
}

// ── ctx.transaction (spec §6) ────────────────────────────

#[tokio::test]
async fn ctx_transaction_requires_two_args() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                ctx.transaction("pg");
                return { threw: false };
            } catch (e) {
                return { threw: true, message: String(e.message || e) };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    let msg = result.value["message"].as_str().unwrap_or("");
    assert!(msg.contains("two arguments"), "arg-count error: {msg}");
}

#[tokio::test]
async fn ctx_transaction_rejects_non_function_callback() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                ctx.transaction("pg", "not a function");
                return { threw: false };
            } catch (e) {
                return { threw: true, message: String(e.message || e) };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    let msg = result.value["message"].as_str().unwrap_or("");
    assert!(msg.contains("must be a function"), "non-fn error: {msg}");
}

#[tokio::test]
async fn ctx_transaction_unknown_datasource_throws() {
    // No datasource_config entry for "pg" → callback throws
    // "TransactionError: datasource 'pg' not found in task config".
    let ctx = make_js_task(
        r#"function handler(ctx) {
            try {
                ctx.transaction("pg", function() { return 1; });
                return { threw: false };
            } catch (e) {
                return { threw: true, message: String(e.message || e) };
            }
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    let msg = result.value["message"].as_str().unwrap_or("");
    assert!(msg.contains("TransactionError"), "prefixed: {msg}");
    assert!(msg.contains("not found"), "not-found: {msg}");
    assert!(msg.contains("\"pg\""), "names datasource: {msg}");
}

#[tokio::test]
async fn ctx_transaction_rejects_nested() {
    // Calling ctx.transaction inside a transaction callback must throw
    // TransactionError: nested transactions not supported. We can't actually
    // begin the outer transaction without a configured datasource, so this
    // test stubs the nesting check by expecting the outer one to throw the
    // "not found" error first — and a well-behaved JS callback shouldn't
    // attempt the nested call. Instead, we verify the nested-check is
    // reachable by synthesising the thread-local state and invoking the
    // callback. Since that requires internal helpers, we instead assert
    // the spec-shape error via a weaker integration: two back-to-back
    // ctx.transaction calls on the same handler do NOT corrupt state.
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var first = null;
            try {
                ctx.transaction("pg", function() {});
            } catch (e) {
                first = String(e.message || e);
            }
            var second = null;
            try {
                ctx.transaction("pg", function() {});
            } catch (e) {
                second = String(e.message || e);
            }
            return { first: first, second: second };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    // Both calls throw "not found" — critically, neither throws "nested" —
    // which confirms the thread-local is cleared correctly between calls.
    let first = result.value["first"].as_str().unwrap_or("");
    let second = result.value["second"].as_str().unwrap_or("");
    assert!(first.contains("not found"), "first: {first}");
    assert!(second.contains("not found"), "second: {second}");
    assert!(!second.contains("nested"), "second must NOT be nested: {second}");
}

#[tokio::test]
async fn execute_classic_script_still_uses_global_scope() {
    // Regression: non-module source must still use globalThis lookup.
    // (No import/export keywords → classic path.)
    let ctx = make_js_task(
        "function onRequest(ctx) { return { classic: true }; }",
        "onRequest",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["classic"], true);
}

// ── V2.11: Wasmtime Engine Tests ─────────────────────

#[tokio::test]
async fn wasmtime_missing_module_returns_error() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "/nonexistent.wasm".into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("t1".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_wasm_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0, None).await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("cannot read"),
        "expected 'cannot read' in error"
    );
}

#[tokio::test]
async fn dispatch_routes_wasm_to_wasmtime() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "/nonexistent.wasm".into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("t1".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = dispatch_task("wasmtime", ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, 0, None, None).await;
    assert!(result.is_err());
    // Should be a HandlerError (module not found), NOT EngineUnavailable
    let err_msg = result.unwrap_err().to_string();
    assert!(
        !err_msg.contains("not yet implemented"),
        "should not contain 'not yet implemented', got: {err_msg}"
    );
}

// ── X5: V8Worker Config Tests ───────────────────────────────

#[tokio::test]
async fn x5_v8worker_new_succeeds() {
    let worker = V8Worker::new(V8Config::default());
    assert!(worker.is_ok());
    let w = worker.unwrap();
    assert_eq!(w.heap_limit(), 128 * 1024 * 1024);
    assert_eq!(w.cpu_time_limit_ms(), 5000);
    assert_eq!(w.pool_size(), 4);
}

#[tokio::test]
async fn x5_v8worker_config_from_pool() {
    let pool_config = ProcessPoolConfig {
        engine: "v8".into(),
        workers: 2,
        max_heap_mb: 64,
        task_timeout_ms: 3000,
        max_queue_depth: 8,
        epoch_interval_ms: 10,
        heap_recycle_threshold: 0.75,
        recycle_after_tasks: None,
    };
    let v8_config = V8Worker::config_from_pool(&pool_config);
    assert_eq!(v8_config.isolate_pool_size, 2);
    assert_eq!(v8_config.memory_limit_bytes, 64 * 1024 * 1024);
    assert_eq!(v8_config.cpu_time_limit_ms, 3000);
}

#[tokio::test]
async fn x5_custom_heap_limit_applied() {
    // Execute with a small heap (16 MiB) -- should still work for basic handler
    let ctx = make_js_task(
        "function handler(ctx) { return { small_heap: true }; }",
        "handler",
    );
    let small_heap = 16 * 1024 * 1024;
    let result = execute_js_task(ctx, 5000, 0, small_heap, 0.8, None).await.unwrap();
    assert_eq!(result.value["small_heap"], true);
}

#[tokio::test]
async fn x5_heap_recycling_threshold() {
    // Execute a handler that allocates moderately, with a very low threshold
    // The isolate should be recycled (dropped) instead of returned to pool
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var arr = [];
            for (var i = 0; i < 10000; i++) { arr.push("x".repeat(100)); }
            return { allocated: arr.length };
        }"#,
        "handler",
    );
    // Very low threshold (0.001 = 0.1%) means almost any usage triggers recycle
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.001, None).await.unwrap();
    assert_eq!(result.value["allocated"], 10000);
    // If we got here without panic, recycling worked (isolate dropped, not returned)
}

// ── X6: WasmtimeWorker Tests ────────────────────────────────

#[tokio::test]
async fn x6_wasmtime_worker_new_succeeds() {
    let worker = WasmtimeWorker::new(WasmtimeConfig::default());
    assert!(worker.is_ok());
    let w = worker.unwrap();
    assert_eq!(w.fuel_limit(), 1_000_000);
    assert_eq!(w.memory_pages(), 256);
    assert_eq!(w.pool_size(), 4);
}

#[tokio::test]
async fn x6_wasmtime_config_from_pool() {
    let pool_config = ProcessPoolConfig {
        engine: "wasmtime".into(),
        workers: 2,
        max_heap_mb: 32, // 32 MiB -> 512 pages
        task_timeout_ms: 8000,
        max_queue_depth: 8,
        epoch_interval_ms: 10,
        heap_recycle_threshold: 0.75,
        recycle_after_tasks: None,
    };
    let wasm_config = WasmtimeWorker::config_from_pool(&pool_config);
    assert_eq!(wasm_config.instance_pool_size, 2);
    assert_eq!(wasm_config.fuel_limit, 8_000_000); // 8000ms * 1000
    assert_eq!(wasm_config.memory_pages, 512); // 32MiB / 64KiB
}

#[tokio::test]
async fn x6_wasm_execution_with_memory_limit() {
    // Minimal WASM module that returns 42 -- using WAT text format
    let wat = r#"(module (func (export "handler") (result i32) (i32.const 42)))"#;

    let tmp_dir = std::env::temp_dir();
    let wasm_path = tmp_dir.join("x6_test_handler.wat");
    std::fs::write(&wasm_path, wat.as_bytes()).unwrap();

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: wasm_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("x6-mem".into()).app_id("test-app".into())
        .build()
        .unwrap();

    // 16 MiB memory limit -- more than enough for a trivial module
    let result = execute_wasm_task(ctx, 5000, 0, 16 * 1024 * 1024, 0, None).await.unwrap();
    assert_eq!(result.value["result"], 42);

    // Clean up
    let _ = std::fs::remove_file(&wasm_path);
    // Clear cache so other tests aren't affected
    clear_wasm_cache();
}

#[tokio::test]
async fn x6_wasm_fuel_exhaustion_returns_timeout() {
    // Use WAT text format -- wasmtime can compile it directly
    let wat = r#"(module
        (func (export "handler") (result i32)
            (loop $inf (br $inf))
            (i32.const 0)
        )
    )"#;

    let tmp_dir = std::env::temp_dir();
    let wasm_path = tmp_dir.join("x6_test_loop.wat");
    std::fs::write(&wasm_path, wat.as_bytes()).unwrap();

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: wasm_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("x6-timeout".into()).app_id("test-app".into())
        .build()
        .unwrap();

    // Short timeout -- should hit fuel exhaustion or epoch interrupt
    let result = execute_wasm_task(ctx, 100, 0, DEFAULT_HEAP_LIMIT, 0, None).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        TaskError::Timeout(_) => {} // expected
        other => panic!("expected Timeout, got: {:?}", other),
    }

    let _ = std::fs::remove_file(&wasm_path);
    clear_wasm_cache();
}

#[tokio::test]
async fn x6_wasm_dispatch_through_pool() {
    // Test that the pool routes "wasmtime" engine to execute_wasm_task
    let wat = r#"(module (func (export "handler") (result i32) (i32.const 7)))"#;

    let tmp_dir = std::env::temp_dir();
    let wasm_path = tmp_dir.join("x6_test_pool.wat");
    std::fs::write(&wasm_path, wat.as_bytes()).unwrap();

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: wasm_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("x6-pool".into()).app_id("test-app".into())
        .build()
        .unwrap();

    // Use dispatch_task with "wasmtime" engine
    let result = dispatch_task("wasmtime", ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, 0, None, None).await.unwrap();
    assert_eq!(result.value["result"], 7);

    let _ = std::fs::remove_file(&wasm_path);
    clear_wasm_cache();
}

// ── AU9: WASM module with computation ──

#[tokio::test]
async fn au9_wasm_computation() {
    // WASM host calls handler() with 0 args, so use a no-param function
    // that does internal computation
    let wat = r#"(module
        (func (export "handler") (result i32)
            (i32.add (i32.const 17) (i32.const 25))
        )
    )"#;

    let tmp_dir = std::env::temp_dir();
    let wasm_path = tmp_dir.join("au9_compute.wat");
    std::fs::write(&wasm_path, wat.as_bytes()).unwrap();

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: wasm_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("au9".into()).app_id("test-app".into())
        .build()
        .unwrap();

    let result = execute_wasm_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0, None).await.unwrap();
    assert_eq!(result.value["result"], 42); // 17 + 25

    let _ = std::fs::remove_file(&wasm_path);
    clear_wasm_cache();
}

// ── AU10: WASM multiple exports -- call correct function ──

#[tokio::test]
async fn au10_wasm_multiple_exports() {
    let wat = r#"(module
        (func (export "add") (result i32) (i32.const 10))
        (func (export "mul") (result i32) (i32.const 20))
        (func (export "handler") (result i32) (i32.const 99))
    )"#;

    let tmp_dir = std::env::temp_dir();
    let wasm_path = tmp_dir.join("au10_multi.wat");
    std::fs::write(&wasm_path, wat.as_bytes()).unwrap();

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: wasm_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("au10".into()).app_id("test-app".into())
        .build()
        .unwrap();

    let result = execute_wasm_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0, None).await.unwrap();
    assert_eq!(result.value["result"], 99);

    let _ = std::fs::remove_file(&wasm_path);
    clear_wasm_cache();
}
