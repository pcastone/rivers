//! FFI host callback implementations for engine plugins (V8, WASM).
//!
//! All `extern "C"` functions referenced in `HostCallbacks` live here.
//! They access subsystem state via `HOST_CONTEXT` and `HOST_KEYSTORE`
//! defined in the sibling `host_context` module.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::host_context::{
    current_task_id, dyn_txn_map, lookup_task_ds, signal_commit_failed, HostContext,
    HOST_CALLBACK_TIMEOUT_MS, HOST_CONTEXT, HOST_KEYSTORE,
};
use rivers_runtime::DataViewExecutor;
// Re-import the module itself behind cfg(test) so the test submodule below
// can reach sibling helpers via `super::host_context::*` paths. The
// non-test build doesn't need this — production code uses fully-qualified
// `super::host_context::APP_ID_MAP` style paths directly.
#[cfg(test)]
use super::host_context;

/// Helper: write a JSON value into the output buffer pointers.
fn write_output(out_ptr: *mut *mut u8, out_len: *mut usize, value: &serde_json::Value) {
    let (ptr, len) = rivers_engine_sdk::json_to_buffer(value);
    unsafe {
        *out_ptr = ptr;
        *out_len = len;
    }
}

/// Helper: read JSON from an input buffer.
fn read_input(input_ptr: *const u8, input_len: usize) -> Result<serde_json::Value, String> {
    unsafe { rivers_engine_sdk::buffer_to_json(input_ptr, input_len) }
}

// ── dataview_execute ────────────────────────────────────────────

pub(super) extern "C" fn host_dataview_execute(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => {
            tracing::error!("host_dataview_execute: HOST_CONTEXT not set");
            return -1;
        }
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "host_dataview_execute: failed to read input");
            return -2;
        }
    };

    let name = match input["name"].as_str() {
        Some(n) => n.to_string(),
        None => {
            tracing::error!(input = %input, "host_dataview_execute: missing 'name' field");
            return -3;
        }
    };
    let trace_id = input["trace_id"].as_str().unwrap_or("engine-callback").to_string();

    // Convert JSON params to HashMap<String, QueryValue>, coercing to native types
    use rivers_runtime::rivers_driver_sdk::QueryValue;
    let params: HashMap<String, QueryValue> = input["params"]
        .as_object()
        .map(|o| o.iter().map(|(k, v)| {
            let qv = match v {
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        QueryValue::Integer(i)
                    } else if let Some(f) = n.as_f64() {
                        QueryValue::Float(f)
                    } else {
                        QueryValue::Json(v.clone())
                    }
                }
                serde_json::Value::String(s) => QueryValue::String(s.clone()),
                serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
                serde_json::Value::Null => QueryValue::Null,
                _ => QueryValue::Json(v.clone()),
            };
            (k.clone(), qv)
        }).collect())
        .unwrap_or_default();

    // Try app-namespace prefix from trace_id or input, fall back to scanning registry
    let app_prefix = input["app_prefix"].as_str().map(|s| s.to_string());

    let executor_lock = ctx.dataview_executor.clone();

    // Phase I6 — capture the task id (if we're inside a TaskGuard scope) so
    // the spawned future can route the call through DYN_TXN_MAP when an
    // active transaction is present. `current_task_id()` reads the riversd
    // `spawn_blocking` thread-local; the spawned tokio task does not see it.
    let task_id = current_task_id();

    // Spawn execution on the Tokio runtime and wait for the result.
    // This is critical: some drivers (e.g. MongoDB, Elasticsearch) require a
    // Tokio reactor on the calling thread. `block_on()` alone doesn't set the
    // thread-local reactor context, but `spawn` runs on a proper Tokio worker
    // thread where the reactor IS available.
    //
    // H2: The recv is bounded by HOST_CALLBACK_TIMEOUT_MS. If the spawned task
    // stalls (driver hang, pool starvation), recv_timeout returns
    // RecvTimeoutError::Timeout, the JoinHandle is aborted, and the caller
    // sees error code -13 rather than pinning forever.
    //
    // If the spawned task panics, the tx sender is dropped without sending,
    // causing recv_timeout to return RecvTimeoutError::Disconnected — error -12.
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = ctx.rt_handle.spawn({
        let executor_lock = executor_lock.clone();
        let name = name.clone();
        let trace_id = trace_id.clone();
        let app_prefix = app_prefix.clone();
        async move {
            let result = async {
                let executor = {
                    let guard = executor_lock.read().await;
                    guard.clone().ok_or_else(|| "DataViewExecutor not initialized".to_string())?
                };
                // Resolve the registered name: bare → "{prefix}:{name}" → ":{name}" suffix scan.
                let resolved = resolve_dataview_name(&executor, &name, app_prefix.as_deref())
                    .ok_or_else(|| format!(
                        "DataView '{}' not found (tried bare and namespaced)",
                        name
                    ))?;

                execute_dataview_with_optional_txn(
                    executor,
                    &resolved,
                    params,
                    &trace_id,
                    task_id,
                )
                .await
                .map_err(|e| format!("{e:?}"))
            }.await;
            let _ = tx.send(result);
        }
    });

    // Wait for the spawned task to complete, bounded by HOST_CALLBACK_TIMEOUT_MS.
    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match rx.recv_timeout(budget) {
        Ok(Ok(response)) => {
            // Serialize DataViewResponse.query_result to JSON
            let result = serde_json::json!({
                "rows": response.query_result.rows,
                "affected_rows": response.query_result.affected_rows,
                "execution_time_ms": response.execution_time_ms,
                "cache_hit": response.cache_hit,
            });
            write_output(out_ptr, out_len, &result);
            0
        }
        Ok(Err(e)) => {
            tracing::error!(dataview = %name, error = %e, "host_dataview_execute failed");
            let err_val = serde_json::json!({"error": e});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            handle.abort();
            tracing::error!(
                dataview = %name,
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_dataview_execute: timed out — spawned task aborted"
            );
            let err_val = serde_json::json!({
                "error": format!(
                    "host callback 'host_dataview_execute' timed out after {}ms",
                    HOST_CALLBACK_TIMEOUT_MS
                )
            });
            write_output(out_ptr, out_len, &err_val);
            -13
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::error!(dataview = %name, "host_dataview_execute: channel disconnected — task panicked");
            -12
        }
    }
}

// ── dataview helpers (I6) ──────────────────────────────────────
//
// Phase I6: split the bare-vs-namespaced resolution and the
// transaction routing into small helpers so the FFI shim above stays
// linear and the txn-vs-no-txn branch is testable in isolation.

/// Resolve the actually-registered DataView name from the user-facing
/// name plus an optional app namespace hint.
///
/// Order: try the bare name first, then `"{app_prefix}:{name}"` if a
/// prefix is supplied, then a `":{name}"` suffix scan over the registry.
/// Returns the canonical key registered with the executor, or `None` when
/// no match exists.
fn resolve_dataview_name(
    executor: &DataViewExecutor,
    name: &str,
    app_prefix: Option<&str>,
) -> Option<String> {
    if executor.datasource_for(name).is_some() {
        return Some(name.to_string());
    }
    if let Some(prefix) = app_prefix {
        let candidate = format!("{prefix}:{name}");
        if executor.datasource_for(&candidate).is_some() {
            return Some(candidate);
        }
    }
    let suffix = format!(":{name}");
    executor.find_by_suffix(&suffix)
}

