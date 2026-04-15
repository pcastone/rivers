//! Rivers Engine SDK — C-ABI contract for dynamic engine plugins.
//!
//! Defines the binary boundary between `riversd` and engine shared libraries
//! (V8, Wasmtime). Engines are loaded from `lib/` at startup via `libloading`.
//!
//! The ABI uses JSON serialization over raw byte buffers to avoid Rust ABI
//! instability across compiler versions. Each engine dylib exports five
//! C symbols:
//!
//! - `_rivers_engine_abi_version` — returns [`ENGINE_ABI_VERSION`]
//! - `_rivers_engine_init` — receives [`HostCallbacks`] and [`EngineConfig`]
//! - `_rivers_engine_execute` — receives [`SerializedTaskContext`], returns [`SerializedTaskResult`]
//! - `_rivers_engine_shutdown` — graceful teardown
//! - `_rivers_engine_cancel` — cancel a running task by ID

#![warn(missing_docs)]

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ── ABI Version ─────────────────────────────────────────────────

/// ABI version for engine plugins. Checked at load time.
/// Bump when the C-ABI function signatures or callback table changes.
pub const ENGINE_ABI_VERSION: u32 = 1;

// ── Serialized Task Types ───────────────────────────────────────

/// Serialized task context — pure data that crosses the dylib boundary as JSON.
///
/// No Arc, no trait objects, no function pointers. The engine deserializes this
/// from JSON, executes the handler, and returns a `SerializedTaskResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedTaskContext {
    /// Declared datasource token names (capability check).
    pub datasource_tokens: HashMap<String, String>,
    /// Declared DataView token names (capability check).
    pub dataview_tokens: HashMap<String, String>,
    /// Resolved datasource configs for ctx.datasource().build().
    pub datasource_configs: HashMap<String, SerializedDatasource>,
    /// Whether outbound HTTP is enabled for this task.
    pub http_enabled: bool,
    /// Environment variables available to the handler.
    pub env: HashMap<String, String>,
    /// Handler entrypoint (module path, function name, language).
    pub entrypoint: SerializedEntrypoint,
    /// Handler arguments (JSON value passed as `__args`).
    pub args: serde_json::Value,
    /// Trace ID for correlation.
    pub trace_id: String,
    /// Application ID.
    pub app_id: String,
    /// Node ID.
    pub node_id: String,
    /// Runtime environment (dev, staging, prod).
    pub runtime_env: String,
    /// Whether StorageEngine is available for ctx.store.
    pub storage_available: bool,
    /// Store namespace prefix (e.g., "app:{app_id}").
    pub store_namespace: Option<String>,
    /// Whether LockBox is available for HMAC key resolution.
    pub lockbox_available: bool,
    /// Whether application keystore is available for encrypt/decrypt.
    pub keystore_available: bool,
    /// Inline source code (for testing — `_source` in args).
    pub inline_source: Option<String>,
    /// Pre-fetched DataView data (keyed by DataView name).
    pub prefetched_data: HashMap<String, serde_json::Value>,
    /// Resolved library contents (for module loading).
    pub libs: Vec<SerializedLib>,
}

/// Serialized handler entrypoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedEntrypoint {
    /// Module file path or "inline" for test source.
    pub module: String,
    /// Function name to call.
    pub function: String,
    /// Language: "javascript", "typescript", "wasm".
    pub language: String,
}

/// Serialized datasource config for ctx.datasource().build().
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedDatasource {
    /// Driver name (e.g. `"postgres"`, `"redis"`).
    pub driver_name: String,
    /// Hostname or IP address.
    pub host: String,
    /// Port number.
    pub port: u16,
    /// Database, bucket, or keyspace name.
    pub database: String,
    /// Authentication username.
    pub username: String,
    /// Driver-specific connection options.
    pub options: HashMap<String, String>,
}

/// Serialized library content (resolved at dispatch time).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedLib {
    /// Library module name (used for `import` resolution).
    pub name: String,
    /// Raw file contents (JavaScript, TypeScript, or WASM bytes).
    pub content: Vec<u8>,
}

/// Result of a task execution — returned from engine as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedTaskResult {
    /// Handler return value (JSON).
    pub value: serde_json::Value,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Engine configuration passed during initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Number of worker threads (isolates or instances) to pre-allocate.
    pub workers: usize,
    /// Maximum V8/WASM heap size per worker in megabytes.
    pub max_heap_mb: usize,
    /// Task execution timeout in milliseconds.
    pub task_timeout_ms: u64,
    /// Maximum pending tasks before backpressure rejects new work.
    pub max_queue_depth: usize,
    /// V8 epoch interrupt interval in milliseconds (for preemption checks).
    pub epoch_interval_ms: u64,
    /// Heap usage fraction (0.0–1.0) that triggers worker recycling.
    pub heap_recycle_threshold: f64,
}

