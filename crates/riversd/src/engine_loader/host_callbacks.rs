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

    // Spawn execution on the Tokio runtime and wait for the result.
    // This is critical: some drivers (e.g. MongoDB, Elasticsearch) require a
    // Tokio reactor on the calling thread. `block_on()` alone doesn't set the
    // thread-local reactor context, but `spawn` runs on a proper Tokio worker
    // thread where the reactor IS available.
    //
    // If the spawned task panics, the tx sender is dropped without sending,
    // causing rx.recv() to return Err — which we handle as error code -12.
    let (tx, rx) = std::sync::mpsc::channel();
    ctx.rt_handle.spawn({
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
                // Try the bare name first
                match executor.execute(&name, params.clone(), "GET", &trace_id, None).await {
                    Ok(r) => Ok(r),
                    Err(rivers_runtime::DataViewError::NotFound { .. }) => {
                        // DataViews are registered as "{entry_point}:{name}" — try with prefix
                        if let Some(prefix) = &app_prefix {
                            let namespaced = format!("{}:{}", prefix, name);
                            executor.execute(&namespaced, params, "GET", &trace_id, None).await
                                .map_err(|e| format!("{e:?}"))
                        } else {
                            // No prefix hint — scan for any match ending in ":{name}"
                            let suffix = format!(":{}", name);
                            if let Some(full_name) = executor.find_by_suffix(&suffix) {
                                executor.execute(&full_name, params, "GET", &trace_id, None).await
                                    .map_err(|e| format!("{e:?}"))
                            } else {
                                Err(format!("DataView '{}' not found (tried bare and namespaced)", name))
                            }
                        }
                    }
                    Err(e) => Err(format!("{e:?}")),
                }
            }.await;
            let _ = tx.send(result);
        }
    });

    // Wait for the spawned task to complete (blocks the V8 thread, which is fine —
    // this is the same blocking behavior as the previous block_on approach)
    match rx.recv() {
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
        Err(e) => {
            tracing::error!(dataview = %name, error = %e, "host_dataview_execute: channel recv failed");
            -12
        }
    }
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
    ctx.rt_handle.spawn(async move {
        let _ = tx.send(engine.get(&ns, &k).await);
    });
    let store_result = match rx.recv() {
        Ok(r) => r,
        Err(_) => return -10, // channel dropped — task panicked
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
    ctx.rt_handle.spawn(async move {
        let _ = tx.send(engine.set(&ns, &k, value_bytes, ttl_ms).await);
    });
    match rx.recv() {
        Ok(Ok(())) => 0,
        Ok(Err(_)) => -10,
        Err(_) => -10, // channel dropped — task panicked
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
    ctx.rt_handle.spawn(async move {
        let _ = tx.send(engine.delete(&ns, &k).await);
    });
    match rx.recv() {
        Ok(Ok(_)) => 0,
        Ok(Err(_)) => -10,
        Err(_) => -10, // channel dropped — task panicked
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
    let (ds_tx, ds_rx) = std::sync::mpsc::channel();
    let factory = Arc::clone(factory);
    ctx.rt_handle.spawn(async move {
        let result = async {
            let mut conn = factory.connect(&driver, &conn_params).await
                .map_err(|e| format!("driver connect failed: {e}"))?;
            conn.execute(&query).await.map_err(|e| e.to_string())
        }.await;
        let _ = ds_tx.send(result);
    });
    match ds_rx.recv().unwrap_or_else(|_| Err("datasource task panicked".to_string())) {
        Ok(result) => {
            let json_result = serde_json::json!({
                "rows": result.rows,
                "affected_rows": result.affected_rows,
            });
            write_output(out_ptr, out_len, &json_result);
            0
        }
        Err(e) => {
            let err_val = serde_json::json!({"error": e});
            write_output(out_ptr, out_len, &err_val);
            -10
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
    ctx.rt_handle.spawn(async move {
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

    // Write DDL result to per-app log via AppLogRouter
    let ddl_result = ds_rx.recv();
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
        Err(e) => {
            tracing::error!(error = %e, "host_ddl_execute: channel recv failed");
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
/// TODO: Wire to full batch execution in Task 8 when DataView engine integration is complete.
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

    // TODO: Wire to full batch execution in Task 8
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::sync::{Arc, OnceLock};

    use super::host_context::{
        dyn_txn_map, set_current_task_id_for_test, store_task_ds_configs, take_commit_failed,
        DatasourceConfigsSnapshot,
    };
    use super::super::dyn_transaction_map::TaskId;

    /// Behavior knobs for the mock connection — set per test to force the
    /// `commit_transaction` path to fail.
    #[derive(Default)]
    struct MockConnBehavior {
        commit_fails: AtomicBool,
    }

    struct MockConn {
        behavior: Arc<MockConnBehavior>,
    }

    #[async_trait]
    impl Connection for MockConn {
        async fn execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
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
            "mock-txn"
        }
        async fn begin_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn commit_transaction(&mut self) -> Result<(), DriverError> {
            if self.behavior.commit_fails.load(Ordering::Relaxed) {
                Err(DriverError::Transaction("forced commit failure".into()))
            } else {
                Ok(())
            }
        }
        async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    struct MockDriver {
        behavior: Arc<MockConnBehavior>,
    }

    #[async_trait]
    impl DatabaseDriver for MockDriver {
        fn name(&self) -> &str {
            "mock-txn-driver"
        }
        async fn connect(
            &self,
            _params: &ConnectionParams,
        ) -> Result<Box<dyn Connection>, DriverError> {
            Ok(Box::new(MockConn {
                behavior: self.behavior.clone(),
            }))
        }
        fn supports_transactions(&self) -> bool {
            true
        }
    }

    /// Behavior knob shared between the mock driver and tests. Tests flip
    /// `commit_fails` on the same `Arc` to force the failure path.
    static SHARED_BEHAVIOR: once_cell::sync::Lazy<Arc<MockConnBehavior>> =
        once_cell::sync::Lazy::new(|| Arc::new(MockConnBehavior::default()));

    /// Process-wide guard around HOST_CONTEXT setup. `OnceLock` semantics
    /// mean we can only `set` once — gate it through a `OnceLock<()>` so all
    /// tests funnel through the same init.
    static SETUP: OnceLock<()> = OnceLock::new();

    /// Initialize HOST_CONTEXT exactly once with a DriverFactory containing
    /// our mock driver. Subsequent calls are no-ops.
    fn ensure_host_context() {
        SETUP.get_or_init(|| {
            let mut factory = DriverFactory::new();
            factory.register_database_driver(Arc::new(MockDriver {
                behavior: SHARED_BEHAVIOR.clone(),
            }));
            super::host_context::set_host_context(
                Arc::new(tokio::sync::RwLock::new(None)),
                None,
                Some(Arc::new(factory)),
            );
        });
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
    /// parallel when each toggles the shared `commit_fails` knob. A test
    /// mutex serializes the txn lifecycle. Each test claims the lock for its
    /// duration so the behavior knob and CURRENT_TASK_ID thread-local don't
    /// collide.
    static TEST_LOCK: once_cell::sync::Lazy<StdMutex<()>> =
        once_cell::sync::Lazy::new(|| StdMutex::new(()));

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn begin_then_commit_happy_path() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        ensure_host_context();
        SHARED_BEHAVIOR.commit_fails.store(false, Ordering::Relaxed);

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
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        ensure_host_context();
        SHARED_BEHAVIOR.commit_fails.store(false, Ordering::Relaxed);

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
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        ensure_host_context();
        // Force the commit path to fail.
        SHARED_BEHAVIOR.commit_fails.store(true, Ordering::Relaxed);

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
        SHARED_BEHAVIOR.commit_fails.store(false, Ordering::Relaxed);

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
}
