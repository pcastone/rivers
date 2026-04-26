//! FFI host callback implementations for engine plugins (V8, WASM).
//!
//! All `extern "C"` functions referenced in `HostCallbacks` live here.
//! They access subsystem state via `HOST_CONTEXT` and `HOST_KEYSTORE`
//! defined in the sibling `host_context` module.

use std::collections::HashMap;
use std::sync::Arc;

use super::host_context::{HOST_CONTEXT, HOST_KEYSTORE};

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
/// Output: JSON `{"ok": true, "datasource": "..."}` on success
///
/// TODO: Wire to TransactionMap in Task 8 when DataView engine integration is complete.
pub(super) extern "C" fn host_db_begin(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let _ctx = match HOST_CONTEXT.get() {
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

    let datasource = match input["datasource"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_begin: missing 'datasource' field");
            let err = serde_json::json!({"error": "missing 'datasource' field"});
            write_output(out_ptr, out_len, &err);
            return -3;
        }
    };

    // TODO: Wire to TransactionMap in Task 8
    tracing::debug!(datasource = %datasource, "Rivers.db.begin (stub)");
    let result = serde_json::json!({"ok": true, "datasource": datasource});
    write_output(out_ptr, out_len, &result);
    0
}

// ── db_commit ───────────────────────────────────────────────────

/// Rivers.db.commit("datasource") — commit an active transaction.
///
/// Input: JSON `{"datasource": "..."}`
/// Output: JSON `{"ok": true, "datasource": "..."}` on success
///
/// TODO: Wire to TransactionMap in Task 8 when DataView engine integration is complete.
pub(super) extern "C" fn host_db_commit(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let _ctx = match HOST_CONTEXT.get() {
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

    let datasource = match input["datasource"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_commit: missing 'datasource' field");
            let err = serde_json::json!({"error": "missing 'datasource' field"});
            write_output(out_ptr, out_len, &err);
            return -3;
        }
    };

    // TODO: Wire to TransactionMap in Task 8
    tracing::debug!(datasource = %datasource, "Rivers.db.commit (stub)");
    let result = serde_json::json!({"ok": true, "datasource": datasource});
    write_output(out_ptr, out_len, &result);
    0
}

// ── db_rollback ─────────────────────────────────────────────────

/// Rivers.db.rollback("datasource") — rollback an active transaction.
///
/// Input: JSON `{"datasource": "..."}`
/// Output: JSON `{"ok": true, "datasource": "..."}` on success
///
/// TODO: Wire to TransactionMap in Task 8 when DataView engine integration is complete.
pub(super) extern "C" fn host_db_rollback(
    input_ptr: *const u8, input_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    let _ctx = match HOST_CONTEXT.get() {
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

    let datasource = match input["datasource"].as_str() {
        Some(s) => s.to_string(),
        None => {
            tracing::error!("host_db_rollback: missing 'datasource' field");
            let err = serde_json::json!({"error": "missing 'datasource' field"});
            write_output(out_ptr, out_len, &err);
            return -3;
        }
    };

    // TODO: Wire to TransactionMap in Task 8
    tracing::debug!(datasource = %datasource, "Rivers.db.rollback (stub)");
    let result = serde_json::json!({"ok": true, "datasource": datasource});
    write_output(out_ptr, out_len, &result);
    0
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
