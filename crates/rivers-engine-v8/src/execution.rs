//! Core V8 execution — script loading, handler invocation, and injected globals.
//!
//! Contains `execute_js()` plus all injected objects: ctx (with store, dataview),
//! Rivers global (log, crypto), and buffer helpers.

use std::time::Instant;

use rivers_engine_sdk::SerializedTaskContext;

use crate::task_context::{clear_task_locals, setup_task_locals, TASK_APP_ID, TASK_STORE};
use crate::v8_runtime::{
    acquire_isolate, release_isolate, v8_str, v8_to_json_value,
    DEFAULT_HEAP_LIMIT, SCRIPT_CACHE,
};
use crate::HOST_CALLBACKS;

// ── Core Execution ──────────────────────────────────────────────

/// Default handler execution timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// RAII guard that ensures the watchdog thread is cancelled and joined on all
/// exit paths (including early returns from compile errors, missing functions,
/// etc.) and resets the HEAP_OOM_TRIGGERED flag.
struct WatchdogGuard {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    watchdog: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WatchdogGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.watchdog.take() {
            let _ = handle.join();
        }
        crate::v8_runtime::HEAP_OOM_TRIGGERED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) fn execute_js(ctx: SerializedTaskContext) -> Result<rivers_engine_sdk::SerializedTaskResult, String> {
    let start = Instant::now();

    // Resolve timeout from args or use default
    let timeout_ms = ctx.args.get("_timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_MS);

    // Set up thread-locals
    setup_task_locals(&ctx);

    // Resolve source: inline (_source in args) or file
    let source = if let Some(ref inline) = ctx.inline_source {
        inline.clone()
    } else {
        // Check cache
        let cached = SCRIPT_CACHE.lock().ok()
            .and_then(|cache| cache.get(&ctx.entrypoint.module).cloned());
        match cached {
            Some(s) => s,
            None => {
                let content = std::fs::read_to_string(&ctx.entrypoint.module)
                    .map_err(|e| format!("cannot read module '{}': {e}", ctx.entrypoint.module))?;
                if let Ok(mut cache) = SCRIPT_CACHE.lock() {
                    cache.insert(ctx.entrypoint.module.clone(), content.clone());
                }
                content
            }
        }
    };

    // Acquire isolate
    let mut isolate = acquire_isolate(DEFAULT_HEAP_LIMIT);

    // Start cancellable watchdog thread — terminates execution after timeout.
    // The `cancelled` flag prevents the watchdog from terminating a recycled
    // isolate after the handler completes normally.
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog_cancelled = cancelled.clone();
    let isolate_handle = isolate.thread_safe_handle();
    let watchdog = std::thread::spawn(move || {
        // Sleep in small increments so we can check the cancel flag
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if watchdog_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return; // Handler finished — do not terminate
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !watchdog_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            isolate_handle.terminate_execution();
        }
    });
    let _watchdog_guard = WatchdogGuard { cancelled: cancelled.clone(), watchdog: Some(watchdog) };
    let result = {
        let handle_scope = &mut v8::HandleScope::new(&mut isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let scope = &mut v8::ContextScope::new(handle_scope, context);

        // Compile and run the source
        let v8_source = v8_str(scope, &source);
        let script = v8::Script::compile(scope, v8_source, None)
            .ok_or_else(|| "script compilation failed".to_string())?;
        script.run(scope).ok_or_else(|| "script execution failed".to_string())?;

        // Inject ctx object
        let ctx_obj = inject_ctx_object(scope, &ctx);
        let global = context.global(scope);
        let ctx_key = v8_str(scope, "ctx");
        global.set(scope, ctx_key.into(), ctx_obj.into());

        // Inject __args
        let args_json = serde_json::to_string(&ctx.args).unwrap_or_default();
        let args_src = format!("var __args = {};", args_json);
        let args_v8 = v8_str(scope, &args_src);
        if let Some(s) = v8::Script::compile(scope, args_v8, None) {
            s.run(scope);
        }

        // Inject Rivers global (log, crypto)
        inject_rivers_global(scope);

        // Call the entrypoint function
        let func_name = v8_str(scope, &ctx.entrypoint.function);
        let func_val = global.get(scope, func_name.into())
            .ok_or_else(|| format!("function '{}' not found", ctx.entrypoint.function))?;

        if !func_val.is_function() {
            return Err(format!("'{}' is not a function", ctx.entrypoint.function));
        }

        let func = v8::Local::<v8::Function>::try_from(func_val)
            .map_err(|_| format!("'{}' is not a function", ctx.entrypoint.function))?;

        let recv = global.into();
        let ctx_arg = ctx_obj.into();
        let tc = &mut v8::TryCatch::new(scope);

        let result = func.call(tc, recv, &[ctx_arg]);

        // Signal watchdog to stop (it will be joined by WatchdogGuard on drop)
        cancelled.store(true, std::sync::atomic::Ordering::Relaxed);

        if tc.has_terminated() {
            clear_task_locals();
            // Isolate is terminated — it will be dropped (not recycled) when
            // the early return unwinds the stack. This is correct: a terminated
            // isolate must not be reused.
            return Err(format!("handler execution timed out after {}ms", timeout_ms));
        }

        if tc.has_caught() {
            let exception = tc.exception().unwrap();
            let msg = exception.to_rust_string_lossy(tc);
            clear_task_locals();
            return Err(msg);
        }

        // Check for resdata on ctx
        let resdata_key = v8_str(tc, "resdata");
        let resdata_val = ctx_obj.get(tc, resdata_key.into());

        let return_value = if let Some(rd) = resdata_val {
            if !rd.is_undefined() && !rd.is_null() {
                v8_to_json_value(tc, rd)
            } else if let Some(rv) = result {
                if !rv.is_undefined() {
                    v8_to_json_value(tc, rv)
                } else {
                    serde_json::Value::Null
                }
            } else {
                serde_json::Value::Null
            }
        } else if let Some(rv) = result {
            if !rv.is_undefined() {
                v8_to_json_value(tc, rv)
            } else {
                serde_json::Value::Null
            }
        } else {
            serde_json::Value::Null
        };

        return_value
    };

    // OOM flag reset is handled by WatchdogGuard::drop on all exit paths.

    // Release isolate back to pool (only reached on successful execution —
    // terminated isolates are dropped by early-return unwinding)
    release_isolate(isolate);
    clear_task_locals();

    Ok(rivers_engine_sdk::SerializedTaskResult {
        value: result,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

// ── ctx Object Injection ────────────────────────────────────────

fn inject_ctx_object<'s>(
    scope: &mut v8::HandleScope<'s>,
    ctx: &SerializedTaskContext,
) -> v8::Local<'s, v8::Object> {
    let obj = v8::Object::new(scope);

    // ctx.trace_id
    let key = v8_str(scope, "trace_id");
    let val = v8_str(scope, &ctx.trace_id);
    obj.set(scope, key.into(), val.into());

    // ctx.app_id
    let key = v8_str(scope, "app_id");
    let val = v8_str(scope, &ctx.app_id);
    obj.set(scope, key.into(), val.into());

    // ctx.node_id
    let key = v8_str(scope, "node_id");
    let val = v8_str(scope, &ctx.node_id);
    obj.set(scope, key.into(), val.into());

    // ctx.env
    let key = v8_str(scope, "env");
    let val = v8_str(scope, &ctx.runtime_env);
    obj.set(scope, key.into(), val.into());

    // ctx.data (pre-fetched DataView results)
    let data_obj = v8::Object::new(scope);
    for (name, value) in &ctx.prefetched_data {
        let k = v8_str(scope, name);
        let json_str = serde_json::to_string(value).unwrap_or_default();
        let v_src = v8_str(scope, &json_str);
        if let Some(parsed) = v8::json::parse(scope, v_src) {
            data_obj.set(scope, k.into(), parsed);
        }
    }
    let data_key = v8_str(scope, "data");
    obj.set(scope, data_key.into(), data_obj.into());

    // ctx.resdata = null (handler sets this)
    let resdata_key = v8_str(scope, "resdata");
    let null_val = v8::null(scope);
    obj.set(scope, resdata_key.into(), null_val.into());

    // ctx.request (from args if present)
    if let Some(request) = ctx.args.get("request") {
        let req_json = serde_json::to_string(request).unwrap_or_default();
        let req_src = v8_str(scope, &req_json);
        if let Some(parsed) = v8::json::parse(scope, req_src) {
            let req_key = v8_str(scope, "request");
            obj.set(scope, req_key.into(), parsed);
        }
    }

    // ctx.session (from args if present — matches ProcessPool injection)
    if let Some(session) = ctx.args.get("session") {
        let sess_json = serde_json::to_string(session).unwrap_or_default();
        let sess_src = v8_str(scope, &sess_json);
        if let Some(parsed) = v8::json::parse(scope, sess_src) {
            let sess_key = v8_str(scope, "session");
            obj.set(scope, sess_key.into(), parsed);
        }
    }

    // ctx.ws (from args if present — WebSocket lifecycle hooks)
    if let Some(ws) = ctx.args.get("ws") {
        let ws_json = serde_json::to_string(ws).unwrap_or_default();
        let ws_src = v8_str(scope, &ws_json);
        if let Some(parsed) = v8::json::parse(scope, ws_src) {
            let ws_key = v8_str(scope, "ws");
            obj.set(scope, ws_key.into(), parsed);
        }
    }

    // ctx.store (in-memory get/set/del)
    inject_store_methods(scope, obj);

    // ctx.dataview() placeholder
    inject_dataview_method(scope, obj);

    // ctx.ddl(datasource, statement) — DDL execution via host callback
    inject_ddl_method(scope, obj);

    obj
}

// ── ctx.store Methods ───────────────────────────────────────────

fn inject_store_methods<'s>(scope: &mut v8::HandleScope<'s>, ctx_obj: v8::Local<'s, v8::Object>) {
    let store_obj = v8::Object::new(scope);

    // store.get
    let get_fn = v8::Function::new(scope, store_get_callback).unwrap();
    let get_key = v8_str(scope, "get");
    store_obj.set(scope, get_key.into(), get_fn.into());

    // store.set
    let set_fn = v8::Function::new(scope, store_set_callback).unwrap();
    let set_key = v8_str(scope, "set");
    store_obj.set(scope, set_key.into(), set_fn.into());

    // store.del
    let del_fn = v8::Function::new(scope, store_del_callback).unwrap();
    let del_key = v8_str(scope, "del");
    store_obj.set(scope, del_key.into(), del_fn.into());

    let key = v8_str(scope, "store");
    ctx_obj.set(scope, key.into(), store_obj.into());
}

fn store_get_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    // Try in-memory store first
    let value = TASK_STORE.with(|s| s.borrow().get(&key).cloned());
    match value {
        Some(val) => {
            let json_str = serde_json::to_string(&val).unwrap_or_default();
            let v8_str_val = v8_str(scope, &json_str);
            if let Some(parsed) = v8::json::parse(scope, v8_str_val) {
                rv.set(parsed);
            }
        }
        None => rv.set(v8::null(scope).into()),
    }
}

