//! Thread-local declarations, TaskLocals guard, LockBoxContext, KeystoreContext.
//!
//! Every thread-local used by V8 host callbacks lives here. The `TaskLocals`
//! guard struct sets them on creation and clears them on Drop, making it
//! impossible to add a setup without a matching teardown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::types::*;
use rivers_runtime::rivers_core::{DriverFactory, StorageEngine};
use rivers_runtime::DataViewExecutor;

/// LockBox context for V8 host functions (HMAC key resolution).
pub(super) struct LockBoxContext {
    pub(super) resolver: Arc<rivers_runtime::rivers_core::lockbox::LockBoxResolver>,
    pub(super) keystore_path: std::path::PathBuf,
    pub(super) identity_str: String,
}

/// Application keystore context for V8 host functions (encrypt/decrypt + metadata).
pub(super) struct KeystoreContext {
    pub(super) keystore: Arc<rivers_keystore_engine::AppKeystore>,
}

// ── Thread-Local Async Bridge ───────────────────────────────────

thread_local! {
    /// Tokio runtime handle available to V8 host function callbacks.
    ///
    /// Set before V8 execution in `execute_js_task()`, used by native
    /// callbacks (ctx.dataview, Rivers.http, etc.) to bridge sync V8
    /// into async tokio via `rt_handle.block_on()`.
    ///
    /// Safe because each task runs on its own `spawn_blocking` thread
    /// and the thread-local is set before V8 starts and cleared after.
    pub(super) static RT_HANDLE: RefCell<Option<tokio::runtime::Handle>> = RefCell::new(None);

    /// Environment variables for the current task.
    /// Set before V8 execution, read by `inject_rivers_global()`.
    pub(super) static TASK_ENV: RefCell<Option<HashMap<String, String>>> = RefCell::new(None);

    /// Per-task key-value store (V2.4.4).
    ///
    /// Persists across the handler call on the same blocking thread.
    /// Set/cleared in `execute_js_task()` alongside the other thread-locals.
    /// Accessible from both JS (via native V8 callbacks) and Rust.
    pub(super) static TASK_STORE: RefCell<HashMap<String, serde_json::Value>> = RefCell::new(HashMap::new());

    /// Trace ID for the current task — included in Rivers.log output (X1.3).
    pub(super) static TASK_TRACE_ID: RefCell<Option<String>> = RefCell::new(None);

    /// Whether outbound HTTP is allowed for the current task (X2.1).
    /// Only `true` when `TaskContext.http` is `Some`.
    pub(super) static TASK_HTTP_ENABLED: RefCell<bool> = RefCell::new(false);

    /// Real StorageEngine backend for ctx.store (X3).
    /// When `Some`, ctx.store.get/set/del use async bridge to StorageEngine.
    /// When `None`, falls back to TASK_STORE in-memory HashMap.
    pub(super) static TASK_STORAGE: RefCell<Option<Arc<dyn StorageEngine>>> = RefCell::new(None);

    /// Namespace prefix for ctx.store operations (X3.2).
    /// Set to `app:{app_id}` for per-app isolation.
    pub(super) static TASK_STORE_NAMESPACE: RefCell<Option<String>> = RefCell::new(None);

    /// DriverFactory for ctx.datasource().build() execution (X7).
    /// When available, .build() resolves the datasource token -> driver -> connection -> execute.
    pub(super) static TASK_DRIVER_FACTORY: RefCell<Option<Arc<DriverFactory>>> = RefCell::new(None);

    /// DataViewExecutor for ctx.dataview() dynamic execution (X4).
    /// When available, ctx.dataview() falls back to executor if not pre-fetched.
    pub(super) static TASK_DV_EXECUTOR: RefCell<Option<Arc<DataViewExecutor>>> = RefCell::new(None);

    /// Entry-point namespace for DataView lookups (e.g. "handlers").
    /// DataViews are registered as "{entry_point}:{name}" — this prefix is
    /// prepended to bare names in ctx.dataview() calls.
    pub(super) static TASK_DV_NAMESPACE: RefCell<Option<String>> = RefCell::new(None);

    /// Resolved datasource configs: token name -> (driver_name, ConnectionParams).
    /// Populated from TaskContext at task start. .build() uses this to resolve connections.
    pub(super) static TASK_DS_CONFIGS: RefCell<HashMap<String, ResolvedDatasource>> = RefCell::new(HashMap::new());

    /// LockBox context for HMAC key resolution (Wave 9).
    /// When `Some`, `Rivers.crypto.hmac()` resolves keys via LockBox alias.
    /// When `None`, falls back to raw key (dev/test mode).
    pub(super) static TASK_LOCKBOX: RefCell<Option<LockBoxContext>> = RefCell::new(None);

    /// Application keystore for encrypt/decrypt and key metadata (App Keystore feature).
    /// When `Some`, `Rivers.keystore.*` and `Rivers.crypto.encrypt/decrypt` are available.
    /// When `None`, those functions throw "keystore not configured".
    pub(super) static TASK_KEYSTORE: RefCell<Option<KeystoreContext>> = RefCell::new(None);
}

