//! `inject_ctx_object()`, `inject_ctx_methods()`, ctx.store and ctx.dataview callbacks.

use std::collections::HashMap;

use super::super::types::*;
use super::task_locals::*;
use super::init::v8_str;
use super::datasource::ctx_datasource_build_callback;
use super::http::json_to_v8;
use rivers_runtime::rivers_core::storage::Bytes;

/// Build the `ctx` global object from the task context.
///
/// Injects `ctx` with trace_id, request, session, data, resdata
/// and `__args` with the raw task arguments.
pub(super) fn inject_ctx_object(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    task: &TaskContext,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);

    // Build ctx JSON and parse into V8
    let ctx_json = serde_json::json!({
        "trace_id": task.trace_id,
        "app_id": task.app_id,
        "node_id": task.node_id,
        "env": task.runtime_env,
        "request": task.args.get("request").cloned().unwrap_or(serde_json::Value::Null),
        "session": task.args.get("session").cloned().unwrap_or(serde_json::Value::Null),
        "data": {},
        "resdata": null,
    });
    let ctx_val = json_to_v8(scope, &ctx_json)?;
    let ctx_key = v8::String::new(scope, "ctx")
        .ok_or_else(|| TaskError::Internal("failed to create 'ctx' key".into()))?;
    global.set(scope, ctx_key.into(), ctx_val);

    // Also register __args for guard handlers
    let args_val = json_to_v8(scope, &task.args)?;
    let args_key = v8::String::new(scope, "__args")
        .ok_or_else(|| TaskError::Internal("failed to create '__args' key".into()))?;
    global.set(scope, args_key.into(), args_val);

    Ok(())
}