fn store_set_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    // Check reserved prefixes
    if key.starts_with("session:") || key.starts_with("csrf:") || key.starts_with("poll:")
        || key.starts_with("cache:") || key.starts_with("rivers:")
    {
        let msg = v8_str(scope, &format!("reserved key prefix in '{}'", key));
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    let val = v8_to_json_value(scope, args.get(1));
    TASK_STORE.with(|s| s.borrow_mut().insert(key, val));
}

fn store_del_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    TASK_STORE.with(|s| s.borrow_mut().remove(&key));
}

// ── ctx.dataview() Method ───────────────────────────────────────

fn inject_dataview_method<'s>(scope: &mut v8::HandleScope<'s>, ctx_obj: v8::Local<'s, v8::Object>) {
    let dv_fn = v8::Function::new(scope, dataview_callback).unwrap();
    let key = v8_str(scope, "dataview");
    ctx_obj.set(scope, key.into(), dv_fn.into());
}

fn dataview_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);

    // Extract parameters from second argument (if provided)
    let params = {
        let arg1 = args.get(1);
        if arg1.is_object() && !arg1.is_null_or_undefined() {
            v8_to_json_value(scope, arg1)
        } else {
            serde_json::Value::Null
        }
    };

    // Check ctx.data first (pre-fetched), but only when no params passed
    // (pre-fetched results are static; dynamic params require re-execution)
    if params.is_null() {
        let this = args.this();
        let data_key = v8_str(scope, "data");
        if let Some(data_obj) = this.get(scope, data_key.into()) {
            if data_obj.is_object() {
                let dv_key = v8_str(scope, &name);
                if let Some(val) = data_obj.to_object(scope).unwrap().get(scope, dv_key.into()) {
                    if !val.is_undefined() {
                        rv.set(val);
                        return;
                    }
                }
            }
        }
    }

    // Try host callback for dynamic execution
    if let Some(callbacks) = HOST_CALLBACKS.get() {
        if let Some(dv_fn) = callbacks.dataview_execute {
            let input = if params.is_null() {
                serde_json::json!({"name": name}).to_string()
            } else {
                serde_json::json!({"name": name, "params": params}).to_string()
            };
            let mut out_ptr: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let result = dv_fn(
                input.as_ptr(), input.len(),
                &mut out_ptr, &mut out_len,
            );
            if !out_ptr.is_null() && out_len > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
                let json_str = String::from_utf8_lossy(bytes);
                if result == 0 {
                    // Success — return the parsed result
                    let v8_val = v8_str(scope, &json_str);
                    if let Some(parsed) = v8::json::parse(scope, v8_val) {
                        rv.set(parsed);
                        unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
                        return;
                    }
                } else {
                    // Error — extract error message and throw JS exception
                    let err_msg = if let Ok(err_obj) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        err_obj["error"].as_str().unwrap_or(&json_str).to_string()
                    } else {
                        json_str.to_string()
                    };
                    unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
                    let msg = v8_str(scope, &err_msg);
                    let exception = v8::Exception::error(scope, msg);
                    scope.throw_exception(exception);
                    return;
                }
                unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
            } else if result < 0 {
                // Host callback returned error but no output buffer — throw generic error
                let msg = v8_str(scope, &format!(
                    "ctx.dataview('{}') failed (host error code {})", name, result
                ));
                let exception = v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
                return;
            }
        }
    }

    // DataView not found in pre-fetched data and no host callback resolved it —
    // throw a JS exception so handlers see a clear error instead of silent null.
    let msg = v8_str(scope, &format!(
        "ctx.dataview('{}') not found. Declare in view config: dataviews = [\"{}\"]",
        name, name
    ));
    let exception = v8::Exception::error(scope, msg);
    scope.throw_exception(exception);
}

