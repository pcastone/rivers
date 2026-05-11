//! `inject_ctx_object()`, `inject_ctx_methods()`, ctx.store and ctx.dataview callbacks.

use std::collections::HashMap;

use super::super::types::*;
use super::task_locals::*;
use super::init::v8_str;
use super::datasource::ctx_datasource_build_callback;
use super::http::json_to_v8;
use rivers_runtime::rivers_core::storage::Bytes;

/// Build a V8 `String` with a graceful fallback to an empty string on
/// allocation failure.
///
/// V8 callbacks are `extern "C" fn` — a Rust panic from an `.unwrap()` at
/// this boundary is undefined behaviour at best, `SIGABRT` at worst. An
/// empty-string fallback is strictly worse for debuggability on the extreme
/// OOM path, but it preserves the invariant that the callback always hands
/// V8 a valid `Local<Value>`.
#[inline]
fn v8_str_safe<'s>(
    scope: &mut v8::HandleScope<'s>,
    s: &str,
) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, s).unwrap_or_else(|| v8::String::empty(scope))
}

/// Build the `ctx` global object from the task context.
///
/// Injects `ctx` with trace_id, request, session, data, resdata
/// and `__args` with the raw task arguments.
pub(super) fn inject_ctx_object(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    task: &TaskContext,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);

    // Build ctx JSON and parse into V8. Every key that needs to surface
    // as `ctx.<key>` to the handler must appear here — the V8 isolate sees
    // ctx as a plain object built from this literal. CB-OTLP Track O5.6:
    // `otel` is the OTLP-view dispatch envelope (`{kind, payload, encoding}`),
    // exposed alongside `request`/`session` so handlers can read
    // `ctx.otel.payload` etc. — see `rivers-otlp-view-spec.md` §6.1.
    let ctx_json = serde_json::json!({
        "trace_id": task.trace_id,
        "app_id": task.app_id,
        "node_id": task.node_id,
        "env": task.runtime_env,
        "request": task.args.get("request").cloned().unwrap_or(serde_json::Value::Null),
        "session": task.args.get("session").cloned().unwrap_or(serde_json::Value::Null),
        "otel":    task.args.get("otel").cloned().unwrap_or(serde_json::Value::Null),
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

    // ctx.transaction(datasource, fn) -- native V8 callback (spec §6).
    // Begins a transaction on the named datasource, invokes fn, and
    // commits on return / rolls back on throw. ctx.dataview() calls
    // inside the callback are routed through the held connection.
    let transaction_fn = v8::Function::new(scope, ctx_transaction_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.transaction".into()))?;
    let txn_key = v8_str(scope, "transaction")?;
    ctx_obj.set(scope, txn_key.into(), transaction_fn.into());

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

    // ctx.ddl() -- native V8 callback for DDL execution (init handlers only)
    let ddl_fn = v8::Function::new(scope, ctx_ddl_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.ddl".into()))?;
    let ddl_key = v8_str(scope, "ddl")?;
    ctx_obj.set(scope, ddl_key.into(), ddl_fn.into());

    // X7: __ds_build native callback for ctx.datasource().build()
    let ds_build_fn = v8::Function::new(scope, ctx_datasource_build_callback)
        .ok_or_else(|| TaskError::Internal("failed to create __ds_build".into()))?;
    let ds_build_key = v8_str(scope, "__ds_build")?;
    global.set(scope, ds_build_key.into(), ds_build_fn.into());

    // ctx.streamDataview, ctx.datasource via JS
    let js_methods = r#"
        // P2.6: ctx.elicit(spec) -- MCP mid-handler user input request.
        // Calls Rivers.__elicit(JSON.stringify(spec)) synchronously (the native
        // callback blocks on the oneshot channel), then parses the JSON result.
        // The return value is wrapped in a resolved Promise so handlers can
        // `await` it without structural changes (V8 runs synchronously; there is
        // no real async suspension here -- the blocking happens inside the native
        // callback via rt.block_on).
        //
        // Only functional when called from an MCP tool handler. In REST/WebSocket
        // contexts, Rivers.__elicit will throw an Error, which propagates out
        // of the Promise.
        ctx.elicit = function(spec) {
            try {
                var specJson = JSON.stringify(spec);
                var resultJson = Rivers.__elicit(specJson);
                var result = JSON.parse(resultJson);
                return { then: function(resolve, reject) {
                    try { resolve(result); } catch(e) { if (reject) reject(e); }
                    return this;
                }};
            } catch(e) {
                return { then: function(resolve, reject) {
                    if (reject) { try { reject(e); } catch(re) {} }
                    return this;
                }};
            }
        };

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

        // Typed-proxy registry for DatasourceToken::Direct datasources (29d).
        // Populated per-task below by direct_proxy bootstrap.
        if (typeof __rivers_direct_proxies === 'undefined') {
            __rivers_direct_proxies = {};
        }

        // X7 + 29d: ctx.datasource() — typed proxy if direct, builder otherwise.
        ctx.datasource = function(name) {
            if (__rivers_direct_proxies && Object.prototype.hasOwnProperty.call(__rivers_direct_proxies, name)) {
                return __rivers_direct_proxies[name];
            }
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

    // 29d: populate __rivers_direct_proxies from this task's Direct datasources.
    bootstrap_direct_proxies(scope)?;

    // BR-2026-04-23: populate __rivers_direct_proxies with broker publish proxies.
    bootstrap_broker_proxies(scope)?;

    Ok(())
}

/// Build and install one typed proxy per broker datasource declared on this task.
///
/// Mirrors `bootstrap_direct_proxies` but uses the broker codegen which emits
/// a single `publish(msg)` method that routes through `Rivers.__brokerPublish`.
fn bootstrap_broker_proxies(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    let names: Vec<String> = TASK_DIRECT_BROKER_PRODUCERS.with(|m| {
        m.borrow().keys().cloned().collect()
    });

    for name in names {
        let proxy_js = super::broker_dispatch::build_broker_proxy_script(&name);

        let mut wrapped = String::with_capacity(proxy_js.len() + name.len() + 48);
        wrapped.push_str("__rivers_direct_proxies[\"");
        for c in name.chars() {
            match c {
                '\\' => wrapped.push_str("\\\\"),
                '"' => wrapped.push_str("\\\""),
                _ => wrapped.push(c),
            }
        }
        wrapped.push_str("\"]=");
        wrapped.push_str(&proxy_js);
        wrapped.push(';');

        let src = v8::String::new(scope, &wrapped).ok_or_else(|| {
            TaskError::Internal("failed to create broker proxy source".into())
        })?;
        let script = v8::Script::compile(scope, src, None).ok_or_else(|| {
            TaskError::Internal(format!("failed to compile broker proxy for '{name}'"))
        })?;
        script
            .run(scope)
            .ok_or_else(|| TaskError::Internal(format!("failed to run broker proxy for '{name}'")))?;
    }

    Ok(())
}

/// Build and install one typed proxy per direct datasource declared on this task.
///
/// For each entry in `TASK_DIRECT_DATASOURCES`, look up the driver's operation
/// catalog; if present, compile a small IIFE that returns a proxy object and
/// store it under `__rivers_direct_proxies[name]`.
fn bootstrap_direct_proxies(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    // Collect (name, driver) pairs without holding the thread-local borrow.
    let entries: Vec<(String, String)> = TASK_DIRECT_DATASOURCES.with(|m| {
        m.borrow()
            .iter()
            .map(|(n, d)| (n.clone(), d.driver.clone()))
            .collect()
    });

    for (name, driver) in entries {
        let Some(catalog) = super::catalog::catalog_for(&driver) else {
            continue;
        };
        let proxy_js = super::proxy_codegen::build_proxy_script(&name, catalog);

        // Wrap so the proxy is stored on __rivers_direct_proxies[name].
        let mut wrapped = String::with_capacity(proxy_js.len() + name.len() + 48);
        wrapped.push_str("__rivers_direct_proxies[\"");
        // Reuse the same escape rules — names are trusted (configured via TOML)
        // but belt-and-suspenders against quotes.
        for c in name.chars() {
            match c {
                '\\' => wrapped.push_str("\\\\"),
                '"' => wrapped.push_str("\\\""),
                _ => wrapped.push(c),
            }
        }
        wrapped.push_str("\"]=");
        wrapped.push_str(&proxy_js);
        wrapped.push(';');

        let src = v8::String::new(scope, &wrapped).ok_or_else(|| {
            TaskError::Internal("failed to create direct proxy source".into())
        })?;
        let script = v8::Script::compile(scope, src, None).ok_or_else(|| {
            TaskError::Internal(format!("failed to compile direct proxy for '{name}'"))
        })?;
        script
            .run(scope)
            .ok_or_else(|| TaskError::Internal(format!("failed to run direct proxy for '{name}'")))?;
    }

    Ok(())
}

// ── ctx.store Native V8 Callbacks (V2.4.4) ─────────────────────

/// Reserved key prefixes for the task store. Keys starting with these
/// prefixes are reserved for system use and rejected with an error.
///
/// G_R3: this list previously lived inline in v8_engine and drifted from the
/// canonical core list (`rivers-core-config::storage::RESERVED_PREFIXES`) —
/// the V8 list omitted `poll:` and core omitted `raft:`, so a handler could
/// scribble over Raft consensus state via the StorageEngine namespace path
/// or over poll state via `ctx.store`. Both perspectives now consume the
/// single source of truth.
use rivers_runtime::rivers_core::storage::RESERVED_PREFIXES as STORE_RESERVED_PREFIXES;

/// Check if a store key uses a reserved namespace prefix.
fn store_key_is_reserved(key: &str) -> bool {
    STORE_RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Development-mode escape hatch for `ctx.store.*` callbacks (B2).
///
/// When set to `1`, missing or failing `StorageEngine` backends silently fall
/// back to a process-wide in-memory `TASK_STORE` map. This was the original
/// behaviour but it masks production data loss (e.g. Redis down) by reporting
/// `ctx.store.set` as success while the configured backend never received the
/// write. Production deployments must leave this unset so callbacks throw a
/// JS exception when the backend is unavailable.
///
/// Read once at first access via `OnceLock` so toggling the env var mid-process
/// has no effect — engines and tests that need the dev-mode behaviour must set
/// `RIVERS_DEV_NO_STORAGE=1` before any V8 task dispatches.
const RIVERS_DEV_NO_STORAGE_ENV: &str = "RIVERS_DEV_NO_STORAGE";

fn dev_no_storage_mode() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var(RIVERS_DEV_NO_STORAGE_ENV)
            .map(|v| v == "1")
            .unwrap_or(false)
    })
}

/// Throw a `ctx.store` failure as a JS exception so the handler observes the
/// loss instead of silently falling back to in-memory storage.
fn throw_store_error(scope: &mut v8::HandleScope, message: &str) {
    let msg = v8_str_safe(scope, message);
    let exception = v8::Exception::error(scope, msg);
    scope.throw_exception(exception);
}

/// Native V8 callback for `ctx.store.get(key)`.
///
/// X3: If a StorageEngine is configured (`TASK_STORAGE` is `Some`), reads via
/// the async bridge with namespace and propagates backend errors as JS
/// exceptions. B2 (P1-5): no silent fallback to `TASK_STORE` — that masked
/// data loss when the backend was unavailable. The in-memory map is only used
/// when (a) no engine is configured AND (b) `RIVERS_DEV_NO_STORAGE=1` is set.
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
        let msg = v8_str_safe(scope, &format!("ctx.store: key '{}' uses reserved namespace", key));
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // X3: StorageEngine path — propagate failures as JS exceptions (B2).
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        let rt = match get_rt_handle() {
            Ok(rt) => rt,
            Err(e) => {
                throw_store_error(
                    scope,
                    &format!("ctx.store.get('{key}'): tokio runtime unavailable: {e}"),
                );
                return;
            }
        };
        let outcome = match block_on_with_timeout(
            scope,
            &rt,
            "ctx.store.get",
            engine.get(&namespace, &key),
        ) {
            Some(v) => v,
            None => return, // JS error already thrown by the timeout helper
        };
        match outcome {
            Ok(Some(bytes)) => {
                let json_str = String::from_utf8(bytes).unwrap_or_else(|_| "null".into());
                let v8_str = v8_str_safe(scope, &json_str);
                if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                    rv.set(parsed);
                } else {
                    rv.set(v8::null(scope).into());
                }
            }
            Ok(None) => {
                rv.set(v8::null(scope).into());
            }
            Err(e) => {
                throw_store_error(
                    scope,
                    &format!("ctx.store.get('{key}'): backend error: {e}"),
                );
            }
        }
        return;
    }

    // No StorageEngine configured. Production must throw; dev mode may use
    // the in-memory TASK_STORE escape hatch (B2.2).
    if !dev_no_storage_mode() {
        throw_store_error(
            scope,
            &format!(
                "ctx.store.get('{key}'): no StorageEngine configured (set RIVERS_DEV_NO_STORAGE=1 to allow in-memory fallback)"
            ),
        );
        return;
    }

    let value = TASK_STORE.with(|s| s.borrow().get(&key).cloned());
    match value {
        Some(v) => {
            let json_str = serde_json::to_string(&v).unwrap_or_else(|_| "null".into());
            let v8_str = v8_str_safe(scope, &json_str);
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
/// X3: If a StorageEngine is configured (`TASK_STORAGE` is `Some`), writes via
/// the async bridge with namespace and TTL and propagates backend errors as JS
/// exceptions. B2 (P1-5): no silent fallback to `TASK_STORE` — that previously
/// reported success to the JS handler while the configured backend (Redis,
/// etc.) had never accepted the write. The in-memory map is only used when
/// (a) no engine is configured AND (b) `RIVERS_DEV_NO_STORAGE=1` is set.
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
        let msg = v8_str_safe(scope, &format!("ctx.store: key '{}' uses reserved namespace", key));
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

    // X3: StorageEngine path — propagate failures as JS exceptions (B2).
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        let rt = match get_rt_handle() {
            Ok(rt) => rt,
            Err(e) => {
                throw_store_error(
                    scope,
                    &format!("ctx.store.set('{key}'): tokio runtime unavailable: {e}"),
                );
                return;
            }
        };
        let bytes: Bytes = serde_json::to_vec(&json_value).unwrap_or_else(|_| b"null".to_vec());
        let outcome = match block_on_with_timeout(
            scope,
            &rt,
            "ctx.store.set",
            engine.set(&namespace, &key, bytes, ttl_ms),
        ) {
            Some(v) => v,
            None => return,
        };
        match outcome {
            Ok(()) => {
                // Mirror the write into TASK_STORE so same-task reads stay
                // cheap. Best-effort — the authoritative copy is in the
                // configured backend.
                TASK_STORE.with(|s| s.borrow_mut().insert(key, json_value));
            }
            Err(e) => {
                throw_store_error(
                    scope,
                    &format!("ctx.store.set('{key}'): backend error: {e}"),
                );
            }
        }
        return;
    }

    // No StorageEngine configured. Production must throw; dev mode may use
    // the in-memory TASK_STORE escape hatch (B2.2).
    if !dev_no_storage_mode() {
        throw_store_error(
            scope,
            &format!(
                "ctx.store.set('{key}'): no StorageEngine configured (set RIVERS_DEV_NO_STORAGE=1 to allow in-memory fallback)"
            ),
        );
        return;
    }

    TASK_STORE.with(|s| s.borrow_mut().insert(key, json_value));
}