/// Execute a (already-resolved) DataView, routing through the dyn-engine
/// transaction map when one is active for the current task.
///
/// Spec §6.2: if a transaction is active on this task for a *different*
/// datasource than the dataview's, reject with a `DataViewError::Driver`
/// carrying a `TransactionError:` prefix — mirrors the V8 path's JS error
/// in `process_pool/v8_engine/context.rs::ctx_dataview_callback`.
///
/// When no transaction is active for this task (or `task_id` is `None`),
/// falls through to the normal pool-acquire path.
///
/// Takes `executor: Arc<DataViewExecutor>` (not `&DataViewExecutor`) because
/// `DynTransactionMap::with_conn_mut` is HRTB on the closure lifetime
/// (`for<'a> ...`), which forces any non-`'static` borrow captured by the
/// closure to be `'static`. Cloning the `Arc` into the closure satisfies
/// that without bending the executor's API.
async fn execute_dataview_with_optional_txn(
    executor: Arc<DataViewExecutor>,
    resolved_name: &str,
    params: HashMap<String, rivers_runtime::rivers_driver_sdk::QueryValue>,
    trace_id: &str,
    task_id: Option<super::dyn_transaction_map::TaskId>,
) -> Result<rivers_runtime::dataview_engine::DataViewResponse, rivers_runtime::DataViewError> {
    // Without a TaskId we can never have an active txn — go straight to
    // the non-txn path. Identical to the V8 "TASK_TRANSACTION = None" case.
    let Some(tid) = task_id else {
        return executor
            .execute(resolved_name, params, "GET", trace_id, None)
            .await;
    };

    // Fast-path: no transactions active for this task at all.
    let active_dses = dyn_txn_map().task_active_datasources(tid);
    if active_dses.is_empty() {
        return executor
            .execute(resolved_name, params, "GET", trace_id, None)
            .await;
    }

    // Look up the dataview's configured datasource for cross-DS enforcement.
    let dv_ds = match executor.datasource_for(resolved_name) {
        Some(ds) => ds,
        None => {
            // Should be unreachable — caller already resolved the name —
            // but defer to the non-txn path so the executor produces the
            // canonical NotFound error for consistency.
            return executor
                .execute(resolved_name, params, "GET", trace_id, None)
                .await;
        }
    };

    // Spec §6.2: if any active txn datasource differs from the dataview's,
    // reject. Today the dyn map permits only one txn per (task, ds), but
    // the loop is correct for the multi-ds future shape.
    if !active_dses.iter().any(|ds| ds == &dv_ds) {
        let other = active_dses.first().cloned().unwrap_or_default();
        return Err(rivers_runtime::DataViewError::Driver(format!(
            "TransactionError: dataview \"{resolved_name}\" uses datasource \"{dv_ds}\" \
             which differs from active transaction datasource \"{other}\""
        )));
    }

    // Active txn for the matching datasource — thread the held connection
    // through. `with_conn_mut` removes the entry under the lock, drops the
    // lock, runs the closure's future, then re-inserts (lock not held
    // across the await — see DynTransactionMap docs).
    let ds_for_closure = dv_ds.clone();
    let resolved_owned = resolved_name.to_string();
    let trace_owned = trace_id.to_string();
    let executor_for_closure = executor.clone();
    let outcome = dyn_txn_map()
        .with_conn_mut(tid, &dv_ds, move |conn| {
            let exec = executor_for_closure;
            let resolved = resolved_owned;
            let trace = trace_owned;
            Box::pin(async move {
                exec.execute(&resolved, params, "GET", &trace, Some(conn))
                    .await
            })
        })
        .await;
    match outcome {
        Some(r) => r,
        None => {
            // Race: the txn entry vanished between snapshot and lookup
            // (commit/rollback raced on a different thread). Surface a
            // clear error rather than silently using a fresh pool conn —
            // a fresh conn would NOT be in the txn and writes would land
            // outside the user's expected scope.
            Err(rivers_runtime::DataViewError::Driver(format!(
                "TransactionError: transaction connection for '{ds_for_closure}' \
                 unavailable (raced with commit/rollback)"
            )))
        }
    }
}

// ── test-only re-exports for cross-module integration tests ────
//
// I7 dispatch tests in `process_pool/mod.rs` need to drive the same
// `host_db_*_inner` paths that the FFI shim drives, but they live in a
// different module so the private `fn host_db_*_inner` signatures aren't
// reachable. Thin re-exports under #[cfg(test)] keep production code
// untouched while letting the dispatch tests run begin/commit through
// the same code paths the production cdylib would.

#[cfg(test)]
pub(crate) fn host_db_begin_inner_for_test(
    input: &serde_json::Value,
    ctx: &super::host_context::HostContext,
) -> Result<serde_json::Value, (i32, serde_json::Value)> {
    host_db_begin_inner(input, ctx)
}

#[cfg(test)]
pub(crate) fn host_db_commit_inner_for_test(
    input: &serde_json::Value,
    ctx: &super::host_context::HostContext,
) -> Result<serde_json::Value, (i32, serde_json::Value)> {
    host_db_commit_inner(input, ctx)
}

#[cfg(test)]
pub(crate) fn host_db_rollback_inner_for_test(
    input: &serde_json::Value,
    ctx: &super::host_context::HostContext,
) -> Result<serde_json::Value, (i32, serde_json::Value)> {
    host_db_rollback_inner(input, ctx)
}

/// Phase I8 — drive `execute_dataview_with_optional_txn` (the helper that
/// `host_dataview_execute` delegates to) directly from cross-module e2e
/// tests. Returns the same `DataViewResponse` / `DataViewError` shape the
/// FFI shim wraps; tests assert on `.query_result.affected_rows` etc.
#[cfg(test)]
pub(crate) async fn execute_dataview_with_optional_txn_for_test(
    executor: Arc<rivers_runtime::DataViewExecutor>,
    resolved_name: &str,
    params: HashMap<String, rivers_runtime::rivers_driver_sdk::QueryValue>,
    trace_id: &str,
    task_id: Option<super::dyn_transaction_map::TaskId>,
) -> Result<rivers_runtime::dataview_engine::DataViewResponse, rivers_runtime::DataViewError> {
    execute_dataview_with_optional_txn(executor, resolved_name, params, trace_id, task_id).await
}

// ── store_get ───────────────────────────────────────────────────

pub(super) extern "C" fn host_store_get(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => return -1,
    };

    let engine = match ctx.storage_engine.as_ref() {
        Some(e) => e,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let namespace = input["namespace"].as_str().unwrap_or("default");
    let key = match input["key"].as_str() {
        Some(k) => k,
        None => return -3,
    };

    let engine = Arc::clone(engine);
    let ns = namespace.to_string();
    let k = key.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = ctx.rt_handle.spawn(async move {
        let _ = tx.send(engine.get(&ns, &k).await);
    });
    // H2: bounded recv — prevents a stalled storage backend from pinning the worker.
    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    let store_result = match rx.recv_timeout(budget) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            handle.abort();
            tracing::error!(
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_store_get: timed out — spawned task aborted"
            );
            let err_val = serde_json::json!({
                "error": format!(
                    "host callback 'host_store_get' timed out after {}ms",
                    HOST_CALLBACK_TIMEOUT_MS
                )
            });
            write_output(out_ptr, out_len, &err_val);
            return -13;
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return -10, // task panicked
    };
    match store_result {
        Ok(Some(bytes)) => {
            // Try to parse as JSON, fall back to string
            let value = serde_json::from_slice::<serde_json::Value>(&bytes)
                .unwrap_or_else(|_| serde_json::Value::String(
                    String::from_utf8_lossy(&bytes).to_string()
                ));
            let result = serde_json::json!({"value": value});
            write_output(out_ptr, out_len, &result);
            0
        }
        Ok(None) => {
            write_output(out_ptr, out_len, &serde_json::Value::Null);
            0
        }
        Err(e) => {
            let err_val = serde_json::json!({"error": e.to_string()});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
    }
}

// ── store_set ───────────────────────────────────────────────────

pub(super) extern "C" fn host_store_set(
    input_ptr: *const u8, input_len: usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => return -1,
    };

    let engine = match ctx.storage_engine.as_ref() {
        Some(e) => e,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let namespace = input["namespace"].as_str().unwrap_or("default");
    let key = match input["key"].as_str() {
        Some(k) => k,
        None => return -3,
    };
    let value_bytes = serde_json::to_vec(&input["value"]).unwrap_or_default();
    let ttl_ms = input["ttl_ms"].as_u64();

    let engine = Arc::clone(engine);
    let ns = namespace.to_string();
    let k = key.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = ctx.rt_handle.spawn(async move {
        let _ = tx.send(engine.set(&ns, &k, value_bytes, ttl_ms).await);
    });
    // H2: bounded recv — prevents a stalled storage backend from pinning the worker.
    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match rx.recv_timeout(budget) {
        Ok(Ok(())) => 0,
        Ok(Err(_)) => -10,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            handle.abort();
            tracing::error!(
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_store_set: timed out — spawned task aborted"
            );
            -13
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => -10, // task panicked
    }
}

// ── store_del ───────────────────────────────────────────────────

