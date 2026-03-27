//! V8 JavaScript engine — thread-locals, isolate pool, execute_js_task, all V8 callbacks.
//!
//! This module contains all code that depends on the `v8` crate.
//! Gated behind #[cfg(feature = "static-engines")].

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use super::types::*;
use super::{ActiveTask, TaskTerminator};
use super::v8_config::compile_typescript;
use rivers_runtime::rivers_core::{DriverFactory, StorageEngine};
use rivers_runtime::rivers_core::storage::Bytes;
use rivers_runtime::DataViewExecutor;

/// LockBox context for V8 host functions (HMAC key resolution).
struct LockBoxContext {
    resolver: Arc<rivers_runtime::rivers_core::lockbox::LockBoxResolver>,
    keystore_path: std::path::PathBuf,
    identity_str: String,
}

/// Application keystore context for V8 host functions (encrypt/decrypt + metadata).
struct KeystoreContext {
    keystore: Arc<rivers_keystore_engine::AppKeystore>,
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
    static RT_HANDLE: RefCell<Option<tokio::runtime::Handle>> = RefCell::new(None);

    /// Environment variables for the current task.
    /// Set before V8 execution, read by `inject_rivers_global()`.
    static TASK_ENV: RefCell<Option<HashMap<String, String>>> = RefCell::new(None);

    /// Per-task key-value store (V2.4.4).
    ///
    /// Persists across the handler call on the same blocking thread.
    /// Set/cleared in `execute_js_task()` alongside the other thread-locals.
    /// Accessible from both JS (via native V8 callbacks) and Rust.
    static TASK_STORE: RefCell<HashMap<String, serde_json::Value>> = RefCell::new(HashMap::new());

    /// Trace ID for the current task — included in Rivers.log output (X1.3).
    static TASK_TRACE_ID: RefCell<Option<String>> = RefCell::new(None);

    /// Whether outbound HTTP is allowed for the current task (X2.1).
    /// Only `true` when `TaskContext.http` is `Some`.
    static TASK_HTTP_ENABLED: RefCell<bool> = RefCell::new(false);

    /// Real StorageEngine backend for ctx.store (X3).
    /// When `Some`, ctx.store.get/set/del use async bridge to StorageEngine.
    /// When `None`, falls back to TASK_STORE in-memory HashMap.
    static TASK_STORAGE: RefCell<Option<Arc<dyn StorageEngine>>> = RefCell::new(None);

    /// Namespace prefix for ctx.store operations (X3.2).
    /// Set to `app:{app_id}` for per-app isolation.
    static TASK_STORE_NAMESPACE: RefCell<Option<String>> = RefCell::new(None);

    /// DriverFactory for ctx.datasource().build() execution (X7).
    /// When available, .build() resolves the datasource token → driver → connection → execute.
    static TASK_DRIVER_FACTORY: RefCell<Option<Arc<DriverFactory>>> = RefCell::new(None);

    /// DataViewExecutor for ctx.dataview() dynamic execution (X4).
    /// When available, ctx.dataview() falls back to executor if not pre-fetched.
    static TASK_DV_EXECUTOR: RefCell<Option<Arc<DataViewExecutor>>> = RefCell::new(None);

    /// Resolved datasource configs: token name → (driver_name, ConnectionParams).
    /// Populated from TaskContext at task start. .build() uses this to resolve connections.
    static TASK_DS_CONFIGS: RefCell<HashMap<String, ResolvedDatasource>> = RefCell::new(HashMap::new());

    /// LockBox context for HMAC key resolution (Wave 9).
    /// When `Some`, `Rivers.crypto.hmac()` resolves keys via LockBox alias.
    /// When `None`, falls back to raw key (dev/test mode).
    static TASK_LOCKBOX: RefCell<Option<LockBoxContext>> = RefCell::new(None);

    /// Application keystore for encrypt/decrypt and key metadata (App Keystore feature).
    /// When `Some`, `Rivers.keystore.*` and `Rivers.crypto.encrypt/decrypt` are available.
    /// When `None`, those functions throw "keystore not configured".
    static TASK_KEYSTORE: RefCell<Option<KeystoreContext>> = RefCell::new(None);
}

type ActiveTaskRegistry = Arc<StdMutex<HashMap<usize, ActiveTask>>>;

/// Get the current tokio runtime handle from the thread-local.
fn get_rt_handle() -> Result<tokio::runtime::Handle, TaskError> {
    RT_HANDLE.with(|h| h.borrow().clone())
        .ok_or_else(|| TaskError::Internal("tokio runtime handle not available".into()))
}

// ── TaskLocals Guard ─────────────────────────────────────────────

/// Guards all task-scoped thread-locals. Set on creation, cleared on Drop.
///
/// Adding a new thread-local to the setup is impossible without adding cleanup —
/// the Drop impl handles all fields. This replaces the previous parallel-list
/// pattern where setup and teardown were separate blocks that had to stay in sync.
struct TaskLocals;

impl TaskLocals {
    /// Populate every task-scoped thread-local from `ctx` and the captured runtime handle.
    fn set(ctx: &TaskContext, rt_handle: tokio::runtime::Handle) -> Self {
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
        TASK_KEYSTORE.with(|ks| {
            *ks.borrow_mut() = ctx.keystore.as_ref().map(|k| KeystoreContext {
                keystore: k.clone(),
            });
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
        TASK_LOCKBOX.with(|lb| *lb.borrow_mut() = None);
        TASK_KEYSTORE.with(|ks| *ks.borrow_mut() = None);
    }
}

// ── V8 JavaScript Engine ────────────────────────────────────────

/// Create a V8 string, returning TaskError if it fails.
///
/// `v8::String::new()` returns `None` only if the string exceeds V8's
/// internal limit (~512 MB).  All call-sites pass short constant or
/// runtime-bounded strings, so failure is effectively impossible — but
/// propagating an error is more idiomatic than `.unwrap()`.
fn v8_str<'s>(
    scope: &mut v8::HandleScope<'s>,
    s: &str,
) -> Result<v8::Local<'s, v8::String>, TaskError> {
    v8::String::new(scope, s)
        .ok_or_else(|| TaskError::Internal(format!("V8 string creation failed for '{}'", s)))
}

/// One-time V8 platform initialization.
static V8_INIT: std::sync::Once = std::sync::Once::new();

pub(crate) fn ensure_v8_initialized() {
    V8_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}
// ── V2.8: Isolate Pool ─────────────────────────────────────────

/// Default heap limit per isolate: 128 MiB.
pub(crate) const DEFAULT_HEAP_LIMIT: usize = 128 * 1024 * 1024;

thread_local! {
    /// Thread-local pool of reusable V8 isolates (V2.8).
    static ISOLATE_POOL: RefCell<Vec<v8::OwnedIsolate>> = RefCell::new(Vec::new());
}

/// Acquire an isolate from the thread-local pool, or create a fresh one.
fn acquire_isolate(heap_limit: usize) -> v8::OwnedIsolate {
    ensure_v8_initialized();
    ISOLATE_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(|| {
            let params = v8::CreateParams::default().heap_limits(0, heap_limit);
            v8::Isolate::new(params)
        })
    })
}

/// Return an isolate to the thread-local pool for reuse.
fn release_isolate(isolate: v8::OwnedIsolate) {
    ISOLATE_POOL.with(|pool| {
        pool.borrow_mut().push(isolate);
    });
}

// ── V2.9: Script Source Cache ───────────────────────────────────

/// Global source-text cache: module path -> JavaScript source.
pub(crate) static SCRIPT_CACHE: std::sync::LazyLock<StdMutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Clear the script source cache (called on hot reload).
pub(crate) fn clear_script_cache() {
    if let Ok(mut cache) = SCRIPT_CACHE.lock() {
        cache.clear();
    }
}

// WASM module cache moved to wasm_engine.rs (AN13.4)
pub use super::wasm_engine::clear_wasm_cache;

// ── ES Module Support (T4) ──────────────────────────────────────