// ── Host Callback Function Pointers ─────────────────────────────

/// C-ABI function pointer table passed from riversd to engine dylib at init.
///
/// These callbacks bridge the engine back into riversd for operations that
/// require access to shared state (DataViewExecutor, StorageEngine, etc.).
///
/// All callbacks use raw byte buffers (ptr + len) for input/output.
/// Returns 0 on success, non-zero on error. Output buffers are allocated
/// by the callee and freed via `free_buffer`.
#[repr(C)]
pub struct HostCallbacks {
    /// Execute a DataView query. Input: JSON `{"name": "...", "params": {...}}`.
    /// Output: JSON result rows.
    pub dataview_execute: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Get a value from StorageEngine. Input: JSON `{"namespace": "...", "key": "..."}`.
    /// Output: JSON value or empty if not found.
    pub store_get: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Set a value in StorageEngine. Input: JSON `{"namespace": "...", "key": "...", "value": ..., "ttl_ms": ...}`.
    pub store_set: Option<
        extern "C" fn(input_ptr: *const u8, input_len: usize) -> i32,
    >,

    /// Delete a key from StorageEngine. Input: JSON `{"namespace": "...", "key": "..."}`.
    pub store_del: Option<
        extern "C" fn(input_ptr: *const u8, input_len: usize) -> i32,
    >,

    /// Execute a datasource query via DriverFactory. Input: JSON `{"driver": "...", "query": "...", "params": {...}}`.
    /// Output: JSON result.
    pub datasource_build: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Execute an outbound HTTP request. Input: JSON `{"method": "...", "url": "...", "body": ...}`.
    /// Output: JSON response.
    pub http_request: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Log a message. Level: 0=trace, 1=debug, 2=info, 3=warn, 4=error.
    pub log_message: Option<
        extern "C" fn(level: u8, msg_ptr: *const u8, msg_len: usize),
    >,

    /// Free a buffer allocated by the host (for output buffers).
    pub free_buffer: Option<
        extern "C" fn(ptr: *mut u8, len: usize),
    >,

    /// Check if a key exists in the application keystore.
    /// Input: JSON `{"name": "..."}`.
    /// Output: JSON `{"exists": true/false}`.
    pub keystore_has: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Get metadata for a key in the application keystore.
    /// Input: JSON `{"name": "..."}`.
    /// Output: JSON `{"name":"...", "type":"...", "version":N, "created_at":"..."}`.
    pub keystore_info: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Encrypt data using the application keystore.
    /// Input: JSON `{"key_name":"...", "plaintext":"...", "aad":"..."}`.
    /// Output: JSON `{"ciphertext":"...", "nonce":"...", "key_version":N}`.
    pub crypto_encrypt: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Decrypt data using the application keystore.
    /// Input: JSON `{"key_name":"...", "ciphertext":"...", "nonce":"...", "key_version":N, "aad":"..."}`.
    /// Output: JSON `{"plaintext":"..."}`.
    pub crypto_decrypt: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Execute a DDL statement (CREATE TABLE, ALTER, etc.) via the init handler.
    /// Input: JSON `{"datasource": "...", "statement": "...", "app_id": "..."}`.
    /// Output: JSON `{"ok": true}` or error.
    /// Only available during ApplicationInit context (Gate 3 whitelist checked by host).
    pub ddl_execute: Option<
        extern "C" fn(
            input_ptr: *const u8, input_len: usize,
            out_ptr: *mut *mut u8, out_len: *mut usize,
        ) -> i32,
    >,

    /// Begin a transaction on a datasource.
    /// Input: JSON `{"datasource": "..."}`.
    /// Output: JSON `{"ok": true, "datasource": "..."}` or error.
    pub db_begin: Option<extern "C" fn(
        input_ptr: *const u8, input_len: usize,
        out_ptr: *mut *mut u8, out_len: *mut usize,
    ) -> i32>,

    /// Commit a transaction on a datasource.
    /// Input: JSON `{"datasource": "..."}`.
    /// Output: JSON `{"ok": true, "datasource": "..."}` or error.
    pub db_commit: Option<extern "C" fn(
        input_ptr: *const u8, input_len: usize,
        out_ptr: *mut *mut u8, out_len: *mut usize,
    ) -> i32>,

    /// Rollback a transaction on a datasource.
    /// Input: JSON `{"datasource": "..."}`.
    /// Output: JSON `{"ok": true, "datasource": "..."}` or error.
    pub db_rollback: Option<extern "C" fn(
        input_ptr: *const u8, input_len: usize,
        out_ptr: *mut *mut u8, out_len: *mut usize,
    ) -> i32>,

