//! V8 bridge contract tests — verify ctx.* injection, Rivers.* APIs, and security.
//!
//! These tests dispatch JS through the real V8 engine via ProcessPoolManager
//! and verify the Rust↔JS boundary works correctly.
//!
//! No server, no HTTP, no cluster. Pure V8 isolate with real injection paths.

use std::collections::HashMap;

use rivers_runtime::rivers_core::config::ProcessPoolConfig;
use riversd::process_pool::{
    Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError,
};

// ── Helpers ─────────────────────────────────────────────────────

fn manager() -> ProcessPoolManager {
    ProcessPoolManager::from_config(&HashMap::new())
}

fn js_file(name: &str, code: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rivers_v8_bridge_{name}_{id}.js"));
    std::fs::write(&path, code).unwrap();
    path
}

async fn eval_js(code: &str) -> serde_json::Value {
    let path = js_file("eval", &format!("function handler(ctx) {{ {code} }}"));
    let mgr = manager();
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("bridge-test".into())
        .app_id("test-app-uuid".into())
        .node_id("test-node-1".into())
        .runtime_env("test".into())
        .build()
        .unwrap();
    let result = mgr.dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    result.value
}

async fn eval_js_with_request(code: &str, request: serde_json::Value) -> serde_json::Value {
    let path = js_file("eval_req", &format!("function handler(ctx) {{ {code} }}"));
    let mgr = manager();
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({"request": request}))
        .trace_id("bridge-req-test".into())
        .app_id("test-app-uuid".into())
        .node_id("test-node-1".into())
        .runtime_env("test".into())
        .build()
        .unwrap();
    let result = mgr.dispatch("default", ctx).await.unwrap();
    let _ = std::fs::remove_file(&path);
    result.value
}

// ── ctx.* Injection Tests ───────────────────────────────────────

#[tokio::test]
async fn ctx_trace_id_injected() {
    let v = eval_js("ctx.resdata = { trace_id: ctx.trace_id };").await;
    assert_eq!(v["trace_id"], "bridge-test");
}

#[tokio::test]
async fn ctx_app_id_injected() {
    let v = eval_js("ctx.resdata = { app_id: ctx.app_id };").await;
    assert_eq!(v["app_id"], "test-app-uuid");
}

/// BUG-010 regression: ctx.app_id must be UUID, not entry point slug.
#[tokio::test]
async fn ctx_app_id_is_uuid_not_slug() {
    let v = eval_js("ctx.resdata = { app_id: ctx.app_id };").await;
    let app_id = v["app_id"].as_str().unwrap();
    assert_ne!(app_id, "handlers", "BUG-010: ctx.app_id is entry point slug, not UUID");
    assert_eq!(app_id, "test-app-uuid");
}

/// BUG-011 regression: ctx.node_id must not be empty.
#[tokio::test]
async fn ctx_node_id_injected() {
    let v = eval_js("ctx.resdata = { node_id: ctx.node_id };").await;
    assert_eq!(v["node_id"], "test-node-1");
}

#[tokio::test]
async fn ctx_env_injected() {
    let v = eval_js("ctx.resdata = { env: ctx.env };").await;
    assert_eq!(v["env"], "test");
}

#[tokio::test]
async fn ctx_resdata_writable() {
    let v = eval_js("ctx.resdata = { result: 'hello' };").await;
    assert_eq!(v["result"], "hello");
}

/// BUG-012 regression: ctx.request.query must be "query", not "query_params".
#[tokio::test]
async fn ctx_request_query_field_name() {
    let req = serde_json::json!({
        "method": "GET",
        "path": "/test",
        "headers": {},
        "query": {"page": "2", "limit": "10"},
        "body": null,
        "path_params": {}
    });
    let v = eval_js_with_request(
        r#"ctx.resdata = {
            has_query: ctx.request.query !== undefined,
            page: ctx.request.query ? ctx.request.query.page : null,
            no_query_params: ctx.request.query_params === undefined
        };"#,
        req,
    ).await;
    assert_eq!(v["has_query"], true, "BUG-012: ctx.request.query should exist");
    assert_eq!(v["page"], "2");
    assert_eq!(v["no_query_params"], true, "BUG-012: query_params ghost field should not exist");
}

