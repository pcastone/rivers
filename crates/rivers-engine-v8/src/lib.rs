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

mod task_context;
mod v8_runtime;
mod execution;

use rivers_engine_sdk::{ENGINE_ABI_VERSION, HostCallbacks, SerializedTaskContext};

use execution::{execute_js, write_error, write_output};
use v8_runtime::{ensure_v8_initialized, SCRIPT_CACHE};

// ── Host Callback Storage ───────────────────────────────────────

static HOST_CALLBACKS: std::sync::OnceLock<HostCallbacks> = std::sync::OnceLock::new();

// ── C-ABI Exports ───────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _rivers_engine_abi_version() -> u32 {
    ENGINE_ABI_VERSION
}

#[no_mangle]
pub extern "C" fn _rivers_engine_init() -> i32 {
    ensure_v8_initialized();
    0
}

#[no_mangle]
pub extern "C" fn _rivers_engine_init_with_callbacks(callbacks: *const HostCallbacks) -> i32 {
    if !callbacks.is_null() {
        let cb = unsafe { std::ptr::read(callbacks) };
        let _ = HOST_CALLBACKS.set(cb);
    }
    _rivers_engine_init()
}

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

#[no_mangle]
pub extern "C" fn _rivers_engine_shutdown() {
    if let Ok(mut cache) = SCRIPT_CACHE.lock() {
        cache.clear();
    }
}

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
