//! Wasmtime WebAssembly engine (V2.11) — extracted from process_pool (AN13.4).
//!
//! Contains WASM module cache, execute_wasm_task, host function bindings,
//! and fuel computation logic.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use super::{
    ActiveTask, ActiveTaskRegistry, TaskContext, TaskError, TaskResult, TaskTerminator,
};

// ── Thread-Local Keystore for WASM Host Functions ───────────────

/// Application keystore context for WASM host functions (encrypt/decrypt + metadata).
struct KeystoreContext {
    keystore: Arc<rivers_keystore_engine::AppKeystore>,
}

thread_local! {
    /// Application keystore for encrypt/decrypt and key metadata (App Keystore feature).
    /// Set before WASM execution, cleared after. WASM host functions access this
    /// to perform keystore operations without exposing key bytes to WASM memory.
    static TASK_KEYSTORE: RefCell<Option<KeystoreContext>> = RefCell::new(None);
}

// ── V2.11: WASM Module Cache ────────────────────────────────────

/// Cache of compiled Wasmtime modules by file path.
///
/// Wasmtime module compilation is expensive. This cache stores compiled
/// modules keyed by their file path so subsequent invocations of the
/// same WASM binary skip compilation entirely.
pub static WASM_MODULE_CACHE: std::sync::LazyLock<StdMutex<HashMap<String, wasmtime::Module>>> =
    std::sync::LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Clear the WASM module cache (called on hot reload).
pub fn clear_wasm_cache() {
    if let Ok(mut cache) = WASM_MODULE_CACHE.lock() {
        cache.clear();
    }
}

