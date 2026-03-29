//! Core V8 execution — script loading, handler invocation, and injected globals.
//!
//! Contains `execute_js()` plus all injected objects: ctx (with store, dataview),
//! Rivers global (log, crypto), and buffer helpers.

use std::time::Instant;

use rivers_engine_sdk::SerializedTaskContext;

use crate::task_context::{clear_task_locals, setup_task_locals, TASK_STORE};
use crate::v8_runtime::{
    acquire_isolate, release_isolate, v8_str, v8_to_json_value,
    DEFAULT_HEAP_LIMIT, SCRIPT_CACHE,
};
use crate::HOST_CALLBACKS;

// ── Core Execution ──────────────────────────────────────────────

pub(crate) fn execute_js(ctx: SerializedTaskContext) -> Result<rivers_engine_sdk::SerializedTaskResult, String> {
    let start = Instant::now();

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

    // Release isolate back to pool
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

    // ctx.store (in-memory get/set/del)
    inject_store_methods(scope, obj);

    // ctx.dataview() placeholder
    inject_dataview_method(scope, obj);

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

    // Check ctx.data first (pre-fetched)
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

    // Try host callback for dynamic execution
    if let Some(callbacks) = HOST_CALLBACKS.get() {
        if let Some(dv_fn) = callbacks.dataview_execute {
            let input = serde_json::json!({"name": name}).to_string();
            let mut out_ptr: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let result = dv_fn(
                input.as_ptr(), input.len(),
                &mut out_ptr, &mut out_len,
            );
            if result == 0 && !out_ptr.is_null() && out_len > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
                let json_str = String::from_utf8_lossy(bytes);
                let v8_val = v8_str(scope, &json_str);
                if let Some(parsed) = v8::json::parse(scope, v8_val) {
                    rv.set(parsed);
                    unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
                    return;
                }
                unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
            }
        }
    }

    rv.set(v8::null(scope).into());
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

    // console.log
    let console_obj = v8::Object::new(scope);
    let console_log_fn = v8::Function::builder(log_callback)
        .data(v8::Integer::new(scope, 2).into())
        .build(scope)
        .unwrap();
    let cl_key = v8_str(scope, "log");
    console_obj.set(scope, cl_key.into(), console_log_fn.into());
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
    let equal = a.len() == b.len() && a.as_bytes().iter().zip(b.as_bytes()).all(|(x, y)| x == y);
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