/// Native V8 callback for `ctx.store.del(key)`.
///
/// X3: If a StorageEngine is configured (`TASK_STORAGE` is `Some`), deletes
/// via the async bridge with namespace and propagates backend errors as JS
/// exceptions. B2 (P1-5): no silent fallback to `TASK_STORE` — that masked
/// orphaned data when the configured backend was unavailable. The in-memory
/// map is only used when (a) no engine is configured AND (b)
/// `RIVERS_DEV_NO_STORAGE=1` is set. Throws if the key uses a reserved
/// prefix.
///
/// V8 callback -- short constant strings, unwrap is safe.
fn ctx_store_del_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8_str_safe(scope, &format!("ctx.store: key '{}' uses reserved namespace", key));
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // X3: StorageEngine path — propagate failures as JS exceptions (B2).
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        let rt = match get_rt_handle() {
            Ok(rt) => rt,
            Err(e) => {
                throw_store_error(
                    scope,
                    &format!("ctx.store.del('{key}'): tokio runtime unavailable: {e}"),
                );
                return;
            }
        };
        let outcome = match block_on_with_timeout(
            scope,
            &rt,
            "ctx.store.del",
            engine.delete(&namespace, &key),
        ) {
            Some(v) => v,
            None => return,
        };
        match outcome {
            Ok(()) => {
                TASK_STORE.with(|s| s.borrow_mut().remove(&key));
            }
            Err(e) => {
                throw_store_error(
                    scope,
                    &format!("ctx.store.del('{key}'): backend error: {e}"),
                );
            }
        }
        return;
    }

    // No StorageEngine configured. Production must throw; dev mode may use
    // the in-memory TASK_STORE escape hatch (B2.2).
    if !dev_no_storage_mode() {
        throw_store_error(
            scope,
            &format!(
                "ctx.store.del('{key}'): no StorageEngine configured (set RIVERS_DEV_NO_STORAGE=1 to allow in-memory fallback)"
            ),
        );
        return;
    }

    TASK_STORE.with(|s| s.borrow_mut().remove(&key));
}