pub(super) extern "C" fn host_store_del(
    input_ptr: *const u8, input_len: usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => return -1,
    };

    let engine = match ctx.storage_engine.as_ref() {
        Some(e) => e,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let namespace = input["namespace"].as_str().unwrap_or("default");
    let key = match input["key"].as_str() {
        Some(k) => k,
        None => return -3,
    };

    let engine = Arc::clone(engine);
    let ns = namespace.to_string();
    let k = key.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = ctx.rt_handle.spawn(async move {
        let _ = tx.send(engine.delete(&ns, &k).await);
    });
    // H2: bounded recv — prevents a stalled storage backend from pinning the worker.
    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match rx.recv_timeout(budget) {
        Ok(Ok(_)) => 0,
        Ok(Err(_)) => -10,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            handle.abort();
            tracing::error!(
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_store_del: timed out — spawned task aborted"
            );
            -13
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => -10, // task panicked
    }
}

// ── datasource_build ────────────────────────────────────────────

pub(super) extern "C" fn host_datasource_build(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => return -1,
    };

    let factory = match ctx.driver_factory.as_ref() {
        Some(f) => f,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let driver = match input["driver"].as_str() {
        Some(d) => d.to_string(),
        None => return -3,
    };
    let statement = input["query"].as_str().unwrap_or("").to_string();
    let params_obj = input["params"].as_object().cloned().unwrap_or_default();

    // Build ConnectionParams from input
    let conn_params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
        host: input["host"].as_str().unwrap_or("").to_string(),
        port: input["port"].as_u64().unwrap_or(0) as u16,
        database: input["database"].as_str().unwrap_or("").to_string(),
        username: input["username"].as_str().unwrap_or("").to_string(),
        password: String::new(),
        options: params_obj.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect(),
    };

    // Build the Query object — convert JSON values to native QueryValue types
    // so driver get_string/get_int helpers can match them correctly.
    // Use with_operation to preserve the exact case of the operation name
    // (Query::new would lowercase it via infer_operation).
    use rivers_runtime::rivers_driver_sdk::{Query, QueryValue};
    let mut query = Query::with_operation(&statement, "", &statement);
    for (k, v) in &params_obj {
        let qv = match v {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    QueryValue::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    QueryValue::Float(f)
                } else {
                    QueryValue::Json(v.clone())
                }
            }
            serde_json::Value::String(s) => QueryValue::String(s.clone()),
            serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
            serde_json::Value::Null => QueryValue::Null,
            _ => QueryValue::Json(v.clone()),
        };
        query.parameters.insert(k.clone(), qv);
    }

    // Execute on the host runtime. Earlier versions spawned a dedicated
    // `Runtime::new()` inside `spawn_blocking` to isolate cdylib plugins,
    // but cdylib plugins are disabled in this build (all drivers are
    // statically linked — see `server/drivers.rs`). Creating + dropping a
    // fresh runtime per query tears down long-lived driver internals
    // (mysql_async pool tasks, tokio_postgres connection tasks), producing
    // "Tokio 1.x context was found, but it is being shutdown" on MySQL and
    // "connection closed" on PG for every call after the first.
    // Use the host runtime directly; `catch_unwind` still guards against
    // driver panics.
    //
    // H2: recv is bounded by HOST_CALLBACK_TIMEOUT_MS. A stalled driver
    // results in error code -13 rather than pinning the worker forever.
    let (ds_tx, ds_rx) = std::sync::mpsc::channel();
    let factory = Arc::clone(factory);
    let ds_handle = ctx.rt_handle.spawn(async move {
        let result = async {
            let mut conn = factory.connect(&driver, &conn_params).await
                .map_err(|e| format!("driver connect failed: {e}"))?;
            conn.execute(&query).await.map_err(|e| e.to_string())
        }.await;
        let _ = ds_tx.send(result);
    });
    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    match ds_rx.recv_timeout(budget).unwrap_or_else(|e| match e {
        std::sync::mpsc::RecvTimeoutError::Disconnected => Err("datasource task panicked".to_string()),
        std::sync::mpsc::RecvTimeoutError::Timeout => {
            ds_handle.abort();
            Err(format!(
                "host callback 'host_datasource_build' timed out after {}ms",
                HOST_CALLBACK_TIMEOUT_MS
            ))
        }
    }) {
        Ok(result) => {
            let json_result = serde_json::json!({
                "rows": result.rows,
                "affected_rows": result.affected_rows,
            });
            write_output(out_ptr, out_len, &json_result);
            0
        }
        Err(e) => {
            let is_timeout = e.contains("timed out after");
            let err_val = serde_json::json!({"error": e});
            write_output(out_ptr, out_len, &err_val);
            if is_timeout { -13 } else { -10 }
        }
    }
}

// ── http_request ────────────────────────────────────────────────

pub(super) extern "C" fn host_http_request(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let method = input["method"].as_str().unwrap_or("GET").to_string();
    let url = match input["url"].as_str() {
        Some(u) => u.to_string(),
        None => return -3,
    };
    let body = input.get("body").cloned();
    let headers = input["headers"].as_object().cloned().unwrap_or_default();

    // Spawn on Tokio runtime for reactor context
    let (http_tx, http_rx) = std::sync::mpsc::channel();
    let http_client = ctx.http_client.clone();
    ctx.rt_handle.spawn(async move {
        let result = async {
            let mut req = match method.to_uppercase().as_str() {
                "GET" => http_client.get(&url),
                "POST" => http_client.post(&url),
                "PUT" => http_client.put(&url),
                "DELETE" => http_client.delete(&url),
                "PATCH" => http_client.patch(&url),
                "HEAD" => http_client.head(&url),
                _ => http_client.get(&url),
            };

            for (k, v) in &headers {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }

            if let Some(body_val) = body {
                if let Some(s) = body_val.as_str() {
                    req = req.body(s.to_string());
                } else {
                    req = req.json(&body_val);
                }
            }

            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let resp_headers: HashMap<String, String> = resp.headers().iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.to_string(), val.to_string())))
                .collect();
            let resp_body = resp.text().await.map_err(|e| e.to_string())?;

            // Try to parse body as JSON, fall back to string
            let body_val = serde_json::from_str::<serde_json::Value>(&resp_body)
                .unwrap_or_else(|_| serde_json::Value::String(resp_body));

            Ok::<_, String>(serde_json::json!({
                "status": status,
                "headers": resp_headers,
                "body": body_val,
            }))
        }.await;
        let _ = http_tx.send(result);
    });
    match http_rx.recv().unwrap_or_else(|_| Err("http request task panicked".to_string())) {
        Ok(result) => {
            write_output(out_ptr, out_len, &result);
            0
        }
        Err(e) => {
            let err_val = serde_json::json!({"error": e});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
    }
}

// ── log_message ─────────────────────────────────────────────────

pub(super) extern "C" fn host_log_message(
    level: u8, msg_ptr: *const u8, msg_len: usize,
) {
    if msg_ptr.is_null() || msg_len == 0 {
        return;
    }
    // SAFETY: ptr/len pair is guaranteed by the engine ABI to point to a valid
    // byte buffer of exactly `msg_len` bytes for the duration of this call.
    let slice = unsafe { std::slice::from_raw_parts(msg_ptr, msg_len) };
    // Use lossy UTF-8 conversion: a buggy or malicious cdylib sending non-UTF-8
    // bytes must not cause UB here. Invalid sequences become U+FFFD. The log
    // path is not hot enough to need `from_utf8_unchecked`.
    let msg = String::from_utf8_lossy(slice);
    match level {
        0 => tracing::trace!(target: "rivers.engine", "{}", msg),
        1 => tracing::debug!(target: "rivers.engine", "{}", msg),
        2 => tracing::info!(target: "rivers.engine", "{}", msg),
        3 => tracing::warn!(target: "rivers.engine", "{}", msg),
        4 => tracing::error!(target: "rivers.engine", "{}", msg),
        _ => tracing::info!(target: "rivers.engine", "{}", msg),
    }
}

// ── free_buffer ─────────────────────────────────────────────────

pub(super) extern "C" fn host_free_buffer(ptr: *mut u8, len: usize) {
    unsafe { rivers_engine_sdk::free_json_buffer(ptr, len) };
}

// ── keystore_has ────────────────────────────────────────────────