/// Execute a JavaScript module (supports import/export) using V8.
///
/// Used when the source contains `export` or `import` statements.
/// Per `rivers-processpool-runtime-spec-v2.md` — ES module execution path.
fn execute_as_module(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    source: &str,
    module_name: &str,
) -> Result<(), TaskError> {
    let source_str = v8::String::new(scope, source)
        .ok_or_else(|| TaskError::Internal("failed to create module source string".into()))?;
    let resource_name = v8::String::new(scope, module_name)
        .ok_or_else(|| TaskError::Internal("failed to create module name string".into()))?;

    let origin = v8::ScriptOrigin::new(
        scope,
        resource_name.into(),
        0,     // line offset
        0,     // column offset
        false, // is shared cross-origin
        -1,    // script id
        None,  // source map URL
        false, // is opaque
        false, // is WASM
        true,  // is module
        None,  // host defined options
    );

    let mut v8_source = v8::script_compiler::Source::new(source_str, Some(&origin));

    let tc = &mut v8::TryCatch::new(scope);
    let module = v8::script_compiler::compile_module(tc, &mut v8_source).ok_or_else(|| {
        let msg = tc
            .exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "unknown".into());
        TaskError::HandlerError(format!("module compilation failed: {msg}"))
    })?;

    // Instantiate with a resolve callback that rejects imports (V1 — no multi-module)
    let instantiate_result = module.instantiate_module(
        tc,
        |_context, _specifier, _import_attributes, _referrer| {
            // V1: reject all imports — single-module only
            // V2: implement module resolution from libraries/
            None
        },
    );

    if instantiate_result != Some(true) {
        let msg = tc
            .exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "module instantiation failed".into());
        return Err(TaskError::HandlerError(format!(
            "module instantiation: {msg}"
        )));
    }

    // Evaluate the module
    let _result = module.evaluate(tc).ok_or_else(|| {
        let msg = tc
            .exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "module evaluation failed".into());
        TaskError::HandlerError(format!("module evaluation: {msg}"))
    })?;

    // Pump microtask queue for top-level await
    tc.perform_microtask_checkpoint();

    Ok(())
}

/// Detect if source uses ES module syntax.
///
/// Checks for `export` or `import` keywords at statement boundaries.
pub(crate) fn is_module_syntax(source: &str) -> bool {
    source.contains("export ") || source.contains("export\n")
        || source.contains("import ") || source.contains("import\n")
}

/// If the return value is a Promise, resolve it by pumping the microtask queue.
///
/// Async handler functions return a Promise. This resolves the promise
/// synchronously by repeatedly pumping the V8 microtask queue until
/// the promise settles or a maximum iteration count is reached.
fn resolve_promise_if_needed(
    scope: &mut v8::HandleScope<'_>,
    value: v8::Local<v8::Value>,
) -> Result<serde_json::Value, TaskError> {
    if value.is_promise() {
        let promise = v8::Local::<v8::Promise>::try_from(value)
            .map_err(|_| TaskError::Internal("promise cast failed".into()))?;

        // Pump microtasks until the promise settles, with a bound to prevent infinite spinning
        let max_ticks = 10_000;
        for _ in 0..max_ticks {
            scope.perform_microtask_checkpoint();
            match promise.state() {
                v8::PromiseState::Fulfilled => {
                    let result = promise.result(scope);
                    return v8_to_json(scope, result);
                }
                v8::PromiseState::Rejected => {
                    let rejection = promise.result(scope);
                    let msg = rejection.to_rust_string_lossy(scope);
                    return Err(TaskError::HandlerError(format!(
                        "async handler rejected: {msg}"
                    )));
                }
                v8::PromiseState::Pending => continue,
            }
        }
        return Err(TaskError::Timeout(0));
    }
    // Not a promise — convert directly
    v8_to_json(scope, value)
}

/// RAII guard that frees a raw pointer when dropped.
/// Used to clean up the IsolateHandle passed to the near-heap-limit callback.
struct RawPtrGuard(*mut std::ffi::c_void);

impl Drop for RawPtrGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                drop(Box::from_raw(self.0 as *mut v8::IsolateHandle));
            }
        }
    }
}

/// Near-heap-limit callback for V8 isolates (P4.1).
///
/// When V8's heap approaches the configured limit, this callback terminates
/// execution via the IsolateHandle passed as the `data` pointer.  This
/// prevents V8 from hitting its fatal OOM handler (which aborts the process)
/// and instead causes a catchable termination exception.
///
/// We grant a small amount of extra headroom (5 MiB) so V8 has enough
/// memory to process the termination rather than immediately triggering
/// the fatal OOM handler.
extern "C" fn near_heap_limit_cb(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    if !data.is_null() {
        let handle = unsafe { &*(data as *const v8::IsolateHandle) };
        handle.terminate_execution();
    }
    // Grant a small amount of extra headroom for the termination to propagate
    current_heap_limit + 5 * 1024 * 1024
}