/// Inject host function bindings on the `ctx` object (P3 -> V2).
///
/// V2 replaces the V1 error stubs with real native callbacks:
/// - `ctx.dataview(name, params)` -- native V8 callback that checks
///   pre-fetched `ctx.data[name]` first (handles 90% of use cases).
///   Falls back to null with a warning if not pre-fetched.
/// - `ctx.store` -- native V8 callbacks backed by `TASK_STORE` thread-local
///   (V2.4.4). Reserved prefix enforcement for session:/csrf:/cache:/raft:/rivers:.
/// - `ctx.streamDataview(name)` -- mock iterator over pre-fetched data (V2.3).
///   Returns an object with `.next()` implementing the iterator protocol.
/// - `ctx.datasource()` -- builder pattern stub (execution deferred to V3).
pub(super) fn inject_ctx_methods(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);
    let ctx_key = v8_str(scope, "ctx")?;
    let ctx_val = global
        .get(scope, ctx_key.into())
        .ok_or_else(|| TaskError::Internal("ctx not found on global".into()))?;
    let ctx_obj = v8::Local::<v8::Object>::try_from(ctx_val)
        .map_err(|_| TaskError::Internal("ctx is not an object".into()))?;

    // ctx.dataview() -- native V8 callback (P3.1 V2)
    let dataview_fn = v8::Function::new(scope, ctx_dataview_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.dataview".into()))?;
    let dv_key = v8_str(scope, "dataview")?;
    ctx_obj.set(scope, dv_key.into(), dataview_fn.into());

    // ctx.store -- native V8 callbacks with reserved prefix enforcement (V2.4.4)
    let store_obj = v8::Object::new(scope);

    let store_get_fn = v8::Function::new(scope, ctx_store_get_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.store.get".into()))?;
    let get_key = v8_str(scope, "get")?;
    store_obj.set(scope, get_key.into(), store_get_fn.into());

    let store_set_fn = v8::Function::new(scope, ctx_store_set_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.store.set".into()))?;
    let set_key = v8_str(scope, "set")?;
    store_obj.set(scope, set_key.into(), store_set_fn.into());

    let store_del_fn = v8::Function::new(scope, ctx_store_del_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.store.del".into()))?;
    let del_key = v8_str(scope, "del")?;
    store_obj.set(scope, del_key.into(), store_del_fn.into());

    let store_key_on_ctx = v8_str(scope, "store")?;
    ctx_obj.set(scope, store_key_on_ctx.into(), store_obj.into());

    // X7: __ds_build native callback for ctx.datasource().build()
    let ds_build_fn = v8::Function::new(scope, ctx_datasource_build_callback)
        .ok_or_else(|| TaskError::Internal("failed to create __ds_build".into()))?;
    let ds_build_key = v8_str(scope, "__ds_build")?;
    global.set(scope, ds_build_key.into(), ds_build_fn.into());

    // ctx.streamDataview, ctx.datasource via JS
    let js_methods = r#"
        // V2.3: ctx.streamDataview(name) -- mock iterator over pre-fetched data
        ctx.streamDataview = function(name) {
            // Get data from pre-fetched ctx.data
            var data = ctx.data[name];
            if (!data) {
                return { next: function() { return { done: true }; } };
            }
            // If it's an array, iterate element by element
            if (Array.isArray(data)) {
                var index = 0;
                return {
                    next: function() {
                        if (index < data.length) {
                            return { value: data[index++], done: false };
                        }
                        return { done: true };
                    }
                };
            }
            // Single value -- return once
            var returned = false;
            return {
                next: function() {
                    if (!returned) {
                        returned = true;
                        return { value: data, done: false };
                    }
                    return { done: true };
                }
            };
        };

        // X7: ctx.datasource() -- builder chain with native .build() execution
        ctx.datasource = function(name) {
            return {
                _datasource: name,
                _query: null,
                _params: null,
                _schema: null,
                fromQuery: function(sql, params) { this._query = sql; this._params = params || null; return this; },
                fromSchema: function(schema, params) { this._schema = schema; this._params = params || null; return this; },
                withGetSchema: function(s) { this._getSchema = s; return this; },
                withPostSchema: function(s) { this._postSchema = s; return this; },
                withPutSchema: function(s) { this._putSchema = s; return this; },
                withDeleteSchema: function(s) { this._deleteSchema = s; return this; },
                build: function() {
                    return __ds_build(this._datasource, this._query, this._params);
                }
            };
        };

        // P3.5: ctx.ws -- undefined by default (only set for WebSocket views)
    "#;

    let js_src = v8::String::new(scope, js_methods)
        .ok_or_else(|| TaskError::Internal("failed to create ctx methods source".into()))?;
    let script = v8::Script::compile(scope, js_src, None)
        .ok_or_else(|| TaskError::Internal("failed to compile ctx methods".into()))?;
    script
        .run(scope)
        .ok_or_else(|| TaskError::Internal("failed to run ctx methods".into()))?;

    Ok(())
}

// ── ctx.store Native V8 Callbacks (V2.4.4) ─────────────────────

/// Reserved key prefixes for the task store. Keys starting with these
/// prefixes are reserved for system use and rejected with an error.
const STORE_RESERVED_PREFIXES: &[&str] = &["session:", "csrf:", "cache:", "raft:", "rivers:"];

/// Check if a store key uses a reserved namespace prefix.
fn store_key_is_reserved(key: &str) -> bool {
    STORE_RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Native V8 callback for `ctx.store.get(key)`.
///
/// X3: If a StorageEngine is available, reads via async bridge with namespace.
/// Falls back to `TASK_STORE` in-memory HashMap if no engine is injected.
/// Throws if the key uses a reserved prefix.
///
/// V8 callback -- short constant strings, unwrap is safe.
fn ctx_store_get_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8::String::new(scope, &format!("ctx.store: key '{}' uses reserved namespace", key)).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // X3: Try real StorageEngine first
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        match get_rt_handle() {
            Ok(rt) => {
                match rt.block_on(engine.get(&namespace, &key)) {
                    Ok(Some(bytes)) => {
                        let json_str = String::from_utf8(bytes).unwrap_or_else(|_| "null".into());
                        let v8_str = v8::String::new(scope, &json_str).unwrap();
                        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                            rv.set(parsed);
                        } else {
                            rv.set(v8::null(scope).into());
                        }
                        return;
                    }
                    Ok(None) => {
                        rv.set(v8::null(scope).into());
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(target: "rivers.store", "StorageEngine get failed: {e}, falling back to in-memory");
                    }
                }
            }
            Err(_) => {
                tracing::warn!(target: "rivers.store", "no runtime handle for StorageEngine, falling back to in-memory");
            }
        }
    }

    // Fallback: in-memory TASK_STORE
    let value = TASK_STORE.with(|s| s.borrow().get(&key).cloned());
    match value {
        Some(v) => {
            let json_str = serde_json::to_string(&v).unwrap_or_else(|_| "null".into());
            let v8_str = v8::String::new(scope, &json_str).unwrap();
            if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                rv.set(parsed);
            } else {
                rv.set(v8::null(scope).into());
            }
        }
        None => rv.set(v8::null(scope).into()),
    }
}

/// Native V8 callback for `ctx.store.set(key, value, ttl?)`.
///
/// X3: If a StorageEngine is available, writes via async bridge with namespace and TTL.
/// Falls back to `TASK_STORE` in-memory HashMap if no engine is injected.
/// Throws if the key uses a reserved prefix.
///
/// V8 callback -- short constant strings, unwrap is safe.
fn ctx_store_set_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8::String::new(scope, &format!("ctx.store: key '{}' uses reserved namespace", key)).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    let value_v8 = args.get(1);
    let json_value = if value_v8.is_undefined() || value_v8.is_null() {
        serde_json::Value::Null
    } else {
        let json_str = v8::json::stringify(scope, value_v8)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_else(|| "null".into());
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null)
    };

    // X3: Extract optional TTL from third argument (milliseconds)
    let ttl_ms = {
        let ttl_v8 = args.get(2);
        if ttl_v8.is_undefined() || ttl_v8.is_null() {
            None
        } else {
            ttl_v8.number_value(scope).map(|n| n as u64)
        }
    };

    // X3: Try real StorageEngine first
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        if let Ok(rt) = get_rt_handle() {
            let bytes: Bytes = serde_json::to_vec(&json_value).unwrap_or_else(|_| b"null".to_vec());
            if let Err(e) = rt.block_on(engine.set(&namespace, &key, bytes, ttl_ms)) {
                tracing::warn!(target: "rivers.store", "StorageEngine set failed: {e}, falling back to in-memory");
            } else {
                // Also update in-memory store for same-task reads
                TASK_STORE.with(|s| s.borrow_mut().insert(key, json_value));
                return;
            }
        }
    }

    // Fallback: in-memory TASK_STORE
    TASK_STORE.with(|s| s.borrow_mut().insert(key, json_value));
}

