//! Rivers V8 Engine — JavaScript execution as a dynamic library.
//!
//! Loaded by `riversd` at startup from `lib/librivers_v8.dylib`.
//! Implements the C-ABI contract defined in `rivers-engine-sdk`.
//!
//! Contains: V8 initialization, isolate pool, script cache, ES module support,
//! handler injection (ctx object, Rivers globals), crypto, logging.
//!
//! Host callbacks (ctx.dataview, ctx.store, etc.) are provided by riversd
//! via the HostCallbacks function pointer table passed during init.

#![warn(missing_docs)]

mod task_context;
mod v8_runtime;
mod execution;

use rivers_engine_sdk::{ENGINE_ABI_VERSION, HostCallbacks, SerializedTaskContext};

use execution::{execute_js, write_error, write_output};
use v8_runtime::{ensure_v8_initialized, SCRIPT_CACHE};

// ── Host Callback Storage ───────────────────────────────────────

static HOST_CALLBACKS: std::sync::OnceLock<HostCallbacks> = std::sync::OnceLock::new();

// ── C-ABI Exports ───────────────────────────────────────────────

/// Return the engine ABI version for compatibility checks.
#[no_mangle]
pub extern "C" fn _rivers_engine_abi_version() -> u32 {
    ENGINE_ABI_VERSION
}

/// Initialize the V8 platform (idempotent). Returns 0 on success.
#[no_mangle]
pub extern "C" fn _rivers_engine_init() -> i32 {
    ensure_v8_initialized();
    0
}

/// Initialize V8 with host callback function pointers for dataview, store, and log.
#[no_mangle]
pub extern "C" fn _rivers_engine_init_with_callbacks(callbacks: *const HostCallbacks) -> i32 {
    if !callbacks.is_null() {
        let cb = unsafe { std::ptr::read(callbacks) };
        let _ = HOST_CALLBACKS.set(cb);
    }
    _rivers_engine_init()
}

/// Execute a JavaScript handler. Deserializes context, runs the script, writes result.
#[no_mangle]
pub extern "C" fn _rivers_engine_execute(
    ctx_ptr: *const u8,
    ctx_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    let ctx_slice = if ctx_ptr.is_null() || ctx_len == 0 {
        return write_error(out_ptr, out_len, "empty task context");
    } else {
        unsafe { std::slice::from_raw_parts(ctx_ptr, ctx_len) }
    };

    let ctx: SerializedTaskContext = match serde_json::from_slice(ctx_slice) {
        Ok(c) => c,
        Err(e) => return write_error(out_ptr, out_len, &format!("deserialize context: {e}")),
    };

    match execute_js(ctx) {
        Ok(result) => {
            match serde_json::to_vec(&result) {
                Ok(bytes) => {
                    write_output(out_ptr, out_len, &bytes);
                    0
                }
                Err(e) => write_error(out_ptr, out_len, &format!("serialize result: {e}")),
            }
        }
        Err(msg) => write_error(out_ptr, out_len, &msg),
    }
}

/// Shut down the engine and clear the script cache.
#[no_mangle]
pub extern "C" fn _rivers_engine_shutdown() {
    if let Ok(mut cache) = SCRIPT_CACHE.lock() {
        cache.clear();
    }
}

