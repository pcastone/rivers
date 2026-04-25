//! Rivers WASM Engine — Wasmtime-based WebAssembly execution as a dynamic library.
//!
//! Loaded by `riversd` at startup from `lib/librivers_wasm.dylib`.
//! Implements the C-ABI contract defined in `rivers-engine-sdk`.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use rivers_engine_sdk::{
    ENGINE_ABI_VERSION, HostCallbacks, SerializedTaskContext, SerializedTaskResult,
};

// ── WASM Module Cache ───────────────────────────────────────────

static WASM_MODULE_CACHE: std::sync::LazyLock<StdMutex<HashMap<String, wasmtime::Module>>> =
    std::sync::LazyLock::new(|| StdMutex::new(HashMap::new()));

static WASM_ENGINE: std::sync::LazyLock<Result<wasmtime::Engine, String>> =
    std::sync::LazyLock::new(|| {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        wasmtime::Engine::new(&config).map_err(|e| format!("wasmtime engine: {e}"))
    });

// ── Host Callback Storage ───────────────────────────────────────

static HOST_CALLBACKS: std::sync::OnceLock<HostCallbacks> = std::sync::OnceLock::new();

// ── C-ABI Exports ───────────────────────────────────────────────

/// Return the engine ABI version.
#[no_mangle]
pub extern "C" fn _rivers_engine_abi_version() -> u32 {
    ENGINE_ABI_VERSION
}

