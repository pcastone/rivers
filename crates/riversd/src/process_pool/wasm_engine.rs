//! Wasmtime WebAssembly engine (V2.11) — extracted from process_pool (AN13.4).
//!
//! Contains WASM module cache, execute_wasm_task, host function bindings,
//! and fuel computation logic.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use super::{
    ActiveTask, ActiveTaskRegistry, TaskContext, TaskError, TaskResult, TaskTerminator,
};

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
    registry: Option<ActiveTaskRegistry>,
) -> Result<TaskResult, TaskError> {
    let result = tokio::task::spawn_blocking(move || {
        let start = Instant::now();

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

        // X6.5: Epoch deadline — timeout_ms / 10ms per tick
        let epoch_ticks = if timeout_ms > 0 {
            (timeout_ms / 10).max(1)
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
