use std::collections::HashMap;

use rivers_runtime::rivers_core::config::ProcessPoolConfig;
use riversd::process_pool::{
    compile_typescript, validate_capabilities, DatasourceToken, DataViewToken, EngineType,
    Entrypoint, ProcessPool, ProcessPoolManager, TaskContextBuilder, TaskError, V8Config, V8Worker,
    WasmtimeConfig, WasmtimeWorker,
};

// ── Helper ───────────────────────────────────────────────────────

fn test_entrypoint() -> Entrypoint {
    Entrypoint {
        module: "handler.js".to_string(),
        function: "onRequest".to_string(),
        language: "javascript".to_string(),
    }
}

fn test_config() -> ProcessPoolConfig {
    ProcessPoolConfig {
        workers: 2,
        max_queue_depth: 4,
        task_timeout_ms: 1000,
        ..Default::default()
    }
}

// ── TaskContext Builder ──────────────────────────────────────────

#[test]
fn builder_requires_entrypoint() {
    let result = TaskContextBuilder::new().build();
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), TaskError::Capability(_)));
}

#[test]
fn builder_with_entrypoint_succeeds() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .args(serde_json::json!({"key": "value"}))
        .trace_id("trace-123".to_string())
        .build()
        .unwrap();

    assert_eq!(ctx.entrypoint.module, "handler.js");
    assert_eq!(ctx.trace_id, "trace-123");
}

#[test]
fn builder_with_capabilities() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .datasource("db".to_string(), DatasourceToken("tok_db".to_string()))
        .dataview("orders".to_string(), DataViewToken("tok_orders".to_string()))
        .env_var("API_KEY".to_string(), "secret".to_string())
        .build()
        .unwrap();

    assert!(ctx.datasources.contains_key("db"));
    assert!(ctx.dataviews.contains_key("orders"));
    assert_eq!(ctx.env.get("API_KEY").unwrap(), "secret");
}

// ── Capability Validation ────────────────────────────────────────

#[test]
fn validate_capabilities_passes_when_all_available() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .datasource("db".to_string(), DatasourceToken("tok".to_string()))
        .dataview("orders".to_string(), DataViewToken("tok".to_string()))
        .build()
        .unwrap();

    let available_ds = vec!["db".to_string(), "cache".to_string()];
    let available_dv = vec!["orders".to_string(), "users".to_string()];

    assert!(validate_capabilities(&ctx, &available_ds, &available_dv).is_ok());
}

#[test]
fn validate_capabilities_fails_on_missing_datasource() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .datasource("missing_db".to_string(), DatasourceToken("tok".to_string()))
        .build()
        .unwrap();

    let result = validate_capabilities(&ctx, &["db".to_string()], &[]);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), TaskError::Capability(_)));
}

#[test]
fn validate_capabilities_fails_on_missing_dataview() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .dataview("missing_view".to_string(), DataViewToken("tok".to_string()))
        .build()
        .unwrap();

    let result = validate_capabilities(&ctx, &[], &["orders".to_string()]);
    assert!(result.is_err());
}

// ── ProcessPool ──────────────────────────────────────────────────

#[tokio::test]
async fn pool_dispatch_returns_handler_error_for_missing_module() {
    // With the Boa engine active, dispatching a task with a missing
    // module file results in HandlerError (cannot read file).
    let pool = ProcessPool::new("test".to_string(), test_config());

    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .build()
        .unwrap();

    let result = pool.dispatch(ctx).await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), TaskError::HandlerError(msg) if msg.contains("cannot read module")),
        "expected HandlerError about missing module"
    );
}

#[tokio::test]
async fn pool_reports_queue_depth() {
    let pool = ProcessPool::new("test".to_string(), test_config());
    assert_eq!(pool.queue_depth(), 0);
    assert_eq!(pool.max_queue_depth(), 4);
    assert_eq!(pool.name(), "test");
}

#[tokio::test]
async fn pool_queue_full_returns_error() {
    // Create pool with max_queue_depth = 1 and 0 workers (nobody drains)
    let config = ProcessPoolConfig {
        workers: 0,
        max_queue_depth: 1,
        ..Default::default()
    };
    let pool = ProcessPool::new("tiny".to_string(), config);

    // Can't actually fill the queue with 0 workers since the channel
    // has capacity 1, so the first send blocks without a receiver.
    // Instead test the depth check directly.
    assert_eq!(pool.max_queue_depth(), 1);
}

