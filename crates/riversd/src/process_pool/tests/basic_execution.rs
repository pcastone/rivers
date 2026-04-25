//! Tests for simple return, args, errors, missing functions, duration,
//! trace IDs, resdata write-back, ctx metadata, exception handling.

use super::*;
use super::helpers::make_js_task;

#[tokio::test]
async fn execute_simple_return() {
    let ctx = make_js_task(
        "function handler(ctx) { return { message: 'hello' }; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["message"], "hello");
}

#[tokio::test]
async fn execute_modifies_resdata() {
    let ctx = make_js_task(
        "function handler(ctx) { ctx.resdata = { count: 42 }; return ctx.resdata; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["count"], 42);
}

#[tokio::test]
async fn execute_reads_args() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({
            "_source": "function handler(ctx) { return { got: __args.name }; }",
            "name": "alice"
        }))
        .trace_id("test-trace".into()).app_id("test-app".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["got"], "alice");
}

#[tokio::test]
async fn execute_handler_error() {
    let ctx = make_js_task(
        "function handler(ctx) { throw new Error('boom'); }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    // TryCatch now captures the actual exception message
    assert!(
        err_msg.contains("boom"),
        "expected exception text 'boom' in: {err_msg}"
    );
}

#[tokio::test]
async fn execute_missing_function() {
    let ctx = make_js_task("var x = 1;", "nonexistent");
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not a function") || err_msg.contains("not found"),
        "expected function error in: {err_msg}"
    );
}

#[tokio::test]
async fn execute_guard_handler_returns_claims() {
    let ctx = make_js_task(
        r#"function authenticate(ctx) {
            return { subject: "user-1", username: "alice", groups: ["admin"] };
        }"#,
        "authenticate",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["subject"], "user-1");
    assert_eq!(result.value["username"], "alice");
}

#[tokio::test]
async fn execute_returns_duration() {
    let ctx = make_js_task(
        "function handler(ctx) { return {}; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    // Duration should be non-negative (likely 0-few ms for trivial work)
    assert!(result.duration_ms < 5000);
}

#[tokio::test]
async fn execute_ctx_has_trace_id() {
    let ctx = make_js_task(
        "function handler(ctx) { return { tid: ctx.trace_id }; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["tid"], "test-trace");
}

#[tokio::test]
async fn execute_undefined_return_is_null() {
    let ctx = make_js_task(
        "function handler(ctx) { /* no return */ }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert!(result.value.is_null());
}

#[tokio::test]
async fn pool_dispatches_to_boa() {
    let config = ProcessPoolConfig {
        engine: "v8".into(),
        workers: 1,
        task_timeout_ms: 5000,
        ..ProcessPoolConfig::default()
    };
    let pool = ProcessPool::new("test".into(), config);
    let ctx = make_js_task(
        "function handler(ctx) { return { ok: true }; }",
        "handler",
    );
    let result = pool.dispatch(ctx).await.unwrap();
    assert_eq!(result.value["ok"], true);
}

// ── P1.1: ctx.resdata write-back tests ──────────────────────

#[tokio::test]
async fn execute_resdata_writeback() {
    // Standard handler: sets ctx.resdata, returns void
    let ctx = make_js_task(
        "function handler(ctx) { ctx.resdata = { status: 'ok' }; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["status"], "ok");
}

#[tokio::test]
async fn execute_return_value_fallback() {
    // Guard-style handler: returns value directly (no resdata)
    let ctx = make_js_task(
        "function authenticate(ctx) { return { subject: 'user-1' }; }",
        "authenticate",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["subject"], "user-1");
}

#[tokio::test]
async fn execute_resdata_takes_priority_over_return() {
    // If handler sets resdata AND returns a value, resdata wins
    let ctx = make_js_task(
        r#"function handler(ctx) {
            ctx.resdata = { from: "resdata" };
            return { from: "return" };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["from"], "resdata");
}

// ── P1.2: ctx.app_id, ctx.node_id, ctx.env tests ───────────

#[tokio::test]
async fn execute_ctx_has_app_metadata() {
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({
            "_source": "function handler(ctx) { return { app: ctx.app_id, node: ctx.node_id, env: ctx.env }; }"
        }))
        .trace_id("t1".into()).app_id("test-app".into())
        .app_id("my-app".into())
        .node_id("node-1".into())
        .runtime_env("prod".into())
        .build()
        .unwrap();
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["app"], "my-app");
    assert_eq!(result.value["node"], "node-1");
    assert_eq!(result.value["env"], "prod");
}

#[tokio::test]
async fn execute_ctx_default_env_is_dev() {
    let ctx = make_js_task(
        "function handler(ctx) { return { env: ctx.env }; }",
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["env"], "dev");
}

// ── P1.3: TryCatch exception message tests ──────────────────

#[tokio::test]
async fn execute_exception_has_message() {
    let ctx = make_js_task(
        "function handler(ctx) { throw new Error('detailed error message'); }",
        "handler",
    );
    let err = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("detailed error message"),
        "should contain exception text: {msg}"
    );
}

#[tokio::test]
async fn execute_exception_with_string_throw() {
    let ctx = make_js_task(
        "function handler(ctx) { throw 'raw string error'; }",
        "handler",
    );
    let err = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("raw string error"),
        "should contain thrown string: {msg}"
    );
}

