//! Dynamic engine loader — loads V8/Wasmtime engine shared libraries at startup.
//!
//! Scans the `[engines].dir` directory for `librivers_*.dylib` (macOS) or
//! `librivers_*.so` (Linux), loads each via `libloading`, checks ABI version,
//! and initializes with host callback function pointers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use rivers_engine_sdk::{ENGINE_ABI_VERSION, HostCallbacks, SerializedTaskContext, SerializedTaskResult};

// ── Loaded Engine ───────────────────────────────────────────────

/// A loaded engine shared library with resolved function pointers.
pub struct LoadedEngine {
    /// Engine name (e.g., "v8", "wasm").
    pub name: String,
    /// Held to keep the library loaded.
    _library: libloading::Library,
    /// Execute function pointer.
    execute_fn: unsafe extern "C" fn(
        ctx_ptr: *const u8, ctx_len: usize,
        out_ptr: *mut *mut u8, out_len: *mut usize,
    ) -> i32,
    /// Cancel function pointer (for watchdog termination).
    cancel_fn: Option<unsafe extern "C" fn(task_id: usize) -> i32>,
}

impl LoadedEngine {
    /// Execute a task via the engine dylib.
    ///
    /// Serializes `ctx` to JSON, calls the engine, deserializes the result.
    pub fn execute(&self, ctx: &SerializedTaskContext) -> Result<SerializedTaskResult, String> {
        let ctx_json = serde_json::to_vec(ctx)
            .map_err(|e| format!("serialize task context: {}", e))?;

        let mut out_ptr: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;

        let result = unsafe {
            (self.execute_fn)(
                ctx_json.as_ptr(), ctx_json.len(),
                &mut out_ptr, &mut out_len,
            )
        };

        if result != 0 {
            // Error — read error message from output buffer
            let err_msg = if !out_ptr.is_null() && out_len > 0 {
                let msg = unsafe {
                    String::from_utf8_lossy(std::slice::from_raw_parts(out_ptr, out_len)).to_string()
                };
                unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };
                msg
            } else {
                format!("engine returned error code {}", result)
            };
            return Err(err_msg);
        }

        // Success — deserialize result
        if out_ptr.is_null() || out_len == 0 {
            return Ok(SerializedTaskResult {
                value: serde_json::Value::Null,
                duration_ms: 0,
            });
        }

        let result_bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
        let task_result: SerializedTaskResult = serde_json::from_slice(result_bytes)
            .map_err(|e| format!("deserialize task result: {}", e))?;

        // Free the buffer allocated by the engine
        unsafe { rivers_engine_sdk::free_json_buffer(out_ptr, out_len) };

        Ok(task_result)
    }

    /// Cancel a running task (called by watchdog).
    pub fn cancel(&self, task_id: usize) -> bool {
        if let Some(cancel_fn) = self.cancel_fn {
            unsafe { cancel_fn(task_id) == 0 }
        } else {
            false
        }
    }
}

// ── Engine Registry ─────────────────────────────────────────────

/// Global engine registry — populated at startup, read on every task dispatch.
static ENGINE_REGISTRY: OnceLock<RwLock<HashMap<String, LoadedEngine>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, LoadedEngine>> {
    ENGINE_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Get an engine by name from the global registry.
pub fn get_engine(name: &str) -> Option<std::sync::RwLockReadGuard<'static, HashMap<String, LoadedEngine>>> {
    let guard = registry().read().ok()?;
    if guard.contains_key(name) {
        Some(guard)
    } else {
        None
    }
}

/// Check if an engine is loaded.
pub fn is_engine_available(name: &str) -> bool {
    registry().read().ok()
        .map(|r| r.contains_key(name))
        .unwrap_or(false)
}

/// Execute a task on a named engine.
///
/// Acquires a read lock on the registry, finds the engine, and calls execute.
/// This function is designed to be called from `spawn_blocking`.
pub fn execute_on_engine(
    name: &str,
    ctx: &rivers_engine_sdk::SerializedTaskContext,
) -> Result<rivers_engine_sdk::SerializedTaskResult, String> {
    let guard = registry().read()
        .map_err(|_| "engine registry lock poisoned".to_string())?;
    let engine = guard.get(name)
        .ok_or_else(|| format!("engine '{}' not loaded", name))?;
    engine.execute(ctx)
}

/// List all loaded engine names.
pub fn loaded_engines() -> Vec<String> {
    registry().read().ok()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default()
}

// ── Engine Loading ──────────────────────────────────────────────

/// Load result for a single engine.
#[derive(Debug)]
pub enum EngineLoadResult {
    Success { name: String, path: String },
    Failed { path: String, reason: String },
}

/// Scan a directory for engine shared libraries and load them.
///
/// Looks for files matching `librivers_v8.*` and `librivers_wasm.*`.
/// Each is loaded, ABI-checked, and initialized.
pub fn load_engines(lib_dir: &Path, callbacks: &HostCallbacks) -> Vec<EngineLoadResult> {
    let mut results = Vec::new();

    if !lib_dir.is_dir() {
        tracing::info!(dir = %lib_dir.display(), "engine lib directory not found — no engines loaded");
        return results;
    }

    let entries = match std::fs::read_dir(lib_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(dir = %lib_dir.display(), error = %e, "failed to read engine lib directory");
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        // Match librivers_v8.* or librivers_wasm.*
        let engine_name = if file_name.starts_with("librivers_v8") {
            "v8"
        } else if file_name.starts_with("librivers_wasm") {
            "wasm"
        } else {
            continue; // Not an engine library
        };

        let path_str = path.display().to_string();
        match load_single_engine(&path, engine_name, callbacks) {
            Ok(engine) => {
                tracing::info!(engine = engine_name, path = %path_str, "engine loaded");
                let mut registry = registry().write().unwrap();
                registry.insert(engine_name.to_string(), engine);
                results.push(EngineLoadResult::Success {
                    name: engine_name.to_string(),
                    path: path_str,
                });
            }
            Err(reason) => {
                tracing::warn!(engine = engine_name, path = %path_str, reason = %reason, "engine load failed");
                results.push(EngineLoadResult::Failed {
                    path: path_str,
                    reason,
                });
            }
        }
    }

    results
}