// ── ctx.ddl() Method ───────────────────────────────────────────

fn inject_ddl_method<'s>(scope: &mut v8::HandleScope<'s>, ctx_obj: v8::Local<'s, v8::Object>) {
    let ddl_fn = v8::Function::new(scope, ddl_callback).unwrap();
    let key = v8_str(scope, "ddl");
    ctx_obj.set(scope, key.into(), ddl_fn.into());
}

/// `ctx.ddl(datasource, statement)` — execute a DDL statement via host callback.
///
/// Returns `{"ok": true}` on success, throws a JS exception on failure.
fn ddl_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let datasource = args.get(0).to_rust_string_lossy(scope);
    let statement = args.get(1).to_rust_string_lossy(scope);

    if datasource.is_empty() || statement.is_empty() {
        let msg = v8_str(scope, "ctx.ddl() requires (datasource, statement) arguments");
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // Get app_id from task-local
    let app_id = TASK_APP_ID.with(|a| {
        a.borrow().clone().unwrap_or_else(|| "unknown".to_string())
    });

    if let Some(callbacks) = HOST_CALLBACKS.get() {
        if let Some(ddl_fn) = callbacks.ddl_execute {
            let input = serde_json::json!({
                "datasource": datasource,
                "statement": statement,
                "app_id": app_id,
            }).to_string();

            let mut out_ptr: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let result = ddl_fn(
                input.as_ptr(), input.len(),
                &mut out_ptr, &mut out_len,
            );

            if !out_ptr.is_null() && out_len > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
                let json_str = String::from_utf8_lossy(bytes);
                if result == 0 {
                    // Success
                    let v8_val = v8_str(scope, &json_str);
                    if let Some(parsed) = v8::json::parse(scope, v8_val) {
                        rv.set(parsed);
                    }
                    unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
                    return;
                } else {
                    // Error — extract message and throw
                    let err_msg = if let Ok(err_obj) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        err_obj["error"].as_str().unwrap_or(&json_str).to_string()
                    } else {
                        json_str.to_string()
                    };
                    unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
                    let msg = v8_str(scope, &err_msg);
                    let exception = v8::Exception::error(scope, msg);
                    scope.throw_exception(exception);
                    return;
                }
            } else if result < 0 {
                let msg = v8_str(scope, &format!(
                    "ctx.ddl('{}', ...): host error code {}", datasource, result
                ));
                let exception = v8::Exception::error(scope, msg);
                scope.throw_exception(exception);
                return;
            }
        }
    }

    // No host callback available
    let msg = v8_str(scope, "ctx.ddl(): host callback not available (ddl_execute not registered)");
    let exception = v8::Exception::error(scope, msg);
    scope.throw_exception(exception);
}