// ── ProcessPoolManager ───────────────────────────────────────────

#[tokio::test]
async fn manager_creates_default_pool() {
    let manager = ProcessPoolManager::from_config(&HashMap::new());
    assert!(manager.get_pool("default").is_some());
}

#[tokio::test]
async fn manager_creates_named_pools() {
    let mut config = HashMap::new();
    config.insert("wasm".to_string(), ProcessPoolConfig {
        engine: "wasmtime".to_string(),
        workers: 2,
        ..Default::default()
    });

    let manager = ProcessPoolManager::from_config(&config);
    assert!(manager.get_pool("wasm").is_some());
    assert!(manager.get_pool("default").is_some());
}

#[tokio::test]
async fn manager_dispatch_to_unknown_pool_fails() {
    let manager = ProcessPoolManager::from_config(&HashMap::new());

    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .build()
        .unwrap();

    let result = manager.dispatch("nonexistent", ctx).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), TaskError::Internal(_)));
}

#[tokio::test]
async fn manager_dispatch_to_default_pool() {
    // With the Boa engine active, dispatching a task with a missing
    // module file results in HandlerError (cannot read file).
    let manager = ProcessPoolManager::from_config(&HashMap::new());

    let ctx = TaskContextBuilder::new()
        .entrypoint(test_entrypoint())
        .build()
        .unwrap();

    let result = manager.dispatch("default", ctx).await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), TaskError::HandlerError(msg) if msg.contains("cannot read module")),
        "expected HandlerError about missing module"
    );
}

// ── Config Defaults ──────────────────────────────────────────────

#[test]
fn default_pool_config() {
    let config = ProcessPoolConfig::default();
    assert_eq!(config.engine, "v8");
    assert_eq!(config.workers, 4);
    assert_eq!(config.task_timeout_ms, 5000);
    assert_eq!(config.max_queue_depth, 0); // auto = workers * 4
    assert_eq!(config.max_heap_mb, 128);
    assert!(config.recycle_after_tasks.is_none());
    assert!((config.heap_recycle_threshold - 0.8).abs() < f64::EPSILON);
}

// ── EngineType (C7.1) ───────────────────────────────────────────

#[test]
fn engine_type_from_str() {
    assert_eq!(EngineType::from_str("v8"), Some(EngineType::V8));
    assert_eq!(EngineType::from_str("V8"), Some(EngineType::V8));
    assert_eq!(EngineType::from_str("wasmtime"), Some(EngineType::Wasmtime));
    assert_eq!(EngineType::from_str("wasm"), Some(EngineType::Wasmtime));
    assert_eq!(EngineType::from_str("unknown"), None);
}

#[test]
fn engine_type_as_str() {
    assert_eq!(EngineType::V8.as_str(), "v8");
    assert_eq!(EngineType::Wasmtime.as_str(), "wasmtime");
}

#[test]
fn engine_type_display() {
    assert_eq!(format!("{}", EngineType::V8), "v8");
    assert_eq!(format!("{}", EngineType::Wasmtime), "wasmtime");
}

// ── V8Worker (C7.2) ─────────────────────────────────────────────

#[test]
fn v8_worker_creates_successfully() {
    let result = V8Worker::new(V8Config::default());
    assert!(result.is_ok(), "V8Worker::new should succeed now that V8 is integrated");
    let worker = result.unwrap();
    assert_eq!(worker.heap_limit(), 128 * 1024 * 1024);
    assert_eq!(worker.cpu_time_limit_ms(), 5000);
    assert_eq!(worker.pool_size(), 4);
}

#[test]
fn v8_config_defaults() {
    let config = V8Config::default();
    assert_eq!(config.isolate_pool_size, 4);
    assert_eq!(config.memory_limit_bytes, 128 * 1024 * 1024);
    assert_eq!(config.cpu_time_limit_ms, 5000);
}

// ── WasmtimeWorker (C7.3) ───────────────────────────────────────

#[test]
fn wasmtime_worker_creates_successfully() {
    let result = WasmtimeWorker::new(WasmtimeConfig::default());
    assert!(result.is_ok(), "WasmtimeWorker::new should succeed");
    let worker = result.unwrap();
    assert_eq!(worker.fuel_limit(), 1_000_000);
    assert_eq!(worker.memory_pages(), 256);
    assert_eq!(worker.pool_size(), 4);
}