/// Cancel a running task (stub — full watchdog integration is Phase 5).
#[no_mangle]
pub extern "C" fn _rivers_engine_cancel(_task_id: usize) -> i32 {
    // V8 termination requires an IsolateHandle — not possible from outside
    // the executing thread in the current architecture. For now, rely on
    // timeout within execute_js. Full watchdog integration is Phase 5.
    0
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use rivers_engine_sdk::SerializedTaskResult;

    fn make_ctx(source: &str, function: &str) -> SerializedTaskContext {
        SerializedTaskContext {
            datasource_tokens: HashMap::new(),
            dataview_tokens: HashMap::new(),
            datasource_configs: HashMap::new(),
            http_enabled: false,
            env: HashMap::new(),
            entrypoint: rivers_engine_sdk::SerializedEntrypoint {
                module: "inline".into(),
                function: function.into(),
                language: "javascript".into(),
            },
            args: serde_json::json!({"_source": source}),
            trace_id: "test".into(),
            app_id: "test".into(),
            node_id: "node-0".into(),
            runtime_env: "dev".into(),
            storage_available: false,
            store_namespace: None,
            lockbox_available: false,
            keystore_available: false,
            inline_source: Some(source.into()),
            prefetched_data: HashMap::new(),
            libs: vec![],
        }
    }

    #[test]
    fn abi_version() {
        assert_eq!(_rivers_engine_abi_version(), ENGINE_ABI_VERSION);
    }

    #[test]
    fn init_succeeds() {
        assert_eq!(_rivers_engine_init(), 0);
    }

    #[test]
    fn execute_simple_return() {
        let ctx = make_ctx("function handler(ctx) { return { message: 'hello' }; }", "handler");
        let result = execute_js(ctx).unwrap();
        assert_eq!(result.value["message"], "hello");
    }

    #[test]
    fn execute_reads_args() {
        let mut ctx = make_ctx(
            "function handler(ctx) { return { got: __args.name }; }",
            "handler",
        );
        ctx.args = serde_json::json!({"_source": ctx.inline_source, "name": "alice"});
        let result = execute_js(ctx).unwrap();
        assert_eq!(result.value["got"], "alice");
    }

    #[test]
    fn execute_resdata() {
        let ctx = make_ctx(
            "function handler(ctx) { ctx.resdata = { count: 42 }; }",
            "handler",
        );
        let result = execute_js(ctx).unwrap();
        assert_eq!(result.value["count"], 42);
    }

    #[test]
    fn execute_error_captured() {
        let ctx = make_ctx("function handler(ctx) { throw new Error('boom'); }", "handler");
        let result = execute_js(ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("boom"));
    }

    #[test]
    fn execute_crypto_random_hex() {
        let ctx = make_ctx(
            "function handler(ctx) { return { hex: Rivers.crypto.randomHex(16) }; }",
            "handler",
        );
        let result = execute_js(ctx).unwrap();
        let hex = result.value["hex"].as_str().unwrap();
        assert_eq!(hex.len(), 32);
    }

    #[test]
    fn execute_store_crud() {
        let ctx = make_ctx(
            r#"function handler(ctx) {
                ctx.store.set("k", { val: 42 });
                var got = ctx.store.get("k");
                ctx.store.del("k");
                var after = ctx.store.get("k");
                return { got: got, after: after };
            }"#,
            "handler",
        );
        let result = execute_js(ctx).unwrap();
        assert_eq!(result.value["got"]["val"], 42);
        assert!(result.value["after"].is_null());
    }

    #[test]
    fn execute_dataview_returns_prefetched() {
        let mut ctx = make_ctx(
            "function handler(ctx) { return { result: ctx.dataview('users') }; }",
            "handler",
        );
        // Pre-fetch data keyed by DataView name
        ctx.prefetched_data.insert(
            "users".to_string(),
            serde_json::json!([{"id": 1, "name": "alice"}]),
        );
        let result = execute_js(ctx).unwrap();
        assert_eq!(result.value["result"][0]["name"], "alice");
    }

    #[test]
    fn execute_dataview_with_params_skips_prefetch() {
        let mut ctx = make_ctx(
            r#"function handler(ctx) {
                var prefetched = ctx.dataview('users');
                var dynamic = ctx.dataview('users', { id: 42 });
                return { prefetched_found: prefetched !== null, dynamic_is_null: dynamic === null };
            }"#,
            "handler",
        );
        ctx.prefetched_data.insert(
            "users".to_string(),
            serde_json::json!([{"id": 1}]),
        );
        let result = execute_js(ctx).unwrap();
        // Prefetched should return data (no params = use cache)
        assert_eq!(result.value["prefetched_found"], true);
        // Dynamic with params should return null (no host callback in unit tests)
        assert_eq!(result.value["dynamic_is_null"], true);
    }

    #[test]
    fn execute_timeout_kills_infinite_loop() {
        let mut ctx = make_ctx(
            "function handler(ctx) { while(true) {} }",
            "handler",
        );
        // Set a short timeout (100ms)
        ctx.args = serde_json::json!({"_source": ctx.inline_source, "_timeout_ms": 100});
        let start = std::time::Instant::now();
        let result = execute_js(ctx);
        let elapsed = start.elapsed().as_millis();
        assert!(result.is_err(), "infinite loop should be terminated");
        assert!(elapsed < 2000, "should terminate within 2s, took {}ms", elapsed);
    }

    #[test]
    fn c_abi_execute_round_trip() {
        let ctx = make_ctx("function handler(ctx) { return { v8: true }; }", "handler");
        let ctx_json = serde_json::to_vec(&ctx).unwrap();

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let result = _rivers_engine_execute(ctx_json.as_ptr(), ctx_json.len(), &mut out_ptr, &mut out_len);

        assert_eq!(result, 0);
        let result_bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
        let task_result: SerializedTaskResult = serde_json::from_slice(result_bytes).unwrap();
        assert_eq!(task_result.value["v8"], true);

        unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
    }
}
