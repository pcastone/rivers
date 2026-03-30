//! `LoadedEngine` — a loaded engine shared library with resolved function pointers.

use rivers_engine_sdk::{SerializedTaskContext, SerializedTaskResult};

/// A loaded engine shared library with resolved function pointers.
pub struct LoadedEngine {
    /// Engine name (e.g., "v8", "wasm").
    pub name: String,
    /// Held to keep the library loaded.
    pub(crate) _library: libloading::Library,
    /// Execute function pointer.
    pub(crate) execute_fn: unsafe extern "C" fn(
        ctx_ptr: *const u8, ctx_len: usize,
        out_ptr: *mut *mut u8, out_len: *mut usize,
    ) -> i32,
    /// Cancel function pointer (for watchdog termination).
    pub(crate) cancel_fn: Option<unsafe extern "C" fn(task_id: usize) -> i32>,
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