/// Execute a JavaScript task using the V8 engine.
///
/// V8 is synchronous from the Rust side, so execution happens on a
/// blocking thread via `tokio::task::spawn_blocking`.
///
/// The pool watchdog thread enforces the CPU timeout by calling
/// `isolate.terminate_execution()` if the handler exceeds `timeout_ms`.
pub(crate) async fn execute_js_task(
    ctx: TaskContext,
    timeout_ms: u64,
    worker_id: usize,
    heap_bytes: usize,
    heap_threshold: f64,
    registry: Option<ActiveTaskRegistry>,
) -> Result<TaskResult, TaskError> {
    ensure_v8_initialized();

    // Capture the tokio runtime handle BEFORE spawn_blocking (async context).
    // Inside spawn_blocking this handle enables V8 callbacks to call async code
    // via rt_handle.block_on().
    let rt_handle = tokio::runtime::Handle::current();

    let result = tokio::task::spawn_blocking(move || {
        // Set all task-scoped thread-locals and arm automatic cleanup.
        // TaskLocals::set populates every thread-local; its Drop impl clears
        // every one — impossible to add a setup without a matching teardown.
        let _locals = TaskLocals::set(&ctx, rt_handle);

        let start = Instant::now();

        // V2.8: Acquire a recycled isolate from the pool (or create a new one).
        // X5.2: Use pool-configured heap limit instead of default.
        let effective_heap = if heap_bytes > 0 { heap_bytes } else { DEFAULT_HEAP_LIMIT };
        let mut isolate = acquire_isolate(effective_heap);

        // Install near-heap-limit callback to terminate execution gracefully
        // instead of letting V8 hit the fatal OOM handler (which aborts the process).
        // We pass the IsolateHandle as the data pointer so the callback can
        // call terminate_execution() before V8 reaches the fatal limit.
        let heap_cb_handle = Box::new(isolate.thread_safe_handle());
        let heap_cb_ptr = Box::into_raw(heap_cb_handle) as *mut std::ffi::c_void;
        let _heap_cb_guard = RawPtrGuard(heap_cb_ptr);
        isolate.add_near_heap_limit_callback(near_heap_limit_cb, heap_cb_ptr);

        // Wave 10: Register in pool watchdog registry for V8 timeout
        let isolate_handle = isolate.thread_safe_handle();
        if let Some(ref reg) = registry {
            reg.lock().unwrap().insert(worker_id, ActiveTask {
                started_at: start,
                timeout_ms,
                terminator: TaskTerminator::V8(isolate_handle.clone()),
            });
        }

        // Resolve source before entering V8 scopes (no V8 refs needed)
        let source = resolve_module_source(&ctx)?;

        // All V8 scope work in a block so scopes drop before pool release (V2.8).
        let task_result: Result<TaskResult, TaskError> = {
            let mut handle_scope = v8::HandleScope::new(&mut isolate);
            let context = v8::Context::new(&mut handle_scope, Default::default());
            let mut scope = v8::ContextScope::new(&mut handle_scope, context);

            // Inject the `ctx` and `__args` globals
            inject_ctx_object(&mut scope, &ctx)?;

            // Inject ctx.dataview(), ctx.store, ctx.datasource() methods (P3)
            inject_ctx_methods(&mut scope)?;

            // Inject Rivers global utilities (Rivers.log, Rivers.crypto, console)
            inject_rivers_global(&mut scope)?;

            // Choose script or module execution (T4: ES module support)
            if is_module_syntax(&source) {
                execute_as_module(&mut scope, &source, &ctx.entrypoint.module)?;
                // For modules, exported functions are set on the module namespace.
                // For V1: the module must set the entrypoint on the global scope
                // (e.g., via a side effect or `globalThis.handler = handler`).
            } else {
                // Existing script-based execution
                let v8_source = v8::String::new(&mut scope, &source)
                    .ok_or_else(|| TaskError::Internal("failed to create V8 string".into()))?;

                let tc_compile = &mut v8::TryCatch::new(&mut scope);
                let script = v8::Script::compile(tc_compile, v8_source, None)
                    .ok_or_else(|| {
                        if tc_compile.has_terminated() {
                            return TaskError::Timeout(timeout_ms);
                        }
                        let msg = tc_compile
                            .exception()
                            .map(|e| e.to_rust_string_lossy(tc_compile))
                            .unwrap_or_else(|| "unknown".into());
                        TaskError::HandlerError(format!("JS compilation failed: {msg}"))
                    })?;
                script.run(tc_compile).ok_or_else(|| {
                    if tc_compile.has_terminated() {
                        return TaskError::Timeout(timeout_ms);
                    }
                    let msg = tc_compile
                        .exception()
                        .map(|e| e.to_rust_string_lossy(tc_compile))
                        .unwrap_or_else(|| "unknown".into());
                    TaskError::HandlerError(format!("JS top-level execution failed: {msg}"))
                })?;
            }

            // Call the entrypoint function (returns JSON via TryCatch)
            let return_value = call_entrypoint(&mut scope, &ctx.entrypoint.function);

            // Handle timeout detected during entrypoint call
            let return_value = match return_value {
                Err(TaskError::Timeout(_)) => return Err(TaskError::Timeout(timeout_ms)),
                other => other?,
            };

            // P1.1: Read ctx.resdata back from V8 (handler may have modified it).
            // If handler set ctx.resdata, use that. Otherwise use the return value.
            // This supports both:
            //   - Standard handlers: set ctx.resdata, return void
            //   - Guard handlers: return claims directly
            let ctx_key = v8_str(&mut scope, "ctx")?;
            let global = scope.get_current_context().global(&mut scope);
            if let Some(ctx_obj) = global.get(&mut scope, ctx_key.into()) {
                if let Ok(ctx_obj) = v8::Local::<v8::Object>::try_from(ctx_obj) {
                    let resdata_key = v8_str(&mut scope, "resdata")?;
                    if let Some(resdata_val) = ctx_obj.get(&mut scope, resdata_key.into()) {
                        if !resdata_val.is_null() && !resdata_val.is_undefined() {
                            // ctx.resdata was set by handler — use it as the result
                            let json_value = v8_to_json(&mut scope, resdata_val)?;
                            let duration_ms = start.elapsed().as_millis() as u64;
                            return Ok(TaskResult { value: json_value, duration_ms });
                        }
                    }
                }
            }

            // Fall back to handler return value
            let duration_ms = start.elapsed().as_millis() as u64;
            Ok(TaskResult {
                value: return_value,
                duration_ms,
            })

        }; // <-- V8 scopes dropped here

        // Wave 10: Deregister from pool watchdog
        if let Some(ref reg) = registry {
            reg.lock().unwrap().remove(&worker_id);
        }

        // V2.8: Return isolate to pool for reuse.
        // Remove the near-heap-limit callback before recycling so it
        // can be re-registered on the next use.
        // X5.5: Check heap usage against threshold — recycle if too high.
        if task_result.is_ok() {
            isolate.remove_near_heap_limit_callback(near_heap_limit_cb, effective_heap);

            let should_recycle = if heap_threshold > 0.0 && effective_heap > 0 {
                let mut stats = v8::HeapStatistics::default();
                isolate.get_heap_statistics(&mut stats);
                let threshold_bytes = (effective_heap as f64 * heap_threshold) as usize;
                stats.used_heap_size() > threshold_bytes
            } else {
                false
            };

            if should_recycle {
                tracing::debug!(
                    target: "rivers.pool",
                    worker_id = worker_id,
                    "recycling V8 isolate — heap usage exceeded threshold"
                );
                // Drop isolate instead of returning to pool — a fresh one will be created next time
                drop(isolate);
            } else {
                release_isolate(isolate);
            }
        }

        task_result
    })
    .await
    .map_err(|e| TaskError::WorkerCrash(format!("worker {worker_id} panicked: {e}")))?;

    result
}

/// Build the `ctx` global object from the task context.
///
/// Injects `ctx` with trace_id, request, session, data, resdata
/// and `__args` with the raw task arguments.
fn inject_ctx_object(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    task: &TaskContext,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);

    // Build ctx JSON and parse into V8
    let ctx_json = serde_json::json!({
        "trace_id": task.trace_id,
        "app_id": task.app_id,
        "node_id": task.node_id,
        "env": task.runtime_env,
        "request": task.args.get("request").cloned().unwrap_or(serde_json::Value::Null),
        "session": task.args.get("session").cloned().unwrap_or(serde_json::Value::Null),
        "data": {},
        "resdata": null,
    });
    let ctx_val = json_to_v8(scope, &ctx_json)?;
    let ctx_key = v8::String::new(scope, "ctx")
        .ok_or_else(|| TaskError::Internal("failed to create 'ctx' key".into()))?;
    global.set(scope, ctx_key.into(), ctx_val);

    // Also register __args for guard handlers
    let args_val = json_to_v8(scope, &task.args)?;
    let args_key = v8::String::new(scope, "__args")
        .ok_or_else(|| TaskError::Internal("failed to create '__args' key".into()))?;
    global.set(scope, args_key.into(), args_val);

    Ok(())
}