pub(super) extern "C" fn host_keystore_has(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let keystore = match HOST_KEYSTORE.get() {
        Some(ks) => ks,
        None => {
            let result = serde_json::json!({"exists": false});
            write_output(out_ptr, out_len, &result);
            return -1;
        }
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let name = match input["name"].as_str() {
        Some(n) => n,
        None => return -3,
    };

    let exists = keystore.has_key(name);
    let result = serde_json::json!({"exists": exists});
    write_output(out_ptr, out_len, &result);
    0
}

// ── keystore_info ───────────────────────────────────────────────

pub(super) extern "C" fn host_keystore_info(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let keystore = match HOST_KEYSTORE.get() {
        Some(ks) => ks,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let name = match input["name"].as_str() {
        Some(n) => n,
        None => return -3,
    };

    match keystore.key_info(name) {
        Ok(info) => {
            let result = serde_json::json!({
                "name": info.name,
                "type": info.key_type,
                "version": info.current_version,
                "created_at": info.created.to_rfc3339(),
            });
            write_output(out_ptr, out_len, &result);
            0
        }
        Err(e) => {
            let err_val = serde_json::json!({"error": e.to_string()});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
    }
}

// ── crypto_encrypt ──────────────────────────────────────────────

pub(super) extern "C" fn host_crypto_encrypt(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let keystore = match HOST_KEYSTORE.get() {
        Some(ks) => ks,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let key_name = match input["key_name"].as_str() {
        Some(n) => n,
        None => return -3,
    };
    let plaintext = match input["plaintext"].as_str() {
        Some(p) => p,
        None => return -3,
    };
    let aad: Option<String> = input["aad"].as_str().map(|s| s.to_string());
    let aad_bytes = aad.as_ref().map(|a| a.as_bytes());

    match keystore.encrypt_with_key(key_name, plaintext.as_bytes(), aad_bytes) {
        Ok(enc) => {
            let result = serde_json::json!({
                "ciphertext": enc.ciphertext,
                "nonce": enc.nonce,
                "key_version": enc.key_version,
            });
            write_output(out_ptr, out_len, &result);
            0
        }
        Err(e) => {
            let err_val = serde_json::json!({"error": e.to_string()});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
    }
}

// ── crypto_decrypt ──────────────────────────────────────────────

pub(super) extern "C" fn host_crypto_decrypt(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let keystore = match HOST_KEYSTORE.get() {
        Some(ks) => ks,
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let key_name = match input["key_name"].as_str() {
        Some(n) => n,
        None => return -3,
    };
    let ciphertext = match input["ciphertext"].as_str() {
        Some(c) => c,
        None => return -3,
    };
    let nonce = match input["nonce"].as_str() {
        Some(n) => n,
        None => return -3,
    };
    let key_version = match input["key_version"].as_u64() {
        Some(v) => v as u32,
        None => return -3,
    };
    let aad: Option<String> = input["aad"].as_str().map(|s| s.to_string());
    let aad_bytes = aad.as_ref().map(|a| a.as_bytes());

    match keystore.decrypt_with_key(key_name, ciphertext, nonce, key_version, aad_bytes) {
        Ok(plaintext_bytes) => {
            let plaintext = String::from_utf8_lossy(&plaintext_bytes);
            let result = serde_json::json!({"plaintext": plaintext});
            write_output(out_ptr, out_len, &result);
            0
        }
        Err(e) => {
            // Generic error for auth failures — no oracle
            let err_msg = match e {
                rivers_keystore_engine::AppKeystoreError::KeyNotFound { .. } => e.to_string(),
                rivers_keystore_engine::AppKeystoreError::KeyVersionNotFound { .. } => e.to_string(),
                _ => "decryption failed".to_string(),
            };
            let err_val = serde_json::json!({"error": err_msg});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
    }
}

// ── ddl_execute ─────────────────────────────────────────────────

/// Execute a DDL statement (CREATE TABLE, ALTER, DROP, etc.) via the driver.
///
/// Only intended for use by ApplicationInit handlers. The DDL whitelist
/// check is performed by the DataViewExecutor (Gate 3).
///
/// Input: JSON `{"datasource": "my-db", "statement": "CREATE TABLE ...", "app_id": "..."}`
/// Output: JSON `{"ok": true}` on success
pub(super) extern "C" fn host_ddl_execute(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => {
            tracing::error!("host_ddl_execute: HOST_CONTEXT not set");
            return -1;
        }
    };

    let factory = match ctx.driver_factory.as_ref() {
        Some(f) => f,
        None => {
            tracing::error!("host_ddl_execute: DriverFactory not available");
            return -1;
        }
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "host_ddl_execute: failed to read input");
            return -2;
        }
    };

    let datasource = match input["datasource"].as_str() {
        Some(d) => d.to_string(),
        None => {
            tracing::error!("host_ddl_execute: missing 'datasource' field");
            return -3;
        }
    };
    let statement = match input["statement"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_ddl_execute: missing 'statement' field");
            return -3;
        }
    };
    let entry_point_id = input["app_id"].as_str().unwrap_or("unknown").to_string();

    // Resolve entry_point name to manifest UUID for whitelist check.
    // The ProcessPool uses entry_point as app_id, but the whitelist
    // format is `{database}@{appId}` with the manifest UUID.
    let app_id = super::host_context::APP_ID_MAP.get()
        .and_then(|map| map.get(&entry_point_id).cloned())
        .unwrap_or_else(|| entry_point_id.clone());

    // Resolve connection params — try namespaced first (entry_point:datasource),
    // then bare name. Gate 3 whitelist check uses the resolved database name.
    let executor_lock = ctx.dataview_executor.clone();

    // Clone for per-app logging after the async block (originals move into spawn)
    let log_datasource = datasource.clone();
    let log_app_id = app_id.clone();
    let log_statement = statement.clone();

    let (ds_tx, ds_rx) = std::sync::mpsc::channel();
    let factory = Arc::clone(factory);
    let whitelist = super::host_context::DDL_WHITELIST.get().cloned();
    let ddl_handle = ctx.rt_handle.spawn(async move {
        let result = async {
            // Get datasource params from executor
            let (ds_params, driver_name) = {
                let guard = executor_lock.read().await;
                let executor = guard.as_ref()
                    .ok_or_else(|| "DataViewExecutor not initialized".to_string())?;

                // Try exact name first, then suffix match for unqualified names
                let params = executor.datasource_params_get(&datasource)
                    .or_else(|| {
                        let suffix = format!(":{}", datasource);
                        executor.datasource_params_by_suffix(&suffix)
                    })
                    .ok_or_else(|| format!("datasource '{}' not found", datasource))?
                    .clone();

                // Driver name from options or inferred from datasource name
                let driver = params.options.get("driver").cloned()
                    .unwrap_or_else(|| datasource.split(':').last().unwrap_or(&datasource).to_string());

                (params, driver)
            };

            // Gate 3: Check DDL whitelist using the resolved database name
            // (not the JS-level datasource name). Whitelist format: "database@appId"
            if let Some(ref whitelist) = whitelist {
                if !whitelist.is_empty() {
                    let database = &ds_params.database;
                    if !rivers_runtime::rivers_core_config::config::security::is_ddl_permitted(
                        database,
                        &app_id,
                        whitelist,
                    ) {
                        tracing::warn!(
                            datasource = %datasource,
                            database = %database,
                            app_id = %app_id,
                            "DDL rejected by whitelist (Gate 3)"
                        );
                        // Log rejection to per-app log
                        if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
                            let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                            let stmt_preview: String = statement.chars().take(80).collect();
                            let line = format!(
                                r#"{{"timestamp":"{ts}","level":"warn","app":"{app_id}","event":"DdlRejected","datasource":"{datasource}","database":"{database}","statement":"{stmt_preview}","reason":"whitelist_gate3"}}"#
                            );
                            router.write(&app_id, &line);
                        }
                        return Err(format!(
                            "DDL not permitted for database '{}' (datasource '{}') in app '{}'",
                            database, datasource, app_id
                        ));
                    }
                }
            }

            // Execute DDL on the host runtime. See host_dataview_execute's
            // comment for why the previous spawn_blocking+Runtime::new
            // isolation was removed — cdylib plugins are disabled; the
            // dedicated-runtime tear-down was breaking long-lived driver
            // internals on the next query.
            let ds_name = datasource.clone();
            let stmt = statement.clone();
            let factory_clone = factory.clone();
            let mut conn = factory_clone.connect(&driver_name, &ds_params).await
                .map_err(|e| format!("DDL connect to '{}' failed: {}", ds_name, e))?;
            let query = rivers_runtime::rivers_driver_sdk::Query::new("ddl", &stmt);
            conn.ddl_execute(&query).await
                .map_err(|e| format!("DDL execute failed: {}", e))?;

            tracing::info!(
                datasource = %datasource,
                app_id = %app_id,
                statement = %statement.chars().take(80).collect::<String>(),
                "DDL executed successfully"
            );
            Ok::<_, String>(())
        }.await;
        let _ = ds_tx.send(result);
    });

    // Write DDL result to per-app log via AppLogRouter.
    //
    // H2: bounded recv — a stalled DDL driver no longer pins the worker.
    // On timeout the spawned task is aborted and -13 is returned.
    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    let ddl_result = ds_rx.recv_timeout(budget);
    let stmt_preview: String = log_statement.chars().take(80).collect();

    match ddl_result {
        Ok(Ok(())) => {
            // Log success to per-app log
            if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
                let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let line = format!(
                    r#"{{"timestamp":"{ts}","level":"info","app":"{log_app_id}","event":"DdlExecuted","datasource":"{log_datasource}","statement":"{stmt_preview}","status":"ok"}}"#
                );
                router.write(&log_app_id, &line);
            }
            let result = serde_json::json!({"ok": true});
            write_output(out_ptr, out_len, &result);
            0
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "host_ddl_execute failed");
            // Log failure to per-app log
            if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
                let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let escaped_err = e.replace('"', r#"\""#);
                let line = format!(
                    r#"{{"timestamp":"{ts}","level":"error","app":"{log_app_id}","event":"DdlFailed","datasource":"{log_datasource}","statement":"{stmt_preview}","error":"{escaped_err}"}}"#
                );
                router.write(&log_app_id, &line);
            }
            let err_val = serde_json::json!({"error": e});
            write_output(out_ptr, out_len, &err_val);
            -10
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            ddl_handle.abort();
            tracing::error!(
                datasource = %log_datasource,
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_ddl_execute: timed out — spawned task aborted"
            );
            let err_val = serde_json::json!({
                "error": format!(
                    "host callback 'host_ddl_execute' timed out after {}ms",
                    HOST_CALLBACK_TIMEOUT_MS
                )
            });
            write_output(out_ptr, out_len, &err_val);
            -13
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            tracing::error!("host_ddl_execute: channel disconnected — task panicked");
            -12
        }
    }
}