/// Initialize the engine. Called once at startup.
#[no_mangle]
pub extern "C" fn _rivers_engine_init() -> i32 {
    // Force lazy engine creation
    match WASM_ENGINE.as_ref() {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Initialize with host callbacks.
#[no_mangle]
pub extern "C" fn _rivers_engine_init_with_callbacks(callbacks: *const HostCallbacks) -> i32 {
    if !callbacks.is_null() {
        let cb = unsafe { std::ptr::read(callbacks) };
        let _ = HOST_CALLBACKS.set(cb);
    }
    _rivers_engine_init()
}

/// Execute a WASM task.
///
/// Input: JSON-serialized `SerializedTaskContext` as byte buffer.
/// Output: JSON-serialized `SerializedTaskResult` as byte buffer.
/// Returns 0 on success, non-zero on error (error message in output buffer).
#[no_mangle]
pub extern "C" fn _rivers_engine_execute(
    ctx_ptr: *const u8,
    ctx_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    // Deserialize task context
    let ctx_slice = if ctx_ptr.is_null() || ctx_len == 0 {
        return write_error(out_ptr, out_len, "empty task context");
    } else {
        unsafe { std::slice::from_raw_parts(ctx_ptr, ctx_len) }
    };

    let ctx: SerializedTaskContext = match serde_json::from_slice(ctx_slice) {
        Ok(c) => c,
        Err(e) => return write_error(out_ptr, out_len, &format!("deserialize context: {e}")),
    };

    // Execute
    match execute_wasm(ctx) {
        Ok(result) => {
            let result_bytes = match serde_json::to_vec(&result) {
                Ok(b) => b,
                Err(e) => return write_error(out_ptr, out_len, &format!("serialize result: {e}")),
            };
            write_output(out_ptr, out_len, &result_bytes);
            0
        }
        Err(msg) => write_error(out_ptr, out_len, &msg),
    }
}

/// Shutdown the engine. Called at process exit.
#[no_mangle]
pub extern "C" fn _rivers_engine_shutdown() {
    if let Ok(mut cache) = WASM_MODULE_CACHE.lock() {
        cache.clear();
    }
}

// ── Compile Check FFI ────────────────────────────────────────────

/// Validate WASM bytes and enumerate exports. No execution.
///
/// Returns a heap-allocated JSON string. Caller frees via `_rivers_free_string`.
#[no_mangle]
pub extern "C" fn _rivers_compile_check(
    source_ptr: *const u8,
    source_len: usize,
    _filename_ptr: *const u8,
    _filename_len: usize,
) -> *const std::ffi::c_char {
    let bytes = if source_ptr.is_null() || source_len == 0 {
        return alloc_json_string(r#"{"ok":false,"error":{"message":"empty WASM bytes"}}"#);
    } else {
        unsafe { std::slice::from_raw_parts(source_ptr, source_len) }
    };

    let engine = match WASM_ENGINE.as_ref() {
        Ok(e) => e,
        Err(e) => {
            return alloc_json_string(&format!(
                r#"{{"ok":false,"error":{{"message":"wasmtime engine unavailable: {}"}}}}"#,
                e.replace('"', "\\\"")
            ));
        }
    };

    // Validate + compile to enumerate exports
    match wasmtime::Module::new(engine, bytes) {
        Ok(module) => {
            let exports: Vec<String> = module
                .exports()
                .filter_map(|e| {
                    if matches!(e.ty(), wasmtime::ExternType::Func(_)) {
                        Some(e.name().to_string())
                    } else {
                        None
                    }
                })
                .collect();

            let exports_json: Vec<String> = exports.iter().map(|e| format!(r#""{}""#, e)).collect();
            alloc_json_string(&format!(r#"{{"ok":true,"exports":[{}]}}"#, exports_json.join(",")))
        }
        Err(e) => {
            let msg = format!("{e}").replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
            alloc_json_string(&format!(
                r#"{{"ok":false,"error":{{"message":"{}"}}}}"#,
                msg,
            ))
        }
    }
}

/// Free a string returned by `_rivers_compile_check`.
#[no_mangle]
pub extern "C" fn _rivers_free_string(ptr: *const std::ffi::c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(std::ffi::CString::from_raw(ptr as *mut std::ffi::c_char));
        }
    }
}

fn alloc_json_string(s: &str) -> *const std::ffi::c_char {
    match std::ffi::CString::new(s) {
        Ok(cstr) => cstr.into_raw() as *const std::ffi::c_char,
        Err(_) => {
            let clean = s.replace('\0', "");
            std::ffi::CString::new(clean)
                .unwrap_or_default()
                .into_raw() as *const std::ffi::c_char
        }
    }
}

/// Cancel a running task (epoch interrupt).
#[no_mangle]
pub extern "C" fn _rivers_engine_cancel(_task_id: usize) -> i32 {
    // Increment the engine epoch — all running WASM instances will be interrupted
    if let Ok(engine) = WASM_ENGINE.as_ref() {
        engine.increment_epoch();
        0
    } else {
        -1
    }
}

// ── Internal Execution ──────────────────────────────────────────

fn execute_wasm(ctx: SerializedTaskContext) -> Result<SerializedTaskResult, String> {
    let start = Instant::now();

    let engine = WASM_ENGINE
        .as_ref()
        .map_err(|e| e.clone())?
        .clone();

    // Check module cache
    let module = {
        WASM_MODULE_CACHE
            .lock()
            .ok()
            .and_then(|cache| cache.get(&ctx.entrypoint.module).cloned())
    };

    let module = match module {
        Some(m) => m,
        None => {
            // Check for inline source (testing)
            let wasm_bytes = if let Some(ref source) = ctx.inline_source {
                source.as_bytes().to_vec()
            } else {
                std::fs::read(&ctx.entrypoint.module).map_err(|e| {
                    format!("cannot read WASM module '{}': {e}", ctx.entrypoint.module)
                })?
            };

            let compiled = wasmtime::Module::new(&engine, &wasm_bytes)
                .map_err(|e| format!("WASM compilation failed: {e}"))?;

            if let Ok(mut cache) = WASM_MODULE_CACHE.lock() {
                cache.insert(ctx.entrypoint.module.clone(), compiled.clone());
            }

            compiled
        }
    };

    // Create store with memory limits
    let mut store_limits = wasmtime::StoreLimitsBuilder::new();
    // Default 16MB memory limit
    store_limits = store_limits.memory_size(16 * 1024 * 1024);
    let mut store = wasmtime::Store::new(&engine, store_limits.build());
    store.limiter(|limits| limits);

    // Fuel limit
    let fuel = 1_000_000u64;
    store.set_fuel(fuel).map_err(|e| format!("wasmtime fuel: {e}"))?;

    // Epoch deadline
    store.set_epoch_deadline(100);

    // Linker with host function bindings
    let mut linker = wasmtime::Linker::new(&engine);

    linker.func_wrap("rivers", "log_info", |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>, ptr: i32, len: i32| {
        if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
            let data = memory.data(&caller);
            if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
                let msg = String::from_utf8_lossy(slice);
                log_to_host(2, &msg);
            }
        }
    }).map_err(|e| format!("linker log_info: {e}"))?;

    linker.func_wrap("rivers", "log_warn", |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>, ptr: i32, len: i32| {
        if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
            let data = memory.data(&caller);
            if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
                let msg = String::from_utf8_lossy(slice);
                log_to_host(3, &msg);
            }
        }
    }).map_err(|e| format!("linker log_warn: {e}"))?;

    linker.func_wrap("rivers", "log_error", |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>, ptr: i32, len: i32| {
        if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
            let data = memory.data(&caller);
            if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
                let msg = String::from_utf8_lossy(slice);
                log_to_host(4, &msg);
            }
        }
    }).map_err(|e| format!("linker log_error: {e}"))?;

    // Instantiate
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| format!("WASM instantiation failed: {e}"))?;

    // Call entrypoint
    let func = instance
        .get_func(&mut store, &ctx.entrypoint.function)
        .ok_or_else(|| format!("WASM function '{}' not found", ctx.entrypoint.function))?;

    let mut results = vec![wasmtime::Val::I32(0)];
    if let Err(e) = func.call(&mut store, &[], &mut results) {
        let err_str = e.to_string();
        if err_str.contains("fuel") || err_str.contains("epoch") || err_str.contains("interrupt")
            || e.downcast_ref::<wasmtime::Trap>().is_some()
        {
            return Err(format!("WASM timeout (fuel/epoch exhausted)"));
        }
        return Err(format!("WASM execution error: {e}"));
    }

    let return_val = match results.first() {
        Some(wasmtime::Val::I32(n)) => serde_json::json!({ "result": n }),
        Some(wasmtime::Val::I64(n)) => serde_json::json!({ "result": n }),
        Some(wasmtime::Val::F64(n)) => serde_json::json!({ "result": n }),
        _ => serde_json::Value::Null,
    };

    Ok(SerializedTaskResult {
        value: return_val,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Log via host callbacks if available, otherwise use tracing directly.
fn log_to_host(level: u8, msg: &str) {
    if let Some(callbacks) = HOST_CALLBACKS.get() {
        if let Some(log_fn) = callbacks.log_message {
            log_fn(level, msg.as_ptr(), msg.len());
            return;
        }
    }
    // Fallback to direct tracing
    match level {
        0 => tracing::trace!(target: "rivers.wasm", "{}", msg),
        1 => tracing::debug!(target: "rivers.wasm", "{}", msg),
        2 => tracing::info!(target: "rivers.wasm", "{}", msg),
        3 => tracing::warn!(target: "rivers.wasm", "{}", msg),
        _ => tracing::error!(target: "rivers.wasm", "{}", msg),
    }
}

// ── Buffer Helpers ──────────────────────────────────────────────

fn write_output(out_ptr: *mut *mut u8, out_len: *mut usize, data: &[u8]) {
    if out_ptr.is_null() || out_len.is_null() {
        return;
    }
    let boxed = data.to_vec().into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    unsafe {
        *out_ptr = ptr;
        *out_len = len;
    }
}

fn write_error(out_ptr: *mut *mut u8, out_len: *mut usize, msg: &str) -> i32 {
    write_output(out_ptr, out_len, msg.as_bytes());
    -1
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(module: &str, function: &str) -> SerializedTaskContext {
        SerializedTaskContext {
            datasource_tokens: HashMap::new(),
            dataview_tokens: HashMap::new(),
            datasource_configs: HashMap::new(),
            http_enabled: false,
            env: HashMap::new(),
            entrypoint: rivers_engine_sdk::SerializedEntrypoint {
                module: module.into(),
                function: function.into(),
                language: "wasm".into(),
            },
            args: serde_json::json!({}),
            trace_id: "test".into(),
            app_id: "test".into(),
            node_id: "node-0".into(),
            runtime_env: "dev".into(),
            storage_available: false,
            store_namespace: None,
            lockbox_available: false,
            keystore_available: false,
            inline_source: None,
            prefetched_data: HashMap::new(),
            libs: vec![],
            task_kind: None,
        }
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(_rivers_engine_abi_version(), ENGINE_ABI_VERSION);
    }

    #[test]
    fn init_succeeds() {
        assert_eq!(_rivers_engine_init(), 0);
    }

    #[test]
    fn execute_simple_wasm() {
        let wat = r#"(module (func (export "handler") (result i32) (i32.const 42)))"#;
        let tmp = std::env::temp_dir().join("bb_wasm_test.wat");
        std::fs::write(&tmp, wat).unwrap();

        let ctx = make_ctx(tmp.to_str().unwrap(), "handler");
        let result = execute_wasm(ctx).unwrap();
        assert_eq!(result.value["result"], 42);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn execute_wasm_computation() {
        let wat = r#"(module (func (export "handler") (result i32) (i32.add (i32.const 17) (i32.const 25))))"#;
        let tmp = std::env::temp_dir().join("bb_wasm_add.wat");
        std::fs::write(&tmp, wat).unwrap();

        let ctx = make_ctx(tmp.to_str().unwrap(), "handler");
        let result = execute_wasm(ctx).unwrap();
        assert_eq!(result.value["result"], 42);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn execute_missing_module_errors() {
        let ctx = make_ctx("/nonexistent/module.wasm", "handler");
        let result = execute_wasm(ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot read WASM module"));
    }

    #[test]
    fn execute_missing_function_errors() {
        let wat = r#"(module (func (export "other") (result i32) (i32.const 1)))"#;
        let tmp = std::env::temp_dir().join("bb_wasm_nofunc.wat");
        std::fs::write(&tmp, wat).unwrap();

        let ctx = make_ctx(tmp.to_str().unwrap(), "nonexistent");
        let result = execute_wasm(ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn c_abi_execute_round_trip() {
        let wat = r#"(module (func (export "handler") (result i32) (i32.const 99)))"#;
        let tmp = std::env::temp_dir().join("bb_wasm_cabi.wat");
        std::fs::write(&tmp, wat).unwrap();

        let ctx = make_ctx(tmp.to_str().unwrap(), "handler");
        let ctx_json = serde_json::to_vec(&ctx).unwrap();

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let result = unsafe {
            _rivers_engine_execute(
                ctx_json.as_ptr(), ctx_json.len(),
                &mut out_ptr, &mut out_len,
            )
        };

        assert_eq!(result, 0, "C-ABI execute should return 0");
        assert!(!out_ptr.is_null());
        assert!(out_len > 0);

        let result_bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
        let task_result: SerializedTaskResult = serde_json::from_slice(result_bytes).unwrap();
        assert_eq!(task_result.value["result"], 99);

        // Free the output buffer
        unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
        let _ = std::fs::remove_file(&tmp);
    }
}