/// Inject host function bindings on the `ctx` object (P3 → V2).
///
/// V2 replaces the V1 error stubs with real native callbacks:
/// - `ctx.dataview(name, params)` — native V8 callback that checks
///   pre-fetched `ctx.data[name]` first (handles 90% of use cases).
///   Falls back to null with a warning if not pre-fetched.
/// - `ctx.store` — native V8 callbacks backed by `TASK_STORE` thread-local
///   (V2.4.4). Reserved prefix enforcement for session:/csrf:/cache:/raft:/rivers:.
/// - `ctx.streamDataview(name)` — mock iterator over pre-fetched data (V2.3).
///   Returns an object with `.next()` implementing the iterator protocol.
/// - `ctx.datasource()` — builder pattern stub (execution deferred to V3).
fn inject_ctx_methods(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);
    let ctx_key = v8_str(scope, "ctx")?;
    let ctx_val = global
        .get(scope, ctx_key.into())
        .ok_or_else(|| TaskError::Internal("ctx not found on global".into()))?;
    let ctx_obj = v8::Local::<v8::Object>::try_from(ctx_val)
        .map_err(|_| TaskError::Internal("ctx is not an object".into()))?;

    // ctx.dataview() — native V8 callback (P3.1 V2)
    let dataview_fn = v8::Function::new(scope, ctx_dataview_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.dataview".into()))?;
    let dv_key = v8_str(scope, "dataview")?;
    ctx_obj.set(scope, dv_key.into(), dataview_fn.into());

    // ctx.store — native V8 callbacks with reserved prefix enforcement (V2.4.4)
    let store_obj = v8::Object::new(scope);

    let store_get_fn = v8::Function::new(scope, ctx_store_get_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.store.get".into()))?;
    let get_key = v8_str(scope, "get")?;
    store_obj.set(scope, get_key.into(), store_get_fn.into());

    let store_set_fn = v8::Function::new(scope, ctx_store_set_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.store.set".into()))?;
    let set_key = v8_str(scope, "set")?;
    store_obj.set(scope, set_key.into(), store_set_fn.into());

    let store_del_fn = v8::Function::new(scope, ctx_store_del_callback)
        .ok_or_else(|| TaskError::Internal("failed to create ctx.store.del".into()))?;
    let del_key = v8_str(scope, "del")?;
    store_obj.set(scope, del_key.into(), store_del_fn.into());

    let store_key_on_ctx = v8_str(scope, "store")?;
    ctx_obj.set(scope, store_key_on_ctx.into(), store_obj.into());

    // X7: __ds_build native callback for ctx.datasource().build()
    let ds_build_fn = v8::Function::new(scope, ctx_datasource_build_callback)
        .ok_or_else(|| TaskError::Internal("failed to create __ds_build".into()))?;
    let ds_build_key = v8_str(scope, "__ds_build")?;
    global.set(scope, ds_build_key.into(), ds_build_fn.into());

    // ctx.streamDataview, ctx.datasource via JS
    let js_methods = r#"
        // V2.3: ctx.streamDataview(name) — mock iterator over pre-fetched data
        ctx.streamDataview = function(name) {
            // Get data from pre-fetched ctx.data
            var data = ctx.data[name];
            if (!data) {
                return { next: function() { return { done: true }; } };
            }
            // If it's an array, iterate element by element
            if (Array.isArray(data)) {
                var index = 0;
                return {
                    next: function() {
                        if (index < data.length) {
                            return { value: data[index++], done: false };
                        }
                        return { done: true };
                    }
                };
            }
            // Single value — return once
            var returned = false;
            return {
                next: function() {
                    if (!returned) {
                        returned = true;
                        return { value: data, done: false };
                    }
                    return { done: true };
                }
            };
        };

        // X7: ctx.datasource() — builder chain with native .build() execution
        ctx.datasource = function(name) {
            return {
                _datasource: name,
                _query: null,
                _params: null,
                _schema: null,
                fromQuery: function(sql, params) { this._query = sql; this._params = params || null; return this; },
                fromSchema: function(schema, params) { this._schema = schema; this._params = params || null; return this; },
                withGetSchema: function(s) { this._getSchema = s; return this; },
                withPostSchema: function(s) { this._postSchema = s; return this; },
                withPutSchema: function(s) { this._putSchema = s; return this; },
                withDeleteSchema: function(s) { this._deleteSchema = s; return this; },
                build: function() {
                    return __ds_build(this._datasource, this._query, this._params);
                }
            };
        };

        // P3.5: ctx.ws — undefined by default (only set for WebSocket views)
    "#;

    let js_src = v8::String::new(scope, js_methods)
        .ok_or_else(|| TaskError::Internal("failed to create ctx methods source".into()))?;
    let script = v8::Script::compile(scope, js_src, None)
        .ok_or_else(|| TaskError::Internal("failed to compile ctx methods".into()))?;
    script
        .run(scope)
        .ok_or_else(|| TaskError::Internal("failed to run ctx methods".into()))?;

    Ok(())
}

// ── ctx.store Native V8 Callbacks (V2.4.4) ─────────────────────

/// Reserved key prefixes for the task store. Keys starting with these
/// prefixes are reserved for system use and rejected with an error.
const STORE_RESERVED_PREFIXES: &[&str] = &["session:", "csrf:", "cache:", "raft:", "rivers:"];

/// Check if a store key uses a reserved namespace prefix.
fn store_key_is_reserved(key: &str) -> bool {
    STORE_RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Native V8 callback for `ctx.store.get(key)`.
///
/// X3: If a StorageEngine is available, reads via async bridge with namespace.
/// Falls back to `TASK_STORE` in-memory HashMap if no engine is injected.
/// Throws if the key uses a reserved prefix.
///
/// V8 callback — short constant strings, unwrap is safe.
fn ctx_store_get_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8::String::new(scope, &format!("ctx.store: key '{}' uses reserved namespace", key)).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // X3: Try real StorageEngine first
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        match get_rt_handle() {
            Ok(rt) => {
                match rt.block_on(engine.get(&namespace, &key)) {
                    Ok(Some(bytes)) => {
                        let json_str = String::from_utf8(bytes).unwrap_or_else(|_| "null".into());
                        let v8_str = v8::String::new(scope, &json_str).unwrap();
                        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                            rv.set(parsed);
                        } else {
                            rv.set(v8::null(scope).into());
                        }
                        return;
                    }
                    Ok(None) => {
                        rv.set(v8::null(scope).into());
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(target: "rivers.store", "StorageEngine get failed: {e}, falling back to in-memory");
                    }
                }
            }
            Err(_) => {
                tracing::warn!(target: "rivers.store", "no runtime handle for StorageEngine, falling back to in-memory");
            }
        }
    }

    // Fallback: in-memory TASK_STORE
    let value = TASK_STORE.with(|s| s.borrow().get(&key).cloned());
    match value {
        Some(v) => {
            let json_str = serde_json::to_string(&v).unwrap_or_else(|_| "null".into());
            let v8_str = v8::String::new(scope, &json_str).unwrap();
            if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                rv.set(parsed);
            } else {
                rv.set(v8::null(scope).into());
            }
        }
        None => rv.set(v8::null(scope).into()),
    }
}

/// Native V8 callback for `ctx.store.set(key, value, ttl?)`.
///
/// X3: If a StorageEngine is available, writes via async bridge with namespace and TTL.
/// Falls back to `TASK_STORE` in-memory HashMap if no engine is injected.
/// Throws if the key uses a reserved prefix.
///
/// V8 callback — short constant strings, unwrap is safe.
fn ctx_store_set_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8::String::new(scope, &format!("ctx.store: key '{}' uses reserved namespace", key)).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    let value_v8 = args.get(1);
    let json_value = if value_v8.is_undefined() || value_v8.is_null() {
        serde_json::Value::Null
    } else {
        let json_str = v8::json::stringify(scope, value_v8)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_else(|| "null".into());
        serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null)
    };

    // X3: Extract optional TTL from third argument (milliseconds)
    let ttl_ms = {
        let ttl_v8 = args.get(2);
        if ttl_v8.is_undefined() || ttl_v8.is_null() {
            None
        } else {
            ttl_v8.number_value(scope).map(|n| n as u64)
        }
    };

    // X3: Try real StorageEngine first
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        if let Ok(rt) = get_rt_handle() {
            let bytes: Bytes = serde_json::to_vec(&json_value).unwrap_or_else(|_| b"null".to_vec());
            if let Err(e) = rt.block_on(engine.set(&namespace, &key, bytes, ttl_ms)) {
                tracing::warn!(target: "rivers.store", "StorageEngine set failed: {e}, falling back to in-memory");
            } else {
                // Also update in-memory store for same-task reads
                TASK_STORE.with(|s| s.borrow_mut().insert(key, json_value));
                return;
            }
        }
    }

    // Fallback: in-memory TASK_STORE
    TASK_STORE.with(|s| s.borrow_mut().insert(key, json_value));
}

/// Native V8 callback for `ctx.store.del(key)`.
///
/// X3: If a StorageEngine is available, deletes via async bridge with namespace.
/// Falls back to `TASK_STORE` in-memory HashMap if no engine is injected.
/// Throws if the key uses a reserved prefix.
///
/// V8 callback — short constant strings, unwrap is safe.
fn ctx_store_del_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args.get(0).to_rust_string_lossy(scope);

    if store_key_is_reserved(&key) {
        let msg = v8::String::new(scope, &format!("ctx.store: key '{}' uses reserved namespace", key)).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // X3: Try real StorageEngine first
    let storage = TASK_STORAGE.with(|s| s.borrow().clone());
    if let Some(engine) = storage {
        let namespace = TASK_STORE_NAMESPACE.with(|n| n.borrow().clone()).unwrap_or_default();
        if let Ok(rt) = get_rt_handle() {
            if let Err(e) = rt.block_on(engine.delete(&namespace, &key)) {
                tracing::warn!(target: "rivers.store", "StorageEngine del failed: {e}, falling back to in-memory");
            } else {
                TASK_STORE.with(|s| s.borrow_mut().remove(&key));
                return;
            }
        }
    }

    // Fallback: in-memory TASK_STORE
    TASK_STORE.with(|s| s.borrow_mut().remove(&key));
}