/// Execute a WASM task using the Wasmtime engine (X6).
///
/// Per spec §6.0-6.3, §7.3:
/// - Fuel-based preemption for CPU time limits
/// - Epoch-based interruption via watchdog thread
/// - Memory limits from pool config (max_heap_mb → bytes → linear memory pages)
/// - Host function bindings: `rivers.log_info/warn/error` → tracing
/// - WASM module cache for avoiding recompilation
///
/// The entrypoint function is called with no WASM arguments. Return value
/// (i32, i64, f64) is wrapped in JSON. Host functions provide side-channel
/// communication (logging, store access in future).
pub(crate) async fn execute_wasm_task(
    ctx: TaskContext,
    timeout_ms: u64,
    worker_id: usize,
    max_memory_bytes: usize,
    epoch_interval_ms: u64,
    registry: Option<ActiveTaskRegistry>,
) -> Result<TaskResult, TaskError> {
    let result = tokio::task::spawn_blocking(move || {
        let start = Instant::now();

        // Set up TASK_KEYSTORE thread-local for WASM host functions (App Keystore feature).
        // TaskContext.keystore is Some when an app has [[keystores]] declared.
        TASK_KEYSTORE.with(|ks| {
            *ks.borrow_mut() = ctx.keystore.as_ref().map(|k| KeystoreContext {
                keystore: k.clone(),
            });
        });

        // Guard: ensure keystore is cleared on all exit paths
        struct KeystoreGuard;
        impl Drop for KeystoreGuard {
            fn drop(&mut self) {
                TASK_KEYSTORE.with(|ks| *ks.borrow_mut() = None);
            }
        }
        let _ks_guard = KeystoreGuard;

        // X6.5: Shared engine with fuel AND epoch-based preemption.
        // Using a singleton engine so cached modules are compatible.
        use std::sync::LazyLock;
        static WASM_ENGINE: LazyLock<Result<wasmtime::Engine, String>> = LazyLock::new(|| {
            let mut config = wasmtime::Config::new();
            config.consume_fuel(true);
            config.epoch_interruption(true);
            wasmtime::Engine::new(&config).map_err(|e| format!("wasmtime engine: {e}"))
        });
        let engine = WASM_ENGINE
            .as_ref()
            .map_err(|e| TaskError::Internal(e.clone()))?
            .clone();

        // X6.3: Check WASM module cache before reading/compiling
        let module = {
            let cached = WASM_MODULE_CACHE
                .lock()
                .ok()
                .and_then(|cache| cache.get(&ctx.entrypoint.module).cloned());
            cached
        };

        let module = match module {
            Some(m) => m,
            None => {
                let wasm_bytes = std::fs::read(&ctx.entrypoint.module).map_err(|e| {
                    TaskError::HandlerError(format!(
                        "cannot read WASM module '{}': {e}",
                        ctx.entrypoint.module
                    ))
                })?;

                let compiled = wasmtime::Module::new(&engine, &wasm_bytes)
                    .map_err(|e| TaskError::HandlerError(format!("WASM compilation failed: {e}")))?;

                if let Ok(mut cache) = WASM_MODULE_CACHE.lock() {
                    cache.insert(ctx.entrypoint.module.clone(), compiled.clone());
                }

                compiled
            }
        };

        // X6.7: Create store with memory limits via StoreLimits
        let mut store_limits = wasmtime::StoreLimitsBuilder::new();
        if max_memory_bytes > 0 {
            store_limits = store_limits.memory_size(max_memory_bytes);
        }
        let mut store = wasmtime::Store::new(&engine, store_limits.build());
        store.limiter(|limits| limits);

        // Fuel limit: rough mapping of timeout_ms → fuel units
        let fuel = if timeout_ms > 0 {
            timeout_ms * 1000
        } else {
            1_000_000
        };
        store
            .set_fuel(fuel)
            .map_err(|e| TaskError::Internal(format!("wasmtime fuel: {e}")))?;

        // X6.5: Epoch deadline — timeout_ms / epoch_interval_ms per tick
        let interval = if epoch_interval_ms > 0 { epoch_interval_ms } else { 10 };
        let epoch_ticks = if timeout_ms > 0 {
            (timeout_ms / interval).max(1)
        } else {
            100
        };
        store.set_epoch_deadline(epoch_ticks);

        // Wave 10: Register in pool watchdog registry for epoch-based timeout
        if let Some(ref reg) = registry {
            reg.lock().unwrap().insert(worker_id, ActiveTask {
                started_at: start,
                timeout_ms,
                terminator: TaskTerminator::WasmEpoch(Arc::new(engine.clone())),
            });
        }

        // X6.4: Create linker with host function bindings
        let mut linker = wasmtime::Linker::new(&engine);

        // X6.4 + X6.6: rivers.log_info — host function for WASM logging
        linker.func_wrap("rivers", "log_info", |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>, ptr: i32, len: i32| {
            if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                let data = memory.data(&caller);
                if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
                    let msg = String::from_utf8_lossy(slice);
                    tracing::info!(target: "rivers.wasm", "{}", msg);
                }
            }
        }).map_err(|e| TaskError::Internal(format!("linker log_info: {e}")))?;

        linker.func_wrap("rivers", "log_warn", |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>, ptr: i32, len: i32| {
            if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                let data = memory.data(&caller);
                if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
                    let msg = String::from_utf8_lossy(slice);
                    tracing::warn!(target: "rivers.wasm", "{}", msg);
                }
            }
        }).map_err(|e| TaskError::Internal(format!("linker log_warn: {e}")))?;

        linker.func_wrap("rivers", "log_error", |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>, ptr: i32, len: i32| {
            if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                let data = memory.data(&caller);
                if let Some(slice) = data.get(ptr as usize..(ptr as usize + len as usize)) {
                    let msg = String::from_utf8_lossy(slice);
                    tracing::error!(target: "rivers.wasm", "{}", msg);
                }
            }
        }).map_err(|e| TaskError::Internal(format!("linker log_error: {e}")))?;

        // ── Keystore host functions (App Keystore feature) ──────────
        //
        // These follow the read-from-WASM-memory / write-to-WASM-memory pattern:
        //   Input:  (name_ptr, name_len) or (json_ptr, json_len)
        //   Output: written to (out_ptr, out_len) caller-allocated buffer
        //   Return: 0 success, 1 true, 0 false (for has), -1 error

        // rivers.keystore_has(name_ptr, name_len) -> i32
        // Returns 1 (true), 0 (false), -1 (error/no keystore)
        linker.func_wrap("rivers", "keystore_has",
            |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>,
             name_ptr: i32, name_len: i32| -> i32 {
                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = memory.data(&caller);
                let name = match data.get(name_ptr as usize..(name_ptr as usize + name_len as usize)) {
                    Some(slice) => String::from_utf8_lossy(slice).to_string(),
                    None => return -1,
                };

                TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => if ctx.keystore.has_key(&name) { 1 } else { 0 },
                        None => -1,
                    }
                })
            }
        ).map_err(|e| TaskError::Internal(format!("linker keystore_has: {e}")))?;

        // rivers.keystore_info(name_ptr, name_len, out_ptr, out_len) -> i32
        // Writes JSON {"name","type","version","created_at"} to output buffer.
        // Returns 0 on success, -1 on error.
        linker.func_wrap("rivers", "keystore_info",
            |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>,
             name_ptr: i32, name_len: i32, out_ptr: i32, out_len: i32| -> i32 {
                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let name = {
                    let data = memory.data(&caller);
                    match data.get(name_ptr as usize..(name_ptr as usize + name_len as usize)) {
                        Some(slice) => String::from_utf8_lossy(slice).to_string(),
                        None => return -1,
                    }
                };

                let json_bytes = TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => match ctx.keystore.key_info(&name) {
                            Ok(info) => {
                                let json = serde_json::json!({
                                    "name": info.name,
                                    "type": info.key_type,
                                    "version": info.current_version,
                                    "created_at": info.created.to_rfc3339(),
                                });
                                Ok(serde_json::to_vec(&json).unwrap_or_default())
                            }
                            Err(e) => Err(e.to_string()),
                        },
                        None => Err("keystore not configured".to_string()),
                    }
                });

                match json_bytes {
                    Ok(bytes) => {
                        let out_buf_len = out_len as usize;
                        if bytes.len() > out_buf_len {
                            return -1; // output buffer too small
                        }
                        let data = memory.data_mut(&mut caller);
                        if let Some(dest) = data.get_mut(out_ptr as usize..(out_ptr as usize + bytes.len())) {
                            dest.copy_from_slice(&bytes);
                            bytes.len() as i32 // return actual bytes written (>0 means success with length)
                        } else {
                            -1
                        }
                    }
                    Err(_) => -1,
                }
            }
        ).map_err(|e| TaskError::Internal(format!("linker keystore_info: {e}")))?;

        // rivers.crypto_encrypt(input_ptr, input_len, out_ptr, out_len) -> i32
        // Input JSON: {"key_name":"...", "plaintext":"...", "aad":"..."}
        // Output JSON: {"ciphertext":"...", "nonce":"...", "key_version":N}
        // Returns bytes written on success (>0), -1 on error.
        linker.func_wrap("rivers", "crypto_encrypt",
            |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>,
             input_ptr: i32, input_len: i32, out_ptr: i32, out_len: i32| -> i32 {
                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let input_json = {
                    let data = memory.data(&caller);
                    match data.get(input_ptr as usize..(input_ptr as usize + input_len as usize)) {
                        Some(slice) => match serde_json::from_slice::<serde_json::Value>(slice) {
                            Ok(v) => v,
                            Err(_) => return -1,
                        },
                        None => return -1,
                    }
                };

                let key_name = match input_json["key_name"].as_str() {
                    Some(n) => n.to_string(),
                    None => return -1,
                };
                let plaintext = match input_json["plaintext"].as_str() {
                    Some(p) => p.to_string(),
                    None => return -1,
                };
                let aad: Option<String> = input_json["aad"].as_str().map(|s| s.to_string());

                let result = TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => {
                            let aad_bytes = aad.as_ref().map(|a| a.as_bytes());
                            ctx.keystore.encrypt_with_key(&key_name, plaintext.as_bytes(), aad_bytes)
                                .map_err(|e| e.to_string())
                        }
                        None => Err("keystore not configured".to_string()),
                    }
                });

                match result {
                    Ok(enc) => {
                        let out_json = serde_json::json!({
                            "ciphertext": enc.ciphertext,
                            "nonce": enc.nonce,
                            "key_version": enc.key_version,
                        });
                        let bytes = serde_json::to_vec(&out_json).unwrap_or_default();
                        let out_buf_len = out_len as usize;
                        if bytes.len() > out_buf_len {
                            return -1;
                        }
                        let data = memory.data_mut(&mut caller);
                        if let Some(dest) = data.get_mut(out_ptr as usize..(out_ptr as usize + bytes.len())) {
                            dest.copy_from_slice(&bytes);
                            bytes.len() as i32
                        } else {
                            -1
                        }
                    }
                    Err(_) => -1,
                }
            }
        ).map_err(|e| TaskError::Internal(format!("linker crypto_encrypt: {e}")))?;

        // rivers.crypto_decrypt(input_ptr, input_len, out_ptr, out_len) -> i32
        // Input JSON: {"key_name":"...", "ciphertext":"...", "nonce":"...", "key_version":N, "aad":"..."}
        // Output: plaintext string bytes
        // Returns bytes written on success (>0), -1 on error.
        linker.func_wrap("rivers", "crypto_decrypt",
            |mut caller: wasmtime::Caller<'_, wasmtime::StoreLimits>,
             input_ptr: i32, input_len: i32, out_ptr: i32, out_len: i32| -> i32 {
                let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let input_json = {
                    let data = memory.data(&caller);
                    match data.get(input_ptr as usize..(input_ptr as usize + input_len as usize)) {
                        Some(slice) => match serde_json::from_slice::<serde_json::Value>(slice) {
                            Ok(v) => v,
                            Err(_) => return -1,
                        },
                        None => return -1,
                    }
                };

                let key_name = match input_json["key_name"].as_str() {
                    Some(n) => n.to_string(),
                    None => return -1,
                };
                let ciphertext = match input_json["ciphertext"].as_str() {
                    Some(c) => c.to_string(),
                    None => return -1,
                };
                let nonce = match input_json["nonce"].as_str() {
                    Some(n) => n.to_string(),
                    None => return -1,
                };
                let key_version = match input_json["key_version"].as_u64() {
                    Some(v) => v as u32,
                    None => return -1,
                };
                let aad: Option<String> = input_json["aad"].as_str().map(|s| s.to_string());

                let result = TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => {
                            let aad_bytes = aad.as_ref().map(|a| a.as_bytes());
                            ctx.keystore.decrypt_with_key(&key_name, &ciphertext, &nonce, key_version, aad_bytes)
                                .map_err(|e| {
                                    // Generic error for auth failures — no oracle
                                    match e {
                                        rivers_keystore_engine::AppKeystoreError::KeyNotFound { .. } => e.to_string(),
                                        rivers_keystore_engine::AppKeystoreError::KeyVersionNotFound { .. } => e.to_string(),
                                        _ => "decryption failed".to_string(),
                                    }
                                })
                        }
                        None => Err("keystore not configured".to_string()),
                    }
                });

                match result {
                    Ok(plaintext_bytes) => {
                        let out_buf_len = out_len as usize;
                        if plaintext_bytes.len() > out_buf_len {
                            return -1;
                        }
                        let data = memory.data_mut(&mut caller);
                        if let Some(dest) = data.get_mut(out_ptr as usize..(out_ptr as usize + plaintext_bytes.len())) {
                            dest.copy_from_slice(&plaintext_bytes);
                            plaintext_bytes.len() as i32
                        } else {
                            -1
                        }
                    }
                    Err(_) => -1,
                }
            }
        ).map_err(|e| TaskError::Internal(format!("linker crypto_decrypt: {e}")))?;

        // Instantiate module with host bindings
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| TaskError::HandlerError(format!("WASM instantiation failed: {e}")))?;

        // Call the entrypoint function
        let func = instance
            .get_func(&mut store, &ctx.entrypoint.function)
            .ok_or_else(|| {
                TaskError::HandlerError(format!(
                    "WASM function '{}' not found in module",
                    ctx.entrypoint.function
                ))
            })?;

        let mut results = vec![wasmtime::Val::I32(0)];
        let call_result = func.call(&mut store, &[], &mut results);

        // Wave 10: Deregister from pool watchdog
        if let Some(ref reg) = registry {
            reg.lock().unwrap().remove(&worker_id);
        }

        match call_result {
            Ok(()) => {}
            Err(e) => {
                let err_str = e.to_string();
                // Check for fuel exhaustion or epoch interruption
                // Wasmtime may report these as "all fuel consumed", "epoch deadline",
                // "wasm trap: interrupt", or generic trap on epoch-interrupted code
                if err_str.contains("fuel")
                    || err_str.contains("epoch")
                    || err_str.contains("interrupt")
                    || e.downcast_ref::<wasmtime::Trap>().is_some()
                {
                    return Err(TaskError::Timeout(timeout_ms));
                }
                // Check for memory limit
                if err_str.contains("memory") && err_str.contains("limit") {
                    return Err(TaskError::HandlerError(format!(
                        "WASM memory limit exceeded (max {} bytes)",
                        max_memory_bytes
                    )));
                }
                return Err(TaskError::HandlerError(format!(
                    "WASM execution error: {e}"
                )));
            }
        }

        let return_val = match results.first() {
            Some(wasmtime::Val::I32(n)) => serde_json::json!({ "result": n }),
            Some(wasmtime::Val::I64(n)) => serde_json::json!({ "result": n }),
            Some(wasmtime::Val::F64(n)) => serde_json::json!({ "result": n }),
            _ => serde_json::Value::Null,
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        Ok(TaskResult {
            value: return_val,
            duration_ms,
        })
    })
    .await
    .map_err(|e| TaskError::WorkerCrash(format!("wasm worker {worker_id} panicked: {e}")))?;

    result
}