// ── P2.4: CPU timeout tests ─────────────────────────────────

#[tokio::test]
async fn execute_timeout_terminates() {
    // Wave 10: Use a real pool watchdog registry to test timeout
    let registry: ActiveTaskRegistry = Arc::new(StdMutex::new(HashMap::new()));
    let (watchdog_cancel_tx, watchdog_cancel_rx) = std::sync::mpsc::channel();
    let registry_clone = registry.clone();
    let _watchdog = std::thread::Builder::new()
        .name("test-watchdog".into())
        .spawn(move || {
            loop {
                match watchdog_cancel_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let tasks = registry_clone.lock().unwrap();
                        let timed_out: Vec<usize> = tasks.iter()
                            .filter(|(_, t)| t.started_at.elapsed().as_millis() as u64 > t.timeout_ms)
                            .map(|(id, _)| *id)
                            .collect();
                        drop(tasks);
                        for id in timed_out {
                            if let Some(task) = registry_clone.lock().unwrap().remove(&id) {
                                match task.terminator {
                                    TaskTerminator::V8(handle) => { handle.terminate_execution(); }
                                    TaskTerminator::WasmEpoch(engine) => { engine.increment_epoch(); }
                                    TaskTerminator::Callback(cb) => { cb(); }
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
        })
        .expect("failed to spawn test watchdog");

    let ctx = make_js_task(
        "function handler(ctx) { while(true) {} }",
        "handler",
    );
    let result = execute_js_task(ctx, 100, 0, DEFAULT_HEAP_LIMIT, 0.8, Some(registry.clone())).await; // 100ms timeout
    let _ = watchdog_cancel_tx.send(());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, TaskError::Timeout(_))
            || err.to_string().contains("timeout")
            || err.to_string().contains("terminated"),
        "expected timeout error, got: {err}"
    );
}

// ── P4.1: Heap Limit Test ────────────────────────────────────

#[tokio::test]
async fn execute_heap_limit_prevents_oom() {
    // This test allocates a large array -- with heap limits it should fail
    // rather than consuming unbounded memory
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var arr = [];
            for (var i = 0; i < 100000000; i++) { arr.push("x".repeat(1000)); }
            return { len: arr.length };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
    // Should either timeout or throw OOM -- should NOT succeed
    assert!(result.is_err(), "expected OOM or timeout, got Ok");
}

// ── Sequential Recovery (regression: bugreport_2026-04-07) ──

#[tokio::test]
async fn execute_timeout_then_success_on_same_pool() {
    // Regression: request A times out, then request B on same pool succeeds.
    // Proves the pool recovers after dropping a tainted isolate.

    // Set up a watchdog thread (same pattern as execute_timeout_terminates)
    let registry: ActiveTaskRegistry = Arc::new(StdMutex::new(HashMap::new()));
    let (watchdog_cancel_tx, watchdog_cancel_rx) = std::sync::mpsc::channel();
    let registry_clone = registry.clone();
    let _watchdog = std::thread::Builder::new()
        .name("test-watchdog-recovery".into())
        .spawn(move || {
            loop {
                match watchdog_cancel_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let tasks = registry_clone.lock().unwrap();
                        let timed_out: Vec<usize> = tasks.iter()
                            .filter(|(_, t)| t.started_at.elapsed().as_millis() as u64 > t.timeout_ms)
                            .map(|(id, _)| *id)
                            .collect();
                        drop(tasks);
                        for id in timed_out {
                            if let Some(task) = registry_clone.lock().unwrap().remove(&id) {
                                match task.terminator {
                                    TaskTerminator::V8(handle) => { handle.terminate_execution(); }
                                    TaskTerminator::WasmEpoch(engine) => { engine.increment_epoch(); }
                                    TaskTerminator::Callback(cb) => { cb(); }
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
        })
        .expect("failed to spawn test watchdog");

    // Request A: infinite loop with short timeout → should fail
    let ctx_a = make_js_task(
        "function handler(ctx) { while(true) {} }",
        "handler",
    );
    let result_a = execute_js_task(ctx_a, 100, 0, DEFAULT_HEAP_LIMIT, 0.8, Some(registry.clone())).await;
    assert!(result_a.is_err(), "infinite loop should fail with timeout");

    // Request B: simple handler → should succeed
    let ctx_b = make_js_task(
        "function handler(ctx) { return { recovered: true, seq: 2 }; }",
        "handler",
    );
    let result_b = execute_js_task(ctx_b, 5000, 1, DEFAULT_HEAP_LIMIT, 0.8, Some(registry.clone())).await;
    let _ = watchdog_cancel_tx.send(());
    assert!(result_b.is_ok(), "handler after timeout should succeed: {:?}", result_b.err());
    assert_eq!(result_b.unwrap().value["recovered"], true);
}