/// Native V8 callback for `ctx.dataview(name, params)`.
///
/// X4: Checks `ctx.data[name]` for pre-fetched data first (fast path).
/// If not found, tries the DataViewExecutor via async bridge.
/// If no executor available, falls back to warn + null.
///
/// V8 callback — short constant strings, unwrap is safe.
fn ctx_dataview_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let name = args.get(0).to_rust_string_lossy(scope);

    // Look up in pre-fetched ctx.data first (fast path — handles 90% of use cases)
    let global = scope.get_current_context().global(scope);
    let ctx_key = v8::String::new(scope, "ctx").unwrap();
    if let Some(ctx_val) = global.get(scope, ctx_key.into()) {
        if let Ok(ctx_obj) = v8::Local::<v8::Object>::try_from(ctx_val) {
            let data_key = v8::String::new(scope, "data").unwrap();
            if let Some(data_val) = ctx_obj.get(scope, data_key.into()) {
                if let Ok(data_obj) = v8::Local::<v8::Object>::try_from(data_val) {
                    let name_key = v8::String::new(scope, &name).unwrap();
                    if let Some(cached) = data_obj.get(scope, name_key.into()) {
                        if !cached.is_undefined() && !cached.is_null() {
                            rv.set(cached);
                            return;
                        }
                    }
                }
            }
        }
    }

    // X4.2: Not in pre-fetched data — try DataViewExecutor via async bridge
    let executor = TASK_DV_EXECUTOR.with(|e| e.borrow().clone());
    if let Some(exec) = executor {
        // X4.3: Extract optional params from second V8 argument
        let params_v8 = args.get(1);
        let query_params: HashMap<String, rivers_runtime::rivers_driver_sdk::types::QueryValue> =
            if params_v8.is_undefined() || params_v8.is_null() {
                HashMap::new()
            } else if let Some(json_str) = v8::json::stringify(scope, params_v8) {
                let json_string = json_str.to_rust_string_lossy(scope);
                match serde_json::from_str::<serde_json::Value>(&json_string) {
                    Ok(serde_json::Value::Object(map)) => {
                        map.into_iter()
                            .map(|(k, v)| (k, json_to_query_value(v)))
                            .collect()
                    }
                    _ => HashMap::new(),
                }
            } else {
                HashMap::new()
            };

        let trace_id = TASK_TRACE_ID.with(|t| t.borrow().clone()).unwrap_or_default();

        match get_rt_handle() {
            Ok(rt) => {
                match rt.block_on(exec.execute(&name, query_params, "GET", &trace_id)) {
                    Ok(response) => {
                        // Convert QueryResult rows to JSON
                        let json = serde_json::json!({
                            "rows": response.query_result.rows,
                            "affected_rows": response.query_result.affected_rows,
                            "last_insert_id": response.query_result.last_insert_id,
                        });
                        let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "null".into());
                        let v8_str = v8::String::new(scope, &json_str).unwrap();
                        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                            rv.set(parsed);
                        } else {
                            rv.set(v8::null(scope).into());
                        }
                        return;
                    }
                    Err(e) => {
                        let msg = v8::String::new(
                            scope,
                            &format!("ctx.dataview('{}') execution error: {e}", name),
                        ).unwrap();
                        let exception = v8::Exception::error(scope, msg);
                        scope.throw_exception(exception);
                        return;
                    }
                }
            }
            Err(_) => {
                tracing::warn!(target: "rivers.handler", "no runtime handle for DataViewExecutor");
            }
        }
    }

    // Fallback: no executor and not pre-fetched — warn and return null
    tracing::warn!(
        target: "rivers.handler",
        "ctx.dataview('{}') not in pre-fetched data and no executor available. \
         Declare in view config: dataviews = [\"{}\"]",
        name, name
    );
    rv.set(v8::null(scope).into());
}

/// Native V8 callback for `__ds_build(datasource_name, sql, params)` (X7).
///
/// Called by `ctx.datasource(name).fromQuery(sql).build()`.
/// Resolves the datasource token → DriverFactory → Connection → execute.
/// Returns the query result as a V8 value.
///
/// V8 callback — cannot return Result.
fn ctx_datasource_build_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);
    let sql_val = args.get(1);
    let params_val = args.get(2);

    // Check capability: datasource must be declared in TaskContext.datasources
    let is_declared = TASK_DS_CONFIGS.with(|c| c.borrow().contains_key(&ds_name));
    if !is_declared {
        let msg = v8::String::new(
            scope,
            &format!("CapabilityError: datasource '{}' not declared in view config", ds_name),
        ).unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }

    // Require a SQL statement from .fromQuery()
    if sql_val.is_undefined() || sql_val.is_null() {
        let msg = v8::String::new(scope, "ctx.datasource().build(): call .fromQuery(sql) before .build()").unwrap();
        let exception = v8::Exception::error(scope, msg);
        scope.throw_exception(exception);
        return;
    }
    let sql = sql_val.to_rust_string_lossy(scope);

    // Extract params if provided
    let query_params: HashMap<String, rivers_runtime::rivers_driver_sdk::types::QueryValue> =
        if params_val.is_undefined() || params_val.is_null() {
            HashMap::new()
        } else if let Some(json_str) = v8::json::stringify(scope, params_val) {
            let json_string = json_str.to_rust_string_lossy(scope);
            // Try to parse as a JSON object and convert values to QueryValue
            match serde_json::from_str::<serde_json::Value>(&json_string) {
                Ok(serde_json::Value::Object(map)) => {
                    map.into_iter()
                        .map(|(k, v)| (k, json_to_query_value(v)))
                        .collect()
                }
                _ => HashMap::new(),
            }
        } else {
            HashMap::new()
        };

    // Get the DriverFactory and resolved config
    let factory = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone());
    let ds_config = TASK_DS_CONFIGS.with(|c| c.borrow().get(&ds_name).cloned());

    let (factory, config) = match (factory, ds_config) {
        (Some(f), Some(c)) => (f, c),
        _ => {
            let msg = v8::String::new(
                scope,
                &format!("ctx.datasource('{}').build(): DriverFactory not available", ds_name),
            ).unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    // Execute via async bridge: connect → build query → execute
    let rt = match get_rt_handle() {
        Ok(rt) => rt,
        Err(_) => {
            let msg = v8::String::new(scope, "ctx.datasource().build(): runtime handle not available").unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            return;
        }
    };

    let result = rt.block_on(async {
        let mut conn = factory.connect(&config.driver_name, &config.params).await
            .map_err(|e| format!("connection failed: {e}"))?;

        let mut query = rivers_runtime::rivers_driver_sdk::types::Query::new(&ds_name, &sql);
        for (k, v) in query_params {
            query.parameters.insert(k, v);
        }

        conn.execute(&query).await
            .map_err(|e| format!("query failed: {e}"))
    });

    match result {
        Ok(query_result) => {
            // Convert QueryResult to JSON
            let json = serde_json::json!({
                "rows": query_result.rows,
                "affected_rows": query_result.affected_rows,
                "last_insert_id": query_result.last_insert_id,
            });
            let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "null".into());
            let v8_str = v8::String::new(scope, &json_str).unwrap();
            if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
                rv.set(parsed);
            } else {
                rv.set(v8::null(scope).into());
            }
        }
        Err(e) => {
            let msg = v8::String::new(scope, &format!("ctx.datasource().build() error: {e}")).unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
        }
    }
}

/// Convert serde_json::Value to QueryValue for driver execution.
fn json_to_query_value(v: serde_json::Value) -> rivers_runtime::rivers_driver_sdk::types::QueryValue {
    match v {
        serde_json::Value::Null => rivers_runtime::rivers_driver_sdk::types::QueryValue::Null,
        serde_json::Value::Bool(b) => rivers_runtime::rivers_driver_sdk::types::QueryValue::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rivers_runtime::rivers_driver_sdk::types::QueryValue::Integer(i)
            } else {
                rivers_runtime::rivers_driver_sdk::types::QueryValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => rivers_runtime::rivers_driver_sdk::types::QueryValue::String(s),
        serde_json::Value::Array(a) => rivers_runtime::rivers_driver_sdk::types::QueryValue::Array(
            a.into_iter().map(json_to_query_value).collect(),
        ),
        serde_json::Value::Object(_) => rivers_runtime::rivers_driver_sdk::types::QueryValue::Json(v),
    }
}