// ── Rivers Global (log, crypto) ─────────────────────────────────

fn inject_rivers_global(scope: &mut v8::HandleScope) {
    let global = scope.get_current_context().global(scope);
    let rivers_obj = v8::Object::new(scope);

    // Rivers.log
    let log_obj = v8::Object::new(scope);
    for (name, level) in [("info", 2u8), ("warn", 3), ("error", 4)] {
        let func = v8::Function::builder(log_callback)
            .data(v8::Integer::new(scope, level as i32).into())
            .build(scope)
            .unwrap();
        let key = v8_str(scope, name);
        log_obj.set(scope, key.into(), func.into());
    }
    let log_key = v8_str(scope, "log");
    rivers_obj.set(scope, log_key.into(), log_obj.into());

    // Rivers.crypto
    let crypto_obj = v8::Object::new(scope);

    let random_hex_fn = v8::Function::new(scope, crypto_random_hex_callback).unwrap();
    let k = v8_str(scope, "randomHex");
    crypto_obj.set(scope, k.into(), random_hex_fn.into());

    let hash_pw_fn = v8::Function::new(scope, crypto_hash_password_callback).unwrap();
    let k = v8_str(scope, "hashPassword");
    crypto_obj.set(scope, k.into(), hash_pw_fn.into());

    let verify_pw_fn = v8::Function::new(scope, crypto_verify_password_callback).unwrap();
    let k = v8_str(scope, "verifyPassword");
    crypto_obj.set(scope, k.into(), verify_pw_fn.into());

    let hmac_fn = v8::Function::new(scope, crypto_hmac_callback).unwrap();
    let k = v8_str(scope, "hmac");
    crypto_obj.set(scope, k.into(), hmac_fn.into());

    let tse_fn = v8::Function::new(scope, crypto_timing_safe_equal_callback).unwrap();
    let k = v8_str(scope, "timingSafeEqual");
    crypto_obj.set(scope, k.into(), tse_fn.into());

    let rb64_fn = v8::Function::new(scope, crypto_random_base64url_callback).unwrap();
    let k = v8_str(scope, "randomBase64url");
    crypto_obj.set(scope, k.into(), rb64_fn.into());

    let crypto_key = v8_str(scope, "crypto");
    rivers_obj.set(scope, crypto_key.into(), crypto_obj.into());

    let rivers_key = v8_str(scope, "Rivers");
    global.set(scope, rivers_key.into(), rivers_obj.into());

    // console.log, console.warn, console.error
    let console_obj = v8::Object::new(scope);
    for (name, level) in [("log", 2i32), ("warn", 3), ("error", 4)] {
        let func = v8::Function::builder(log_callback)
            .data(v8::Integer::new(scope, level).into())
            .build(scope)
            .unwrap();
        let key = v8_str(scope, name);
        console_obj.set(scope, key.into(), func.into());
    }
    let console_key = v8_str(scope, "console");
    global.set(scope, console_key.into(), console_obj.into());
}

