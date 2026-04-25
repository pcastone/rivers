//! ProcessPool type definitions — TaskContext, TaskResult, TaskError, Tokens, Builder.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::rivers_core::{DriverFactory, StorageEngine};
use crate::DataViewExecutor;

pub use crate::rivers_engine_sdk::TaskKind;

// ── Opaque Tokens ────────────────────────────────────────────────

/// Opaque handle to a datasource — the isolate never holds a real connection.
///
/// Two dispatch modes:
/// - `Pooled` — resolves to a host-side connection pool by id (default for all
///   request/response drivers: postgres, mysql, redis, etc.).
/// - `Direct` — worker performs I/O directly against the given resource root.
///   Reserved for self-contained drivers like `filesystem` (spec §7.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DatasourceToken {
    /// Pool-backed — isolate dispatches to host pool by id.
    Pooled { pool_id: String },
    /// Self-contained — worker performs I/O directly with the given resource handle.
    Direct {
        /// Driver name (e.g. "filesystem").
        driver: String,
        /// Canonical root path the worker is allowed to operate within.
        root: std::path::PathBuf,
    },
    /// Message-broker publish capability — worker invokes
    /// `BrokerProducer::publish` via the V8 broker-dispatch bridge (BR-2026-04-23).
    /// ConnectionParams aren't carried here (keeps the token cheaply hashable);
    /// the worker looks them up from `TaskContext.datasource_configs`.
    Broker {
        /// Broker driver name (e.g. "kafka", "rabbitmq", "nats", "redis-streams").
        driver: String,
    },
}

impl DatasourceToken {
    /// Construct a `Pooled` token from a pool id.
    pub fn pooled(pool_id: impl Into<String>) -> Self {
        DatasourceToken::Pooled {
            pool_id: pool_id.into(),
        }
    }

    /// Construct a `Direct` token for a self-contained driver bound to `root`.
    pub fn direct(driver: impl Into<String>, root: std::path::PathBuf) -> Self {
        DatasourceToken::Direct {
            driver: driver.into(),
            root,
        }
    }

    /// Construct a `Broker` token for a message-broker publish capability.
    pub fn broker(driver: impl Into<String>) -> Self {
        DatasourceToken::Broker {
            driver: driver.into(),
        }
    }
}

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
    /// Driver name (e.g. "postgres", "mysql", "faker").
    pub driver_name: String,
    /// Connection parameters for the driver.
    pub params: crate::rivers_driver_sdk::ConnectionParams,
}

/// Canonical broker driver names (BR-2026-04-23). Kept as a `const` list
/// rather than a DriverFactory trait query because `resolve_token_for_dispatch`
/// is called from `bundle_loader::load` where the factory isn't yet fully
/// populated — static classification is simpler and safe because new broker
/// drivers are rare and always land alongside this list being updated.
pub const BROKER_DRIVER_NAMES: &[&str] = &["kafka", "rabbitmq", "nats", "redis-streams"];

fn is_broker_driver(driver_name: &str) -> bool {
    BROKER_DRIVER_NAMES.contains(&driver_name)
}