    /// Batch execute a DataView with multiple parameter sets.
    /// Input: JSON `{"dataview": "...", "params": [{...}, {...}]}`.
    /// Output: JSON array of results or error.
    pub db_batch: Option<extern "C" fn(
        input_ptr: *const u8, input_len: usize,
        out_ptr: *mut *mut u8, out_len: *mut usize,
    ) -> i32>,
}

// ── C-ABI Function Signatures ───────────────────────────────────

/// Expected C-ABI symbols exported by each engine dylib:
///
/// ```c
/// uint32_t _rivers_engine_abi_version(void);
/// int32_t  _rivers_engine_init(const HostCallbacks* callbacks, const uint8_t* config, size_t config_len);
/// int32_t  _rivers_engine_execute(const uint8_t* ctx, size_t ctx_len, uint8_t** out, size_t* out_len);
/// void     _rivers_engine_shutdown(void);
/// int32_t  _rivers_engine_cancel(size_t task_id);
/// ```
///
/// These are loaded via `libloading` at runtime — no Rust-level trait required.

// ── Helper: JSON buffer utilities ───────────────────────────────

/// Allocate a buffer and write JSON bytes into it.
/// Returns (ptr, len) for passing across C-ABI.
pub fn json_to_buffer(value: &serde_json::Value) -> (*mut u8, usize) {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes.into_boxed_slice()) as *mut u8;
    (ptr, len)
}

/// Read JSON from a raw buffer (does NOT free the buffer).
///
/// # Safety
/// Caller must ensure ptr is valid for len bytes.
pub unsafe fn buffer_to_json(ptr: *const u8, len: usize) -> Result<serde_json::Value, String> {
    if ptr.is_null() || len == 0 {
        return Ok(serde_json::Value::Null);
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    serde_json::from_slice(slice).map_err(|e| format!("JSON deserialize: {}", e))
}

/// Free a buffer allocated by `json_to_buffer`.
///
/// # Safety
/// Must only be called with a pointer returned by `json_to_buffer`.
pub unsafe fn free_json_buffer(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(ptr, len));
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_task_context_round_trip() {
        let ctx = SerializedTaskContext {
            datasource_tokens: [("db".into(), "tok_db".into())].into_iter().collect(),
            dataview_tokens: HashMap::new(),
            datasource_configs: HashMap::new(),
            http_enabled: false,
            env: HashMap::new(),
            entrypoint: SerializedEntrypoint {
                module: "handler.js".into(),
                function: "onRequest".into(),
                language: "javascript".into(),
            },
            args: serde_json::json!({"name": "test"}),
            trace_id: "trace-1".into(),
            app_id: "app-1".into(),
            node_id: "node-0".into(),
            runtime_env: "dev".into(),
            storage_available: true,
            store_namespace: Some("app:app-1".into()),
            lockbox_available: false,
            keystore_available: false,
            inline_source: None,
            prefetched_data: HashMap::new(),
            libs: vec![],
        };

        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: SerializedTaskContext = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.trace_id, "trace-1");
        assert_eq!(deserialized.entrypoint.module, "handler.js");
        assert_eq!(deserialized.args["name"], "test");
    }

    #[test]
    fn serialized_task_result_round_trip() {
        let result = SerializedTaskResult {
            value: serde_json::json!({"message": "hello"}),
            duration_ms: 42,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SerializedTaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.value["message"], "hello");
        assert_eq!(deserialized.duration_ms, 42);
    }

    #[test]
    fn engine_config_serialization() {
        let config = EngineConfig {
            workers: 4,
            max_heap_mb: 128,
            task_timeout_ms: 5000,
            max_queue_depth: 16,
            epoch_interval_ms: 10,
            heap_recycle_threshold: 0.8,
        };

        let json = serde_json::to_string(&config).unwrap();
        let d: EngineConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(d.workers, 4);
        assert_eq!(d.max_heap_mb, 128);
    }

    #[test]
    fn json_buffer_round_trip() {
        let value = serde_json::json!({"key": "value", "num": 42});
        let (ptr, len) = json_to_buffer(&value);
        assert!(len > 0);

        let recovered = unsafe { buffer_to_json(ptr, len) }.unwrap();
        assert_eq!(recovered["key"], "value");
        assert_eq!(recovered["num"], 42);

        unsafe { free_json_buffer(ptr, len) };
    }

    #[test]
    fn abi_version_is_one() {
        assert_eq!(ENGINE_ABI_VERSION, 1);
    }
}