// ── db_begin ────────────────────────────────────────────────────

/// Rivers.db.begin("datasource") — begin a transaction on a datasource.
///
/// Input: JSON `{"datasource": "..."}`
/// Output: JSON `{"ok": true, "datasource": "..."}` on success;
/// `{"error": "..."}` on failure (with negative i32 return code).
///
/// Phase I3 — wires to `DynTransactionMap` (see
/// `crates/riversd/src/engine_loader/dyn_transaction_map.rs` and
/// `changedecisionlog.md` TXN-I1.1). Mirrors the V8-side semantics of
/// `ctx_transaction_callback` in `process_pool/v8_engine/context.rs`.
pub(super) extern "C" fn host_db_begin(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => {
            tracing::error!("host_db_begin: HOST_CONTEXT not set");
            return -1;
        }
    };
    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "host_db_begin: failed to read input");
            return -2;
        }
    };
    match host_db_begin_inner(&input, ctx) {
        Ok(value) => {
            write_output(out_ptr, out_len, &value);
            0
        }
        Err((code, value)) => {
            write_output(out_ptr, out_len, &value);
            code
        }
    }
}

/// Body of `host_db_begin` separated from the FFI shim so it can be unit-tested
/// without crossing the `*const u8` / `*mut u8` boundary.
fn host_db_begin_inner(
    input: &serde_json::Value,
    ctx: &HostContext,
) -> Result<serde_json::Value, (i32, serde_json::Value)> {
    let datasource = match input["datasource"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_begin: missing 'datasource' field");
            return Err((
                -3,
                serde_json::json!({"error": "missing 'datasource' field"}),
            ));
        }
    };

    let task_id = match current_task_id() {
        Some(id) => id,
        None => {
            tracing::error!(
                datasource = %datasource,
                "host_db_begin called outside a TaskGuard scope"
            );
            return Err((
                -1,
                serde_json::json!({
                    "error": "host_db_begin called outside a TaskGuard scope (programmer error)"
                }),
            ));
        }
    };

    let (driver_name, params) = match lookup_task_ds(task_id, &datasource) {
        Some(t) => t,
        None => {
            tracing::error!(
                ?task_id,
                datasource = %datasource,
                "host_db_begin: unknown datasource for current task"
            );
            return Err((
                -3,
                serde_json::json!({
                    "error": format!(
                        "unknown datasource '{datasource}' for current task"
                    )
                }),
            ));
        }
    };

    let factory = match ctx.driver_factory.as_ref() {
        Some(f) => f.clone(),
        None => {
            tracing::error!("host_db_begin: DriverFactory not available");
            return Err((
                -1,
                serde_json::json!({"error": "DriverFactory not available"}),
            ));
        }
    };

    // We are on a `spawn_blocking` worker (NOT a tokio runtime worker), so
    // `block_on` is the safe sync→async bridge here, matching V8's pattern
    // and the design note in `host_context::TaskGuard::drop`.
    let connect_result = ctx.rt_handle.block_on(async {
        let mut conn = factory.connect(&driver_name, &params).await?;
        conn.begin_transaction().await?;
        Ok::<_, rivers_runtime::rivers_driver_sdk::DriverError>(conn)
    });

    let conn = match connect_result {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                ?task_id,
                datasource = %datasource,
                error = %e,
                "host_db_begin: begin failed"
            );
            return Err((
                -1,
                serde_json::json!({
                    "error": format!("begin failed: {e}"),
                }),
            ));
        }
    };

    if let Err(e) = dyn_txn_map().insert(task_id, &datasource, conn) {
        // `insert` only takes ownership of `conn` on success — on
        // `AlreadyActive` the conn is returned to us via Drop. We have no
        // separate handle here because the by-value `conn` is consumed by the
        // `insert(...)` call before this branch runs. Looking at I2's actual
        // signature: `insert(&self, task_id, ds, conn) -> Result<(), DynTxnError>`.
        // Rust's by-value semantics: on `Err` the function still returns,
        // and the moved `conn` is dropped inside `insert` when it falls out
        // of scope at the function boundary. The conn we just begun is
        // therefore dropped without a rollback — its `Drop` releases the
        // pool slot but the txn on the server side will be reaped by the
        // driver's idle/abort timeout. For PG/MySQL pools that close the
        // connection on Drop this is correct.
        tracing::error!(
            ?task_id,
            datasource = %datasource,
            error = %e,
            "host_db_begin: dyn_txn_map insert failed"
        );
        return Err((
            -1,
            serde_json::json!({"error": format!("{e}")}),
        ));
    }

    Ok(serde_json::json!({"ok": true, "datasource": datasource}))
}

// ── db_commit ───────────────────────────────────────────────────

/// Rivers.db.commit("datasource") — commit an active transaction.
///
/// Input: JSON `{"datasource": "..."}`
/// Output: JSON `{"ok": true}` on success;
/// `{"error": "...", "fatal": true}` on driver-level commit failure or timeout
/// (financial-correctness gate — `signal_commit_failed` is set so dispatch
/// can upgrade `TaskError::HandlerError` to `TaskError::TransactionCommitFailed`).
///
/// Phase I4. Mirrors V8 commit semantics in
/// `process_pool/v8_engine/context.rs::ctx_transaction_callback` (clean-return
/// branch).
pub(super) extern "C" fn host_db_commit(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => {
            tracing::error!("host_db_commit: HOST_CONTEXT not set");
            return -1;
        }
    };
    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "host_db_commit: failed to read input");
            return -2;
        }
    };
    match host_db_commit_inner(&input, ctx) {
        Ok(value) => {
            write_output(out_ptr, out_len, &value);
            0
        }
        Err((code, value)) => {
            write_output(out_ptr, out_len, &value);
            code
        }
    }
}