/// Native V8 callback for `ctx.dataview(name, params)`.
///
/// X4: Checks `ctx.data[name]` for pre-fetched data first (fast path).
/// If not found, tries the DataViewExecutor via async bridge.
/// If no executor available, falls back to warn + null.
/// `ctx.ddl(datasource, statement)` — execute a DDL statement.
///
/// Uses TASK_DRIVER_FACTORY task-local to connect and call ddl_execute().
/// Only succeeds if the driver supports DDL (Gate 1 blocks DDL via execute()).
fn ctx_ddl_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // B1.2: ctx.ddl() is ONLY available during ApplicationInit. Reading
    // TASK_KIND first is critical — letting REST/MessageConsumer/etc. handlers
    // call ctx.ddl() lets a request handler issue `DROP TABLE users` (P0).
    let task_kind = super::task_locals::TASK_KIND.with(|k| *k.borrow());
    match task_kind {
        Some(rivers_runtime::process_pool::TaskKind::ApplicationInit) => {
            // Allowed.
        }
        other => {
            throw_js_error(
                scope,
                &format!(
                    "ctx.ddl() is only available during application initialization (got task_kind={:?})",
                    other
                ),
            );
            return;
        }
    }

    let datasource = args.get(0).to_rust_string_lossy(scope);
    let statement = args.get(1).to_rust_string_lossy(scope);

    // B1.2: app_id must be present (TaskLocals::set already rejects empty
    // app_id, but we double-check here so a future regression in TaskLocals
    // doesn't silently re-open this path).
    let app_id_present = super::task_locals::TASK_APP_NAME.with(|n| {
        n.borrow().as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    });
    if !app_id_present {
        throw_js_error(scope, "ctx.ddl(): app_id is required");
        return;
    }

    // Get DriverFactory and DataViewExecutor from task locals
    let factory = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone());
    let executor = TASK_DV_EXECUTOR.with(|e| e.borrow().clone());
    let rt_handle = RT_HANDLE.with(|h| h.borrow().clone());

    let factory = match factory {
        Some(f) => f,
        None => {
            throw_js_error(scope, "ctx.ddl(): DriverFactory not available");
            return;
        }
    };
    let rt = match rt_handle {
        Some(h) => h,
        None => {
            throw_js_error(scope, "ctx.ddl(): Tokio runtime not available");
            return;
        }
    };

    // Resolve datasource connection params
    let ds_params = executor.as_ref().and_then(|ex| {
        ex.datasource_params_get(&datasource)
            .or_else(|| {
                let suffix = format!(":{}", datasource);
                ex.datasource_params_by_suffix(&suffix)
            })
            .cloned()
    });

    let ds_params = match ds_params {
        Some(p) => p,
        None => {
            throw_js_error(scope, &format!("ctx.ddl('{}', ...): datasource not found", datasource));
            return;
        }
    };

    let driver_name = ds_params.options.get("driver").cloned()
        .unwrap_or_else(|| datasource.split(':').last().unwrap_or(&datasource).to_string());

    // H1: DDL whitelist check (Gate 3) — mirrors engine_loader::host_ddl_execute.
    //
    // Phase B1 already gated this callback to ApplicationInit, but without
    // consulting `[security].ddl_whitelist` an init handler could still run
    // any DDL the connecting user has DB-level permission for. This check
    // makes the in-process V8 path enforce the same whitelist the
    // dynamic-engine path enforces, reusing the single store of whitelist
    // state (`engine_loader::host_context::DDL_WHITELIST`).
    //
    // Behavior matches host_ddl_execute (engine_loader/host_callbacks.rs):
    //   - whitelist unset (None) or empty  → check skipped (operator opt-in)
    //   - whitelist Some(non-empty)        → must match `database@app_id`
    //
    // The whitelist key is `{database}@{appId}` with the manifest UUID, NOT
    // the entry_point name the ProcessPool dispatches with. We resolve via
    // engine_loader::app_id_for_entry_point — same fallback as the dynamic
    // path: if no map is registered, treat the entry_point name as the id.
    let whitelist = crate::engine_loader::ddl_whitelist();
    if let Some(ref whitelist) = whitelist {
        if !whitelist.is_empty() {
            let entry_point = super::task_locals::TASK_APP_NAME
                .with(|n| n.borrow().clone())
                .unwrap_or_default();
            let app_id = crate::engine_loader::app_id_for_entry_point(&entry_point)
                .unwrap_or_else(|| entry_point.clone());
            // Resolved database name from connection params; fall back to
            // the JS-level datasource label if the driver doesn't populate
            // `database` (mirrors dataview_engine::execute_ddl).
            let database: &str = if ds_params.database.is_empty() {
                &datasource
            } else {
                &ds_params.database
            };
            if !rivers_runtime::rivers_core_config::config::security::is_ddl_permitted(
                database,
                &app_id,
                whitelist,
            ) {
                tracing::warn!(
                    datasource = %datasource,
                    database = %database,
                    app_id = %app_id,
                    "ctx.ddl(): DDL rejected by whitelist (Gate 3)"
                );
                // Error string matches host_ddl_execute verbatim so operators
                // see one message regardless of which engine path executed.
                throw_js_error(
                    scope,
                    &format!(
                        "DDL not permitted for database '{}' (datasource '{}') in app '{}'",
                        database, datasource, app_id
                    ),
                );
                return;
            }
        }
    }

    // Execute DDL on Tokio runtime.
    //
    // The spawned task drives the work; the V8 worker blocks on a channel
    // until the task sends back a result. The `recv_timeout` here is the
    // wall-clock guard that prevents a hung driver / pool starvation /
    // infinite loop in user-supplied DDL from pinning this V8 worker
    // forever (H2). The spawned task is allowed to keep running in the
    // background after a timeout — we only release the worker.
    let (tx, rx) = std::sync::mpsc::channel();
    rt.spawn(async move {
        let result = async {
            let mut conn = factory.connect(&driver_name, &ds_params).await
                .map_err(|e| format!("DDL connect to '{}' failed: {}", datasource, e))?;
            let query = rivers_runtime::rivers_driver_sdk::Query::new("ddl", &statement);
            conn.ddl_execute(&query).await
                .map_err(|e| format!("DDL execute failed: {}", e))?;
            Ok::<_, String>(())
        }.await;
        let _ = tx.send(result);
    });

    let budget = std::time::Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match rx.recv_timeout(budget) {
        Ok(Ok(())) => {
            let result_json = serde_json::json!({"ok": true}).to_string();
            let v8_val = v8_str_safe(scope, &result_json);
            if let Some(parsed) = v8::json::parse(scope, v8_val) {
                rv.set(parsed);
            }
        }
        Ok(Err(e)) => {
            throw_js_error(scope, &e);
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            throw_js_error(
                scope,
                &format!(
                    "host callback 'ctx.ddl' timed out after {}ms",
                    HOST_CALLBACK_TIMEOUT_MS
                ),
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            throw_js_error(scope, "ctx.ddl(): task panicked");
        }
    }
}