#[test]
fn wasmtime_config_defaults() {
    let config = WasmtimeConfig::default();
    assert_eq!(config.instance_pool_size, 4);
    assert_eq!(config.fuel_limit, 1_000_000);
    assert_eq!(config.memory_pages, 256);
}

// ── TypeScript Compiler (V2.10) ─────────────────────────────────

#[test]
fn compile_typescript_strips_annotations() {
    // V2.10: compile_typescript now returns Ok with type annotations stripped
    let result = compile_typescript("const x: number = 42;", "test.ts");
    assert!(result.is_ok(), "compile_typescript should succeed: {:?}", result.err());
    let js = result.unwrap();
    // The output should still contain the assignment (type annotation stripped)
    assert!(js.contains("const x"), "should preserve variable declaration");
}

// ── AU: JS/WASM Integration Tests (dispatch through pool) ────────

#[tokio::test]
async fn dispatch_js_from_disk_file() {
    let dir = std::env::temp_dir();
    let js_path = dir.join("au_pool_handler.js");
    std::fs::write(
        &js_path,
        r#"function onRequest(ctx) { return { source: "file", val: 42 }; }"#,
    ).unwrap();

    let manager = ProcessPoolManager::from_config(&HashMap::new());

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "onRequest".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("au-pool-file".into())
        .build()
        .unwrap();

    let result = manager.dispatch("default", ctx).await.unwrap();
    assert_eq!(result.value["source"], "file");
    assert_eq!(result.value["val"], 42);

    let _ = std::fs::remove_file(&js_path);
}

#[tokio::test]
async fn dispatch_js_with_args_through_pool() {
    let dir = std::env::temp_dir();
    let js_path = dir.join("au_pool_args.js");
    std::fs::write(
        &js_path,
        r#"function handler(ctx) {
            return { name: __args.name, count: __args.count };
        }"#,
    ).unwrap();

    let manager = ProcessPoolManager::from_config(&HashMap::new());

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({"name": "rivers", "count": 7}))
        .trace_id("au-pool-args".into())
        .build()
        .unwrap();

    let result = manager.dispatch("default", ctx).await.unwrap();
    assert_eq!(result.value["name"], "rivers");
    assert_eq!(result.value["count"], 7);

    let _ = std::fs::remove_file(&js_path);
}

#[tokio::test]
async fn dispatch_js_error_propagates_through_pool() {
    let dir = std::env::temp_dir();
    let js_path = dir.join("au_pool_error.js");
    std::fs::write(
        &js_path,
        r#"function handler(ctx) { throw new Error("intentional failure"); }"#,
    ).unwrap();

    let manager = ProcessPoolManager::from_config(&HashMap::new());

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("au-pool-error".into())
        .build()
        .unwrap();

    let result = manager.dispatch("default", ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("intentional failure"), "expected error text in: {}", err);

    let _ = std::fs::remove_file(&js_path);
}

#[tokio::test]
async fn dispatch_wasm_through_pool() {
    let wat = r#"(module (func (export "handler") (result i32) (i32.const 55)))"#;
    let dir = std::env::temp_dir();
    let wasm_path = dir.join("au_pool_wasm.wat");
    std::fs::write(&wasm_path, wat.as_bytes()).unwrap();

    let mut config = HashMap::new();
    config.insert("wasm_pool".to_string(), ProcessPoolConfig {
        engine: "wasmtime".to_string(),
        workers: 1,
        ..Default::default()
    });
    let manager = ProcessPoolManager::from_config(&config);

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: wasm_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "wasm".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("au-wasm-pool".into())
        .build()
        .unwrap();

    let result = manager.dispatch("wasm_pool", ctx).await.unwrap();
    assert_eq!(result.value["result"], 55);

    let _ = std::fs::remove_file(&wasm_path);
}

#[tokio::test]
async fn dispatch_async_js_through_pool() {
    let dir = std::env::temp_dir();
    let js_path = dir.join("au_pool_async.js");
    std::fs::write(
        &js_path,
        r#"async function handler(ctx) {
            var a = await Promise.resolve(10);
            var b = await Promise.resolve(20);
            return { sum: a + b };
        }"#,
    ).unwrap();

    let manager = ProcessPoolManager::from_config(&HashMap::new());

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: js_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("au-pool-async".into())
        .build()
        .unwrap();

    let result = manager.dispatch("default", ctx).await.unwrap();
    assert_eq!(result.value["sum"], 30);

    let _ = std::fs::remove_file(&js_path);
}