fn host_db_commit_inner(
    input: &serde_json::Value,
    ctx: &HostContext,
) -> Result<serde_json::Value, (i32, serde_json::Value)> {
    let datasource = match input["datasource"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_commit: missing 'datasource' field");
            return Err((
                -3,
                serde_json::json!({"error": "missing 'datasource' field"}),
            ));
        }
    };

    let task_id = match current_task_id() {
        Some(id) => id,
        None => {
            tracing::error!(
                datasource = %datasource,
                "host_db_commit called outside a TaskGuard scope"
            );
            return Err((
                -1,
                serde_json::json!({
                    "error": "host_db_commit called outside a TaskGuard scope (programmer error)"
                }),
            ));
        }
    };

    let conn = match dyn_txn_map().take(task_id, &datasource) {
        Some(c) => c,
        None => {
            return Err((
                -1,
                serde_json::json!({
                    "error": format!(
                        "no active transaction for datasource '{datasource}' on current task"
                    )
                }),
            ));
        }
    };

    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    let result = ctx.rt_handle.block_on(async move {
        let mut conn = conn;
        tokio::time::timeout(budget, conn.commit_transaction()).await
        // `conn` drops at the end of this async block — its pool slot is
        // released. Matches V8 semantics where commit() returns the conn
        // back to the caller, who lets it drop.
    });

    match result {
        Ok(Ok(())) => Ok(serde_json::json!({"ok": true})),
        Ok(Err(e)) => {
            // Driver-level commit failure: writes may or may not have
            // persisted. Stash for dispatch upgrade.
            let driver_msg = e.to_string();
            tracing::error!(
                ?task_id,
                datasource = %datasource,
                error = %driver_msg,
                "host_db_commit: driver error"
            );
            signal_commit_failed(datasource.clone(), driver_msg.clone());
            Err((
                -1,
                serde_json::json!({
                    "error": format!(
                        "TransactionError: commit failed on datasource '{datasource}': {driver_msg}"
                    ),
                    "fatal": true,
                }),
            ))
        }
        Err(_elapsed) => {
            let driver_msg = format!(
                "commit timed out after {}ms",
                HOST_CALLBACK_TIMEOUT_MS
            );
            tracing::error!(
                ?task_id,
                datasource = %datasource,
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_db_commit: timed out"
            );
            signal_commit_failed(datasource.clone(), driver_msg);
            Err((
                -1,
                serde_json::json!({
                    "error": format!(
                        "TransactionError: commit on datasource '{datasource}' timed out after {}ms",
                        HOST_CALLBACK_TIMEOUT_MS
                    ),
                    "fatal": true,
                }),
            ))
        }
    }
}

// ── db_rollback ─────────────────────────────────────────────────

/// Rivers.db.rollback("datasource") — rollback an active transaction.
///
/// Input: JSON `{"datasource": "..."}`
/// Output: JSON `{"ok": true}` on success (or when no txn is active —
/// rollback is idempotent).
///
/// Phase I5. Rollback failures (driver error or timeout) are warn-logged but
/// do **not** trigger the commit-failed signal: a failed rollback leaves the
/// transaction not-committed (writes never reached durable storage), so
/// persistence determinism is unaffected.
pub(super) extern "C" fn host_db_rollback(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => {
            tracing::error!("host_db_rollback: HOST_CONTEXT not set");
            return -1;
        }
    };
    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "host_db_rollback: failed to read input");
            return -2;
        }
    };
    match host_db_rollback_inner(&input, ctx) {
        Ok(value) => {
            write_output(out_ptr, out_len, &value);
            0
        }
        Err((code, value)) => {
            write_output(out_ptr, out_len, &value);
            code
        }
    }
}

fn host_db_rollback_inner(
    input: &serde_json::Value,
    ctx: &HostContext,
) -> Result<serde_json::Value, (i32, serde_json::Value)> {
    let datasource = match input["datasource"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_rollback: missing 'datasource' field");
            return Err((
                -3,
                serde_json::json!({"error": "missing 'datasource' field"}),
            ));
        }
    };

    let task_id = match current_task_id() {
        Some(id) => id,
        None => {
            tracing::error!(
                datasource = %datasource,
                "host_db_rollback called outside a TaskGuard scope"
            );
            return Err((
                -1,
                serde_json::json!({
                    "error": "host_db_rollback called outside a TaskGuard scope (programmer error)"
                }),
            ));
        }
    };

    let conn = match dyn_txn_map().take(task_id, &datasource) {
        Some(c) => c,
        None => {
            // Idempotent: rolling back when no txn is active is a no-op.
            return Ok(serde_json::json!({"ok": true}));
        }
    };

    let budget = Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS);
    let result = ctx.rt_handle.block_on(async move {
        let mut conn = conn;
        tokio::time::timeout(budget, conn.rollback_transaction()).await
    });

    match result {
        Ok(Ok(())) => Ok(serde_json::json!({"ok": true})),
        Ok(Err(e)) => {
            tracing::warn!(
                ?task_id,
                datasource = %datasource,
                error = %e,
                "host_db_rollback: driver error — connection abandoned"
            );
            Ok(serde_json::json!({
                "ok": true,
                "warning": format!("rollback driver error: {e}"),
            }))
        }
        Err(_elapsed) => {
            tracing::warn!(
                ?task_id,
                datasource = %datasource,
                budget_ms = HOST_CALLBACK_TIMEOUT_MS,
                "host_db_rollback: timed out — connection abandoned"
            );
            Ok(serde_json::json!({
                "ok": true,
                "warning": format!(
                    "rollback timed out after {}ms",
                    HOST_CALLBACK_TIMEOUT_MS
                ),
            }))
        }
    }
}

// ── db_batch ────────────────────────────────────────────────────

/// Rivers.db.batch("dataview", [...params]) — execute a DataView with multiple parameter sets.
///
/// Input: JSON `{"dataview": "...", "params": [{...}, {...}]}`
/// Output: JSON array of results or error
///
/// Note (Phase I): `Rivers.db.batch` is a DataView batch-execute primitive,
/// not a transaction wrapper. Each `dataview` invocation under the same
/// `batch` call would land as N independent DataView executes (each its
/// own transaction at the driver level). To run a batch *inside* a
/// transaction, the caller wraps it in `Rivers.db.begin(ds)` /
/// `Rivers.db.commit(ds)` explicitly and the DataView execute path
/// (`host_dataview_execute`) routes through the held connection. The
/// batch primitive itself remains a stub pending DataView batch wiring
/// at the engine layer (separate from the transaction work).
pub(super) extern "C" fn host_db_batch(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let _ctx = match HOST_CONTEXT.get() {
        Some(c) => c,
        None => {
            tracing::error!("host_db_batch: HOST_CONTEXT not set");
            return -1;
        }
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "host_db_batch: failed to read input");
            return -2;
        }
    };

    let dataview = match input["dataview"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_batch: missing 'dataview' field");
            let err = serde_json::json!({"error": "missing 'dataview' field"});
            write_output(out_ptr, out_len, &err);
            return -3;
        }
    };

    let params = match input["params"].as_array() {
        Some(p) => p.clone(),
        None => {
            tracing::error!("host_db_batch: missing or non-array 'params' field");
            let err = serde_json::json!({"error": "missing or non-array 'params' field"});
            write_output(out_ptr, out_len, &err);
            return -3;
        }
    };

    // Stub — see fn-doc above. Phase I scope is transactions; the
    // DataView batch-execute primitive lands separately.
    tracing::debug!(dataview = %dataview, count = params.len(), "Rivers.db.batch (stub)");
    let result = serde_json::json!({"ok": true, "dataview": dataview, "count": params.len()});
    write_output(out_ptr, out_len, &result);
    0
}