/// Extract optional structured fields from a V8 value for `Rivers.log`.
///
/// Per spec §5.2: `Rivers.log.info(msg, fields?)` supports an optional
/// second argument containing a fields object for structured logging.
/// Returns a JSON string of the fields, or empty string if no fields.
fn extract_log_fields(scope: &mut v8::HandleScope, val: v8::Local<v8::Value>) -> String {
    if val.is_undefined() || val.is_null() {
        return String::new();
    }
    if let Ok(obj) = v8::Local::<v8::Object>::try_from(val) {
        if let Some(json_str) = v8::json::stringify(scope, obj.into()) {
            return json_str.to_rust_string_lossy(scope);
        }
    }
    String::new()
}

/// Inject the `Rivers` global utility namespace.
///
/// - `Rivers.log.{info,warn,error}` — native V8 callbacks → Rust `tracing` (P2.1).
///   Supports optional structured fields: `Rivers.log.info(msg, { key: val })`.
/// - `Rivers.crypto.randomHex` — real randomness via `rand` (P2.2).
/// - `Rivers.crypto.hashPassword/verifyPassword` — bcrypt cost 12 (P3.6).
/// - `Rivers.crypto.timingSafeEqual` — constant-time comparison (P3.6).
/// - `Rivers.crypto.randomBase64url` — real random base64url (P3.6).
/// - `Rivers.crypto.hmac` — real HMAC-SHA256 via `hmac` crate (V2).
/// - `Rivers.http.{get,post,put,del}` — real outbound HTTP via reqwest + async bridge (V2).
///   Only injected when `TaskContext.http` is `Some` (capability gating per spec §10.5).
/// - `Rivers.env` — task environment variables from `TaskContext.env` (V2).
/// - `console.{log,warn,error}` — delegates to `Rivers.log` (P2.3).
fn inject_rivers_global(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);

    // ── Rivers object ────────────────────────────────────────────
    let rivers_key = v8::String::new(scope, "Rivers")
        .ok_or_else(|| TaskError::Internal("failed to create 'Rivers' key".into()))?;
    let rivers_obj = v8::Object::new(scope);

    // ── Rivers.log (native V8 → tracing, with optional structured fields) ──
    let log_obj = v8::Object::new(scope);

    let info_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            if fields.is_empty() {
                tracing::info!(target: "rivers.handler", "{}", msg);
            } else {
                tracing::info!(target: "rivers.handler", fields = %fields, "{}", msg);
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.info".into()))?;
    let info_key = v8_str(scope, "info")?;
    log_obj.set(scope, info_key.into(), info_fn.into());

    let warn_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            if fields.is_empty() {
                tracing::warn!(target: "rivers.handler", "{}", msg);
            } else {
                tracing::warn!(target: "rivers.handler", fields = %fields, "{}", msg);
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.warn".into()))?;
    let warn_key = v8_str(scope, "warn")?;
    log_obj.set(scope, warn_key.into(), warn_fn.into());

    let error_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            if fields.is_empty() {
                tracing::error!(target: "rivers.handler", "{}", msg);
            } else {
                tracing::error!(target: "rivers.handler", fields = %fields, "{}", msg);
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.error".into()))?;
    let error_key = v8_str(scope, "error")?;
    log_obj.set(scope, error_key.into(), error_fn.into());

    let log_key = v8_str(scope, "log")?;
    rivers_obj.set(scope, log_key.into(), log_obj.into());

    // ── Rivers.crypto (native implementations) ───────────────────
    let crypto_obj = v8::Object::new(scope);

    // Rivers.crypto.randomHex — real randomness via rand (P2.2)
    let random_hex_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use rand::Rng;
            let len = args.get(0).int32_value(scope).unwrap_or(16) as usize;
            let len = len.min(1024); // cap to prevent abuse
            let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
            let hex_str = hex::encode(&bytes);
            if let Some(v8_str) = v8::String::new(scope, &hex_str) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.randomHex".into()))?;
    let random_hex_key = v8_str(scope, "randomHex")?;
    crypto_obj.set(scope, random_hex_key.into(), random_hex_fn.into());

    // Rivers.crypto.hashPassword — bcrypt cost 12 (P3.6)
    let hash_pw_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let pw = args.get(0).to_rust_string_lossy(scope);
            match bcrypt::hash(pw, 12) {
                Ok(hashed) => {
                    if let Some(v8_str) = v8::String::new(scope, &hashed) {
                        rv.set(v8_str.into());
                    }
                }
                Err(e) => {
                    let msg = v8::String::new(scope, &format!("hashPassword failed: {e}")).unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.hashPassword".into()))?;
    let hash_pw_key = v8_str(scope, "hashPassword")?;
    crypto_obj.set(scope, hash_pw_key.into(), hash_pw_fn.into());

    // Rivers.crypto.verifyPassword — bcrypt verify (P3.6)
    let verify_pw_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let pw = args.get(0).to_rust_string_lossy(scope);
            let hash = args.get(1).to_rust_string_lossy(scope);
            match bcrypt::verify(pw, &hash) {
                Ok(valid) => rv.set(v8::Boolean::new(scope, valid).into()),
                Err(_) => rv.set(v8::Boolean::new(scope, false).into()),
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.verifyPassword".into()))?;
    let verify_pw_key = v8_str(scope, "verifyPassword")?;
    crypto_obj.set(scope, verify_pw_key.into(), verify_pw_fn.into());

    // Rivers.crypto.timingSafeEqual — constant-time comparison (P3.6)
    let timing_safe_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let a = args.get(0).to_rust_string_lossy(scope);
            let b = args.get(1).to_rust_string_lossy(scope);
            // Constant-time comparison: always compare all bytes
            let equal = a.len() == b.len()
                && a.as_bytes()
                    .iter()
                    .zip(b.as_bytes())
                    .fold(0u8, |acc, (x, y)| acc | (x ^ y))
                    == 0;
            rv.set(v8::Boolean::new(scope, equal).into());
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.timingSafeEqual".into()))?;
    let timing_safe_key = v8_str(scope, "timingSafeEqual")?;
    crypto_obj.set(scope, timing_safe_key.into(), timing_safe_fn.into());

    // Rivers.crypto.randomBase64url — real random base64url (P3.6)
    let random_b64_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use base64::Engine;
            use rand::Rng;
            let len = args.get(0).int32_value(scope).unwrap_or(16) as usize;
            let len = len.min(1024); // cap to prevent abuse
            let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
            if let Some(v8_str) = v8::String::new(scope, &encoded) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.randomBase64url".into()))?;
    let random_b64_key = v8_str(scope, "randomBase64url")?;
    crypto_obj.set(scope, random_b64_key.into(), random_b64_fn.into());

    // Rivers.crypto.hmac — HMAC-SHA256 with LockBox alias resolution (Wave 9)
    //
    // Arg 0: alias name (resolved via LockBox) or raw key (fallback when no lockbox)
    // Arg 1: data string to HMAC
    // Returns: hex-encoded HMAC-SHA256
    let hmac_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            type HmacSha256 = Hmac<Sha256>;

            let alias_or_key = args.get(0).to_rust_string_lossy(scope);
            let data = args.get(1).to_rust_string_lossy(scope);

            // Try LockBox resolution first, fall back to raw key
            let key_result: Result<String, String> = TASK_LOCKBOX.with(|lb| {
                let lb = lb.borrow();
                match lb.as_ref() {
                    Some(ctx) => {
                        let metadata = ctx.resolver.resolve(&alias_or_key)
                            .ok_or_else(|| format!("lockbox alias not found: '{alias_or_key}'"))?;
                        let resolved = rivers_runtime::rivers_core::lockbox::fetch_secret_value(
                            metadata, &ctx.keystore_path, &ctx.identity_str,
                        ).map_err(|e| format!("lockbox fetch failed: {e}"))?;
                        Ok(resolved.value)
                    }
                    None => {
                        // No lockbox configured — use as raw key (dev/test mode)
                        Ok(alias_or_key.clone())
                    }
                }
            });

            match key_result {
                Ok(key) => {
                    match HmacSha256::new_from_slice(key.as_bytes()) {
                        Ok(mut mac) => {
                            mac.update(data.as_bytes());
                            let result = hex::encode(mac.finalize().into_bytes());
                            if let Some(v8_str) = v8::String::new(scope, &result) {
                                rv.set(v8_str.into());
                            }
                        }
                        Err(e) => {
                            let msg = v8::String::new(
                                scope,
                                &format!("Rivers.crypto.hmac() key error: {e}"),
                            )
                            .unwrap();
                            let exception = v8::Exception::error(scope, msg);
                            scope.throw_exception(exception);
                        }
                    }
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.hmac".into()))?;
    let hmac_key = v8_str(scope, "hmac")?;
    crypto_obj.set(scope, hmac_key.into(), hmac_fn.into());

    let crypto_key = v8_str(scope, "crypto")?;
    rivers_obj.set(scope, crypto_key.into(), crypto_obj.into());

    // ── Rivers.keystore (key metadata — App Keystore feature) ────
    let ks_available = TASK_KEYSTORE.with(|ks| ks.borrow().is_some());
    if ks_available {
        let keystore_obj = v8::Object::new(scope);

        // Rivers.keystore.has(name) — returns boolean
        let has_fn = v8::Function::new(
            scope,
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let name = args.get(0).to_rust_string_lossy(scope);
                let result = TASK_KEYSTORE.with(|ks| {
                    ks.borrow().as_ref()
                        .map(|ctx| ctx.keystore.has_key(&name))
                        .unwrap_or(false)
                });
                rv.set(v8::Boolean::new(scope, result).into());
            },
        )
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.keystore.has".into()))?;
        let has_key = v8_str(scope, "has")?;
        keystore_obj.set(scope, has_key.into(), has_fn.into());

        // Rivers.keystore.info(name) — returns {name, type, version, created_at} or throws
        let info_fn = v8::Function::new(
            scope,
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let name = args.get(0).to_rust_string_lossy(scope);
                let result = TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => ctx.keystore.key_info(&name)
                            .map_err(|e| e.to_string()),
                        None => Err("keystore not configured".to_string()),
                    }
                });

                match result {
                    Ok(info) => {
                        // Build a V8 object with the metadata
                        let obj = v8::Object::new(scope);

                        let name_key = v8::String::new(scope, "name").unwrap();
                        let name_val = v8::String::new(scope, &info.name).unwrap();
                        obj.set(scope, name_key.into(), name_val.into());

                        let type_key = v8::String::new(scope, "type").unwrap();
                        let type_val = v8::String::new(scope, &info.key_type).unwrap();
                        obj.set(scope, type_key.into(), type_val.into());

                        let ver_key = v8::String::new(scope, "version").unwrap();
                        let ver_val = v8::Integer::new(scope, info.current_version as i32);
                        obj.set(scope, ver_key.into(), ver_val.into());

                        let created_key = v8::String::new(scope, "created_at").unwrap();
                        let created_val = v8::String::new(scope, &info.created.to_rfc3339()).unwrap();
                        obj.set(scope, created_key.into(), created_val.into());

                        rv.set(obj.into());
                    }
                    Err(msg) => {
                        let err_msg = v8::String::new(scope, &msg).unwrap();
                        let exception = v8::Exception::error(scope, err_msg);
                        scope.throw_exception(exception);
                    }
                }
            },
        )
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.keystore.info".into()))?;
        let info_key = v8_str(scope, "info")?;
        keystore_obj.set(scope, info_key.into(), info_fn.into());

        let ks_key = v8_str(scope, "keystore")?;
        rivers_obj.set(scope, ks_key.into(), keystore_obj.into());
    }

    // ── Rivers.http — real outbound HTTP via async bridge (V2) ──
    // Per spec §10.5: only injected when allow_outbound_http = true (capability gating).
    // When not injected, `Rivers.http` is undefined in JS — natural V8 behavior.
    let http_enabled = TASK_HTTP_ENABLED.with(|h| *h.borrow());
    if http_enabled {
        let http_obj = v8::Object::new(scope);

        let http_get_fn = v8::Function::new(scope, rivers_http_get_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.get".into()))?;
        let get_key = v8_str(scope, "get")?;
        http_obj.set(scope, get_key.into(), http_get_fn.into());

        let http_post_fn = v8::Function::new(scope, rivers_http_post_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.post".into()))?;
        let post_key = v8_str(scope, "post")?;
        http_obj.set(scope, post_key.into(), http_post_fn.into());

        let http_put_fn = v8::Function::new(scope, rivers_http_put_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.put".into()))?;
        let put_key = v8_str(scope, "put")?;
        http_obj.set(scope, put_key.into(), http_put_fn.into());

        let http_del_fn = v8::Function::new(scope, rivers_http_del_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.del".into()))?;
        let del_key = v8_str(scope, "del")?;
        http_obj.set(scope, del_key.into(), http_del_fn.into());

        let http_key = v8_str(scope, "http")?;
        rivers_obj.set(scope, http_key.into(), http_obj.into());
    }

    // ── Rivers.env — task environment variables (V2) ─────────────
    let env_map = TASK_ENV.with(|e| e.borrow().clone()).unwrap_or_default();
    let env_json = serde_json::to_value(&env_map)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let env_val = json_to_v8(scope, &env_json)?;
    let env_key = v8_str(scope, "env")?;
    rivers_obj.set(scope, env_key.into(), env_val);

    // Set Rivers on global
    global.set(scope, rivers_key.into(), rivers_obj.into());

    // ── console.{log,warn,error} via JS eval ─────────────────────
    // X1.2: console delegates forward structured fields when the last argument is an object.
    let js_extras = r#"
        // console.{log,warn,error} → Rivers.log (P2.3)
        var console = {
            log: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.info(args.join(' '), last);
            },
            warn: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.warn(args.join(' '), last);
            },
            error: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.error(args.join(' '), last);
            },
        };
    "#;
    let js_src = v8::String::new(scope, js_extras)
        .ok_or_else(|| TaskError::Internal("failed to create extras source string".into()))?;
    let script = v8::Script::compile(scope, js_src, None)
        .ok_or_else(|| TaskError::Internal("failed to compile Rivers extras".into()))?;
    script
        .run(scope)
        .ok_or_else(|| TaskError::Internal("failed to run Rivers extras".into()))?;

    Ok(())
}

// ── Rivers.http Native Callbacks ────────────────────────────────

/// Helper: extract headers from a V8 object into a reqwest HeaderMap.
///
/// V8 callback helper — short constant strings, unwrap is safe.
fn extract_headers_from_opts(
    scope: &mut v8::HandleScope,
    opts: v8::Local<v8::Value>,
) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    if opts.is_undefined() || opts.is_null() {
        return headers;
    }
    if let Ok(opts_obj) = v8::Local::<v8::Object>::try_from(opts) {
        let headers_key = v8::String::new(scope, "headers").unwrap();
        if let Some(h_val) = opts_obj.get(scope, headers_key.into()) {
            if let Ok(h_obj) = v8::Local::<v8::Object>::try_from(h_val) {
                if let Some(names) = h_obj.get_own_property_names(scope, Default::default()) {
                    for i in 0..names.length() {
                        if let Some(key) = names.get_index(scope, i) {
                            let key_str = key.to_rust_string_lossy(scope);
                            if let Some(val) = h_obj.get(scope, key) {
                                let val_str = val.to_rust_string_lossy(scope);
                                headers.insert(key_str, val_str);
                            }
                        }
                    }
                }
            }
        }
    }
    headers
}

/// Helper: convert an HTTP response (status + body) into a V8 value.
///
/// V8 callback helper — short constant strings, unwrap is safe.
fn http_result_to_v8<'s>(
    scope: &mut v8::HandleScope<'s>,
    result: Result<serde_json::Value, String>,
) -> Option<v8::Local<'s, v8::Value>> {
    match result {
        Ok(json) => {
            let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "{}".into());
            let v8_str = v8::String::new(scope, &json_str)?;
            v8::json::parse(scope, v8_str.into())
        }
        Err(e) => {
            let msg =
                v8::String::new(scope, &format!("Rivers.http request failed: {e}")).unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            None
        }
    }
}

