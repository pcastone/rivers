//! FFI host callback implementations for engine plugins (V8, WASM).
//!
//! All `extern "C"` functions referenced in `HostCallbacks` live here.
//! They access subsystem state via `HOST_CONTEXT` and `HOST_KEYSTORE`
//! defined in the sibling `host_context` module.

use std::collections::HashMap;

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
        None => return -1,
    };

    let input = match read_input(input_ptr, input_len) {
        Ok(v) => v,
        Err(_) => return -2,
    };

    let name = match input["name"].as_str() {
        Some(n) => n.to_string(),
        None => return -3,
    };
    let trace_id = input["trace_id"].as_str().unwrap_or("engine-callback").to_string();

    // Convert JSON params to HashMap<String, QueryValue>
    use rivers_runtime::rivers_driver_sdk::QueryValue;
    let params: HashMap<String, QueryValue> = input["params"]
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), QueryValue::Json(v.clone()))).collect())
        .unwrap_or_default();

    let executor_lock = ctx.dataview_executor.clone();
    match ctx.rt_handle.block_on(async {
        let executor = {
            let guard = executor_lock.read().await;
            guard.clone().ok_or_else(|| "DataViewExecutor not initialized".to_string())?
        };
        executor.execute(&name, params, "GET", &trace_id).await.map_err(|e| e.to_string())
    }) {
        Ok(response) => {
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
        Err(e) => {
            let err_val = serde_json::json!({"error": e});
            write_output(out_ptr, out_len, &err_val);
            -10
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

    match ctx.rt_handle.block_on(engine.get(namespace, key)) {
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

    match ctx.rt_handle.block_on(engine.set(namespace, key, value_bytes, ttl_ms)) {
        Ok(()) => 0,
        Err(_) => -10,
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

    match ctx.rt_handle.block_on(engine.delete(namespace, key)) {
        Ok(_) => 0,
        Err(_) => -10,
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

    // Build the Query object
    use rivers_runtime::rivers_driver_sdk::{Query, QueryValue};
    let mut query = Query::new("", &statement);
    for (k, v) in &params_obj {
        query.parameters.insert(k.clone(), QueryValue::Json(v.clone()));
    }

    match ctx.rt_handle.block_on(async {
        let mut conn = factory.connect(&driver, &conn_params).await
            .map_err(|e| e.to_string())?;
        conn.execute(&query).await.map_err(|e| e.to_string())
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

    match ctx.rt_handle.block_on(async {
        let mut req = match method.to_uppercase().as_str() {
            "GET" => ctx.http_client.get(&url),
            "POST" => ctx.http_client.post(&url),
            "PUT" => ctx.http_client.put(&url),
            "DELETE" => ctx.http_client.delete(&url),
            "PATCH" => ctx.http_client.patch(&url),
            "HEAD" => ctx.http_client.head(&url),
            _ => ctx.http_client.get(&url),
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
    }) {
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
    let msg = unsafe {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len))
    };
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