// ── Log Callback ────────────────────────────────────────────────

fn log_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let level = {
        let data = args.data();
        if data.is_int32() {
            data.int32_value(scope).unwrap_or(2) as u8
        } else {
            2u8
        }
    };

    let msg = if args.length() > 0 {
        args.get(0).to_rust_string_lossy(scope)
    } else {
        String::new()
    };

    if let Some(callbacks) = HOST_CALLBACKS.get() {
        if let Some(log_fn) = callbacks.log_message {
            log_fn(level, msg.as_ptr(), msg.len());
            return;
        }
    }

    match level {
        2 => tracing::info!(target: "rivers.js", "{}", msg),
        3 => tracing::warn!(target: "rivers.js", "{}", msg),
        4 => tracing::error!(target: "rivers.js", "{}", msg),
        _ => tracing::debug!(target: "rivers.js", "{}", msg),
    }
}

// ── Crypto Callbacks ────────────────────────────────────────────

fn crypto_random_hex_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let bytes = args.get(0).to_integer(scope).map(|i| i.value() as usize).unwrap_or(16);
    let mut buf = vec![0u8; bytes];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut buf);
    let hex = hex::encode(&buf);
    let val = v8_str(scope, &hex);
    rv.set(val.into());
}

fn crypto_hash_password_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let password = args.get(0).to_rust_string_lossy(scope);
    match bcrypt::hash(&password, 12) {
        Ok(hash) => {
            let val = v8_str(scope, &hash);
            rv.set(val.into());
        }
        Err(e) => {
            let msg = v8_str(scope, &format!("bcrypt error: {}", e));
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
        }
    }
}