/// Native V8 callback for `ctx.store.del(key)`.
///
/// X3: If a StorageEngine is available, deletes via async bridge with namespace.
/// Falls back to `TASK_STORE` in-memory HashMap if no engine is injected.
/// Throws if the key uses a reserved prefix.
///
/// V8 callback -- short constant strings, unwrap is safe.
fn ctx_store_del_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8::String::new(scope, &format!("ctx.store: key '{}' uses reserved namespace", key)).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // X3: Try real StorageEngine first
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        if let Ok(rt) = get_rt_handle() {
            if let Err(e) = rt.block_on(engine.delete(&namespace, &key)) {
                tracing::warn!(target: "rivers.store", "StorageEngine del failed: {e}, falling back to in-memory");
            } else {
                TASK_STORE.with(|s| s.borrow_mut().remove(&key));
                return;
            }
        }
    }

    // Fallback: in-memory TASK_STORE
    TASK_STORE.with(|s| s.borrow_mut().remove(&key));
}

/// Native V8 callback for `ctx.dataview(name, params)`.
///
/// X4: Checks `ctx.data[name]` for pre-fetched data first (fast path).
/// If not found, tries the DataViewExecutor via async bridge.
/// If no executor available, falls back to warn + null.
///
/// V8 callback -- short constant strings, unwrap is safe.
fn ctx_dataview_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);

    // Look up in pre-fetched ctx.data first (fast path -- handles 90% of use cases)
    let global = scope.get_current_context().global(scope);
    let ctx_key = v8::String::new(scope, "ctx").unwrap();
    if let Some(ctx_val) = global.get(scope, ctx_key.into()) {
        if let Ok(ctx_obj) = v8::Local::<v8::Object>::try_from(ctx_val) {
            let data_key = v8::String::new(scope, "data").unwrap();
            if let Some(data_val) = ctx_obj.get(scope, data_key.into()) {
                if let Ok(data_obj) = v8::Local::<v8::Object>::try_from(data_val) {
                    let name_key = v8::String::new(scope, &name).unwrap();
                    if let Some(cached) = data_obj.get(scope, name_key.into()) {
                        if !cached.is_undefined() && !cached.is_null() {
                            rv.set(cached);
                            return;
                        }
                    }
                }
            }
        }
    }

    // X4.2: Not in pre-fetched data -- try DataViewExecutor via async bridge
    let executor = TASK_DV_EXECUTOR.with(|e| e.borrow().clone());
    if let Some(exec) = executor {
        // X4.3: Extract optional params from second V8 argument
        let params_v8 = args.get(1);
        let query_params: HashMap<String, rivers_runtime::rivers_driver_sdk::types::QueryValue> =
            if params_v8.is_undefined() || params_v8.is_null() {
                HashMap::new()
            } else if let Some(json_str) = v8::json::stringify(scope, params_v8) {
                let json_string = json_str.to_rust_string_lossy(scope);
                match serde_json::from_str::<serde_json::Value>(&json_string) {
                    Ok(serde_json::Value::Object(map)) => {
                        map.into_iter()
                            .map(|(k, v)| (k, super::datasource::json_to_query_value(v)))
                            .collect()
                    }
                    _ => HashMap::new(),
                }
            } else {
                HashMap::new()
            };

        let trace_id = TASK_TRACE_ID.with(|t| t.borrow().clone()).unwrap_or_default();

        match get_rt_handle() {
            Ok(rt) => {
                match rt.block_on(exec.execute(&name, query_params, "GET", &trace_id)) {
                    Ok(response) => {
                        // Convert QueryResult rows to JSON
                        let json = serde_json::json!({
                            "rows": response.query_result.rows,
                            "affected_rows": response.query_result.affected_rows,
                            "last_insert_id": response.query_result.last_insert_id,
                        });
                        let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "null".into());
                        let v8_str = v8::String::new(scope, &json_str).unwrap();
                        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                            rv.set(parsed);
                        } else {
                            rv.set(v8::null(scope).into());
                        }
                        return;
                    }
                    Err(e) => {
                        let msg = v8::String::new(
                            scope,
                            &format!("ctx.dataview('{}') execution error: {e}", name),
                        ).unwrap();
                        let exception = v8::Exception::error(scope, msg);
                        scope.throw_exception(exception);
                        return;
                    }
                }
            }
            Err(_) => {
                tracing::warn!(target: "rivers.handler", "no runtime handle for DataViewExecutor");
            }
        }
    }

    // Fallback: no executor and not pre-fetched -- warn and return null
    tracing::warn!(
        target: "rivers.handler",
        "ctx.dataview('{}') not in pre-fetched data and no executor available. \
         Declare in view config: dataviews = [\"{}\"]",
        name, name
    );
    rv.set(v8::null(scope).into());
}