/// Extract host from URL for logging (avoids leaking query params / secrets).
fn extract_host(url: &str) -> &str {
    // Try to find host between :// and the next / or end
    if let Some(start) = url.find("://") {
        let after_scheme = &url[start + 3..];
        match after_scheme.find('/') {
            Some(end) => &url[start + 3..start + 3 + end],
            None => after_scheme,
        }
    } else {
        url
    }
}

/// Perform an HTTP request via the async bridge.
///
/// Per spec §10.5: each call is logged at INFO with destination host and trace ID.
fn do_http_request(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    // Log outbound request with host only (not full URL to avoid leaking secrets)
    let host = extract_host(url);
    let trace_id = TASK_TRACE_ID.with(|t| t.borrow().clone()).unwrap_or_default();
    tracing::info!(
        target: "rivers.http",
        method = %method,
        host = %host,
        trace_id = %trace_id,
        "outbound HTTP request"
    );

    let rt = get_rt_handle().map_err(|e| e.to_string())?;

    rt.block_on(async {
        let client = reqwest::Client::new();
        let mut builder = match method {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            _ => return Err(format!("unsupported HTTP method: {method}")),
        };

        for (k, v) in headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        if let Some(body_str) = body {
            builder = builder
                .header("content-type", "application/json")
                .body(body_str.to_string());
        }

        let resp = builder.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body_text = resp.text().await.map_err(|e| e.to_string())?;

        // Try to parse body as JSON, fall back to string
        let body_val: serde_json::Value = serde_json::from_str(&body_text)
            .unwrap_or(serde_json::Value::String(body_text));

        Ok(serde_json::json!({ "status": status, "body": body_val }))
    })
}