/// Get the current tokio runtime handle from the thread-local.
pub(super) fn get_rt_handle() -> Result<tokio::runtime::Handle, TaskError> {
    RT_HANDLE.with(|h| h.borrow().clone())
        .ok_or_else(|| TaskError::Internal("tokio runtime handle not available".into()))
}

// ── TaskLocals Guard ─────────────────────────────────────────────

/// Guards all task-scoped thread-locals. Set on creation, cleared on Drop.
///
/// Adding a new thread-local to the setup is impossible without adding cleanup —
/// the Drop impl handles all fields. This replaces the previous parallel-list
/// pattern where setup and teardown were separate blocks that had to stay in sync.
pub(super) struct TaskLocals;

impl TaskLocals {
    /// Populate every task-scoped thread-local from `ctx` and the captured runtime handle.
    pub(super) fn set(ctx: &TaskContext, rt_handle: tokio::runtime::Handle) -> Self {
        RT_HANDLE.with(|h| *h.borrow_mut() = Some(rt_handle));
        TASK_ENV.with(|e| *e.borrow_mut() = Some(ctx.env.clone()));
        TASK_STORE.with(|s| s.borrow_mut().clear());
        TASK_TRACE_ID.with(|t| *t.borrow_mut() = Some(ctx.trace_id.clone()));
        TASK_HTTP_ENABLED.with(|h| *h.borrow_mut() = ctx.http.is_some());
        TASK_STORAGE.with(|s| *s.borrow_mut() = ctx.storage.clone());
        let store_ns = if ctx.app_id.is_empty() {
            "app:default".to_string()
        } else {
            format!("app:{}", ctx.app_id)
        };
        TASK_STORE_NAMESPACE.with(|n| *n.borrow_mut() = Some(store_ns));
        TASK_DRIVER_FACTORY.with(|f| *f.borrow_mut() = ctx.driver_factory.clone());
        TASK_DS_CONFIGS.with(|c| *c.borrow_mut() = ctx.datasource_configs.clone());
        TASK_DV_EXECUTOR.with(|e| *e.borrow_mut() = ctx.dataview_executor.clone());
        // Extract _dv_namespace from args if present (set by pipeline for CodeComponent views)
        let dv_ns = ctx.args.get("_dv_namespace")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        TASK_DV_NAMESPACE.with(|n| *n.borrow_mut() = dv_ns);
        TASK_LOCKBOX.with(|lb| {
            *lb.borrow_mut() = match (&ctx.lockbox, &ctx.lockbox_keystore_path, &ctx.lockbox_identity) {
                (Some(resolver), Some(path), Some(identity)) => Some(LockBoxContext {
                    resolver: resolver.clone(),
                    keystore_path: path.clone(),
                    identity_str: identity.clone(),
                }),
                _ => None,
            };
        });
        // App keystore: first check TaskContext, then fall back to shared resolver.
        // Dispatch sites don't call .keystore() on the builder, so ctx.keystore is
        // typically None. The shared resolver (set at startup) provides the fallback.
        let keystore_arc = ctx.keystore.clone().or_else(|| {
            if ctx.app_id.is_empty() { return None; }
            let resolver = super::super::get_keystore_resolver()?;
            // app_id is used as entry_point in bundle loading
            resolver.get_for_entry_point(&ctx.app_id).cloned()
        });
        TASK_KEYSTORE.with(|ks| {
            *ks.borrow_mut() = keystore_arc.map(|k| KeystoreContext { keystore: k });
        });
        TaskLocals
    }
}

impl Drop for TaskLocals {
    fn drop(&mut self) {
        RT_HANDLE.with(|h| *h.borrow_mut() = None);
        TASK_ENV.with(|e| *e.borrow_mut() = None);
        TASK_STORE.with(|s| s.borrow_mut().clear());
        TASK_TRACE_ID.with(|t| *t.borrow_mut() = None);
        TASK_HTTP_ENABLED.with(|h| *h.borrow_mut() = false);
        TASK_STORAGE.with(|s| *s.borrow_mut() = None);
        TASK_STORE_NAMESPACE.with(|n| *n.borrow_mut() = None);
        TASK_DRIVER_FACTORY.with(|f| *f.borrow_mut() = None);
        TASK_DS_CONFIGS.with(|c| c.borrow_mut().clear());
        TASK_DV_EXECUTOR.with(|e| *e.borrow_mut() = None);
        TASK_DV_NAMESPACE.with(|n| *n.borrow_mut() = None);
        TASK_LOCKBOX.with(|lb| *lb.borrow_mut() = None);
        TASK_KEYSTORE.with(|ks| *ks.borrow_mut() = None);
    }
}