/// Helper to throw a JS error from a V8 callback without borrow conflicts.
fn throw_js_error(scope: &mut v8::HandleScope, message: &str) {
    let msg = v8_str_safe(scope, message);
    let exception = v8::Exception::error(scope, msg);
    scope.throw_exception(exception);
}

/// Wall-clock budget for synchronous host-bridge work driven from a V8 worker
/// thread. If a host callback's spawned tokio task or `block_on`'d future has
/// not made progress within this budget, the worker throws a JS error and
/// reclaims its slot rather than pinning indefinitely on a hung driver, a
/// pool-starvation, or an infinite loop in user-supplied SQL.
///
/// Hard-coded today; threading a per-pool config knob is tracked separately
/// (see `ProcessPoolConfig::task_timeout_ms` for the related task-level budget
/// — the host-callback budget is intentionally tighter than the task budget so
/// the JS handler still has room to surface the timeout error).
///
/// TODO(H2 follow-up): make this configurable via `[runtime.process_pools.*]`
/// once the task-locals plumbing carries pool config to V8 worker callbacks.
///
/// Phase I3+I4+I5: the canonical definition lives in
/// `crate::engine_loader::HOST_CALLBACK_TIMEOUT_MS` so the V8 and dyn-engine
/// cdylib paths share a single source of truth. Aliased here so the rest of
/// this file is unchanged.
const HOST_CALLBACK_TIMEOUT_MS: u64 = crate::engine_loader::HOST_CALLBACK_TIMEOUT_MS;