/// Rivers.http.get(url, opts?) callback.
/// V8 callback — cannot return Result.
fn rivers_http_get_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let headers = extract_headers_from_opts(scope, args.get(1));
    let result = do_http_request("GET", &url, None, &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Rivers.http.post(url, body, opts?) callback.
fn rivers_http_post_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let body_val = args.get(1);
    let body_str = if body_val.is_undefined() || body_val.is_null() {
        None
    } else {
        v8::json::stringify(scope, body_val).map(|s| s.to_rust_string_lossy(scope))
    };
    let headers = extract_headers_from_opts(scope, args.get(2));
    let result = do_http_request("POST", &url, body_str.as_deref(), &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Rivers.http.put(url, body, opts?) callback.
fn rivers_http_put_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let body_val = args.get(1);
    let body_str = if body_val.is_undefined() || body_val.is_null() {
        None
    } else {
        v8::json::stringify(scope, body_val).map(|s| s.to_rust_string_lossy(scope))
    };
    let headers = extract_headers_from_opts(scope, args.get(2));
    let result = do_http_request("PUT", &url, body_str.as_deref(), &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Rivers.http.del(url, opts?) callback.
fn rivers_http_del_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let headers = extract_headers_from_opts(scope, args.get(1));
    let result = do_http_request("DELETE", &url, None, &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Call the named entrypoint function with `ctx` as the sole argument.
///
/// Uses a `v8::TryCatch` scope to capture exception details including
/// error messages. Returns `serde_json::Value` to avoid V8 lifetime
/// escape issues with the TryCatch scope.
fn call_entrypoint(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    function_name: &str,
) -> Result<serde_json::Value, TaskError> {
    let global = scope.get_current_context().global(scope);

    let func_key = v8::String::new(scope, function_name)
        .ok_or_else(|| TaskError::Internal(format!("failed to create key '{function_name}'")))?;
    let func_val = global.get(scope, func_key.into()).ok_or_else(|| {
        TaskError::HandlerError(format!("function '{function_name}' not found"))
    })?;

    let func = v8::Local::<v8::Function>::try_from(func_val).map_err(|_| {
        TaskError::HandlerError(format!("'{function_name}' is not a function"))
    })?;

    let ctx_key = v8::String::new(scope, "ctx")
        .ok_or_else(|| TaskError::Internal("failed to create 'ctx' key".into()))?;
    let ctx_val = global
        .get(scope, ctx_key.into())
        .ok_or_else(|| TaskError::Internal("ctx not found on global".into()))?;

    let undefined = v8::undefined(scope).into();

    // Use TryCatch to capture exception details
    let tc_scope = &mut v8::TryCatch::new(scope);
    match func.call(tc_scope, undefined, &[ctx_val]) {
        Some(result) => {
            // T4: If the return value is a Promise (async function), resolve it
            if result.is_promise() {
                return resolve_promise_if_needed(tc_scope, result);
            }
            // Convert to JSON inside the TryCatch scope
            if result.is_undefined() || result.is_null() {
                Ok(serde_json::Value::Null)
            } else {
                let json_str = v8::json::stringify(tc_scope, result)
                    .ok_or_else(|| TaskError::Internal("V8 JSON.stringify failed".into()))?;
                let rust_str = json_str.to_rust_string_lossy(tc_scope);
                serde_json::from_str(&rust_str)
                    .map_err(|e| TaskError::Internal(format!("parse JSON result: {e}")))
            }
        }
        None => {
            // Check for isolate termination (timeout)
            if tc_scope.has_terminated() {
                return Err(TaskError::Timeout(0));
            }
            let msg = if let Some(exception) = tc_scope.exception() {
                exception.to_rust_string_lossy(tc_scope)
            } else {
                "unknown exception".to_string()
            };
            Err(TaskError::HandlerError(format!(
                "handler '{function_name}' threw: {msg}"
            )))
        }
    }
}

/// Resolve the JavaScript source code for a task.
///
/// V2.10: TypeScript sources (detected by file extension or entrypoint
/// language) are compiled to JavaScript via `compile_typescript()`.
fn resolve_module_source(ctx: &TaskContext) -> Result<String, TaskError> {
    if let Some(source) = ctx.args.get("_source").and_then(|v| v.as_str()) {
        // Check if the entrypoint language is typescript
        if ctx.entrypoint.language == "typescript" {
            return compile_typescript(source, &ctx.entrypoint.module);
        }
        return Ok(source.to_string());
    }
    let path = &ctx.entrypoint.module;

    let source = std::fs::read_to_string(path)
        .map_err(|e| TaskError::HandlerError(format!("cannot read module '{path}': {e}")))?;

    // Auto-detect TypeScript by file extension
    let compiled = if path.ends_with(".ts") || ctx.entrypoint.language == "typescript" {
        compile_typescript(&source, path)?
    } else {
        source
    };

    Ok(compiled)
}

/// Convert serde_json::Value → V8 value via JSON.parse.
fn json_to_v8<'s>(
    scope: &mut v8::HandleScope<'s>,
    value: &serde_json::Value,
) -> Result<v8::Local<'s, v8::Value>, TaskError> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| TaskError::Internal(format!("json serialize: {e}")))?;
    let v8_str = v8::String::new(scope, &json_str)
        .ok_or_else(|| TaskError::Internal("failed to create V8 JSON string".into()))?;
    v8::json::parse(scope, v8_str.into())
        .ok_or_else(|| TaskError::Internal("V8 JSON.parse failed".into()))
}

/// Convert V8 value → serde_json::Value via JSON.stringify.
fn v8_to_json(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
) -> Result<serde_json::Value, TaskError> {
    if value.is_undefined() || value.is_null() {
        return Ok(serde_json::Value::Null);
    }
    let json_str = v8::json::stringify(scope, value)
        .ok_or_else(|| TaskError::Internal("V8 JSON.stringify failed".into()))?;
    let rust_str = json_str.to_rust_string_lossy(scope);
    serde_json::from_str(&rust_str)
        .map_err(|e| TaskError::Internal(format!("parse JSON result: {e}")))
}