#[tokio::test]
async fn ctx_request_has_all_fields() {
    let req = serde_json::json!({
        "method": "POST",
        "path": "/api/test",
        "headers": {"content-type": "application/json"},
        "query": {"limit": "10"},
        "body": {"name": "Alice"},
        "path_params": {"id": "42"}
    });
    let v = eval_js_with_request(
        r#"ctx.resdata = {
            method: ctx.request.method,
            path: ctx.request.path,
            header: ctx.request.headers['content-type'],
            query_limit: ctx.request.query.limit,
            body_name: ctx.request.body.name,
            param_id: ctx.request.path_params.id
        };"#,
        req,
    ).await;
    assert_eq!(v["method"], "POST");
    assert_eq!(v["path"], "/api/test");
    assert_eq!(v["header"], "application/json");
    assert_eq!(v["query_limit"], "10");
    assert_eq!(v["body_name"], "Alice");
    assert_eq!(v["param_id"], "42");
}

// ── Rivers.* API Tests ──────────────────────────────────────────

#[tokio::test]
async fn rivers_log_callable() {
    let v = eval_js(r#"
        try {
            Rivers.log.info("test");
            Rivers.log.warn("test");
            Rivers.log.error("test");
            ctx.resdata = { ok: true };
        } catch(e) { ctx.resdata = { ok: false, err: String(e) }; }
    "#).await;
    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn rivers_crypto_random_hex() {
    let v = eval_js(r#"
        var hex = Rivers.crypto.randomHex(32);
        ctx.resdata = { len: hex.length, hex: hex };
    "#).await;
    assert_eq!(v["len"], 64); // 32 bytes = 64 hex chars
}

#[tokio::test]
async fn rivers_crypto_random_not_deterministic() {
    let v = eval_js(r#"
        var a = Rivers.crypto.randomHex(16);
        var b = Rivers.crypto.randomHex(16);
        ctx.resdata = { same: a === b };
    "#).await;
    assert_eq!(v["same"], false);
}

#[tokio::test]
async fn rivers_crypto_hash_verify() {
    let v = eval_js(r#"
        var hash = Rivers.crypto.hashPassword("secret123");
        var valid = Rivers.crypto.verifyPassword("secret123", hash);
        var invalid = Rivers.crypto.verifyPassword("wrong", hash);
        ctx.resdata = { valid: valid, invalid: invalid };
    "#).await;
    assert_eq!(v["valid"], true);
    assert_eq!(v["invalid"], false);
}

#[tokio::test]
async fn rivers_crypto_hmac_deterministic() {
    let v = eval_js(r#"
        var a = Rivers.crypto.hmac("sha256", "key", "message");
        var b = Rivers.crypto.hmac("sha256", "key", "message");
        ctx.resdata = { same: a === b };
    "#).await;
    assert_eq!(v["same"], true);
}

#[tokio::test]
async fn rivers_crypto_timing_safe() {
    let v = eval_js(r#"
        ctx.resdata = {
            equal: Rivers.crypto.timingSafeEqual("abc", "abc"),
            not_equal: Rivers.crypto.timingSafeEqual("abc", "xyz"),
            diff_len: Rivers.crypto.timingSafeEqual("short", "muchlonger")
        };
    "#).await;
    assert_eq!(v["equal"], true);
    assert_eq!(v["not_equal"], false);
    assert_eq!(v["diff_len"], false);
}

/// Ghost API detection: every Rivers.* method in the spec must exist.
#[tokio::test]
async fn all_spec_rivers_apis_exist() {
    let v = eval_js(r#"
        var apis = [
            "Rivers.log.info", "Rivers.log.warn", "Rivers.log.error",
            "Rivers.crypto.hashPassword", "Rivers.crypto.verifyPassword",
            "Rivers.crypto.randomHex", "Rivers.crypto.randomBase64url",
            "Rivers.crypto.hmac", "Rivers.crypto.timingSafeEqual"
        ];
        var ghosts = [];
        for (var i = 0; i < apis.length; i++) {
            try {
                var parts = apis[i].split('.');
                var obj = this;
                for (var j = 0; j < parts.length; j++) obj = obj[parts[j]];
                if (typeof obj !== 'function') ghosts.push(apis[i]);
            } catch(e) { ghosts.push(apis[i]); }
        }
        ctx.resdata = { ghosts: ghosts, all_exist: ghosts.length === 0 };
    "#).await;
    assert_eq!(v["all_exist"], true, "Ghost APIs: {:?}", v["ghosts"]);
}

/// Console should be available (delegates to Rivers.log).
#[tokio::test]
async fn console_delegates_to_rivers_log() {
    let v = eval_js(r#"
        ctx.resdata = {
            has_console: typeof console === 'object',
            has_log: typeof console.log === 'function',
            has_warn: typeof console.warn === 'function',
            has_error: typeof console.error === 'function'
        };
    "#).await;
    assert_eq!(v["has_console"], true);
    assert_eq!(v["has_log"], true);
    assert_eq!(v["has_warn"], true);
    assert_eq!(v["has_error"], true);
}

// ── V8 Security Tests ───────────────────────────────────────────

/// BUG-003 regression: code generation from strings must be blocked.
#[tokio::test]
async fn v8_codegen_blocked() {
    let v = eval_js(r#"
        var blocked = false;
        try { var fn = Function('return 42'); fn(); }
        catch(e) { blocked = true; }
        ctx.resdata = { blocked: blocked };
    "#).await;
    assert_eq!(v["blocked"], true, "BUG-003: Function() from string not blocked");
}

/// BUG-002 regression: infinite loop must terminate via watchdog.
#[tokio::test]
async fn v8_timeout_terminates_infinite_loop() {
    let path = js_file("timeout", "function handler(ctx) { while(true) {} }");
    let mgr = manager();
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("timeout-test".into())
        .build()
        .unwrap();

    let start = std::time::Instant::now();
    let result = mgr.dispatch("default", ctx).await;
    let elapsed = start.elapsed();

    let _ = std::fs::remove_file(&path);

    // Must terminate, not hang
    assert!(elapsed < std::time::Duration::from_secs(15),
        "infinite loop ran for {:?} — watchdog did not fire", elapsed);
    // Should be an error
    assert!(result.is_err(), "infinite loop should return error, not success");
}

/// BUG-006: massive allocation must not crash the test process.
#[tokio::test]
async fn v8_heap_limit_does_not_crash_process() {
    let path = js_file("heap", r#"function handler(ctx) {
        var arrays = [];
        for (var i = 0; i < 1000000; i++) {
            arrays.push(new Array(100000));
        }
        ctx.resdata = { should_not_reach: true };
    }"#);
    let mgr = manager();
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("heap-test".into())
        .build()
        .unwrap();

    let result = mgr.dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&path);

    // We don't care if it's Ok or Err — we care that we reached this line (process survived)
    assert!(true, "process survived OOM attempt");
}

// ── ctx.store Tests ─────────────────────────────────────────────

#[tokio::test]
async fn ctx_store_set_get_del() {
    let v = eval_js(r#"
        var key = "bridge-test-" + Date.now();
        ctx.store.set(key, "hello");
        var got = ctx.store.get(key);
        ctx.store.del(key);
        var after = ctx.store.get(key);
        ctx.resdata = { got: got, deleted: after === null || after === undefined };
    "#).await;
    assert_eq!(v["got"], "hello");
    assert_eq!(v["deleted"], true);
}

/// Store must reject reserved namespace prefixes.
#[tokio::test]
async fn ctx_store_rejects_reserved_prefixes() {
    let v = eval_js(r#"
        var prefixes = ["session:", "csrf:", "cache:", "raft:", "rivers:"];
        var blocked = 0;
        for (var i = 0; i < prefixes.length; i++) {
            try { ctx.store.get(prefixes[i] + "test"); }
            catch(e) { blocked++; }
        }
        ctx.resdata = { blocked: blocked, total: prefixes.length };
    "#).await;
    assert_eq!(v["blocked"], v["total"], "not all reserved prefixes blocked");
}