fn crypto_verify_password_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let password = args.get(0).to_rust_string_lossy(scope);
    let hash = args.get(1).to_rust_string_lossy(scope);
    let valid = bcrypt::verify(&password, &hash).unwrap_or(false);
    rv.set(v8::Boolean::new(scope, valid).into());
}

fn crypto_hmac_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let key = args.get(0).to_rust_string_lossy(scope);
    let data = args.get(1).to_rust_string_lossy(scope);

    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(data.as_bytes());
    let result = hex::encode(mac.finalize().into_bytes());

    let val = v8_str(scope, &result);
    rv.set(val.into());
}

fn crypto_timing_safe_equal_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let a = args.get(0).to_rust_string_lossy(scope);
    let b = args.get(1).to_rust_string_lossy(scope);
    // Constant-time comparison — XOR accumulation, no short-circuit
    let equal = if a.len() != b.len() {
        false
    } else {
        let mut diff = 0u8;
        for (x, y) in a.as_bytes().iter().zip(b.as_bytes()) {
            diff |= x ^ y;
        }
        diff == 0
    };
    rv.set(v8::Boolean::new(scope, equal).into());
}

fn crypto_random_base64url_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let bytes = args.get(0).to_integer(scope).map(|i| i.value() as usize).unwrap_or(32);
    let mut buf = vec![0u8; bytes];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut buf);
    use base64::Engine;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&buf);
    let val = v8_str(scope, &encoded);
    rv.set(val.into());
}

// ── Buffer Helpers ──────────────────────────────────────────────

pub(crate) fn write_output(out_ptr: *mut *mut u8, out_len: *mut usize, data: &[u8]) {
    if out_ptr.is_null() || out_len.is_null() { return; }
    let boxed = data.to_vec().into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    unsafe { *out_ptr = ptr; *out_len = len; }
}

pub(crate) fn write_error(out_ptr: *mut *mut u8, out_len: *mut usize, msg: &str) -> i32 {
    write_output(out_ptr, out_len, msg.as_bytes());
    -1
}