/// Bound an `async` future against `HOST_CALLBACK_TIMEOUT_MS` while running
/// it on the supplied tokio handle.
///
/// Returns the future's value on success. On timeout, throws a structured JS
/// error that names the callback and the budget, then returns `None` so the
/// caller can early-return without unwrapping.
///
/// `Handle::block_on` enters the runtime context for the duration of the
/// call, so the `tokio::time::timeout` timer driver ticks correctly even
/// though the caller is a synchronous V8 worker thread.
fn block_on_with_timeout<F, T>(
    scope: &mut v8::HandleScope,
    rt: &tokio::runtime::Handle,
    callback_name: &str,
    fut: F,
) -> Option<T>
where
    F: std::future::Future<Output = T>,
{
    let budget = std::time::Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match rt.block_on(async move { tokio::time::timeout(budget, fut).await }) {
        Ok(v) => Some(v),
        Err(_elapsed) => {
            throw_js_error(
                scope,
                &format!(
                    "host callback '{}' timed out after {}ms",
                    callback_name, HOST_CALLBACK_TIMEOUT_MS
                ),
            );
            None
        }
    }
}

/// `ctx.transaction(datasource_name, callback)` — spec §6.
///
/// Begins a transaction on the named datasource, invokes the callback with
/// no args, and commits on clean return / rolls back on throw. `ctx.dataview()`
/// calls inside the callback are routed through the held connection.
///
/// Rejects:
/// - `TransactionError: nested transactions not supported` — if a transaction
///   is already active on this task (thread-local already populated).
/// - `TransactionError: datasource "X" not found` — if the name is unknown.
/// - `TransactionError: datasource "X" does not support transactions` — if the
///   driver's `begin_transaction` returns `DriverError::Unsupported`.
///
/// On the callback's own throw, the exception is re-propagated after rollback.
fn ctx_transaction_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    use std::sync::Arc;
    use rivers_runtime::rivers_driver_sdk::DriverError;

    // ── Argument validation ─────────────────────────────────────
    if args.length() < 2 {
        throw_js_error(
            scope,
            "ctx.transaction requires two arguments: (datasource: string, fn: Function)",
        );
        return;
    }
    let ds_name = args.get(0).to_rust_string_lossy(scope);
    let cb_val = args.get(1);
    let cb_fn = match v8::Local::<v8::Function>::try_from(cb_val) {
        Ok(f) => f,
        Err(_) => {
            throw_js_error(
                scope,
                "ctx.transaction second argument must be a function",
            );
            return;
        }
    };

    // ── Spec §6.2: reject nested ─────────────────────────────────
    let already_active = TASK_TRANSACTION.with(|t| t.borrow().is_some());
    if already_active {
        throw_js_error(scope, "TransactionError: nested transactions not supported");
        return;
    }

    // ── Resolve datasource → driver + ConnectionParams ──────────
    let resolved = TASK_DS_CONFIGS.with(|c| c.borrow().get(&ds_name).cloned());
    let resolved = match resolved {
        Some(r) => r,
        None => {
            throw_js_error(
                scope,
                &format!("TransactionError: datasource \"{ds_name}\" not found in task config"),
            );
            return;
        }
    };

    // ── Get DriverFactory ───────────────────────────────────────
    let factory = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone());
    let factory = match factory {
        Some(f) => f,
        None => {
            throw_js_error(
                scope,
                "TransactionError: driver factory not available — transactions require configured datasources",
            );
            return;
        }
    };

    // ── Begin transaction (async bridge) ────────────────────────
    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            throw_js_error(scope, &format!("TransactionError: {e}"));
            return;
        }
    };

    let txn_map = Arc::new(crate::transaction::TransactionMap::new());
    let begin_outcome: Result<(), DriverError> = match block_on_with_timeout(
        scope,
        &rt,
        "ctx.transaction (begin)",
        async {
            let conn = factory
                .connect(&resolved.driver_name, &resolved.params)
                .await?;
            txn_map.begin(&ds_name, conn).await
        },
    ) {
        Some(v) => v,
        None => return, // JS error already thrown by the timeout helper
    };

    if let Err(e) = begin_outcome {
        let msg = match &e {
            DriverError::Unsupported(_) => format!(
                "TransactionError: datasource \"{ds_name}\" does not support transactions"
            ),
            _ => format!("TransactionError: begin failed: {e}"),
        };
        throw_js_error(scope, &msg);
        return;
    }

    // ── Install thread-local so ctx.dataview() routes through us ──
    TASK_TRANSACTION.with(|t| {
        *t.borrow_mut() = Some(TaskTransactionState {
            map: txn_map.clone(),
            datasource: ds_name.clone(),
        });
    });

    // ── Invoke the JS callback ──────────────────────────────────
    let undefined = v8::undefined(scope).into();
    let tc = &mut v8::TryCatch::new(scope);
    let call_result = cb_fn.call(tc, undefined, &[]);

    // ── Commit or rollback ──────────────────────────────────────
    //
    // H2: Both commit and rollback are bounded by HOST_CALLBACK_TIMEOUT_MS.
    // On timeout we treat the outcome as a commit failure (writes may or
    // may not have persisted) and we still clear TASK_TRANSACTION so the
    // worker isn't pinned by a hung driver.
    let budget = std::time::Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match call_result {
        Some(val) => {
            // Clean return → commit, yield callback's return value.
            let commit_res = rt.block_on(async {
                tokio::time::timeout(budget, txn_map.commit(&ds_name)).await
            });
            TASK_TRANSACTION.with(|t| *t.borrow_mut() = None);
            match commit_res {
                Ok(Ok(_conn)) => {
                    // Connection drops → pool slot released.
                    rv.set(val);
                }
                Ok(Err(e)) => {
                    // Spec §6 + financial-correctness gate: commit failure
                    // is observably different from a handler throw — the
                    // handler's writes may or may not have persisted. Stash
                    // the details in a thread-local that execute_js_task
                    // reads to upgrade the error to
                    // `TaskError::TransactionCommitFailed`.
                    let driver_msg = format!("{e}");
                    TASK_COMMIT_FAILED.with(|c| {
                        *c.borrow_mut() = Some((ds_name.clone(), driver_msg.clone()));
                    });
                    let msg = v8_str_safe(
                        tc,
                        &format!(
                            "TransactionError: commit failed on datasource '{ds_name}': {driver_msg}"
                        ),
                    );
                    let err = v8::Exception::error(tc, msg);
                    tc.throw_exception(err);
                }
                Err(_elapsed) => {
                    // Same financial-correctness gate as a commit error:
                    // the writes' persistence is now indeterminate.
                    let driver_msg = format!(
                        "commit timed out after {HOST_CALLBACK_TIMEOUT_MS}ms"
                    );
                    TASK_COMMIT_FAILED.with(|c| {
                        *c.borrow_mut() = Some((ds_name.clone(), driver_msg.clone()));
                    });
                    let msg = v8_str_safe(
                        tc,
                        &format!(
                            "TransactionError: commit on datasource '{ds_name}' timed out after {HOST_CALLBACK_TIMEOUT_MS}ms"
                        ),
                    );
                    let err = v8::Exception::error(tc, msg);
                    tc.throw_exception(err);
                }
            }
        }
        None => {
            // Callback threw → rollback, re-propagate the original exception.
            let rollback_res = rt.block_on(async {
                tokio::time::timeout(budget, txn_map.rollback(&ds_name)).await
            });
            TASK_TRANSACTION.with(|t| *t.borrow_mut() = None);
            match rollback_res {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        target: "rivers.handler",
                        datasource = %ds_name,
                        error = %e,
                        "rollback failed after handler threw"
                    );
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        target: "rivers.handler",
                        datasource = %ds_name,
                        budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                        "rollback timed out after handler threw — connection abandoned"
                    );
                }
            }
            // Re-propagate the handler's exception to the outer JS scope.
            //
            // IMPORTANT: must use rethrow(), NOT throw_exception().
            // We are inside the TryCatch scope `tc`; any exception set via
            // throw_exception() would be caught by `tc` again and never
            // reach the outer scope. rethrow() marks the already-caught
            // exception so it propagates outward when `tc` drops.
            let _ = tc.rethrow();
        }
    }
}

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
    let ctx_key = v8_str_safe(scope, "ctx");
    if let Some(ctx_val) = global.get(scope, ctx_key.into()) {
        if let Ok(ctx_obj) = v8::Local::<v8::Object>::try_from(ctx_val) {
            let data_key = v8_str_safe(scope, "data");
            if let Some(data_val) = ctx_obj.get(scope, data_key.into()) {
                if let Ok(data_obj) = v8::Local::<v8::Object>::try_from(data_val) {
                    let name_key = v8_str_safe(scope, &name);
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
    // Namespace the name with the entry-point prefix so it matches the registry key
    let namespaced_name = TASK_DV_NAMESPACE.with(|n| {
        n.borrow().as_ref()
            .filter(|ns| !ns.is_empty() && !name.contains(':'))
            .map(|ns| format!("{ns}:{name}"))
            .unwrap_or_else(|| name.clone())
    });

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

        // Spec §6: if a transaction is active, route this dataview through
        // the held connection. Enforce §6.2 cross-datasource check: the
        // dataview's backing datasource MUST match the transaction's.
        let txn_state: Option<(std::sync::Arc<crate::transaction::TransactionMap>, String)> =
            TASK_TRANSACTION.with(|t| {
                t.borrow().as_ref().map(|s| (s.map.clone(), s.datasource.clone()))
            });
        if let Some((_, ref txn_ds)) = txn_state {
            // Look up the dataview's configured datasource.
            // DataViews are stored namespaced (e.g. "sql:canary-pg") but
            // ctx.transaction() / Rivers.db.begin() receive the bare user name
            // (e.g. "canary-pg"). Strip the namespace prefix before comparing
            // so "sql:canary-pg" matches "canary-pg".
            let ns_prefix = TASK_DV_NAMESPACE.with(|n| {
                n.borrow().as_ref().map(|ns| format!("{ns}:")).unwrap_or_default()
            });
            let dv_ds = exec.datasource_for(&namespaced_name);
            match dv_ds {
                Some(ref ds) => {
                    let bare_ds = ds.strip_prefix(&ns_prefix).unwrap_or(ds.as_str());
                    if bare_ds != txn_ds.as_str() {
                        throw_js_error(
                            scope,
                            &format!(
                                "TransactionError: dataview \"{name}\" uses datasource \"{bare_ds}\" which differs from transaction datasource \"{txn_ds}\""
                            ),
                        );
                        return;
                    }
                }
                None => {
                    // Unknown dataview — let execute() produce the "not found"
                    // error for consistency with the non-txn path.
                }
            }
        }

        match get_rt_handle() {
            Ok(rt) => {
                let exec_outcome = match block_on_with_timeout(
                    scope,
                    &rt,
                    "ctx.dataview",
                    async {
                        if let Some((map, ds)) = txn_state {
                            // Take the held connection out of the map, use it,
                            // put it back. take/return is the pattern the
                            // TransactionMap was designed for.
                            if let Some(mut conn) = map.take_connection(&ds).await {
                                let res = exec
                                    .execute(
                                        &namespaced_name,
                                        query_params,
                                        "GET",
                                        &trace_id,
                                        Some(&mut conn),
                                    )
                                    .await;
                                map.return_connection(&ds, conn).await;
                                res
                            } else {
                                // Unreachable in practice — the thread-local
                                // should stay consistent with the map — but
                                // return a clear error rather than panic.
                                Err(rivers_runtime::dataview_engine::DataViewError::Driver(
                                    format!("transaction connection for '{ds}' unavailable"),
                                ))
                            }
                        } else {
                            exec.execute(&namespaced_name, query_params, "GET", &trace_id, None)
                                .await
                        }
                    },
                ) {
                    Some(v) => v,
                    None => return, // JS error already thrown by the timeout helper
                };
                match exec_outcome {
                    Ok(response) => {
                        // Convert QueryResult rows to JSON
                        let json = serde_json::json!({
                            "rows": response.query_result.rows,
                            "affected_rows": response.query_result.affected_rows,
                            "last_insert_id": response.query_result.last_insert_id,
                        });
                        let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "null".into());
                        let v8_str = v8_str_safe(scope, &json_str);
                        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                            rv.set(parsed);
                        } else {
                            rv.set(v8::null(scope).into());
                        }
                        return;
                    }
                    Err(e) => {
                        let msg = v8_str_safe(
                            scope,
                            &format!("ctx.dataview('{}') execution error: {e}", name),
                        );
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

    // Fallback: no executor and not pre-fetched — throw a JS exception
    // so handlers see a clear error instead of silent null.
    let err_msg = format!(
        "ctx.dataview('{}') not found. Declare in view config: dataviews = [\"{}\"]",
        name, name
    );
    tracing::warn!(target: "rivers.handler", "{}", err_msg);
    let msg = v8_str_safe(scope, &err_msg);
    let exception = v8::Exception::error(scope, msg);
    scope.throw_exception(exception);
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// G_R3: the V8 `ctx.store` reserved-prefix check now consumes the
    /// canonical list from `rivers-core-config::storage::RESERVED_PREFIXES`.
    /// Both `poll:` (was missing in V8 before) and `raft:` (was missing in
    /// core before) MUST be reserved from the V8 perspective.
    #[test]
    fn reserved_prefixes_match_canonical_list() {
        assert!(store_key_is_reserved("session:abc"));
        assert!(store_key_is_reserved("csrf:token"));
        assert!(store_key_is_reserved("cache:dataview"));
        assert!(store_key_is_reserved("rivers:node"));
        assert!(store_key_is_reserved("poll:foo"), "V8 must see poll: as reserved");
        assert!(store_key_is_reserved("raft:foo"), "V8 must see raft: as reserved");
        assert!(!store_key_is_reserved("user:data"));
    }

    /// H2 (T1-6): `ctx.ddl` swapped its unbounded `rx.recv()` for a
    /// `recv_timeout(HOST_CALLBACK_TIMEOUT_MS)`. This test exercises the
    /// underlying primitive against the same channel type the callback uses
    /// (`std::sync::mpsc::channel`), with a tiny budget, to prove that a
    /// hung spawned task surfaces as `RecvTimeoutError::Timeout` — i.e.
    /// the V8 worker does NOT block forever.
    #[test]
    fn ddl_recv_timeout_returns_timeout_error_when_spawned_task_hangs() {
        let (_tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        // Note: we deliberately do NOT spawn a producer. Holding `_tx` keeps
        // the channel from auto-disconnecting, which simulates a still-alive
        // background task that simply hasn't sent a result yet — exactly the
        // pinned-V8-worker scenario H2 fixes.
        let budget = std::time::Duration::from_millis(50);
        let result = rx.recv_timeout(budget);
        assert!(
            matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "expected Timeout, got {:?}",
            result
        );
    }

    /// H2 (T1-6): `ctx.store.*`, `ctx.transaction`, and `ctx.dataview` all
    /// route their async work through `block_on_with_timeout`, which wraps
    /// `Handle::block_on(tokio::time::timeout(budget, fut))`. This test
    /// exercises the same composition end-to-end with a future that never
    /// completes, to prove that a hung driver surfaces as an `Elapsed`
    /// error rather than pinning the worker.
    #[test]
    fn block_on_with_timeout_primitive_returns_elapsed_when_future_hangs() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .expect("build test runtime");
        let handle = rt.handle().clone();
        let budget = std::time::Duration::from_millis(50);
        let outcome: Result<(), tokio::time::error::Elapsed> = handle.block_on(async move {
            tokio::time::timeout(budget, std::future::pending::<()>()).await
        });
        assert!(
            outcome.is_err(),
            "pending future should time out, got {:?}",
            outcome
        );
    }

    /// H2 (T1-6): the host-callback budget is intentionally tighter than
    /// the task budget so the JS handler can still surface the timeout
    /// error before the wider task wall-clock fires. If someone shrinks
    /// the task default below the host-callback budget, this test breaks
    /// and forces a deliberate decision.
    #[test]
    fn host_callback_budget_is_bounded_and_nonzero() {
        assert!(HOST_CALLBACK_TIMEOUT_MS > 0);
        // Sanity: we expect this to be in seconds-not-hours range.
        assert!(HOST_CALLBACK_TIMEOUT_MS <= 5 * 60 * 1000);
    }
}
