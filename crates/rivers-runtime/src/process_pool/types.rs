//! ProcessPool type definitions — TaskContext, TaskResult, TaskError, Tokens, Builder.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::rivers_core::{DriverFactory, StorageEngine};
use crate::DataViewExecutor;

// ── Opaque Tokens ────────────────────────────────────────────────

/// Opaque handle to a datasource — the isolate never holds a real connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DatasourceToken(pub String);

/// Opaque handle to a DataView.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DataViewToken(pub String);

/// Opaque handle to the outbound HTTP client.
#[derive(Debug, Clone)]
pub struct HttpToken;

/// Resolved datasource configuration for ctx.datasource().build() (X7).
/// Maps a datasource token name to the driver + connection params needed for execution.
#[derive(Debug, Clone)]
pub struct ResolvedDatasource {
    pub driver_name: String,
    pub params: crate::rivers_driver_sdk::ConnectionParams,
}

// ── Entrypoint ───────────────────────────────────────────────────

/// Which module and function to call in the isolate.
#[derive(Debug, Clone)]
pub struct Entrypoint {
    /// Path to the module file (JS or WASM).
    pub module: String,
    /// Name of the function to call.
    pub function: String,
    /// Source language: "javascript", "typescript", "wasm".
    pub language: String,
}

// ── Resolved Library ─────────────────────────────────────────────

/// A library resolved and ready for injection into the isolate.
#[derive(Debug, Clone)]
pub struct ResolvedLib {
    pub name: String,
    /// Compiled JS source or WASM binary.
    pub content: Vec<u8>,
}

// ── TaskContext ───────────────────────────────────────────────────

/// Context for a single CodeComponent task execution.
///
/// Per spec §4.1: built by the host, passed to the worker.
/// Contains only opaque tokens — never raw connections or credentials.
pub struct TaskContext {
    pub datasources: HashMap<String, DatasourceToken>,
    pub dataviews: HashMap<String, DataViewToken>,
    pub libs: Vec<ResolvedLib>,
    pub http: Option<HttpToken>,
    pub env: HashMap<String, String>,
    pub entrypoint: Entrypoint,
    pub args: serde_json::Value,
    pub trace_id: String,
    /// Application ID for this task's owning app.
    pub app_id: String,
    /// Node ID of the Rivers instance executing this task.
    pub node_id: String,
    /// Runtime environment name (e.g. "dev", "staging", "prod").
    pub runtime_env: String,
    /// Optional StorageEngine backend for ctx.store (X3).
    /// When provided, ctx.store operations use real persistence with TTL.
    pub storage: Option<Arc<dyn StorageEngine>>,
    /// Optional DriverFactory for ctx.datasource().build() (X7).
    pub driver_factory: Option<Arc<DriverFactory>>,
    /// Resolved datasource configs for ctx.datasource().build() (X7).
    pub datasource_configs: HashMap<String, ResolvedDatasource>,
    /// Optional DataViewExecutor for ctx.dataview() dynamic execution (X4).
    pub dataview_executor: Option<Arc<DataViewExecutor>>,
    /// Optional LockBox resolver for HMAC key resolution (Wave 9).
    #[cfg(feature = "lockbox")]
    pub lockbox: Option<Arc<crate::rivers_core::lockbox::LockBoxResolver>>,
    /// Keystore path for LockBox secret fetching (Wave 9).
    #[cfg(feature = "lockbox")]
    pub lockbox_keystore_path: Option<std::path::PathBuf>,
    /// Age identity string for LockBox decryption (Wave 9).
    #[cfg(feature = "lockbox")]
    pub lockbox_identity: Option<String>,
    /// Unlocked application keystore for encrypt/decrypt (App Keystore feature).
    #[cfg(feature = "keystore")]
    pub keystore: Option<Arc<rivers_keystore_engine::AppKeystore>>,
}

impl std::fmt::Debug for TaskContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("TaskContext");
        s.field("datasources", &self.datasources)
            .field("dataviews", &self.dataviews)
            .field("libs", &self.libs)
            .field("http", &self.http)
            .field("entrypoint", &self.entrypoint)
            .field("trace_id", &self.trace_id)
            .field("app_id", &self.app_id)
            .field("storage", &self.storage.as_ref().map(|_| "<StorageEngine>"))
            .field("driver_factory", &self.driver_factory.as_ref().map(|_| "<DriverFactory>"))
            .field("datasource_configs", &self.datasource_configs.keys().collect::<Vec<_>>())
            .field("dataview_executor", &self.dataview_executor.as_ref().map(|_| "<DataViewExecutor>"));
        #[cfg(feature = "lockbox")]
        s.field("lockbox", &self.lockbox.as_ref().map(|_| "<LockBoxResolver>"));
        #[cfg(feature = "keystore")]
        s.field("keystore", &self.keystore.as_ref().map(|_| "<AppKeystore>"));
        s.finish()
    }
}

// ── TaskResult / TaskError ───────────────────────────────────────

/// Result from a CodeComponent task execution.
#[derive(Debug)]
pub struct TaskResult {
    /// JSON result from the handler function.
    pub value: serde_json::Value,
    /// Execution wall-clock time in milliseconds.
    pub duration_ms: u64,
}

/// Errors that can occur during task dispatch or execution.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("queue full: pool has reached max_queue_depth")]
    QueueFull,

    #[error("task timeout after {0}ms")]
    Timeout(u64),

    #[error("worker crashed: {0}")]
    WorkerCrash(String),

    #[error("handler error: {0}")]
    HandlerError(String),

    #[error("capability error: {0}")]
    Capability(String),

    #[error("engine not available: {0}")]
    EngineUnavailable(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ── Worker Trait ─────────────────────────────────────────────────

/// Engine-agnostic worker trait.
///
/// Per spec §14: V8Worker and WasmWorker both implement this.
/// The pool dispatches to either transparently.
#[async_trait]
pub trait Worker: Send + Sync {
    /// Execute a task in this worker's sandbox.
    async fn execute(&self, ctx: TaskContext) -> Result<TaskResult, TaskError>;

    /// Reset the worker's internal state between tasks.
    async fn reset(&self) -> Result<(), TaskError>;

    /// Check if the worker is healthy and ready for tasks.
    fn is_healthy(&self) -> bool;

    /// Get the engine type name ("v8" or "wasmtime").
    fn engine_type(&self) -> &str;
}

// ── Capability Validation ────────────────────────────────────────

/// Validate task capabilities before dispatch.
///
/// Per spec §3.2: all declared resources must be available.
pub fn validate_capabilities(
    ctx: &TaskContext,
    available_datasources: &[String],
    available_dataviews: &[String],
) -> Result<(), TaskError> {
    for name in ctx.datasources.keys() {
        if !available_datasources.contains(name) {
            return Err(TaskError::Capability(format!(
                "datasource '{}' not available",
                name
            )));
        }
    }

    for name in ctx.dataviews.keys() {
        if !available_dataviews.contains(name) {
            return Err(TaskError::Capability(format!(
                "dataview '{}' not available",
                name
            )));
        }
    }

    Ok(())
}
