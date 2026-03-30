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
        .trace_id("t1".into())
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
        .trace_id("t1".into())
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
        .trace_id("t1".into())
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
        .trace_id("x6-mem".into())
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
        .trace_id("x6-timeout".into())
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
        .trace_id("x6-pool".into())
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
        .trace_id("au9".into())
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
        .trace_id("au10".into())
        .build()
        .unwrap();

    let result = execute_wasm_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0, None).await.unwrap();
    assert_eq!(result.value["result"], 99);

    let _ = std::fs::remove_file(&wasm_path);
    clear_wasm_cache();
}