/// Load a single engine shared library.
fn load_single_engine(
    path: &Path,
    name: &str,
    _callbacks: &HostCallbacks,
) -> Result<LoadedEngine, String> {
    // Load the library
    let lib = unsafe {
        libloading::Library::new(path)
            .map_err(|e| format!("dlopen failed: {}", e))?
    };

    // Check ABI version
    let abi_version: u32 = unsafe {
        let func: libloading::Symbol<unsafe extern "C" fn() -> u32> = lib
            .get(b"_rivers_engine_abi_version")
            .map_err(|e| format!("missing _rivers_engine_abi_version: {}", e))?;
        func()
    };

    if abi_version != ENGINE_ABI_VERSION {
        return Err(format!(
            "ABI version mismatch: engine has {}, expected {}",
            abi_version, ENGINE_ABI_VERSION
        ));
    }

    // Resolve execute function
    let execute_fn = unsafe {
        let func: libloading::Symbol<
            unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32
        > = lib
            .get(b"_rivers_engine_execute")
            .map_err(|e| format!("missing _rivers_engine_execute: {}", e))?;
        *func
    };

    // Resolve cancel function (optional)
    let cancel_fn = unsafe {
        lib.get::<unsafe extern "C" fn(usize) -> i32>(b"_rivers_engine_cancel")
            .ok()
            .map(|f| *f)
    };

    // Call init (optional)
    unsafe {
        if let Ok(init_fn) = lib.get::<unsafe extern "C" fn() -> i32>(b"_rivers_engine_init") {
            let result = init_fn();
            if result != 0 {
                return Err(format!("_rivers_engine_init returned {}", result));
            }
        }
    }

    Ok(LoadedEngine {
        name: name.to_string(),
        _library: lib,
        execute_fn,
        cancel_fn,
    })
}

// ── Host Context (OnceLock subsystem references) ────────────────

/// Subsystem references for host callbacks. Set once after server init.
struct HostContext {
    dataview_executor: Arc<tokio::sync::RwLock<Option<rivers_runtime::DataViewExecutor>>>,
    storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
    http_client: reqwest::Client,
    rt_handle: tokio::runtime::Handle,
}

static HOST_CONTEXT: OnceLock<HostContext> = OnceLock::new();

/// Application keystore for dynamic engine callbacks (App Keystore feature).
/// Separate OnceLock because keystore resolution happens per-app and may
/// occur after the main host context is wired.
static HOST_KEYSTORE: OnceLock<Arc<rivers_keystore_engine::AppKeystore>> = OnceLock::new();

/// Wire host subsystem references so callbacks can reach DataViewExecutor,
/// StorageEngine, DriverFactory, and HTTP client. Called once during server
/// startup after all subsystems are initialized.
pub fn set_host_context(
    dataview_executor: Arc<tokio::sync::RwLock<Option<rivers_runtime::DataViewExecutor>>>,
    storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
) {
    let _ = HOST_CONTEXT.set(HostContext {
        dataview_executor,
        storage_engine,
        driver_factory,
        http_client: reqwest::Client::new(),
        rt_handle: tokio::runtime::Handle::current(),
    });
}

/// Set the application keystore for dynamic engine callbacks.
/// Called after `set_host_context` when an app has [[keystores]] declared.
pub fn set_host_keystore(keystore: Arc<rivers_keystore_engine::AppKeystore>) {
    let _ = HOST_KEYSTORE.set(keystore);
}

// ── Host Callback Implementations ───────────────────────────────
//
// NOTE: The callbacks below return JSON over FFI boundaries using `{"error": ...}`
// format. This is an FFI protocol contract with cdylib engine plugins (V8, WASM).
// Do NOT replace with ErrorResponse — changing the shape would break dynamic
// engine plugins that parse these responses.

/// Build the HostCallbacks table with all callbacks wired.
pub fn build_host_callbacks() -> HostCallbacks {
    HostCallbacks {
        dataview_execute: Some(host_dataview_execute),
        store_get: Some(host_store_get),
        store_set: Some(host_store_set),
        store_del: Some(host_store_del),
        datasource_build: Some(host_datasource_build),
        http_request: Some(host_http_request),
        log_message: Some(host_log_message),
        free_buffer: Some(host_free_buffer),
        keystore_has: Some(host_keystore_has),
        keystore_info: Some(host_keystore_info),
        crypto_encrypt: Some(host_crypto_encrypt),
        crypto_decrypt: Some(host_crypto_decrypt),
    }
}

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

extern "C" fn host_dataview_execute(
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
        let guard = executor_lock.read().await;
        let executor = guard.as_ref().ok_or_else(|| "DataViewExecutor not initialized".to_string())?;
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

extern "C" fn host_store_get(
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

extern "C" fn host_store_set(
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

extern "C" fn host_store_del(
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

extern "C" fn host_datasource_build(
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

extern "C" fn host_http_request(
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

extern "C" fn host_log_message(
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

extern "C" fn host_free_buffer(ptr: *mut u8, len: usize) {
    unsafe { rivers_engine_sdk::free_json_buffer(ptr, len) };
}

// ── keystore_has ────────────────────────────────────────────────

extern "C" fn host_keystore_has(
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

extern "C" fn host_keystore_info(
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

extern "C" fn host_crypto_encrypt(
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

extern "C" fn host_crypto_decrypt(
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