/// Emit the `DatasourceToken` variant appropriate for a resolved datasource.
///
/// Dispatch classification:
/// - **Filesystem** → `Direct` (worker performs chroot-sandboxed I/O).
/// - **Broker drivers** (kafka / rabbitmq / nats / redis-streams) → `Broker`
///   (worker publishes via a lazy-created `BrokerProducer`, spec:
///   `bugs/bugreport_2026-04-23.md`).
/// - **Everything else** → `Pooled` (host pool manager).
pub fn resolve_token_for_dispatch(rd: &ResolvedDatasource) -> DatasourceToken {
    if rd.driver_name == "filesystem" {
        return DatasourceToken::direct(
            rd.driver_name.clone(),
            std::path::PathBuf::from(&rd.params.database),
        );
    }
    if is_broker_driver(&rd.driver_name) {
        return DatasourceToken::broker(rd.driver_name.clone());
    }
    DatasourceToken::pooled(format!("{}:{}", rd.driver_name, rd.params.database))
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
    /// Library module name.
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
    /// Opaque datasource handles available to this task.
    pub datasources: HashMap<String, DatasourceToken>,
    /// Opaque DataView handles available to this task.
    pub dataviews: HashMap<String, DataViewToken>,
    /// Pre-resolved library modules to inject into the isolate.
    pub libs: Vec<ResolvedLib>,
    /// Outbound HTTP capability token (None = no HTTP access).
    pub http: Option<HttpToken>,
    /// Environment variables exposed to the handler.
    pub env: HashMap<String, String>,
    /// Module and function to invoke.
    pub entrypoint: Entrypoint,
    /// JSON arguments passed to the handler function.
    pub args: serde_json::Value,
    /// Distributed trace ID for observability.
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
    /// Dispatch-site classification — gates elevated capabilities (e.g. ctx.ddl).
    /// Required: every dispatch site MUST set this via `TaskContextBuilder::task_kind()`.
    pub task_kind: TaskKind,
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
    /// Pool task queue is full — backpressure signal.
    #[error("queue full: pool has reached max_queue_depth")]
    QueueFull,

    /// Task exceeded its execution timeout.
    #[error("task timeout after {0}ms")]
    Timeout(u64),

    /// Worker process crashed during execution.
    #[error("worker crashed: {0}")]
    WorkerCrash(String),

    /// Handler function returned an error.
    #[error("handler error: {0}")]
    HandlerError(String),

    /// Handler threw an uncaught exception, with the remapped `.ts` stack
    /// attached. Spec §5.2. Emitted from V8 dispatch; consumed by the
    /// per-app log router (always) and the debug-mode error envelope
    /// (when the app has `debug = true`).
    #[error("handler error: {message}")]
    HandlerErrorWithStack {
        /// Short error message (the error's `toString()` output without stack).
        message: String,
        /// Remapped stack trace — `.ts:line:col` positions resolved via
        /// `BundleModuleCache.source_map`.
        stack: String,
    },

    /// `ctx.transaction(ds, fn)` callback returned cleanly but the subsequent
    /// `driver.commit_transaction()` failed. The handler's side-effects MAY
    /// or MAY NOT have persisted — treat the transaction outcome as
    /// **unknown**. Spec §6 + financial-correctness gate: distinct from
    /// `HandlerErrorWithStack` so clients can distinguish "handler threw, no
    /// writes" from "commit failed, writes ambiguous" and choose retry policy
    /// accordingly.
    #[error("transaction commit failed on datasource '{datasource}': {message}")]
    TransactionCommitFailed {
        /// Datasource the transaction was scoped to (spec §6.2 single-ds rule).
        datasource: String,
        /// Driver-layer error message from `commit_transaction`.
        message: String,
    },

    /// SWC TypeScript compilation exceeded its per-module wall-clock budget.
    /// Spec / F2 (P1-7): `compile_typescript_with_imports_timeout` wraps each
    /// per-file compile so pathological TS (deep nested generics, runaway
    /// macro expansion, etc.) cannot hang `populate_module_cache` at
    /// bundle-load time. Distinct from `Timeout(u64)` (which is the V8
    /// per-task CPU limit) so callers can distinguish "compiler hung" from
    /// "handler hung". `module` is sanitized via `redact_to_app_relative`.
    #[error("typescript compile timeout in {module} after {timeout_ms}ms")]
    CompileTimeout {
        /// Redacted (app-relative) module path. Never contains host
        /// filesystem prefixes — safe to surface in HTTP responses + logs.
        module: String,
        /// The wall-clock budget (ms) that was exceeded. Matches whatever
        /// `RIVERS_SWC_COMPILE_TIMEOUT_MS` resolved to (or the 5000ms default).
        timeout_ms: u64,
    },

    /// Required capability (datasource, DataView, HTTP) not available.
    #[error("capability error: {0}")]
    Capability(String),

    /// Requested engine (V8 or WASM) is not loaded.
    #[error("engine not available: {0}")]
    EngineUnavailable(String),

    /// Unexpected internal error.
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

#[cfg(test)]
mod direct_token_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn pooled_token_constructs() {
        let t = DatasourceToken::pooled("pool-42");
        assert!(matches!(t, DatasourceToken::Pooled { .. }));
    }

    #[test]
    fn direct_token_carries_driver_and_root() {
        let t = DatasourceToken::direct("filesystem", PathBuf::from("/tmp/x"));
        match t {
            DatasourceToken::Direct { driver, root } => {
                assert_eq!(driver, "filesystem");
                assert_eq!(root, PathBuf::from("/tmp/x"));
            }
            _ => panic!("expected Direct variant"),
        }
    }

    fn mk_resolved(driver: &str, database: &str) -> ResolvedDatasource {
        ResolvedDatasource {
            driver_name: driver.into(),
            params: crate::rivers_driver_sdk::ConnectionParams {
                host: String::new(),
                port: 0,
                database: database.into(),
                username: String::new(),
                password: String::new(),
                options: std::collections::HashMap::new(),
            },
        }
    }

    #[test]
    fn filesystem_driver_yields_direct_token() {
        let rd = mk_resolved("filesystem", "/tmp");
        let tok = resolve_token_for_dispatch(&rd);
        match tok {
            DatasourceToken::Direct { driver, root } => {
                assert_eq!(driver, "filesystem");
                assert_eq!(root, PathBuf::from("/tmp"));
            }
            _ => panic!("expected Direct variant for filesystem driver"),
        }
    }

    #[test]
    fn postgres_driver_yields_pooled_token() {
        let rd = mk_resolved("postgres", "db");
        let tok = resolve_token_for_dispatch(&rd);
        assert!(matches!(tok, DatasourceToken::Pooled { .. }));
    }

    #[test]
    fn faker_driver_yields_pooled_token() {
        let rd = mk_resolved("faker", "noop");
        let tok = resolve_token_for_dispatch(&rd);
        assert!(matches!(tok, DatasourceToken::Pooled { .. }));
    }

    // ── BR-2026-04-23: broker tokens ────────────────────────────────

    #[test]
    fn br1_t1_broker_token_constructs() {
        let t = DatasourceToken::broker("kafka");
        match t {
            DatasourceToken::Broker { driver } => assert_eq!(driver, "kafka"),
            _ => panic!("expected Broker variant"),
        }
    }

    #[test]
    fn br1_t2_resolve_broker_driver_yields_broker_token() {
        for name in ["kafka", "rabbitmq", "nats", "redis-streams"] {
            let rd = mk_resolved(name, "noop");
            let tok = resolve_token_for_dispatch(&rd);
            match tok {
                DatasourceToken::Broker { driver } => assert_eq!(driver, name),
                other => panic!("expected Broker variant for {name}, got {other:?}"),
            }
        }
    }

    #[test]
    fn br1_t3_pooled_drivers_still_yield_pooled() {
        // Regression guard — request/response drivers must stay Pooled
        // after the broker-classification extension.
        for name in ["postgres", "mysql", "sqlite", "redis", "elasticsearch", "mongodb", "faker"] {
            let rd = mk_resolved(name, "x");
            let tok = resolve_token_for_dispatch(&rd);
            assert!(
                matches!(tok, DatasourceToken::Pooled { .. }),
                "expected {name} → Pooled, got {tok:?}"
            );
        }
    }
}