// ── Tests ───────────────────────────────────────────────────────
//
// Phase I3+I4+I5: exercise `host_db_begin_inner`, `host_db_commit_inner`,
// and `host_db_rollback_inner` against a mock `DatabaseDriver` /
// `Connection`. The FFI shims are 5-line wrappers (`read_input` → inner →
// `write_output`) and don't need their own coverage; the inner-fn pattern
// keeps these tests free of `*const u8` / `*mut u8` ceremony.

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rivers_runtime::rivers_core::DriverFactory;
    use rivers_runtime::rivers_driver_sdk::{
        Connection, ConnectionParams, DatabaseDriver, DriverError, Query, QueryResult,
    };
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, OnceLock};

    use super::host_context::{
        dyn_txn_map, set_current_task_id_for_test, store_task_ds_configs, take_commit_failed,
        DatasourceConfigsSnapshot,
    };
    use super::super::dyn_transaction_map::TaskId;
    use super::super::txn_test_fixtures;

    /// Shared behavior + setup come from `engine_loader::txn_test_fixtures`
    /// so the I3-I6 tests in this file and the I7 dispatch tests in
    /// `process_pool/mod.rs` can share one `HOST_CONTEXT` init (the OnceLock
    /// only fires once per test binary).
    fn ensure_host_context() {
        let _ = txn_test_fixtures::ensure_host_context();
    }

    /// Behavior knob — forwarded from the shared fixture.
    fn shared_behavior() -> Arc<txn_test_fixtures::SharedConnBehavior> {
        txn_test_fixtures::behavior()
    }

    /// Build a per-test datasource snapshot pointing `ds_name` at
    /// `driver_name`. Connection params are dummies — the mock driver
    /// ignores them.
    fn stash_ds(task_id: TaskId, ds_name: &str, driver_name: &str) {
        let mut configs = std::collections::HashMap::new();
        configs.insert(
            ds_name.to_string(),
            (
                driver_name.to_string(),
                ConnectionParams {
                    host: "test".into(),
                    port: 0,
                    database: "test".into(),
                    username: "test".into(),
                    password: "test".into(),
                    options: Default::default(),
                },
            ),
        );
        store_task_ds_configs(task_id, DatasourceConfigsSnapshot { configs });
    }

    /// Atomic counter for synthesizing per-test TaskIds without colliding
    /// with the production NEXT_TASK_ID counter (we pick high values).
    fn fresh_task_id() -> TaskId {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000_000);
        TaskId(N.fetch_add(1, Ordering::Relaxed))
    }

    /// Tests share a single mock driver, so they cannot reliably run in
    /// parallel when each toggles the shared `commit_fails` knob. The
    /// shared lock from `txn_test_fixtures` serializes I3-I6 here AND
    /// the I7 dispatch tests in `process_pool/mod.rs` so the behavior knob
    /// and CURRENT_TASK_ID thread-local don't collide across modules.
    fn TEST_LOCK() -> &'static std::sync::Mutex<()> {
        txn_test_fixtures::test_lock()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn begin_then_commit_happy_path() {
        let _g = TEST_LOCK().lock().unwrap_or_else(|p| p.into_inner());
        ensure_host_context();
        shared_behavior().commit_fails.store(false, Ordering::Relaxed);

        let task = fresh_task_id();
        let ds = "pg_happy";
        stash_ds(task, ds, "mock-txn-driver");

        // Run the host fn bodies on a `spawn_blocking` worker — this matches
        // the production environment: `block_on` (used inside the inner fns)
        // is safe on a `spawn_blocking` thread but would deadlock on a tokio
        // runtime worker. The CURRENT_TASK_ID thread-local must be set on
        // the same thread that calls the inner fn.
        let result = tokio::task::spawn_blocking(move || {
            set_current_task_id_for_test(Some(task));
            // Clear any leftover commit-failed signal from prior runs on this
            // worker thread (spawn_blocking workers are reused across tests).
            let _ = take_commit_failed();
            let ctx = HOST_CONTEXT.get().expect("HOST_CONTEXT");

            let begin = host_db_begin_inner(
                &serde_json::json!({"datasource": ds}),
                ctx,
            )
            .expect("begin ok");
            assert_eq!(begin["ok"], true);
            assert!(dyn_txn_map().has(task, ds), "txn must be recorded");

            let commit = host_db_commit_inner(
                &serde_json::json!({"datasource": ds}),
                ctx,
            )
            .expect("commit ok");
            assert_eq!(commit["ok"], true);
            assert!(
                !dyn_txn_map().has(task, ds),
                "txn must be removed after commit"
            );

            let cf = take_commit_failed();
            set_current_task_id_for_test(None);
            cf
        })
        .await
        .unwrap();

        assert!(
            result.is_none(),
            "happy-path commit must NOT signal commit-failed; got {result:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn begin_then_rollback() {
        let _g = TEST_LOCK().lock().unwrap_or_else(|p| p.into_inner());
        ensure_host_context();
        shared_behavior().commit_fails.store(false, Ordering::Relaxed);

        let task = fresh_task_id();
        let ds = "pg_rollback";
        stash_ds(task, ds, "mock-txn-driver");

        let result = tokio::task::spawn_blocking(move || {
            set_current_task_id_for_test(Some(task));
            let _ = take_commit_failed();
            let ctx = HOST_CONTEXT.get().expect("HOST_CONTEXT");

            let begin = host_db_begin_inner(
                &serde_json::json!({"datasource": ds}),
                ctx,
            )
            .expect("begin ok");
            assert_eq!(begin["ok"], true);
            assert!(dyn_txn_map().has(task, ds));

            let rb = host_db_rollback_inner(
                &serde_json::json!({"datasource": ds}),
                ctx,
            )
            .expect("rollback ok");
            assert_eq!(rb["ok"], true);
            assert!(!dyn_txn_map().has(task, ds));

            let cf = take_commit_failed();
            set_current_task_id_for_test(None);
            cf
        })
        .await
        .unwrap();

        assert!(
            result.is_none(),
            "rollback must NOT signal commit-failed; got {result:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn begin_then_commit_fails_signals_commit_failed() {
        let _g = TEST_LOCK().lock().unwrap_or_else(|p| p.into_inner());
        ensure_host_context();
        // Force the commit path to fail.
        shared_behavior().commit_fails.store(true, Ordering::Relaxed);

        let task = fresh_task_id();
        let ds = "pg_commit_fails";
        stash_ds(task, ds, "mock-txn-driver");

        let outcome = tokio::task::spawn_blocking(move || {
            set_current_task_id_for_test(Some(task));
            let _ = take_commit_failed();
            let ctx = HOST_CONTEXT.get().expect("HOST_CONTEXT");

            host_db_begin_inner(
                &serde_json::json!({"datasource": ds}),
                ctx,
            )
            .expect("begin ok");

            let commit = host_db_commit_inner(
                &serde_json::json!({"datasource": ds}),
                ctx,
            );
            let cf = take_commit_failed();
            set_current_task_id_for_test(None);
            (commit, cf)
        })
        .await
        .unwrap();

        // Reset behavior so the next test starts clean.
        shared_behavior().commit_fails.store(false, Ordering::Relaxed);

        let (commit, cf) = outcome;
        let (code, body) = commit.expect_err("commit must fail");
        assert_eq!(code, -1);
        assert_eq!(body["fatal"], true);
        let err_msg = body["error"].as_str().unwrap_or_default();
        assert!(
            err_msg.contains("TransactionError"),
            "error must mention TransactionError; got {err_msg}"
        );

        let (signal_ds, signal_msg) =
            cf.expect("commit-failed signal must be set on driver-error commit");
        assert_eq!(signal_ds, ds);
        assert!(
            signal_msg.contains("forced commit failure"),
            "commit-failed reason must propagate driver msg; got {signal_msg}"
        );
        // After commit, the entry must be gone (take() removed it before
        // commit_transaction ran).
        assert!(!dyn_txn_map().has(task, ds));
    }

    // ── I6 tests: dataview-in-txn vs no-txn routing ────────────────
    //
    // These cover `execute_dataview_with_optional_txn`, the helper that
    // sits between `host_dataview_execute` and `DataViewExecutor::execute`.
    // The unit-level pattern here is: pre-insert a mock connection into
    // `DYN_TXN_MAP` (skipping `host_db_begin`), bind the test task id, and
    // verify the executor used the inserted conn (its per-conn counter
    // advanced) vs a fresh `factory.connect(...)` conn (the inserted
    // conn's counter stays at 0 and the driver's "connect count" advances).

    use rivers_runtime::dataview_engine::{DataViewExecutor, DataViewRegistry};
    use rivers_runtime::tiered_cache::{DataViewCache, NoopDataViewCache};
    use rivers_runtime::DataViewConfig;

    /// Per-connection execute counter shared between MockTxnConn and the
    /// test that inserts it into DYN_TXN_MAP, so the test can verify the
    /// closure inside `with_conn_mut` actually saw THIS connection.
    #[derive(Default)]
    struct ExecCounter(std::sync::atomic::AtomicU64);

    impl ExecCounter {
        fn get(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    /// Mock connection for I6 tests. Each instance carries an ExecCounter
    /// so the test can tell connections apart.
    struct DvMockConn {
        counter: Arc<ExecCounter>,
    }

    #[async_trait]
    impl Connection for DvMockConn {
        async fn execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
            self.counter.0.fetch_add(1, Ordering::Relaxed);
            Ok(QueryResult {
                rows: vec![],
                affected_rows: 0,
                last_insert_id: None,
                column_names: None,
            })
        }
        async fn ping(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn driver_name(&self) -> &str {
            "dv-mock"
        }
        async fn begin_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn commit_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    /// Driver that returns connections sharing one "fresh-conn" counter so
    /// the test can detect when the executor went around the txn map and
    /// acquired a fresh pool connection instead.
    struct DvMockDriver {
        fresh_counter: Arc<ExecCounter>,
        connect_count: Arc<std::sync::atomic::AtomicU64>,
    }

    #[async_trait]
    impl DatabaseDriver for DvMockDriver {
        fn name(&self) -> &str {
            "dv-mock-driver"
        }
        async fn connect(
            &self,
            _params: &ConnectionParams,
        ) -> Result<Box<dyn Connection>, DriverError> {
            self.connect_count.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(DvMockConn {
                counter: self.fresh_counter.clone(),
            }))
        }
        fn supports_transactions(&self) -> bool {
            true
        }
    }

    /// Bare-minimum DataViewConfig pointing at `datasource`. Only `name`
    /// and `datasource` are load-bearing for our test path; the executor
    /// takes the GET branch with an empty statement and the mock conn's
    /// `execute` returns empty rows regardless.
    fn make_dv_config(name: &str, datasource: &str) -> DataViewConfig {
        DataViewConfig {
            name: name.into(),
            datasource: datasource.into(),
            query: Some(String::new()),
            parameters: vec![],
            return_schema: None,
            get_query: Some(String::new()),
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
            circuit_breaker_id: None,
            prepared: false,
            query_params: Default::default(),
            caching: None,
            invalidates: vec![],
            validate_result: false,
            strict_parameters: false,
            max_rows: 1000,
        }
    }

    /// Build a DataViewExecutor wired to `DvMockDriver` and a single
    /// dataview definition.
    fn build_test_executor(
        dataview_name: &str,
        ds: &str,
    ) -> (
        Arc<DataViewExecutor>,
        Arc<ExecCounter>,
        Arc<std::sync::atomic::AtomicU64>,
    ) {
        let fresh_counter = Arc::new(ExecCounter::default());
        let connect_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
        factory.register_database_driver(Arc::new(DvMockDriver {
            fresh_counter: fresh_counter.clone(),
            connect_count: connect_count.clone(),
        }));

        let mut registry = DataViewRegistry::new();
        registry.register(make_dv_config(dataview_name, ds));

        // Connection params for this datasource. The "driver" option steers
        // DataViewExecutor::execute to look up the right driver in the
        // factory; without it the executor would fall back to using
        // `config.datasource` as the driver name.
        let mut options = std::collections::HashMap::new();
        options.insert("driver".to_string(), "dv-mock-driver".to_string());
        let params = ConnectionParams {
            host: "test".into(),
            port: 0,
            database: "test".into(),
            username: "test".into(),
            password: "test".into(),
            options,
        };
        let mut params_map = std::collections::HashMap::new();
        params_map.insert(ds.to_string(), params);

        let cache: Arc<dyn DataViewCache> = Arc::new(NoopDataViewCache);
        let exec = DataViewExecutor::new(
            registry,
            Arc::new(factory),
            Arc::new(params_map),
            cache,
        );
        (Arc::new(exec), fresh_counter, connect_count)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dataview_in_txn_uses_txn_conn() {
        let _g = TEST_LOCK().lock().unwrap_or_else(|p| p.into_inner());
        let task = fresh_task_id();
        let ds = "dv_pg_in_txn";
        let (executor, fresh_counter, connect_count) =
            build_test_executor("list_records", ds);

        // Pre-seat the txn map with a connection whose counter we can
        // distinguish from any fresh pool conn. This skips host_db_begin
        // because the helper under test is the dataview-side wiring, not
        // the begin path (covered by I3 tests).
        let txn_counter = Arc::new(ExecCounter::default());
        dyn_txn_map()
            .insert(
                task,
                ds,
                Box::new(DvMockConn {
                    counter: txn_counter.clone(),
                }),
            )
            .expect("seed txn map");

        let result = super::execute_dataview_with_optional_txn(
            executor,
            "list_records",
            std::collections::HashMap::new(),
            "test-trace",
            Some(task),
        )
        .await
        .expect("dataview execute ok");
        assert_eq!(result.query_result.affected_rows, 0);

        // The TXN conn's counter must have advanced.
        assert_eq!(
            txn_counter.get(),
            1,
            "txn conn should have been used"
        );
        // No fresh pool connection should have been acquired.
        assert_eq!(
            connect_count.load(Ordering::Relaxed),
            0,
            "factory.connect must NOT have been called when txn is active"
        );
        assert_eq!(
            fresh_counter.get(),
            0,
            "no fresh pool conn was created, so its counter must be 0"
        );

        // Map entry must still be present (with_conn_mut re-inserts after
        // running the closure).
        assert!(dyn_txn_map().has(task, ds), "txn entry must be retained");

        // Cleanup: drain so test leaves no residue for the next test.
        let _ = dyn_txn_map().drain_task(task);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dataview_no_txn_uses_fresh_conn() {
        let _g = TEST_LOCK().lock().unwrap_or_else(|p| p.into_inner());
        let task = fresh_task_id();
        let ds = "dv_pg_no_txn";
        let (executor, fresh_counter, connect_count) =
            build_test_executor("list_records", ds);

        // Sanity: no txn for this task.
        assert!(!dyn_txn_map().has(task, ds));

        let _ = super::execute_dataview_with_optional_txn(
            executor,
            "list_records",
            std::collections::HashMap::new(),
            "test-trace",
            Some(task),
        )
        .await
        .expect("dataview execute ok");

        assert_eq!(
            connect_count.load(Ordering::Relaxed),
            1,
            "factory.connect must be called exactly once when no txn is active"
        );
        assert_eq!(
            fresh_counter.get(),
            1,
            "fresh conn's counter advances on the non-txn path"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dataview_cross_datasource_in_txn_rejects() {
        let _g = TEST_LOCK().lock().unwrap_or_else(|p| p.into_inner());
        let task = fresh_task_id();
        let dv_ds = "dv_ds_a";
        let other_ds = "dv_ds_b";
        let (executor, _fresh, connect_count) =
            build_test_executor("list_records", dv_ds);

        // Pre-seat a txn on the OTHER datasource — dataview wants dv_ds.
        let txn_counter = Arc::new(ExecCounter::default());
        dyn_txn_map()
            .insert(
                task,
                other_ds,
                Box::new(DvMockConn {
                    counter: txn_counter.clone(),
                }),
            )
            .expect("seed txn map");

        let err = super::execute_dataview_with_optional_txn(
            executor,
            "list_records",
            std::collections::HashMap::new(),
            "test-trace",
            Some(task),
        )
        .await
        .expect_err("cross-datasource call must reject");

        let msg = format!("{err:?}");
        assert!(
            msg.contains("TransactionError")
                && msg.contains(dv_ds)
                && msg.contains(other_ds),
            "error must mention both datasources; got {msg}"
        );
        // Crucially: no fresh pool conn was acquired (rejection happens
        // before factory.connect).
        assert_eq!(connect_count.load(Ordering::Relaxed), 0);
        // The seeded txn conn was never used either — it stays at 0.
        assert_eq!(txn_counter.get(), 0);

        // Cleanup.
        let _ = dyn_txn_map().drain_task(task);
    }

    // ── H2 tests: dyn-engine recv_timeout primitive ────────────────
    //
    // These tests exercise the `recv_timeout` primitive used in the
    // dynamic-engine host callbacks (host_store_get, host_store_set,
    // host_store_del, host_dataview_execute, host_datasource_build,
    // host_ddl_execute). They prove that a spawned async task that never
    // sends a result surfaces as RecvTimeoutError::Timeout rather than
    // blocking forever.

    /// H2 (T1-6 dyn-engine): a channel whose sender is held but never
    /// used — simulating a background Tokio task that stalls — returns
    /// RecvTimeoutError::Timeout from recv_timeout, proving the
    /// dyn-engine host callbacks do NOT block forever.
    #[test]
    fn dyn_engine_recv_timeout_returns_timeout_when_task_hangs() {
        let (_tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        // _tx is kept alive (not dropped) so the channel stays connected —
        // this mirrors a spawned Tokio task that is alive but never sends.
        let budget = std::time::Duration::from_millis(50);
        let result = rx.recv_timeout(budget);
        assert!(
            matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "expected Timeout, got {:?}",
            result
        );
    }

    /// H2 (T1-6 dyn-engine): HOST_CALLBACK_TIMEOUT_MS is the canonical
    /// timeout shared across both V8 and dyn-engine paths. Assert it is
    /// nonzero and within a sane range so a bump to 0 or to hours would
    /// break this test and force a deliberate decision.
    #[test]
    fn dyn_engine_host_callback_budget_is_bounded_and_nonzero() {
        assert!(HOST_CALLBACK_TIMEOUT_MS > 0);
        // Sanity: we expect this to be in seconds-not-hours range.
        assert!(HOST_CALLBACK_TIMEOUT_MS <= 5 * 60 * 1000);
    }
}
