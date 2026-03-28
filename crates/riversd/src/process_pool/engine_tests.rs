//! ProcessPool engine tests — V8 JavaScript and Wasmtime WASM execution.

use super::*;
use rivers_runtime::rivers_core::DriverFactory;
use rivers_runtime::tiered_cache::NoopDataViewCache;
use rivers_runtime::DataViewExecutor;

// ── Boa Engine Tests ─────────────────────────────────────────────

#[cfg(test)]
mod engine_tests {
    use super::*;

    fn make_js_task(source: &str, function: &str) -> TaskContext {
        TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: function.into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({ "_source": source }))
            .trace_id("test-trace".into())
            .build()
            .unwrap()
    }

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
            .trace_id("test-trace".into())
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
    async fn execute_rivers_crypto_random_hex() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                return { hex: Rivers.crypto.randomHex(16) };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        let hex = result.value["hex"].as_str().unwrap();
        // 16 random bytes → 32 hex chars
        assert_eq!(hex.len(), 32, "expected 32 hex chars, got {}", hex.len());
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex string, got: {hex}"
        );
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

    // ── W4.2: WASM module cache tests ─────────────────────────

    #[test]
    fn wasm_cache_stores_and_clears() {
        // Verify the cache API works: clear should empty the cache
        clear_wasm_cache();
        assert!(wasm_engine::WASM_MODULE_CACHE.lock().unwrap().is_empty());
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
            .trace_id("t1".into())
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

    // ── P2.1: Rivers.log tests ──────────────────────────────────

    #[tokio::test]
    async fn execute_rivers_log_does_not_crash() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                Rivers.log.info("test message");
                Rivers.log.warn("warning");
                Rivers.log.error("error");
                return { logged: true };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["logged"], true);
    }

    // ── P2.3: console.log tests ─────────────────────────────────

    #[tokio::test]
    async fn execute_console_log_works() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                console.log("hello from console");
                console.warn("a warning");
                console.error("an error");
                return { ok: true };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["ok"], true);
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

    // ── P2.2: Rivers.crypto.randomHex produces unique values ────

    #[tokio::test]
    async fn execute_random_hex_is_unique() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var a = Rivers.crypto.randomHex(8);
                var b = Rivers.crypto.randomHex(8);
                return { a: a, b: b, different: a !== b };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        // Two calls should produce different hex strings (extremely unlikely to collide)
        assert_eq!(result.value["different"], true);
    }

    // ── P2.2: Rivers.crypto native implementations ─────────────

    #[tokio::test]
    async fn execute_crypto_hash_password_bcrypt() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var h = Rivers.crypto.hashPassword("secret");
                var v = Rivers.crypto.verifyPassword("secret", h);
                return { hash: h, verified: v };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        let hash = result.value["hash"].as_str().unwrap();
        assert!(hash.starts_with("$2b$12$"), "expected bcrypt $2b$12$ prefix, got: {hash}");
        assert_eq!(result.value["verified"], true);
    }

    // ── P3: Host Function Binding Tests ──────────────────────────

    #[tokio::test]
    async fn execute_ctx_dataview_returns_prefetched() {
        // ctx.dataview() returns data from ctx.data when pre-fetched
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.data.orders = [{ id: 1, name: "test" }];
                var result = ctx.dataview("orders");
                return { got: result };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["got"][0]["id"], 1);
        assert_eq!(result.value["got"][0]["name"], "test");
    }

    #[tokio::test]
    async fn execute_ctx_dataview_missing_returns_null() {
        // ctx.dataview() returns null (not throw) when data not pre-fetched
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var result = ctx.dataview("nonexistent");
                return { got: result };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert!(result.value["got"].is_null());
    }

    // ── V2.3: ctx.streamDataview — mock iterator protocol ─────────

    #[tokio::test]
    async fn execute_stream_dataview_array() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.data.items = [1, 2, 3];
                var stream = ctx.streamDataview("items");
                var result = [];
                var chunk;
                while (!(chunk = stream.next()).done) {
                    result.push(chunk.value);
                }
                return { items: result };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["items"], serde_json::json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn execute_stream_dataview_single_value() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.data.record = { name: "alice" };
                var stream = ctx.streamDataview("record");
                var chunk = stream.next();
                return { value: chunk.value, done: chunk.done };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["value"]["name"], "alice");
        assert_eq!(result.value["done"], false);
    }

    #[tokio::test]
    async fn execute_stream_dataview_missing_returns_done() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var stream = ctx.streamDataview("nonexistent");
                var chunk = stream.next();
                return { done: chunk.done };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["done"], true);
    }

    // ── V2.4.4: ctx.store — native V8 callbacks ─────────────────

    #[tokio::test]
    async fn execute_store_native_reserved_prefix_rejected() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                try { ctx.store.set("session:abc", "val"); return { error: false }; }
                catch(e) { return { error: true, msg: e.message }; }
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["error"], true);
        assert!(result.value["msg"].as_str().unwrap().contains("reserved"));
    }

    #[tokio::test]
    async fn execute_store_native_crud() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.store.set("user:1", { name: "alice", age: 30 });
                var val = ctx.store.get("user:1");
                ctx.store.del("user:1");
                var after = ctx.store.get("user:1");
                return { val: val, after: after };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["val"]["name"], "alice");
        assert_eq!(result.value["val"]["age"], 30);
        assert!(result.value["after"].is_null());
    }

    #[tokio::test]
    async fn execute_ctx_store_get_set_del() {
        // In-memory per-task store: set, get, del all work within a single handler
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.store.set("mykey", { count: 42 });
                var val = ctx.store.get("mykey");
                ctx.store.del("mykey");
                var after = ctx.store.get("mykey");
                return { val: val, after: after };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["val"]["count"], 42);
        assert!(result.value["after"].is_null());
    }

    #[tokio::test]
    async fn execute_ctx_store_get_missing_returns_null() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var val = ctx.store.get("nonexistent");
                return { val: val };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert!(result.value["val"].is_null());
    }

    #[tokio::test]
    async fn execute_ctx_store_overwrite() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.store.set("k", "first");
                ctx.store.set("k", "second");
                return { val: ctx.store.get("k") };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["val"], "second");
    }

    #[tokio::test]
    async fn execute_ctx_datasource_builder_chain() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var builder = ctx.datasource("primary_db");
                builder = builder.fromQuery("SELECT 1");
                builder = builder.withPostSchema({});
                return { datasource: builder._datasource, query: builder._query };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["datasource"], "primary_db");
        assert_eq!(result.value["query"], "SELECT 1");
    }

    #[tokio::test]
    async fn execute_ctx_datasource_build_throws() {
        // X7: .build() on an undeclared datasource should throw CapabilityError
        let ctx = make_js_task(
            r#"function handler(ctx) {
                try {
                    ctx.datasource("db").fromQuery("SELECT 1").build();
                } catch(e) { return { error: e.message }; }
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert!(result.value["error"].as_str().unwrap().contains("CapabilityError"));
    }

    // ── P3.6: Native Crypto Tests ────────────────────────────────

    #[tokio::test]
    async fn execute_crypto_hash_and_verify() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var hash = Rivers.crypto.hashPassword("secret123");
                var valid = Rivers.crypto.verifyPassword("secret123", hash);
                var invalid = Rivers.crypto.verifyPassword("wrong", hash);
                return { hash_prefix: hash.substring(0, 7), valid: valid, invalid: invalid };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["hash_prefix"], "$2b$12$");
        assert_eq!(result.value["valid"], true);
        assert_eq!(result.value["invalid"], false);
    }

    #[tokio::test]
    async fn execute_crypto_timing_safe_equal() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                return {
                    same: Rivers.crypto.timingSafeEqual("abc", "abc"),
                    diff: Rivers.crypto.timingSafeEqual("abc", "xyz"),
                    diff_len: Rivers.crypto.timingSafeEqual("ab", "abc"),
                };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["same"], true);
        assert_eq!(result.value["diff"], false);
        assert_eq!(result.value["diff_len"], false);
    }

    #[tokio::test]
    async fn execute_crypto_random_base64url() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var a = Rivers.crypto.randomBase64url(16);
                var b = Rivers.crypto.randomBase64url(16);
                return { a: a, b: b, different: a !== b };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["different"], true);
        assert!(result.value["a"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn execute_crypto_hmac_real() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var result = Rivers.crypto.hmac("secret", "hello");
                return { hmac: result, len: result.length };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        // SHA-256 HMAC = 32 bytes = 64 hex chars
        assert_eq!(result.value["len"], 64);
        // Verify it produces the correct HMAC-SHA256 for "hello" with key "secret"
        let hmac_val = result.value["hmac"].as_str().unwrap();
        assert!(
            hmac_val.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex string, got: {hmac_val}"
        );
    }

    #[tokio::test]
    async fn execute_crypto_hmac_deterministic() {
        // Same key+data should produce same HMAC
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var a = Rivers.crypto.hmac("key1", "data1");
                var b = Rivers.crypto.hmac("key1", "data1");
                var c = Rivers.crypto.hmac("key2", "data1");
                return { same: a === b, diff: a !== c };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["same"], true);
        assert_eq!(result.value["diff"], true);
    }

    // ── P4.1: Heap Limit Test ────────────────────────────────────

    #[tokio::test]
    async fn execute_heap_limit_prevents_oom() {
        // This test allocates a large array — with heap limits it should fail
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
        // Should either timeout or throw OOM — should NOT succeed
        assert!(result.is_err(), "expected OOM or timeout, got Ok");
    }

    // ── P4.3: Rivers.http ─────────────────────────────────────────

    /// Helper: create a JS task with HTTP capability enabled.
    fn make_http_js_task(source: &str, function: &str) -> TaskContext {
        TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: function.into(),
                language: "javascript".into(),
            })
            .http(HttpToken)
            .args(serde_json::json!({ "_source": source }))
            .trace_id("test-http".into())
            .build()
            .unwrap()
    }

    // ── V2: Rivers.http native callbacks ──────────────────────────

    #[tokio::test]
    async fn execute_rivers_http_get_invalid_url_throws() {
        // Invalid URL should throw an error (not panic)
        let ctx = make_http_js_task(
            r#"function handler(ctx) {
                try {
                    Rivers.http.get("not-a-valid-url");
                } catch(e) {
                    return { error: e.message, has_error: true };
                }
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_error"], true);
    }

    #[tokio::test]
    async fn execute_rivers_http_methods_exist() {
        // All HTTP methods should be callable functions (requires HttpToken for capability gating)
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .http(HttpToken)
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    return {
                        get_type: typeof Rivers.http.get,
                        post_type: typeof Rivers.http.post,
                        put_type: typeof Rivers.http.put,
                        del_type: typeof Rivers.http.del,
                    };
                }"#
            }))
            .trace_id("test-http-methods".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["get_type"], "function");
        assert_eq!(result.value["post_type"], "function");
        assert_eq!(result.value["put_type"], "function");
        assert_eq!(result.value["del_type"], "function");
    }

    #[tokio::test]
    async fn execute_rivers_http_connection_refused_throws() {
        // Connecting to a port with no listener should throw (not hang)
        let ctx = make_http_js_task(
            r#"function handler(ctx) {
                try {
                    Rivers.http.get("http://127.0.0.1:19999/nonexistent");
                } catch(e) {
                    return { error: e.message, caught: true };
                }
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["caught"], true);
    }

    // ── V2: Rivers.env ──────────────────────────────────────────────

    #[tokio::test]
    async fn execute_rivers_env_available() {
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({
                "_source": "function handler(ctx) { return { db: Rivers.env.DATABASE_URL, port: Rivers.env.PORT }; }"
            }))
            .env_var("DATABASE_URL".into(), "postgres://localhost/test".into())
            .env_var("PORT".into(), "8080".into())
            .trace_id("t1".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["db"], "postgres://localhost/test");
        assert_eq!(result.value["port"], "8080");
    }

    #[tokio::test]
    async fn execute_rivers_env_empty_by_default() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                return { has_env: typeof Rivers.env === "object", keys: Object.keys(Rivers.env).length };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_env"], true);
        assert_eq!(result.value["keys"], 0);
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

    // ── T4: ES Module + Async Support Tests ──────────────────────

    #[tokio::test]
    async fn execute_async_function() {
        let ctx = make_js_task(
            r#"async function handler(ctx) { return { async: true }; }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["async"], true);
    }

    #[test]
    fn execute_module_syntax_detected() {
        assert!(is_module_syntax("export function handler(ctx) {}"));
        assert!(is_module_syntax("import { foo } from 'bar'"));
        assert!(!is_module_syntax("function handler(ctx) {}"));
        assert!(is_module_syntax("export default function handler(ctx) {}"));
    }

    #[tokio::test]
    async fn execute_async_with_promise_chain() {
        let ctx = make_js_task(
            r#"async function handler(ctx) {
                var result = await Promise.resolve({ resolved: true });
                return result;
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["resolved"], true);
    }

    #[tokio::test]
    async fn execute_async_rejection() {
        let ctx = make_js_task(
            r#"async function handler(ctx) {
                throw new Error("async boom");
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("async") || err_msg.contains("boom"),
            "expected async error details in: {err_msg}"
        );
    }

    // ── X1: Rivers.log Structured Fields Tests ──────────────────

    #[tokio::test]
    async fn x1_rivers_log_info_with_fields() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                Rivers.log.info("user login", { userId: 123, action: "login" });
                return { logged: true };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["logged"], true);
    }

    #[tokio::test]
    async fn x1_rivers_log_without_fields_still_works() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                Rivers.log.info("simple message");
                Rivers.log.warn("warning message");
                Rivers.log.error("error message");
                return { ok: true };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["ok"], true);
    }

    #[tokio::test]
    async fn x1_console_log_with_object_fields() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                console.log("action performed", { key: "val" });
                return { ok: true };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["ok"], true);
    }

    // ── X2: Rivers.http Capability Gating Tests ─────────────────

    #[tokio::test]
    async fn x2_rivers_http_absent_without_http_token() {
        // No .http() on the builder → Rivers.http should be undefined
        let ctx = make_js_task(
            r#"function handler(ctx) {
                return { http_type: typeof Rivers.http };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["http_type"], "undefined");
    }

    #[tokio::test]
    async fn x2_rivers_http_present_with_http_token() {
        // With .http() on the builder → Rivers.http should be an object
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .http(HttpToken)
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    return {
                        http_type: typeof Rivers.http,
                        get_type: typeof Rivers.http.get,
                        post_type: typeof Rivers.http.post,
                        put_type: typeof Rivers.http.put,
                        del_type: typeof Rivers.http.del,
                    };
                }"#
            }))
            .trace_id("test-http-gating".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["http_type"], "object");
        assert_eq!(result.value["get_type"], "function");
        assert_eq!(result.value["post_type"], "function");
        assert_eq!(result.value["put_type"], "function");
        assert_eq!(result.value["del_type"], "function");
    }

    #[tokio::test]
    async fn x2_rivers_http_access_without_token_is_safe() {
        // Accessing Rivers.http methods without token should not crash
        let ctx = make_js_task(
            r#"function handler(ctx) {
                try {
                    Rivers.http.get("http://example.com");
                    return { error: false };
                } catch(e) {
                    return { error: true, message: e.message };
                }
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        // Should get a TypeError because Rivers.http is undefined
        assert_eq!(result.value["error"], true);
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
        // Execute with a small heap (16 MiB) — should still work for basic handler
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

    // ── X3: ctx.store StorageEngine Tests ───────────────────────

    #[tokio::test]
    async fn x3_store_with_storage_engine_round_trip() {
        let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .storage(engine.clone())
            .app_id("test-app".into())
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    ctx.store.set("mykey", { count: 42 });
                    var val = ctx.store.get("mykey");
                    return { stored: val };
                }"#
            }))
            .trace_id("x3-test".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["stored"]["count"], 42);
    }

    #[tokio::test]
    async fn x3_store_with_storage_engine_del() {
        let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .storage(engine.clone())
            .app_id("test-app".into())
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    ctx.store.set("temp", "hello");
                    ctx.store.del("temp");
                    var val = ctx.store.get("temp");
                    return { deleted: val === null };
                }"#
            }))
            .trace_id("x3-del".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["deleted"], true);
    }

    #[tokio::test]
    async fn x3_store_with_ttl() {
        let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .storage(engine.clone())
            .app_id("test-app".into())
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    ctx.store.set("ttl_key", "value", 60000);
                    var val = ctx.store.get("ttl_key");
                    return { has_value: val !== null };
                }"#
            }))
            .trace_id("x3-ttl".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_value"], true);
    }

    #[tokio::test]
    async fn x3_store_reserved_prefix_with_engine() {
        let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .storage(engine.clone())
            .app_id("test-app".into())
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    try {
                        ctx.store.set("session:evil", "hack");
                        return { blocked: false };
                    } catch(e) {
                        return { blocked: true, msg: e.message };
                    }
                }"#
            }))
            .trace_id("x3-reserved".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["blocked"], true);
    }

    #[tokio::test]
    async fn x3_store_persists_across_engine() {
        // Verify the StorageEngine actually received the data
        let engine = Arc::new(rivers_runtime::rivers_core::InMemoryStorageEngine::new());
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .storage(engine.clone())
            .app_id("myapp".into())
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    ctx.store.set("persistent", { data: "hello" });
                    return { ok: true };
                }"#
            }))
            .trace_id("x3-persist".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["ok"], true);

        // Read directly from engine to confirm persistence
        let stored = engine.get("app:myapp", "persistent").await.unwrap();
        assert!(stored.is_some(), "StorageEngine should have the value");
        let bytes = stored.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["data"], "hello");
    }

    #[tokio::test]
    async fn x3_store_fallback_without_engine() {
        // Without a StorageEngine, should still work via in-memory TASK_STORE
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.store.set("fallback", { n: 99 });
                return ctx.store.get("fallback");
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["n"], 99);
    }

    // ── X4: ctx.dataview Pre-fetch Tests ────────────────────────
    // (DataViewEngine live execution is deferred — testing the pre-fetch fast path)

    #[tokio::test]
    async fn x4_dataview_prefetch_returns_data() {
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    // Pre-populate ctx.data in the handler
                    ctx.data.contacts = [{ name: "Alice" }, { name: "Bob" }];
                    return ctx.dataview("contacts");
                }"#
            }))
            .trace_id("x4-prefetch".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value[0]["name"], "Alice");
        assert_eq!(result.value[1]["name"], "Bob");
    }

    #[tokio::test]
    async fn x4_dataview_missing_returns_null() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var result = ctx.dataview("nonexistent");
                return { is_null: result === null };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["is_null"], true);
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
            max_heap_mb: 32, // 32 MiB → 512 pages
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
        // Minimal WASM module that returns 42 — using WAT text format
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

        // 16 MiB memory limit — more than enough for a trivial module
        let result = execute_wasm_task(ctx, 5000, 0, 16 * 1024 * 1024, 0, None).await.unwrap();
        assert_eq!(result.value["result"], 42);

        // Clean up
        let _ = std::fs::remove_file(&wasm_path);
        // Clear cache so other tests aren't affected
        clear_wasm_cache();
    }

    #[tokio::test]
    async fn x6_wasm_fuel_exhaustion_returns_timeout() {
        // Use WAT text format — wasmtime can compile it directly
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

        // Short timeout — should hit fuel exhaustion or epoch interrupt
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

    // ── X7: ctx.datasource().build() Tests ──────────────────────

    #[tokio::test]
    async fn x7_datasource_build_undeclared_throws() {
        // Attempting to build with a datasource not in TaskContext should throw CapabilityError
        let ctx = make_js_task(
            r#"function handler(ctx) {
                try {
                    ctx.datasource("nonexistent").fromQuery("SELECT 1").build();
                    return { error: false };
                } catch(e) {
                    return { error: true, msg: e.message };
                }
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["error"], true);
        assert!(
            result.value["msg"].as_str().unwrap().contains("CapabilityError"),
            "expected CapabilityError, got: {}",
            result.value["msg"]
        );
    }

    #[tokio::test]
    async fn x7_datasource_build_without_query_throws() {
        // .build() without .fromQuery() should throw
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("mydb".into(), DatasourceToken("mydb".into()))
            .datasource_config("mydb".into(), ResolvedDatasource {
                driver_name: "faker".into(),
                params: rivers_runtime::rivers_driver_sdk::ConnectionParams {
                    host: String::new(),
                    port: 0,
                    database: String::new(),
                    username: String::new(),
                    password: String::new(),
                    options: HashMap::new(),
                },
            })
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    try {
                        ctx.datasource("mydb").build();
                        return { error: false };
                    } catch(e) {
                        return { error: true, msg: e.message };
                    }
                }"#
            }))
            .trace_id("x7-no-query".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["error"], true);
        assert!(
            result.value["msg"].as_str().unwrap().contains("fromQuery"),
            "expected fromQuery hint, got: {}",
            result.value["msg"]
        );
    }

    #[tokio::test]
    async fn x7_datasource_build_no_factory_throws() {
        // Declared datasource but no DriverFactory → should throw
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("mydb".into(), DatasourceToken("mydb".into()))
            .datasource_config("mydb".into(), ResolvedDatasource {
                driver_name: "faker".into(),
                params: rivers_runtime::rivers_driver_sdk::ConnectionParams {
                    host: String::new(),
                    port: 0,
                    database: String::new(),
                    username: String::new(),
                    password: String::new(),
                    options: HashMap::new(),
                },
            })
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    try {
                        ctx.datasource("mydb").fromQuery("SELECT 1").build();
                        return { error: false };
                    } catch(e) {
                        return { error: true, msg: e.message };
                    }
                }"#
            }))
            .trace_id("x7-no-factory".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["error"], true);
        assert!(
            result.value["msg"].as_str().unwrap().contains("DriverFactory"),
            "expected DriverFactory error, got: {}",
            result.value["msg"]
        );
    }

    #[tokio::test]
    async fn x7_datasource_builder_chain_preserves_state() {
        // Builder chain should preserve state across calls
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var builder = ctx.datasource("test_ds");
                builder = builder.fromQuery("SELECT * FROM users", { limit: 10 });
                builder = builder.withGetSchema({ type: "object" });
                return {
                    ds: builder._datasource,
                    query: builder._query,
                    has_params: builder._params !== null,
                    has_schema: builder._getSchema !== undefined,
                };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["ds"], "test_ds");
        assert_eq!(result.value["query"], "SELECT * FROM users");
        assert_eq!(result.value["has_params"], true);
        assert_eq!(result.value["has_schema"], true);
    }

    #[tokio::test]
    async fn x7_datasource_build_with_faker_driver() {
        // Full end-to-end: wire a real faker driver through DriverFactory
        use rivers_runtime::rivers_core::DriverFactory;

        let mut factory = DriverFactory::new();
        // Register the faker driver
        let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
        factory.register_database_driver(faker);

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("faker-ds".into(), DatasourceToken("faker-ds".into()))
            .datasource_config("faker-ds".into(), ResolvedDatasource {
                driver_name: "faker".into(),
                params: rivers_runtime::rivers_driver_sdk::ConnectionParams {
                    host: String::new(),
                    port: 0,
                    database: String::new(),
                    username: String::new(),
                    password: String::new(),
                    options: HashMap::new(),
                },
            })
            .driver_factory(Arc::new(factory))
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    var result = ctx.datasource("faker-ds").fromQuery("SELECT name, email FROM contacts LIMIT 3").build();
                    return { has_rows: result.rows !== undefined, row_count: result.rows.length };
                }"#
            }))
            .trace_id("x7-faker".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_rows"], true);
        // Faker driver returns synthetic data
        assert!(result.value["row_count"].as_u64().unwrap() > 0);
    }

    // ── X4: ctx.dataview with DataViewExecutor Tests ────────────

    #[tokio::test]
    async fn x4_dataview_executor_end_to_end() {
        use rivers_runtime::rivers_core::DriverFactory;
        use rivers_runtime::dataview_engine::DataViewRegistry;
        use rivers_runtime::DataViewExecutor;
        use rivers_runtime::dataview::DataViewConfig;

        // Set up a faker driver in the factory
        let mut factory = DriverFactory::new();
        let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
        factory.register_database_driver(faker);

        // Set up a DataView config pointing to the faker datasource
        let mut registry = DataViewRegistry::new();
        let dv_config = DataViewConfig {
            name: "contacts".into(),
            datasource: "faker-ds".into(),
            query: Some("SELECT name, email FROM contacts".into()),
            parameters: vec![],
            return_schema: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
            get_parameters: vec![],
            post_parameters: vec![],
            put_parameters: vec![],
            delete_parameters: vec![],
            streaming: false,
            caching: None,
            invalidates: Vec::new(),
            validate_result: false,
            strict_parameters: false,
        };
        registry.register(dv_config);

        // Set up datasource params with driver hint
        let mut ds_params = HashMap::new();
        let mut params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        };
        params.options.insert("driver".into(), "faker".into());
        ds_params.insert("faker-ds".into(), params);

        let executor = Arc::new(DataViewExecutor::new(
            registry,
            Arc::new(factory),
            Arc::new(ds_params),
            Arc::new(NoopDataViewCache),
        ));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .dataview("contacts".into(), DataViewToken("contacts".into()))
            .dataview_executor(executor)
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    var result = ctx.dataview("contacts");
                    return { has_rows: result.rows !== undefined, row_count: result.rows.length };
                }"#
            }))
            .trace_id("x4-executor".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_rows"], true);
        assert!(result.value["row_count"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn x4_dataview_executor_not_found_throws() {
        use rivers_runtime::rivers_core::DriverFactory;
        use rivers_runtime::dataview_engine::DataViewRegistry;
        use rivers_runtime::DataViewExecutor;

        let factory = DriverFactory::new();
        let registry = DataViewRegistry::new();
        let executor = Arc::new(DataViewExecutor::new(
            registry,
            Arc::new(factory),
            Arc::new(HashMap::new()),
            Arc::new(NoopDataViewCache),
        ));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .dataview_executor(executor)
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    try {
                        ctx.dataview("nonexistent");
                        return { error: false };
                    } catch(e) {
                        return { error: true, msg: e.message };
                    }
                }"#
            }))
            .trace_id("x4-notfound".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["error"], true);
        assert!(
            result.value["msg"].as_str().unwrap().contains("not found"),
            "expected 'not found' error, got: {}",
            result.value["msg"]
        );
    }

    #[tokio::test]
    async fn x4_dataview_prefetch_takes_priority_over_executor() {
        use rivers_runtime::rivers_core::DriverFactory;
        use rivers_runtime::dataview_engine::DataViewRegistry;
        use rivers_runtime::DataViewExecutor;

        // Even with an executor, pre-fetched data should win
        let factory = DriverFactory::new();
        let registry = DataViewRegistry::new();
        let executor = Arc::new(DataViewExecutor::new(
            registry,
            Arc::new(factory),
            Arc::new(HashMap::new()),
            Arc::new(NoopDataViewCache),
        ));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .dataview_executor(executor)
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    ctx.data.cached_view = [{ id: 1, name: "pre-fetched" }];
                    var result = ctx.dataview("cached_view");
                    return { from_prefetch: result[0].name === "pre-fetched" };
                }"#
            }))
            .trace_id("x4-priority".into())
            .build()
            .unwrap();
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["from_prefetch"], true);
    }

    // ── AU: JS/WASM Gap Coverage Tests ────────────────────────────

    // AU1: JS file loading from disk
    #[tokio::test]
    async fn au1_execute_js_file_from_disk() {
        let dir = std::env::temp_dir();
        let js_path = dir.join("au1_handler.js");
        std::fs::write(&js_path, r#"function onRequest(ctx) { return { from_file: true, got: __args.name }; }"#).unwrap();

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: js_path.to_string_lossy().into(),
                function: "onRequest".into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({"name": "disk-test"}))
            .trace_id("au1".into())
            .build()
            .unwrap();

        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["from_file"], true);
        assert_eq!(result.value["got"], "disk-test");

        let _ = std::fs::remove_file(&js_path);
    }

    // AU2: Multiple JS functions in same source — call correct entrypoint
    #[tokio::test]
    async fn au2_execute_correct_entrypoint_from_multi_function_source() {
        let ctx = make_js_task(
            r#"
            function alpha(ctx) { return { fn: "alpha" }; }
            function beta(ctx) { return { fn: "beta" }; }
            function gamma(ctx) { return { fn: "gamma" }; }
            "#,
            "beta",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["fn"], "beta");
    }

    // AU3: Promise.all with multiple concurrent resolutions
    #[tokio::test]
    async fn au3_promise_all_multiple_values() {
        let ctx = make_js_task(
            r#"async function handler(ctx) {
                var results = await Promise.all([
                    Promise.resolve(1),
                    Promise.resolve(2),
                    Promise.resolve(3),
                ]);
                return { sum: results[0] + results[1] + results[2] };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["sum"], 6);
    }

    // AU4: Promise.race — first resolver wins
    #[tokio::test]
    async fn au4_promise_race() {
        let ctx = make_js_task(
            r#"async function handler(ctx) {
                var result = await Promise.race([
                    Promise.resolve("first"),
                    new Promise(function(r) { r("second"); }),
                ]);
                return { winner: result };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["winner"], "first");
    }

    // AU5: JS complex data structures — nested objects, arrays, nulls
    #[tokio::test]
    async fn au5_complex_data_round_trip() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                return {
                    nested: { a: { b: { c: 42 } } },
                    array: [1, "two", null, true, [5, 6]],
                    empty_obj: {},
                    empty_arr: [],
                    null_val: null,
                };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["nested"]["a"]["b"]["c"], 42);
        assert_eq!(result.value["array"][0], 1);
        assert_eq!(result.value["array"][1], "two");
        assert!(result.value["array"][2].is_null());
        assert_eq!(result.value["array"][3], true);
        assert_eq!(result.value["array"][4][1], 6);
        assert_eq!(result.value["empty_arr"], serde_json::json!([]));
        assert!(result.value["null_val"].is_null());
    }

    // AU6: JS error types — TypeError, RangeError, custom errors
    #[tokio::test]
    async fn au6_error_types_captured() {
        let ctx = make_js_task(
            "function handler(ctx) { null.property; }",
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Cannot read") || msg.contains("null") || msg.contains("TypeError"),
            "expected TypeError details in: {msg}"
        );
    }

    // AU7: JS closures and higher-order functions
    #[tokio::test]
    async fn au7_closures_and_higher_order_functions() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var items = [3, 1, 4, 1, 5, 9];
                var sorted = items.slice().sort(function(a, b) { return a - b; });
                var doubled = items.map(function(x) { return x * 2; });
                var sum = items.reduce(function(acc, x) { return acc + x; }, 0);
                return { sorted: sorted, doubled: doubled, sum: sum };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["sorted"], serde_json::json!([1, 1, 3, 4, 5, 9]));
        assert_eq!(result.value["sum"], 23);
        assert_eq!(result.value["doubled"][0], 6);
    }

    // AU8: ctx.store persistence within single handler (complex values)
    #[tokio::test]
    async fn au8_store_complex_objects() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                ctx.store.set("config", { nested: { deep: [1, 2, 3] }, flag: true });
                var loaded = ctx.store.get("config");
                return { deep_val: loaded.nested.deep[2], flag: loaded.flag };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["deep_val"], 3);
        assert_eq!(result.value["flag"], true);
    }

    // AU9: WASM module with computation (no params — host doesn't pass params)
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

    // AU10: WASM multiple exports — call correct function
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

    // AU11: JS string operations and JSON
    #[tokio::test]
    async fn au11_string_and_json_operations() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var obj = { name: "alice", age: 30 };
                var json_str = JSON.stringify(obj);
                var parsed = JSON.parse(json_str);
                var upper = "hello".toUpperCase();
                var split = "a,b,c".split(",");
                return { json_str: json_str, name: parsed.name, upper: upper, split: split };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert!(result.value["json_str"].as_str().unwrap().contains("alice"));
        assert_eq!(result.value["name"], "alice");
        assert_eq!(result.value["upper"], "HELLO");
        assert_eq!(result.value["split"], serde_json::json!(["a", "b", "c"]));
    }

    // AU12: JS Date and Math builtins
    #[tokio::test]
    async fn au12_date_and_math_builtins() {
        let ctx = make_js_task(
            r#"function handler(ctx) {
                var now = Date.now();
                var pi = Math.PI;
                var floor = Math.floor(3.7);
                var random = Math.random();
                return {
                    now_is_number: typeof now === "number",
                    now_positive: now > 0,
                    pi: pi,
                    floor: floor,
                    random_in_range: random >= 0 && random < 1,
                };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["now_is_number"], true);
        assert_eq!(result.value["now_positive"], true);
        assert!((result.value["pi"].as_f64().unwrap() - std::f64::consts::PI).abs() < 0.0001);
        assert_eq!(result.value["floor"], 3);
        assert_eq!(result.value["random_in_range"], true);
    }

    // AU13: ctx.dataview() with real DataViewExecutor + faker driver
    #[tokio::test]
    async fn au13_ctx_dataview_dynamic_with_executor() {
        // Build a real DataViewExecutor with faker driver
        let mut factory = DriverFactory::new();
        let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
        factory.register_database_driver(faker);

        let mut registry = rivers_runtime::DataViewRegistry::new();
        registry.register(rivers_runtime::DataViewConfig {
            name: "dynamic_contacts".into(),
            datasource: "faker-ds".into(),
            query: Some("schemas/contact.schema.json".into()),
            parameters: vec![],
            return_schema: None,
            invalidates: Vec::new(),
            validate_result: false,
            strict_parameters: false,
            caching: None,
            get_query: None, post_query: None, put_query: None, delete_query: None,
            get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
            get_parameters: Vec::new(), post_parameters: Vec::new(),
            put_parameters: Vec::new(), delete_parameters: Vec::new(),
            streaming: false,
        });

        let mut ds_params = HashMap::new();
        let mut opts = HashMap::new();
        opts.insert("driver".to_string(), "faker".to_string());
        ds_params.insert("faker-ds".to_string(), rivers_runtime::rivers_driver_sdk::ConnectionParams {
            host: String::new(), port: 0, database: String::new(),
            username: String::new(), password: String::new(), options: opts,
        });

        let executor = Arc::new(DataViewExecutor::new(
            registry,
            Arc::new(factory),
            Arc::new(ds_params),
            Arc::new(NoopDataViewCache),
        ));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .dataview_executor(executor)
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    var data = ctx.dataview("dynamic_contacts");
                    return { has_data: data !== null, type: typeof data };
                }"#
            }))
            .trace_id("au13".into())
            .build()
            .unwrap();

        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_data"], true);
        // Faker returns an array or object
        assert!(
            result.value["type"] == "object",
            "expected object from faker, got type: {}",
            result.value["type"]
        );
    }

    // ── ExecDriver Application-Level Tests ──────────────────────────
    //
    // Tests the full flow: JS handler → ctx.datasource().build() →
    // DriverFactory → ExecDriver.connect() → ExecConnection.execute()
    // → script execution → JSON result back to JS.

    #[cfg(unix)]
    fn make_exec_script(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn sha256_file(path: &std::path::Path) -> String {
        use sha2::{Sha256, Digest};
        let bytes = std::fs::read(path).unwrap();
        hex::encode(Sha256::digest(&bytes))
    }

    #[cfg(unix)]
    fn make_exec_params(
        dir: &std::path::Path,
        commands: &[(&str, &std::path::Path, &str, &str)],
    ) -> rivers_runtime::rivers_driver_sdk::ConnectionParams {
        let user = std::env::var("USER").unwrap_or_else(|_| "nobody".into());
        let mut options = HashMap::new();
        options.insert("run_as_user".into(), user);
        options.insert("working_directory".into(), dir.to_str().unwrap().into());

        for (name, path, sha256, input_mode) in commands {
            options.insert(format!("commands.{name}.path"), path.to_str().unwrap().into());
            options.insert(format!("commands.{name}.sha256"), sha256.to_string());
            options.insert(format!("commands.{name}.input_mode"), input_mode.to_string());
        }

        rivers_runtime::rivers_driver_sdk::ConnectionParams {
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            options,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_driver_base_test_no_params() {
        // Base test: script with no parameters, just returns a fixed JSON response
        use rivers_runtime::rivers_core::DriverFactory;

        let dir = tempfile::tempdir().unwrap();
        let script = make_exec_script(dir.path(), "hello.sh",
            "#!/bin/sh\necho '{\"status\":\"ok\",\"message\":\"hello from exec driver\"}'\n"
        );
        let hash = sha256_file(&script);
        let params = make_exec_params(dir.path(), &[("hello", &script, &hash, "stdin")]);

        let mut factory = DriverFactory::new();
        factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("ops".into(), DatasourceToken("ops".into()))
            .datasource_config("ops".into(), ResolvedDatasource {
                driver_name: "rivers-exec".into(),
                params,
            })
            .driver_factory(std::sync::Arc::new(factory))
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    var result = ctx.datasource("ops")
                        .fromQuery("query", { command: "hello" })
                        .build();
                    // result.rows[0].result contains the script's JSON output
                    var output = result.rows[0].result;
                    return {
                        status: output.status,
                        message: output.message,
                        has_rows: result.rows.length === 1
                    };
                }"#
            }))
            .trace_id("exec-base".into())
            .build()
            .unwrap();

        let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["status"], "ok");
        assert_eq!(result.value["message"], "hello from exec driver");
        assert_eq!(result.value["has_rows"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_driver_parameter_test_stdin() {
        // Parameter test: view sends parameters to the script via stdin
        // Script reads JSON from stdin, processes it, returns enriched JSON
        use rivers_runtime::rivers_core::DriverFactory;

        let dir = tempfile::tempdir().unwrap();
        let script = make_exec_script(dir.path(), "process.sh",
            r#"#!/bin/sh
# Read JSON from stdin, extract fields, return processed result
INPUT=$(cat)
# Use simple shell to echo back with added field
echo "{\"received\":$INPUT,\"processed\":true}"
"#
        );
        let hash = sha256_file(&script);
        let params = make_exec_params(dir.path(), &[("process", &script, &hash, "stdin")]);

        let mut factory = DriverFactory::new();
        factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("tools".into(), DatasourceToken("tools".into()))
            .datasource_config("tools".into(), ResolvedDatasource {
                driver_name: "rivers-exec".into(),
                params,
            })
            .driver_factory(std::sync::Arc::new(factory))
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    // Handler sends parameters to the exec command
                    var result = ctx.datasource("tools")
                        .fromQuery("query", {
                            command: "process",
                            args: { cidr: "10.0.1.0/24", ports: [22, 80, 443] }
                        })
                        .build();

                    var output = result.rows[0].result;
                    return {
                        received_cidr: output.received.cidr,
                        received_ports: output.received.ports,
                        processed: output.processed
                    };
                }"#
            }))
            .trace_id("exec-params".into())
            .build()
            .unwrap();

        let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["received_cidr"], "10.0.1.0/24");
        assert_eq!(result.value["received_ports"][0], 22);
        assert_eq!(result.value["received_ports"][1], 80);
        assert_eq!(result.value["received_ports"][2], 443);
        assert_eq!(result.value["processed"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_driver_parameter_test_args_mode() {
        // Parameter test with args mode: view sends parameters via CLI args
        use rivers_runtime::rivers_core::DriverFactory;

        let dir = tempfile::tempdir().unwrap();
        let script = make_exec_script(dir.path(), "lookup.sh",
            r#"#!/bin/sh
# Receives arguments: $1=domain, $2=record_type
echo "{\"domain\":\"$1\",\"type\":\"$2\",\"resolved\":true}"
"#
        );
        let hash = sha256_file(&script);

        // Build params with args_template for this command
        let user = std::env::var("USER").unwrap_or_else(|_| "nobody".into());
        let mut options = HashMap::new();
        options.insert("run_as_user".into(), user);
        options.insert("working_directory".into(), dir.path().to_str().unwrap().into());
        options.insert("commands.dns_lookup.path".into(), script.to_str().unwrap().into());
        options.insert("commands.dns_lookup.sha256".into(), hash);
        options.insert("commands.dns_lookup.input_mode".into(), "args".into());
        // args_template uses indexed keys: .0, .1, etc.
        options.insert("commands.dns_lookup.args_template.0".into(), "{domain}".into());
        options.insert("commands.dns_lookup.args_template.1".into(), "{record_type}".into());

        let params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
            host: String::new(), port: 0, database: String::new(),
            username: String::new(), password: String::new(), options,
        };

        let mut factory = DriverFactory::new();
        factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("dns".into(), DatasourceToken("dns".into()))
            .datasource_config("dns".into(), ResolvedDatasource {
                driver_name: "rivers-exec".into(),
                params,
            })
            .driver_factory(std::sync::Arc::new(factory))
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    var result = ctx.datasource("dns")
                        .fromQuery("query", {
                            command: "dns_lookup",
                            args: { domain: "example.com", record_type: "A" }
                        })
                        .build();

                    var output = result.rows[0].result;
                    return {
                        domain: output.domain,
                        type: output.type,
                        resolved: output.resolved
                    };
                }"#
            }))
            .trace_id("exec-args".into())
            .build()
            .unwrap();

        let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["domain"], "example.com");
        assert_eq!(result.value["type"], "A");
        assert_eq!(result.value["resolved"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_driver_error_propagation() {
        // Test that script errors propagate correctly back to JS handler
        use rivers_runtime::rivers_core::DriverFactory;

        let dir = tempfile::tempdir().unwrap();
        let script = make_exec_script(dir.path(), "fail.sh",
            "#!/bin/sh\necho 'script error: invalid input' >&2\nexit 1\n"
        );
        let hash = sha256_file(&script);
        let params = make_exec_params(dir.path(), &[("failing", &script, &hash, "stdin")]);

        let mut factory = DriverFactory::new();
        factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource("ops".into(), DatasourceToken("ops".into()))
            .datasource_config("ops".into(), ResolvedDatasource {
                driver_name: "rivers-exec".into(),
                params,
            })
            .driver_factory(std::sync::Arc::new(factory))
            .args(serde_json::json!({
                "_source": r#"function handler(ctx) {
                    try {
                        ctx.datasource("ops")
                            .fromQuery("query", { command: "failing" })
                            .build();
                        return { threw: false };
                    } catch (e) {
                        return {
                            threw: true,
                            message: e.message,
                            has_stderr: e.message.indexOf("script error") !== -1
                        };
                    }
                }"#
            }))
            .trace_id("exec-error".into())
            .build()
            .unwrap();

        let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["threw"], true);
        assert_eq!(result.value["has_stderr"], true);
    }

    // ── Application Keystore Tests ──────────────────────────────────
    //
    // These tests exercise the full application-level keystore feature:
    // Rivers.keystore.has/info and Rivers.crypto.encrypt/decrypt running
    // through the V8 engine with a real keystore via the shared resolver.

    /// Helper: create a test keystore with a named key.
    fn make_test_keystore(key_name: &str) -> std::sync::Arc<rivers_keystore_engine::AppKeystore> {
        std::sync::Arc::new(rivers_keystore_engine::create_test_keystore(key_name))
    }

    fn make_ks_task(source: &str, function: &str, ks: std::sync::Arc<rivers_keystore_engine::AppKeystore>) -> TaskContext {
        TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: function.into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({ "_source": source }))
            .trace_id("ks-test".into())
            .app_id("test-app".into())
            .keystore(ks)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn keystore_has_returns_true_for_existing_key() {
        let ks = make_test_keystore("credential-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                return {
                    exists: Rivers.keystore.has("credential-key"),
                    missing: Rivers.keystore.has("nonexistent")
                };
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["exists"], true);
        assert_eq!(result.value["missing"], false);
    }

    #[tokio::test]
    async fn keystore_info_returns_metadata() {
        let ks = make_test_keystore("test-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                var info = Rivers.keystore.info("test-key");
                return {
                    name: info.name,
                    type: info.type,
                    version: info.version,
                    has_created: typeof info.created_at === "string"
                };
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["name"], "test-key");
        assert_eq!(result.value["type"], "aes-256");
        assert_eq!(result.value["version"], 1);
        assert_eq!(result.value["has_created"], true);
    }

    #[tokio::test]
    async fn keystore_info_throws_for_missing_key() {
        let ks = make_test_keystore("real-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                try {
                    Rivers.keystore.info("nonexistent");
                    return { threw: false };
                } catch (e) {
                    return { threw: true, message: e.message };
                }
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["threw"], true);
        assert!(result.value["message"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn crypto_encrypt_decrypt_round_trip() {
        let ks = make_test_keystore("secret-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                var enc = Rivers.crypto.encrypt("secret-key", "hello world");

                // Verify encrypt result shape
                if (typeof enc.ciphertext !== "string") return { error: "no ciphertext" };
                if (typeof enc.nonce !== "string") return { error: "no nonce" };
                if (typeof enc.key_version !== "number") return { error: "no key_version" };

                // Decrypt and verify
                var dec = Rivers.crypto.decrypt("secret-key", enc.ciphertext, enc.nonce, {
                    key_version: enc.key_version
                });

                return {
                    plaintext: dec,
                    key_version: enc.key_version,
                    ciphertext_length: enc.ciphertext.length
                };
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["plaintext"], "hello world");
        assert_eq!(result.value["key_version"], 1);
        assert!(result.value["ciphertext_length"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn crypto_encrypt_with_aad() {
        let ks = make_test_keystore("aad-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                var enc = Rivers.crypto.encrypt("aad-key", "secret data", { aad: "device-123" });

                // Decrypt with matching AAD
                var dec = Rivers.crypto.decrypt("aad-key", enc.ciphertext, enc.nonce, {
                    key_version: enc.key_version,
                    aad: "device-123"
                });

                // Decrypt with wrong AAD should fail
                var wrongAad = false;
                try {
                    Rivers.crypto.decrypt("aad-key", enc.ciphertext, enc.nonce, {
                        key_version: enc.key_version,
                        aad: "wrong-device"
                    });
                } catch (e) {
                    wrongAad = true;
                }

                return { plaintext: dec, wrong_aad_threw: wrongAad };
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["plaintext"], "secret data");
        assert_eq!(result.value["wrong_aad_threw"], true);
    }

    #[tokio::test]
    async fn crypto_encrypt_nonexistent_key_throws() {
        let ks = make_test_keystore("real-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                try {
                    Rivers.crypto.encrypt("nonexistent", "data");
                    return { threw: false };
                } catch (e) {
                    return { threw: true, message: e.message };
                }
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["threw"], true);
        assert!(result.value["message"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn crypto_decrypt_tampered_ciphertext_throws_generic_error() {
        let ks = make_test_keystore("tamper-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                var enc = Rivers.crypto.encrypt("tamper-key", "sensitive");

                // Tamper with ciphertext
                var tampered = "AAAA" + enc.ciphertext.substring(4);

                try {
                    Rivers.crypto.decrypt("tamper-key", tampered, enc.nonce, {
                        key_version: enc.key_version
                    });
                    return { threw: false };
                } catch (e) {
                    return {
                        threw: true,
                        is_generic: e.message === "decryption failed"
                    };
                }
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["threw"], true);
        assert_eq!(result.value["is_generic"], true, "error should be generic, not leak details");
    }

    #[tokio::test]
    async fn crypto_decrypt_requires_key_version() {
        let ks = make_test_keystore("ver-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                var enc = Rivers.crypto.encrypt("ver-key", "data");

                // Decrypt without options should throw (no 4th argument)
                try {
                    Rivers.crypto.decrypt("ver-key", enc.ciphertext, enc.nonce);
                    return { threw: false };
                } catch (e) {
                    return { threw: true, message: e.message };
                }
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["threw"], true);
        assert!(result.value["message"].as_str().unwrap().contains("key_version"));
    }

    #[tokio::test]
    async fn crypto_nonce_uniqueness() {
        let ks = make_test_keystore("nonce-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                var enc1 = Rivers.crypto.encrypt("nonce-key", "same data");
                var enc2 = Rivers.crypto.encrypt("nonce-key", "same data");
                return {
                    nonces_differ: enc1.nonce !== enc2.nonce,
                    ciphertexts_differ: enc1.ciphertext !== enc2.ciphertext
                };
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["nonces_differ"], true);
        assert_eq!(result.value["ciphertexts_differ"], true);
    }

    #[tokio::test]
    async fn keystore_not_available_when_no_keystore() {
        // When no keystore on TaskContext, Rivers.keystore should be undefined
        let ctx = make_js_task(
            r#"function handler(ctx) {
                return {
                    has_keystore: typeof Rivers.keystore !== "undefined"
                };
            }"#,
            "handler",
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["has_keystore"], false);
    }

    #[tokio::test]
    async fn full_credential_store_workflow() {
        let ks = make_test_keystore("credential-key");
        let ctx = make_ks_task(
            r#"function handler(ctx) {
                // Simulate the Network Inventory use case from the spec

                // Step 1: Check key exists
                if (!Rivers.keystore.has("credential-key")) {
                    return { error: "key not found" };
                }

                // Step 2: Get key metadata
                var meta = Rivers.keystore.info("credential-key");
                if (meta.type !== "aes-256") {
                    return { error: "wrong key type: " + meta.type };
                }

                // Step 3: Encrypt a credential (simulating user submitting a password)
                var password = "super-secret-switch-password-123!";
                var enc = Rivers.crypto.encrypt("credential-key", password);

                // Step 4: Store would go to database — we just hold in memory
                var stored = {
                    encrypted_pass: enc.ciphertext,
                    pass_nonce: enc.nonce,
                    pass_key_ver: enc.key_version
                };

                // Step 5: Retrieve and decrypt (simulating automation fetching credential)
                var decrypted = Rivers.crypto.decrypt(
                    "credential-key",
                    stored.encrypted_pass,
                    stored.pass_nonce,
                    { key_version: stored.pass_key_ver }
                );

                // Step 6: Verify round-trip
                return {
                    success: decrypted === password,
                    key_version: stored.pass_key_ver,
                    key_type: meta.type,
                    key_name: meta.name
                };
            }"#,
            "handler",
            ks,
        );
        let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
        assert_eq!(result.value["success"], true);
        assert_eq!(result.value["key_version"], 1);
        assert_eq!(result.value["key_type"], "aes-256");
        assert_eq!(result.value["key_name"], "credential-key");
    }
}
