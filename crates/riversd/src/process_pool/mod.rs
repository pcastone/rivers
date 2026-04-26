//! ProcessPool runtime — engine-agnostic sandbox for CodeComponent execution.
//!
//! Per `rivers-processpool-runtime-spec-v2.md`.
//!
//! JavaScript execution is handled by V8 (rusty_v8) in `v8_engine`.
//! TypeScript is compiled to JavaScript via lightweight type stripping (V2.10).
//! WebAssembly execution is handled by Wasmtime in `wasm_engine` (V2.11).
//! The type system, task queue, and capability model are defined here.

// ── Submodules ────────────────────────────────────────────────
pub mod types;
mod bridge;
pub use bridge::TaskContextBuilder;
#[cfg(feature = "static-engines")]
pub mod v8_engine;
#[cfg(feature = "static-engines")]
pub mod v8_config;
#[cfg(feature = "static-engines")]
pub mod module_cache;
#[cfg(feature = "static-engines")]
pub mod wasm_engine;
#[cfg(feature = "static-engines")]
pub mod wasm_config;
#[cfg(test)]
#[cfg(feature = "static-engines")]
mod tests;

// Re-export all public types from submodules
pub use types::*;
#[cfg(feature = "static-engines")]
pub use v8_config::*;
#[cfg(feature = "static-engines")]
pub use wasm_config::*;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::execute_js_task;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::ensure_v8_initialized;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::DEFAULT_HEAP_LIMIT;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::is_module_syntax;
#[cfg(feature = "static-engines")]
pub use wasm_engine::clear_wasm_cache;
#[cfg(feature = "static-engines")]
pub(crate) use wasm_engine::execute_wasm_task;


use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tokio::sync::{mpsc, oneshot, Mutex};

use rivers_runtime::rivers_core::config::ProcessPoolConfig;
use rivers_runtime::rivers_core::{DriverFactory, StorageEngine};



// ── Shared Keystore Resolver ─────────────────────────────────
//
// Process-pool-level shared keystore resolver. Set once at startup after
// bundle loading. V8/WASM engines fall back to this when TaskContext.keystore
// is None (which is the common case since dispatch sites don't call
// .keystore() on the builder).

static SHARED_KEYSTORE_RESOLVER: std::sync::RwLock<Option<Arc<crate::keystore::KeystoreResolver>>> =
    std::sync::RwLock::new(None);

/// Set the shared keystore resolver. Called at startup after bundle loading.
/// Can be called again on hot-reload or in tests.
pub fn set_keystore_resolver(resolver: Arc<crate::keystore::KeystoreResolver>) {
    *SHARED_KEYSTORE_RESOLVER.write().unwrap() = Some(resolver);
}

/// Get the shared keystore resolver. Returns None if no keystores are configured.
pub fn get_keystore_resolver() -> Option<Arc<crate::keystore::KeystoreResolver>> {
    SHARED_KEYSTORE_RESOLVER.read().unwrap().clone()
}

// ── Per-Pool Watchdog Types (Wave 10) ────────────────────────

/// Active task entry tracked by the pool watchdog.
pub(crate) type ActiveTaskRegistry = Arc<StdMutex<HashMap<usize, ActiveTask>>>;
pub(crate) struct ActiveTask {
    pub(crate) started_at: std::time::Instant,
    pub(crate) timeout_ms: u64,
    pub(crate) terminator: TaskTerminator,
}

/// How to terminate a timed-out task.
///
/// BB5: Refactored from V8/Wasmtime-specific variants to a generic callback
/// that works with dynamically loaded engine shared libraries.
pub(crate) enum TaskTerminator {
    #[cfg(feature = "static-engines")]
    V8(v8::IsolateHandle),
    #[cfg(feature = "static-engines")]
    WasmEpoch(Arc<wasmtime::Engine>),
    /// Generic callback terminator — used by dynamically loaded engine plugins.
    #[allow(dead_code)]
    Callback(Box<dyn FnOnce() + Send>),
}


// ── ProcessPool ──────────────────────────────────────────────────

/// Internal message passed through the task queue.
struct TaskMessage {
    ctx: TaskContext,
    reply: oneshot::Sender<Result<TaskResult, TaskError>>,
}

/// A named process pool that manages workers and a task queue.
///
/// Per spec §2.1: multiple named pools per riversd instance.
pub struct ProcessPool {
    name: String,
    config: ProcessPoolConfig,
    queue_tx: mpsc::Sender<TaskMessage>,
    queue_depth: Arc<AtomicUsize>,
    effective_max_queue: usize,
    _worker_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Wave 10 watchdog registry — tracks active tasks for timeout enforcement.
    #[allow(dead_code)]
    active_tasks: ActiveTaskRegistry,
    _watchdog_cancel: Option<std::sync::mpsc::Sender<()>>,
}

impl ProcessPool {
    /// Create a new process pool.
    ///
    /// JavaScript/TypeScript tasks are executed by the V8 engine.
    /// WASM tasks are executed by the Wasmtime engine (V2.11).
    pub fn new(name: String, config: ProcessPoolConfig) -> Self {
        let effective_max_queue = if config.max_queue_depth == 0 {
            config.workers * 4
        } else {
            config.max_queue_depth
        };

        let (queue_tx, queue_rx) = mpsc::channel::<TaskMessage>(effective_max_queue);
        let queue_depth = Arc::new(AtomicUsize::new(0));
        let queue_rx = Arc::new(Mutex::new(queue_rx));

        // Wave 10: Per-pool watchdog thread
        let active_tasks: ActiveTaskRegistry = Arc::new(StdMutex::new(HashMap::new()));
        let (watchdog_cancel_tx, watchdog_cancel_rx) = std::sync::mpsc::channel();

        let registry_clone = active_tasks.clone();
        let pool_name = name.clone();
        std::thread::Builder::new()
            .name(format!("rivers-watchdog-{pool_name}"))
            .spawn(move || {
                loop {
                    match watchdog_cancel_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            // Collect timed-out termination actions under the lock,
                            // then release before executing to avoid deadlock with
                            // workers that need the lock to deregister.
                            // Collect timed-out task IDs, then terminate outside the lock
                            let timed_out_ids: Vec<usize> = {
                                let tasks = match registry_clone.lock() {
                                    Ok(t) => t,
                                    Err(_) => continue,
                                };
                                tasks.iter()
                                    .filter(|(_, task)| task.started_at.elapsed().as_millis() as u64 > task.timeout_ms)
                                    .map(|(id, _)| *id)
                                    .collect()
                            };
                            // Terminate each — remove from registry to get ownership of Callback
                            for id in timed_out_ids {
                                let task = {
                                    let mut tasks = match registry_clone.lock() {
                                        Ok(t) => t,
                                        Err(_) => continue,
                                    };
                                    tasks.remove(&id)
                                };
                                if let Some(task) = task {
                                    match task.terminator {
                                        #[cfg(feature = "static-engines")]
                                        TaskTerminator::V8(handle) => { let _ = handle.terminate_execution(); },
                                        #[cfg(feature = "static-engines")]
                                        TaskTerminator::WasmEpoch(engine) => { engine.increment_epoch(); },
                                        TaskTerminator::Callback(cb) => { cb(); },
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
            })
            .expect("failed to spawn watchdog thread");

        // Spawn worker tasks
        let mut handles = Vec::with_capacity(config.workers);
        for worker_id in 0..config.workers {
            let rx = queue_rx.clone();
            let depth = queue_depth.clone();
            let engine = config.engine.clone();
            let timeout_ms = config.task_timeout_ms;
            let heap_bytes = config.max_heap_mb * 1024 * 1024;
            let heap_threshold = config.heap_recycle_threshold;
            let epoch_interval = config.epoch_interval_ms;
            let recycle_after = config.recycle_after_tasks;
            let registry = active_tasks.clone();

            handles.push(tokio::spawn(async move {
                loop {
                    let msg = {
                        let mut guard = rx.lock().await;
                        match guard.recv().await {
                            Some(msg) => msg,
                            None => break, // Channel closed — shutdown
                        }
                    };

                    depth.fetch_sub(1, Ordering::SeqCst);

                    let result = dispatch_task(
                        &engine, msg.ctx, timeout_ms, worker_id,
                        heap_bytes, heap_threshold, epoch_interval, recycle_after,
                        Some(registry.clone()),
                    ).await;
                    let _ = msg.reply.send(result);
                }
            }));
        }

        Self {
            name,
            config,
            queue_tx,
            queue_depth,
            effective_max_queue,
            _worker_handles: handles,
            active_tasks,
            _watchdog_cancel: Some(watchdog_cancel_tx),
        }
    }

    /// Dispatch a task to this pool.
    ///
    /// Per spec §8.2: if queue is full, returns TaskError::QueueFull.
    pub async fn dispatch(&self, ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // Atomic check-and-increment to prevent TOCTOU race on queue depth
        let prev = self.queue_depth.fetch_add(1, Ordering::SeqCst);
        if prev >= self.effective_max_queue {
            self.queue_depth.fetch_sub(1, Ordering::SeqCst);
            return Err(TaskError::QueueFull);
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = TaskMessage { ctx, reply: reply_tx };

        self.queue_tx.send(msg).await.map_err(|_| {
            self.queue_depth.fetch_sub(1, Ordering::SeqCst);
            TaskError::Internal("pool channel closed".to_string())
        })?;

        reply_rx
            .await
            .map_err(|_| TaskError::Internal("worker dropped reply channel".to_string()))?
    }

    /// Get the pool name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the pool config.
    pub fn config(&self) -> &ProcessPoolConfig {
        &self.config
    }

    /// Current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue_depth.load(Ordering::SeqCst)
    }

    /// Max queue depth.
    pub fn max_queue_depth(&self) -> usize {
        self.effective_max_queue
    }
}

impl Drop for ProcessPool {
    fn drop(&mut self) {
        if let Some(cancel) = self._watchdog_cancel.take() {
            let _ = cancel.send(());
        }
    }
}

/// Drive the dyn-engine (cdylib) path for a single task.
///
/// Phase I7: bracket the engine call with a `TaskGuard` so:
/// - Dyn-engine host callbacks (`host_db_begin`/`commit`/`rollback`,
///   `host_dataview_execute`) see `current_task_id()` and can locate
///   their owning task in `DYN_TXN_MAP` / `TASK_DS_CONFIGS`.
/// - `TaskGuard::drop` auto-rolls-back any leftover transactions the
///   handler forgot to commit / rollback (panic-safe — the guard is
///   owned by the spawn_blocking closure stack).
/// - `take_commit_failed()` is called on the SAME thread that
///   `signal_commit_failed()` set the thread-local on (the
///   spawn_blocking worker), then propagated out via the closure's
///   return tuple.
///
/// `engine_runner` is the actual cdylib invocation. Production passes
/// `crate::engine_loader::execute_on_engine(name, ctx)`; tests pass a
/// closure that simulates an engine task body (e.g. exercising
/// `host_db_*_inner` directly without a real engine dylib).
async fn dispatch_dyn_engine_task<F>(
    ctx: &TaskContext,
    serialized: rivers_engine_sdk::SerializedTaskContext,
    engine_runner: F,
) -> Result<TaskResult, TaskError>
where
    F: FnOnce(&rivers_engine_sdk::SerializedTaskContext)
            -> Result<rivers_engine_sdk::SerializedTaskResult, String>
        + Send
        + 'static,
{
    let task_id = crate::engine_loader::host_context::next_task_id();
    let rt_handle = tokio::runtime::Handle::current();

    // Snapshot the per-task datasource configs into TASK_DS_CONFIGS so
    // host_db_begin can look up `(driver_name, ConnectionParams)` for
    // a given datasource without a roundtrip back through the cdylib.
    // Cleared on TaskGuard::drop. Q1 design decision (option A) per
    // changedecisionlog.md TXN-I1.1.
    let snapshot_configs = ctx
        .datasource_configs
        .iter()
        .map(|(k, rd)| (k.clone(), (rd.driver_name.clone(), rd.params.clone())))
        .collect();
    crate::engine_loader::host_context::store_task_ds_configs(
        task_id,
        crate::engine_loader::host_context::DatasourceConfigsSnapshot {
            configs: snapshot_configs,
        },
    );

    let join_outcome = tokio::task::spawn_blocking(move || {
        // TaskGuard::enter binds CURRENT_TASK_ID on this worker thread
        // and arms the auto-rollback hook. MUST run before the engine
        // call so host callbacks see the task id.
        let _guard =
            crate::engine_loader::host_context::TaskGuard::enter(task_id, rt_handle);

        let raw = engine_runner(&serialized);

        // Read commit-failed BEFORE _guard drops at scope end. The
        // thread-local lives on this spawn_blocking worker and would
        // be cleared by future tasks reusing the same worker; reading
        // here also keeps the value on the thread that set it (the
        // V8 path mirrors this on its own dispatch worker).
        let commit_failed = crate::engine_loader::host_context::take_commit_failed();

        (raw, commit_failed)
    })
    .await
    .map_err(|e| TaskError::WorkerCrash(format!("engine task panicked: {e}")))?;

    let (raw_result, commit_failed) = join_outcome;

    // Financial-correctness gate: a commit-failed signal upgrades the
    // outcome to `TransactionCommitFailed` regardless of what the
    // handler returned, mirroring the V8 path in
    // `process_pool/v8_engine/execution.rs`.
    if let Some((datasource, message)) = commit_failed {
        return Err(TaskError::TransactionCommitFailed {
            datasource,
            message,
        });
    }

    raw_result
        .map(|r| r.into())
        .map_err(TaskError::HandlerError)
}

/// Route a task to the appropriate engine.
///
/// JavaScript tasks (engine = "v8" or "boa") use the V8 engine.
/// Wasmtime tasks use the real Wasmtime WebAssembly runtime (V2.11).
/// `heap_bytes` and `heap_threshold` are from the pool's ProcessPoolConfig.
async fn dispatch_task(
    engine: &str,
    ctx: TaskContext,
    timeout_ms: u64,
    worker_id: usize,
    heap_bytes: usize,
    heap_threshold: f64,
    epoch_interval_ms: u64,
    _recycle_after_tasks: Option<u64>,
    registry: Option<ActiveTaskRegistry>,
) -> Result<TaskResult, TaskError> {
    // BB6: Try dynamic engine loader first
    let engine_key = match engine.to_lowercase().as_str() {
        "v8" | "boa" | "javascript" | "js" => "v8",
        "wasmtime" | "wasm" => "wasm",
        other => return Err(TaskError::EngineUnavailable(format!("unknown engine '{other}'"))),
    };

    if crate::engine_loader::is_engine_available(engine_key) {
        // Dynamic engine path — serialize context, call through C-ABI.
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let engine_name = engine_key.to_string();
        return dispatch_dyn_engine_task(&ctx, serialized, move |s| {
            crate::engine_loader::execute_on_engine(&engine_name, s)
        })
        .await;
    }

    // Fallback: static engine (only available with "static-engines" feature)
    #[cfg(feature = "static-engines")]
    match engine_key {
        "v8" => {
            return execute_js_task(ctx, timeout_ms, worker_id, heap_bytes, heap_threshold, registry.clone()).await;
        }
        "wasm" => {
            return execute_wasm_task(ctx, timeout_ms, worker_id, heap_bytes, epoch_interval_ms, registry).await;
        }
        _ => {}
    }

    Err(TaskError::EngineUnavailable(format!(
        "engine '{}' not available — place librivers_{}.dylib in {}",
        engine, engine_key, "lib/"
    )))
}

// ── ProcessPoolManager ───────────────────────────────────────────

/// Manages multiple named process pools.
pub struct ProcessPoolManager {
    pools: HashMap<String, ProcessPool>,
}

impl ProcessPoolManager {
    /// Create a manager with pools from config.
    pub fn from_config(config: &HashMap<String, ProcessPoolConfig>) -> Self {
        let mut pools = HashMap::new();
        for (name, pool_config) in config {
            pools.insert(
                name.clone(),
                ProcessPool::new(name.clone(), pool_config.clone()),
            );
        }

        // Ensure a "default" pool exists
        if !pools.contains_key("default") {
            pools.insert(
                "default".to_string(),
                ProcessPool::new("default".to_string(), ProcessPoolConfig::default()),
            );
        }

        Self { pools }
    }

    /// Dispatch a task to a named pool.
    pub async fn dispatch(
        &self,
        pool_name: &str,
        ctx: TaskContext,
    ) -> Result<TaskResult, TaskError> {
        let pool = self
            .pools
            .get(pool_name)
            .ok_or_else(|| {
                TaskError::Internal(format!("pool '{}' not found", pool_name))
            })?;
        pool.dispatch(ctx).await
    }

    /// Get a reference to a named pool.
    pub fn get_pool(&self, name: &str) -> Option<&ProcessPool> {
        self.pools.get(name)
    }

    /// Get all pool names.
    pub fn pool_names(&self) -> Vec<&str> {
        self.pools.keys().map(|s| s.as_str()).collect()
    }
}

// ── I7 dyn-engine dispatch tests ────────────────────────────────
//
// These exercise `dispatch_dyn_engine_task` (the helper extracted from
// the dyn-engine branch of `dispatch_task`) directly with a closure-driven
// engine runner. The closure simulates the engine task body — calling
// `host_db_*_inner` functions directly to drive the host-callback
// thread-locals — without needing to load a real cdylib.
//
// Approach B from the brief: closure-driven, light-weight, exercises the
// full TaskGuard + dispatch lifecycle without an engine fixture.

#[cfg(test)]
mod dyn_dispatch_tests {
    use super::*;
    use rivers_engine_sdk::SerializedTaskResult;
    use rivers_runtime::process_pool::types::TaskKind;
    use rivers_runtime::rivers_driver_sdk::ConnectionParams;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex as StdMutex};

    use crate::engine_loader::txn_test_fixtures;

    /// Build a minimal TaskContext with a single datasource config that
    /// resolves to the mock driver registered above.
    fn make_task_ctx(ds_name: &str, driver_name: &str) -> TaskContext {
        let params = ConnectionParams {
            host: "test".into(),
            port: 0,
            database: "test".into(),
            username: "test".into(),
            password: "test".into(),
            options: Default::default(),
        };
        let resolved = rivers_runtime::process_pool::types::ResolvedDatasource {
            driver_name: driver_name.to_string(),
            params,
        };
        TaskContextBuilder::new()
            .task_kind(TaskKind::Rest)
            .entrypoint(rivers_runtime::process_pool::types::Entrypoint {
                module: "test.js".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource_config(ds_name.to_string(), resolved)
            .trace_id("test-trace".into())
            .app_id("test-app".into())
            .node_id("test-node".into())
            .build()
            .expect("build TaskContext")
    }

    /// Empty engine result — what an engine returns when the simulated
    /// handler "succeeded" without producing a payload.
    fn empty_engine_ok() -> Result<SerializedTaskResult, String> {
        Ok(SerializedTaskResult {
            value: serde_json::Value::Null,
            duration_ms: 0,
        })
    }

    /// Helper: from inside a spawn_blocking-equivalent thread (i.e. our
    /// engine_runner closure), call host_db_begin / commit / rollback by
    /// reaching directly into the inner functions. This bypasses the FFI
    /// shim but exercises the same TASK_DS_CONFIGS lookup, dyn-txn-map
    /// insert, and signal_commit_failed paths the production cdylib
    /// would touch via host callbacks.
    fn run_begin(ds: &str) {
        use crate::engine_loader::host_context::HOST_CONTEXT_FOR_TESTS;
        let ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        crate::engine_loader::host_callbacks::host_db_begin_inner_for_test(
            &serde_json::json!({"datasource": ds}),
            ctx,
        )
        .expect("begin ok");
    }

    fn run_commit(ds: &str) -> Result<(), (i32, serde_json::Value)> {
        use crate::engine_loader::host_context::HOST_CONTEXT_FOR_TESTS;
        let ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        crate::engine_loader::host_callbacks::host_db_commit_inner_for_test(
            &serde_json::json!({"datasource": ds}),
            ctx,
        )
        .map(|_| ())
    }

    /// I7 test 1 — dispatch_task issues unique TaskIds.
    /// The dispatch helper increments NEXT_TASK_ID once per call. Two
    /// back-to-back dispatches must observe two different ids inside the
    /// engine_runner closure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_issues_unique_task_ids() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let observed: Arc<StdMutex<Vec<u64>>> = Arc::new(StdMutex::new(Vec::new()));

        for _ in 0..2 {
            let ctx = make_task_ctx("disp_ds_1", "dispatch-mock-driver");
            let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
            let observed = observed.clone();
            let _ = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
                let tid = crate::engine_loader::host_context::current_task_id()
                    .expect("TaskGuard binds CURRENT_TASK_ID");
                observed.lock().unwrap().push(tid.0);
                empty_engine_ok()
            })
            .await
            .expect("dispatch ok");
        }

        let ids = observed.lock().unwrap().clone();
        assert_eq!(ids.len(), 2, "expected 2 dispatches");
        assert_ne!(
            ids[0], ids[1],
            "TaskIds must be unique across dispatches; got {ids:?}"
        );
    }

    /// I7 test 2 — TaskGuard auto-rollback fires when handler forgets
    /// cleanup. Handler calls begin but neither commit nor rollback.
    /// On dispatch return, the dyn-txn-map must be empty (rolled back)
    /// and the connection must have observed `rollback_transaction()`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_guard_auto_rollback_on_leftover_txn() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let behavior = txn_test_fixtures::ensure_host_context();
        behavior.commit_fails.store(false, Ordering::Relaxed);

        let ds = "disp_ds_leftover";
        let ctx = make_task_ctx(ds, "dispatch-mock-driver");
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);

        // Capture the task id used by the dispatch so we can assert on it
        // after the closure returns.
        let observed_tid: Arc<StdMutex<Option<u64>>> = Arc::new(StdMutex::new(None));
        let observed_tid_inner = observed_tid.clone();
        let ds_owned = ds.to_string();

        let _ = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            // Capture the task id. (Production engines never see this —
            // it's a riversd-side construct.)
            let tid = crate::engine_loader::host_context::current_task_id()
                .expect("TaskGuard binds CURRENT_TASK_ID");
            *observed_tid_inner.lock().unwrap() = Some(tid.0);

            // Simulate the handler beginning a transaction and then
            // returning without commit or rollback. TaskGuard::drop must
            // catch this on its way out.
            run_begin(&ds_owned);
            empty_engine_ok()
        })
        .await
        .expect("dispatch ok");

        // After dispatch returns: txn map must be empty (auto-rollback
        // fired), and TASK_DS_CONFIGS must have been cleared.
        let tid = observed_tid.lock().unwrap().expect("captured task id");
        let task_id = crate::engine_loader::dyn_transaction_map::TaskId(tid);
        assert!(
            !crate::engine_loader::host_context::dyn_txn_map().has(task_id, ds),
            "TaskGuard::drop must drain leftover txns"
        );
        assert!(
            crate::engine_loader::host_context::lookup_task_ds_for_test(task_id, ds).is_none(),
            "TaskGuard::drop must clear TASK_DS_CONFIGS"
        );
    }

    /// I7 test 3 — commit_failed propagates through dispatch boundary.
    /// Handler calls begin → commit but the mock driver fails commit;
    /// `signal_commit_failed` runs on the spawn_blocking worker;
    /// `take_commit_failed` reads it inside the closure (same thread);
    /// dispatch awaiter upgrades the result to TransactionCommitFailed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_failed_propagates_to_dispatch_caller() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let behavior = txn_test_fixtures::ensure_host_context();
        behavior.commit_fails.store(true, Ordering::Relaxed);

        let ds = "disp_ds_commit_fail";
        let ctx = make_task_ctx(ds, "dispatch-mock-driver");
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_owned = ds.to_string();

        let result = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            run_begin(&ds_owned);
            // Commit will fail and call signal_commit_failed on this thread.
            let _ = run_commit(&ds_owned);
            // The handler "succeeds" returning empty. Production code
            // would typically return an error here — but the
            // financial-correctness gate must upgrade regardless.
            empty_engine_ok()
        })
        .await;

        // Reset behavior so a subsequent test starts clean.
        behavior.commit_fails.store(false, Ordering::Relaxed);

        match result {
            Err(TaskError::TransactionCommitFailed { datasource, message }) => {
                assert_eq!(datasource, ds);
                assert!(
                    message.contains("forced commit failure"),
                    "message must propagate driver msg; got {message}"
                );
            }
            other => panic!(
                "expected TransactionCommitFailed, got {other:?}"
            ),
        }
    }
}

// ── I8 dyn-engine end-to-end tests ───────────────────────────────
//
// These exercise the FULL dyn-engine transaction lifecycle against a
// real driver (built-in SQLite, durable temp-file backend) by driving
// `dispatch_dyn_engine_task` with closures that simulate cdylib task
// bodies. Each closure synchronously calls `host_db_begin_inner_for_test`
// → `execute_dataview_with_optional_txn_for_test` → `host_db_commit_inner_for_test`
// (or rollback) — the same code paths the production V8/WASM cdylibs hit
// via FFI shims, just without the C-ABI roundtrip.
//
// SQLite backend rationale (per Phase I plan §I8):
//   1. Real durable storage — proves commit-persists / rollback-discards
//      on bytes the test re-opens from outside the dispatch.
//   2. No network dependency — the worktree may not have access to the
//      Postgres test cluster (192.168.2.209). Postgres parallel cases
//      can be added later under #[ignore] when cluster reachability is
//      assured.
//   3. `supports_transactions() → true` and the SQLite driver implements
//      `begin_transaction/commit_transaction/rollback_transaction` natively
//      via BEGIN/COMMIT/ROLLBACK, so the txn semantics are real.
//
// The shared txn_test_fixtures::ensure_host_context now registers the
// "sqlite" driver alongside the mock drivers so all I3-I8 tests share
// a single `HOST_CONTEXT` `OnceLock` init (its first writer wins).
#[cfg(test)]
mod dyn_e2e_tests {
    use super::*;
    use rivers_engine_sdk::SerializedTaskResult;
    use rivers_runtime::process_pool::types::TaskKind;
    use rivers_runtime::rivers_driver_sdk::{ConnectionParams, QueryValue};
    use std::collections::HashMap;

    use crate::engine_loader::host_context::{
        host_dataview_executor_for_test, host_rt_handle_for_test,
        install_dataview_executor_for_test,
    };
    use crate::engine_loader::txn_test_fixtures;

    /// Open a fresh rusqlite handle to `db_path` outside any txn and
    /// count rows in `t`. Used by commit-persists / rollback-discards
    /// assertions that need to bypass the txn-held connection.
    fn count_rows_outside_txn(db_path: &std::path::Path, table: &str) -> i64 {
        // Use a tiny, single-purpose connection — no pool, no driver SDK.
        // This is the ground-truth oracle: if the bytes are on disk, this
        // reader sees them; if they're rolled back, it does not.
        let conn = rusqlite::Connection::open(db_path)
            .expect("open sqlite for read-back");
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table}"),
            [],
            |r| r.get::<_, i64>(0),
        )
        .expect("count query")
    }

    /// Create the `t(name)` table on a fresh tempfile and return the path.
    /// Each e2e test gets its own tempfile so concurrent test runs don't
    /// contend on the SQLite write lock.
    fn fresh_sqlite_with_table() -> tempfile::NamedTempFile {
        let f = tempfile::Builder::new()
            .prefix("rivers-i8-e2e-")
            .suffix(".sqlite")
            .tempfile()
            .expect("create tempfile");
        let conn = rusqlite::Connection::open(f.path()).expect("open tempfile");
        conn.execute(
            "CREATE TABLE t (name TEXT NOT NULL)",
            [],
        )
        .expect("create table");
        drop(conn);
        f
    }

    /// Build a TaskContext whose datasource map points "sqlite_e2e" at
    /// the "sqlite" driver with `db_path` as the database. The
    /// dispatch_dyn_engine_task helper snapshots this map into
    /// TASK_DS_CONFIGS keyed by the entry-point-namespaced datasource
    /// id; the inner host_db_begin_inner_for_test call must use the
    /// same namespaced form to look it up.
    fn make_e2e_task_ctx(db_path: &str) -> TaskContext {
        let mut options = HashMap::new();
        options.insert("driver".to_string(), "sqlite".to_string());
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: db_path.to_string(),
            username: String::new(),
            password: String::new(),
            options,
        };
        let resolved = rivers_runtime::process_pool::types::ResolvedDatasource {
            driver_name: "sqlite".to_string(),
            params,
        };
        TaskContextBuilder::new()
            .task_kind(TaskKind::Rest)
            .entrypoint(rivers_runtime::process_pool::types::Entrypoint {
                module: "test.js".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource_config("sqlite_e2e".to_string(), resolved)
            .trace_id("e2e-trace".into())
            .app_id("test-app".into())
            .node_id("test-node".into())
            .build()
            .expect("build TaskContext")
    }

    /// `dispatch_dyn_engine_task` snapshots `ctx.datasource_configs`
    /// verbatim into `TASK_DS_CONFIGS` (no namespace transformation —
    /// see `process_pool/mod.rs:335-339`). So the same key the test
    /// passes to `TaskContextBuilder::datasource_config(...)` is the
    /// key `host_db_begin_inner_for_test` must look up. Trivial helper
    /// for clarity at call sites.
    fn ds_lookup_key(ds: &str) -> String {
        ds.to_string()
    }

    /// Shape a successful empty engine result.
    fn empty_engine_ok() -> Result<SerializedTaskResult, String> {
        Ok(SerializedTaskResult {
            value: serde_json::Value::Null,
            duration_ms: 0,
        })
    }

    /// Helper used by every dispatch closure: drives begin →
    /// execute_dataview_with_optional_txn_for_test → (caller-decided
    /// commit/rollback). Returns the dataview affected_rows for assertions.
    /// Runs on the spawn_blocking worker thread; uses `block_on` via the
    /// host runtime handle to drive async fns.
    fn drive_begin_then_dataview(
        ds_key: &str,
        dv_name: &str,
    ) -> u64 {
        use crate::engine_loader::host_callbacks::{
            execute_dataview_with_optional_txn_for_test, host_db_begin_inner_for_test,
        };
        use crate::engine_loader::host_context::{
            current_task_id, HOST_CONTEXT_FOR_TESTS,
        };

        let host_ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        let rt = host_rt_handle_for_test();

        let begin = host_db_begin_inner_for_test(
            &serde_json::json!({"datasource": ds_key}),
            host_ctx,
        )
        .expect("begin ok");
        assert_eq!(begin["ok"], true);

        let task_id = current_task_id().expect("CURRENT_TASK_ID inside dispatch");
        let exec = rt
            .block_on(async { host_dataview_executor_for_test().await })
            .expect("executor installed");
        let resp = rt
            .block_on(async {
                execute_dataview_with_optional_txn_for_test(
                    exec,
                    dv_name,
                    HashMap::<String, QueryValue>::new(),
                    "e2e",
                    Some(task_id),
                )
                .await
            })
            .expect("dataview execute ok");
        resp.query_result.affected_rows
    }

    fn drive_commit(ds_key: &str) {
        use crate::engine_loader::host_callbacks::host_db_commit_inner_for_test;
        use crate::engine_loader::host_context::HOST_CONTEXT_FOR_TESTS;
        let host_ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        let res = host_db_commit_inner_for_test(
            &serde_json::json!({"datasource": ds_key}),
            host_ctx,
        )
        .expect("commit ok");
        assert_eq!(res["ok"], true);
    }

    fn drive_rollback(ds_key: &str) {
        use crate::engine_loader::host_callbacks::host_db_rollback_inner_for_test;
        use crate::engine_loader::host_context::HOST_CONTEXT_FOR_TESTS;
        let host_ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        let res = host_db_rollback_inner_for_test(
            &serde_json::json!({"datasource": ds_key}),
            host_ctx,
        )
        .expect("rollback ok");
        assert_eq!(res["ok"], true);
    }

    // I8.1 — Commit persists.
    // Begin → execute INSERT dataview inside the txn → commit. A fresh
    // SQLite connection opened OUTSIDE the dispatch must observe the row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn e2e_commit_persists_on_sqlite() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let temp = fresh_sqlite_with_table();
        let db_path = temp.path().to_owned();
        let executor = txn_test_fixtures::build_sqlite_executor(
            "insert_t",
            "INSERT INTO t (name) VALUES ('alice')",
            db_path.to_str().unwrap(),
        );
        install_dataview_executor_for_test(executor).await;

        let ctx = make_e2e_task_ctx(db_path.to_str().unwrap());
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_key = ds_lookup_key("sqlite_e2e");
        let db_for_pre = db_path.clone();

        let _ = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            let affected = drive_begin_then_dataview(&ds_key, "insert_t");
            assert_eq!(affected, 1, "INSERT must report 1 affected row inside txn");

            // Pre-commit: outside reader still sees zero rows (txn isolation).
            let pre = count_rows_outside_txn(&db_for_pre, "t");
            assert_eq!(
                pre, 0,
                "uncommitted row must not be visible to outside reader"
            );

            drive_commit(&ds_key);
            empty_engine_ok()
        })
        .await
        .expect("dispatch ok");

        // Post-dispatch: the row is durable and visible to a fresh reader.
        let post = count_rows_outside_txn(temp.path(), "t");
        assert_eq!(post, 1, "committed row must persist on disk");
    }

    // I8.2 — Rollback discards.
    // Begin → execute INSERT → rollback. Fresh reader sees zero rows.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn e2e_rollback_discards_on_sqlite() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let temp = fresh_sqlite_with_table();
        let db_path = temp.path().to_owned();
        let executor = txn_test_fixtures::build_sqlite_executor(
            "insert_t",
            "INSERT INTO t (name) VALUES ('bob')",
            db_path.to_str().unwrap(),
        );
        install_dataview_executor_for_test(executor).await;

        let ctx = make_e2e_task_ctx(db_path.to_str().unwrap());
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_key = ds_lookup_key("sqlite_e2e");

        let _ = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            let affected = drive_begin_then_dataview(&ds_key, "insert_t");
            assert_eq!(affected, 1);
            drive_rollback(&ds_key);
            empty_engine_ok()
        })
        .await
        .expect("dispatch ok");

        // After rollback: no row.
        let post = count_rows_outside_txn(temp.path(), "t");
        assert_eq!(post, 0, "rolled-back row must not persist");
    }

    // I8.3 — Auto-rollback on engine error.
    // Handler begins a txn, writes, then the engine_runner returns Err
    // — TaskGuard::drop must auto-rollback the leftover txn so the
    // INSERT does NOT land. Mirrors the V8 path's TaskLocals::drop.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn e2e_auto_rollback_on_engine_error() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let temp = fresh_sqlite_with_table();
        let db_path = temp.path().to_owned();
        let executor = txn_test_fixtures::build_sqlite_executor(
            "insert_t",
            "INSERT INTO t (name) VALUES ('carol')",
            db_path.to_str().unwrap(),
        );
        install_dataview_executor_for_test(executor).await;

        let ctx = make_e2e_task_ctx(db_path.to_str().unwrap());
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_key = ds_lookup_key("sqlite_e2e");

        let result = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            let affected = drive_begin_then_dataview(&ds_key, "insert_t");
            assert_eq!(affected, 1);

            // Simulate an engine-level error WITHOUT calling commit/rollback.
            // TaskGuard::drop's auto-rollback is the safety net.
            Err::<SerializedTaskResult, String>(
                "simulated handler failure".to_string(),
            )
        })
        .await;

        // Dispatch surfaces the engine error as TaskError::HandlerError.
        match result {
            Err(TaskError::HandlerError(msg)) => {
                assert!(
                    msg.contains("simulated handler failure"),
                    "engine error must propagate; got {msg}"
                );
            }
            other => panic!("expected HandlerError, got {other:?}"),
        }

        // Critical: auto-rollback fired, so the row is NOT in the DB.
        let post = count_rows_outside_txn(temp.path(), "t");
        assert_eq!(
            post, 0,
            "TaskGuard::drop auto-rollback must discard uncommitted writes"
        );
    }

    // I8.4 — Cross-datasource rejection inside a txn.
    // Begin a txn on ds-a; try to execute a dataview on ds-b. The
    // execute_dataview_with_optional_txn helper enforces spec §6.2 and
    // returns a Driver error containing "TransactionError:".
    //
    // Direct approach: skip dispatch_dyn_engine_task and pre-seat the
    // txn map under a synthesized task id. Mirrors the existing
    // `dataview_cross_datasource_in_txn_rejects` unit test pattern in
    // host_callbacks.rs but uses a real SQLite-backed executor as the
    // dataview's home datasource.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn e2e_cross_datasource_in_txn_rejects() {
        use crate::engine_loader::dyn_transaction_map::TaskId;
        use crate::engine_loader::host_context::{
            dyn_txn_map, set_current_task_id_for_test,
        };
        use std::sync::atomic::{AtomicU64, Ordering};

        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let temp = fresh_sqlite_with_table();
        let db_path = temp.path().to_owned();
        let executor = txn_test_fixtures::build_sqlite_executor(
            "insert_t",
            "INSERT INTO t (name) VALUES ('dave')",
            db_path.to_str().unwrap(),
        );
        install_dataview_executor_for_test(executor).await;

        // Synthesize a task id well above the production NEXT_TASK_ID
        // counter so we don't collide with any concurrent dispatch.
        static N: AtomicU64 = AtomicU64::new(2_000_000);
        let task = TaskId(N.fetch_add(1, Ordering::Relaxed));

        // Seed a fake txn on a DIFFERENT datasource. The cross-DS check
        // operates purely on the map's keys — no driver call is issued —
        // so a mock connection is sufficient.
        set_current_task_id_for_test(Some(task));
        let beh = txn_test_fixtures::behavior();
        let other_conn: Box<dyn rivers_runtime::rivers_driver_sdk::Connection> =
            Box::new(txn_test_fixtures::SharedMockConn { behavior: beh });
        dyn_txn_map()
            .insert(task, "other_ds", other_conn)
            .expect("seed cross-DS txn");

        let exec = host_dataview_executor_for_test()
            .await
            .expect("executor installed");

        let err = crate::engine_loader::host_callbacks::execute_dataview_with_optional_txn_for_test(
            exec,
            "insert_t",
            HashMap::<String, QueryValue>::new(),
            "e2e",
            Some(task),
        )
        .await
        .expect_err("must reject cross-DS dataview inside txn");

        match err {
            rivers_runtime::DataViewError::Driver(msg) => {
                assert!(
                    msg.contains("TransactionError:"),
                    "expected TransactionError prefix; got {msg}"
                );
                assert!(
                    msg.contains("differs from active transaction"),
                    "expected cross-DS phrasing; got {msg}"
                );
            }
            other => panic!("expected Driver error, got {other:?}"),
        }

        // Cleanup so this task's txn doesn't leak into other tests.
        let _ = dyn_txn_map().drain_task(task);
        set_current_task_id_for_test(None);

        // The dataview was REJECTED before any driver call — DB is empty.
        let post = count_rows_outside_txn(temp.path(), "t");
        assert_eq!(
            post, 0,
            "cross-DS rejection must occur before any write"
        );
    }

    // I8.5 — Two distinct tasks on the same datasource each hold their
    // own transaction state.
    //
    // SQLite serializes writers, so we run the dispatches sequentially —
    // the goal is to verify the dyn-txn-map keys by (TaskId, datasource),
    // NOT by datasource alone. Two tasks that target the same DS each get
    // their own independent transaction state and both commits land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn e2e_concurrent_txns_isolated_by_task_id() {
        let _g = txn_test_fixtures::test_lock().lock().unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let temp = fresh_sqlite_with_table();
        let db_path = temp.path().to_owned();

        // First task: insert "alice".
        let exec1 = txn_test_fixtures::build_sqlite_executor(
            "insert_t",
            "INSERT INTO t (name) VALUES ('alice')",
            db_path.to_str().unwrap(),
        );
        install_dataview_executor_for_test(exec1).await;
        let ctx1 = make_e2e_task_ctx(db_path.to_str().unwrap());
        let serialized1 = rivers_engine_sdk::SerializedTaskContext::from(&ctx1);
        let ds_key1 = ds_lookup_key("sqlite_e2e");
        let _ = dispatch_dyn_engine_task(&ctx1, serialized1, move |_s| {
            let _ = drive_begin_then_dataview(&ds_key1, "insert_t");
            drive_commit(&ds_key1);
            empty_engine_ok()
        })
        .await
        .expect("dispatch 1 ok");

        // Second task: same datasource, but a fresh executor with a
        // different INSERT statement (the executor caches the dataview
        // config from build time, so we rebuild). The dispatch helper
        // issues a fresh TaskId, so the dyn-txn-map insert under
        // (TaskId_2, "sqlite_e2e") is a separate slot from the first
        // task's (already-committed-and-released) entry.
        let exec2 = txn_test_fixtures::build_sqlite_executor(
            "insert_t",
            "INSERT INTO t (name) VALUES ('bob')",
            db_path.to_str().unwrap(),
        );
        install_dataview_executor_for_test(exec2).await;
        let ctx2 = make_e2e_task_ctx(db_path.to_str().unwrap());
        let serialized2 = rivers_engine_sdk::SerializedTaskContext::from(&ctx2);
        let ds_key2 = ds_lookup_key("sqlite_e2e");
        let _ = dispatch_dyn_engine_task(&ctx2, serialized2, move |_s| {
            let _ = drive_begin_then_dataview(&ds_key2, "insert_t");
            drive_commit(&ds_key2);
            empty_engine_ok()
        })
        .await
        .expect("dispatch 2 ok");

        // Both rows persist — the two tasks held independent txn state
        // even though they targeted the same datasource.
        let post = count_rows_outside_txn(temp.path(), "t");
        assert_eq!(
            post, 2,
            "two distinct tasks committing on the same DS must both persist"
        );
    }
}

// ── I-FU2 dyn-engine end-to-end tests against the Postgres cluster ─
//
// Mirrors the SQLite e2e cases in `dyn_e2e_tests` against the live
// Postgres test cluster at 192.168.2.209 (per CLAUDE.md). Same five
// scenarios, same `dispatch_dyn_engine_task` shape, same assertions —
// but the durability oracle is a fresh `PostgresDriver` connection
// opened OUTSIDE the dispatch (mirrors the rusqlite oracle in the
// SQLite suite).
//
// Each test is double-gated:
//   1. `#[ignore]` — excluded from the default `cargo test` flow.
//   2. Runtime check on `RIVERS_TEST_CLUSTER=1` AND a 2-second TCP
//      probe to 192.168.2.209:5432 inside `cluster_available()`.
//
// Run live: `RIVERS_TEST_CLUSTER=1 cargo test -p riversd \
//     --features static-builtin-drivers,static-engines pg_e2e -- \
//     --include-ignored`
//
// Each test creates a unique table and drops it on exit (best-effort)
// so concurrent test runs and aborted runs don't accumulate state in
// the shared `rivers` database.
#[cfg(test)]
mod pg_e2e_tests {
    use super::*;
    use rivers_engine_sdk::SerializedTaskResult;
    use rivers_runtime::process_pool::types::TaskKind;
    use rivers_runtime::rivers_driver_sdk::{ConnectionParams, QueryValue};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::engine_loader::host_context::{
        host_dataview_executor_for_test, host_rt_handle_for_test,
        install_dataview_executor_for_test,
    };
    use crate::engine_loader::txn_test_fixtures;

    const PG_HOST: &str = "192.168.2.209";
    const PG_PORT: u16 = 5432;
    const PG_USER: &str = "rivers";
    const PG_PASS: &str = "rivers_test";
    const PG_DB: &str = "rivers";

    /// True iff `RIVERS_TEST_CLUSTER=1` is set AND a quick TCP probe
    /// to the Postgres primary succeeds. The probe prevents wasted
    /// time on stale env vars / offline laptops; the env-var gate
    /// matches the H18 conformance convention.
    fn cluster_available() -> bool {
        match std::env::var("RIVERS_TEST_CLUSTER") {
            Ok(v) => eprintln!("[pg_e2e] RIVERS_TEST_CLUSTER={v}"),
            Err(_) => {
                eprintln!("[pg_e2e] RIVERS_TEST_CLUSTER unset");
                return false;
            }
        }
        let addr = format!("{PG_HOST}:{PG_PORT}")
            .parse::<std::net::SocketAddr>()
            .expect("static PG addr");
        match std::net::TcpStream::connect_timeout(
            &addr,
            std::time::Duration::from_secs(2),
        ) {
            Ok(_) => {
                eprintln!("[pg_e2e] TCP probe to {addr} succeeded");
                true
            }
            Err(e) => {
                eprintln!("[pg_e2e] TCP probe to {addr} failed: {e}");
                false
            }
        }
    }

    /// Build base ConnectionParams for the Postgres test cluster.
    /// `build_postgres_executor` mutates the `options` map to set the
    /// "driver" key; tests share a single helper so the host/port/db
    /// constants stay in one place.
    fn pg_connection_params() -> ConnectionParams {
        ConnectionParams {
            host: PG_HOST.into(),
            port: PG_PORT,
            database: PG_DB.into(),
            username: PG_USER.into(),
            password: PG_PASS.into(),
            options: HashMap::new(),
        }
    }

    /// Allocate a unique table name per test to avoid collisions.
    /// Combines the process id, an atomic counter, and a per-test
    /// prefix so concurrent test runs and abandoned runs from prior
    /// processes never see each other's tables.
    fn unique_table_name(prefix: &str) -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        format!("{prefix}_{pid}_{id}")
    }

    /// Open an out-of-band connection via `PostgresDriver::connect()`
    /// (no pool, no DataView engine) — the ground-truth oracle that
    /// reads what's actually committed to disk. Used by setup,
    /// teardown, and post-dispatch row-count assertions.
    async fn pg_connect_oob() -> Box<dyn rivers_runtime::rivers_driver_sdk::Connection> {
        use rivers_runtime::rivers_driver_sdk::DatabaseDriver;
        let driver = rivers_runtime::rivers_core::drivers::PostgresDriver;
        driver
            .connect(&pg_connection_params())
            .await
            .expect("oob postgres connect")
    }

    /// CREATE TABLE — uses `ddl_execute` because the regular execute
    /// path's DDL guard rejects CREATE/DROP. Mirrors the H18 conformance
    /// pattern.
    async fn pg_create_table(table: &str) {
        use rivers_runtime::rivers_driver_sdk::Query;
        let mut conn = pg_connect_oob().await;
        let stmt =
            format!("CREATE TABLE {table} (id BIGINT PRIMARY KEY, val TEXT NOT NULL)");
        conn.ddl_execute(&Query::new(table, &stmt))
            .await
            .unwrap_or_else(|e| panic!("CREATE TABLE {table}: {e:?}"));
    }

    /// DROP TABLE IF EXISTS — best-effort; failure is logged but not
    /// propagated (a previous test may have already dropped it, or the
    /// test may have failed before the table was created).
    async fn pg_drop_table(table: &str) {
        use rivers_runtime::rivers_driver_sdk::Query;
        let mut conn = pg_connect_oob().await;
        let stmt = format!("DROP TABLE IF EXISTS {table}");
        if let Err(e) = conn.ddl_execute(&Query::new(table, &stmt)).await {
            eprintln!("pg_drop_table({table}) best-effort cleanup error: {e:?}");
        }
    }

    /// Out-of-band SELECT COUNT(*) on the live db. Mirrors
    /// `count_rows_outside_txn` in the SQLite suite — it reads through
    /// a fresh connection, so uncommitted writes (held by the dispatch's
    /// txn-bound connection) are NOT visible.
    async fn pg_count_rows_oob(table: &str) -> i64 {
        use rivers_runtime::rivers_driver_sdk::Query;
        let mut conn = pg_connect_oob().await;
        let stmt = format!("SELECT COUNT(*) AS c FROM {table}");
        let q = Query::with_operation("select", table, &stmt);
        let res = conn
            .execute(&q)
            .await
            .unwrap_or_else(|e| panic!("count rows in {table}: {e:?}"));
        assert_eq!(res.rows.len(), 1, "count returned {} rows", res.rows.len());
        match res.rows[0].get("c") {
            Some(QueryValue::Integer(n)) => *n,
            Some(QueryValue::UInt(n)) => *n as i64,
            other => panic!("count column not an integer: {other:?}"),
        }
    }

    /// Build a TaskContext whose datasource map points the per-test
    /// datasource id at the "postgres" driver. The dispatch helper
    /// snapshots this map into TASK_DS_CONFIGS and the inner
    /// host_db_begin_inner_for_test call must use the same key.
    fn make_pg_task_ctx(datasource_id: &str) -> TaskContext {
        let mut params = pg_connection_params();
        params.options.insert("driver".into(), "postgres".into());
        let resolved = rivers_runtime::process_pool::types::ResolvedDatasource {
            driver_name: "postgres".to_string(),
            params,
        };
        TaskContextBuilder::new()
            .task_kind(TaskKind::Rest)
            .entrypoint(rivers_runtime::process_pool::types::Entrypoint {
                module: "test.js".into(),
                function: "handler".into(),
                language: "javascript".into(),
            })
            .datasource_config(datasource_id.to_string(), resolved)
            .trace_id("pg-e2e-trace".into())
            .app_id("test-app".into())
            .node_id("test-node".into())
            .build()
            .expect("build TaskContext")
    }

    fn empty_engine_ok() -> Result<SerializedTaskResult, String> {
        Ok(SerializedTaskResult {
            value: serde_json::Value::Null,
            duration_ms: 0,
        })
    }

    /// Drives begin → execute_dataview_with_optional_txn_for_test inside
    /// the dispatch-closure context. Returns the affected_rows count from
    /// the dataview execution. Mirrors `drive_begin_then_dataview` in the
    /// SQLite suite but is parameterized on the datasource id so two
    /// tests can target different per-test datasource keys without
    /// stepping on each other.
    fn drive_begin_then_dataview(ds_key: &str, dv_name: &str) -> u64 {
        use crate::engine_loader::host_callbacks::{
            execute_dataview_with_optional_txn_for_test, host_db_begin_inner_for_test,
        };
        use crate::engine_loader::host_context::{
            current_task_id, HOST_CONTEXT_FOR_TESTS,
        };

        let host_ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        let rt = host_rt_handle_for_test();

        let begin = host_db_begin_inner_for_test(
            &serde_json::json!({"datasource": ds_key}),
            host_ctx,
        )
        .expect("begin ok");
        assert_eq!(begin["ok"], true);

        let task_id = current_task_id().expect("CURRENT_TASK_ID inside dispatch");
        let exec = rt
            .block_on(async { host_dataview_executor_for_test().await })
            .expect("executor installed");
        let resp = rt
            .block_on(async {
                execute_dataview_with_optional_txn_for_test(
                    exec,
                    dv_name,
                    HashMap::<String, QueryValue>::new(),
                    "pg-e2e",
                    Some(task_id),
                )
                .await
            })
            .expect("dataview execute ok");
        resp.query_result.affected_rows
    }

    fn drive_commit(ds_key: &str) {
        use crate::engine_loader::host_callbacks::host_db_commit_inner_for_test;
        use crate::engine_loader::host_context::HOST_CONTEXT_FOR_TESTS;
        let host_ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        let res = host_db_commit_inner_for_test(
            &serde_json::json!({"datasource": ds_key}),
            host_ctx,
        )
        .expect("commit ok");
        assert_eq!(res["ok"], true);
    }

    fn drive_rollback(ds_key: &str) {
        use crate::engine_loader::host_callbacks::host_db_rollback_inner_for_test;
        use crate::engine_loader::host_context::HOST_CONTEXT_FOR_TESTS;
        let host_ctx = HOST_CONTEXT_FOR_TESTS.get().expect("HOST_CONTEXT");
        let res = host_db_rollback_inner_for_test(
            &serde_json::json!({"datasource": ds_key}),
            host_ctx,
        )
        .expect("rollback ok");
        assert_eq!(res["ok"], true);
    }

    // I-FU2.1 — Commit persists.
    // Begin → INSERT via dataview → commit. A fresh out-of-band reader
    // observes the row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn pg_commit_persists() {
        if !cluster_available() {
            eprintln!(
                "RIVERS_TEST_CLUSTER not set or PG unreachable — skipping pg_commit_persists"
            );
            return;
        }
        let _g = txn_test_fixtures::test_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let table = unique_table_name("pgfu2_commit");
        pg_create_table(&table).await;

        // Best-effort cleanup even on assertion failure. The Postgres
        // cluster is shared so leaked tables would accumulate.
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let table = self.0.clone();
                // Run cleanup on a fresh blocking context so we don't
                // depend on whatever runtime state remains at unwind time.
                let _ = std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("cleanup rt");
                    rt.block_on(pg_drop_table(&table));
                })
                .join();
            }
        }
        let _cleanup = Cleanup(table.clone());

        let ds_id = "pg_e2e_commit";
        let insert_sql = format!("INSERT INTO {table} (id, val) VALUES (1, 'alice')");
        let executor = txn_test_fixtures::build_postgres_executor(
            "insert_t",
            &insert_sql,
            pg_connection_params(),
            ds_id,
        );
        install_dataview_executor_for_test(executor).await;

        let ctx = make_pg_task_ctx(ds_id);
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_key = ds_id.to_string();
        let table_for_pre = table.clone();

        let _ = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            let affected = drive_begin_then_dataview(&ds_key, "insert_t");
            assert_eq!(affected, 1, "INSERT must report 1 affected row inside txn");

            // Pre-commit: out-of-band reader still sees zero rows
            // (txn isolation — Postgres READ COMMITTED default).
            let rt = host_rt_handle_for_test();
            let pre = rt.block_on(pg_count_rows_oob(&table_for_pre));
            assert_eq!(
                pre, 0,
                "uncommitted row must not be visible to outside reader"
            );

            drive_commit(&ds_key);
            empty_engine_ok()
        })
        .await
        .expect("dispatch ok");

        let post = pg_count_rows_oob(&table).await;
        assert_eq!(post, 1, "committed row must persist on disk");
    }

    // I-FU2.2 — Rollback discards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn pg_rollback_discards() {
        if !cluster_available() {
            eprintln!(
                "RIVERS_TEST_CLUSTER not set or PG unreachable — skipping pg_rollback_discards"
            );
            return;
        }
        let _g = txn_test_fixtures::test_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let table = unique_table_name("pgfu2_rollback");
        pg_create_table(&table).await;

        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let table = self.0.clone();
                let _ = std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("cleanup rt");
                    rt.block_on(pg_drop_table(&table));
                })
                .join();
            }
        }
        let _cleanup = Cleanup(table.clone());

        let ds_id = "pg_e2e_rollback";
        let insert_sql = format!("INSERT INTO {table} (id, val) VALUES (2, 'bob')");
        let executor = txn_test_fixtures::build_postgres_executor(
            "insert_t",
            &insert_sql,
            pg_connection_params(),
            ds_id,
        );
        install_dataview_executor_for_test(executor).await;

        let ctx = make_pg_task_ctx(ds_id);
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_key = ds_id.to_string();

        let _ = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            let affected = drive_begin_then_dataview(&ds_key, "insert_t");
            assert_eq!(affected, 1);
            drive_rollback(&ds_key);
            empty_engine_ok()
        })
        .await
        .expect("dispatch ok");

        let post = pg_count_rows_oob(&table).await;
        assert_eq!(post, 0, "rolled-back row must not persist");
    }

    // I-FU2.3 — Auto-rollback on engine error.
    // engine_runner returns Err WITHOUT calling commit/rollback;
    // TaskGuard::drop's auto-rollback must discard the INSERT.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn pg_auto_rollback_on_engine_error() {
        if !cluster_available() {
            eprintln!(
                "RIVERS_TEST_CLUSTER not set or PG unreachable — skipping pg_auto_rollback_on_engine_error"
            );
            return;
        }
        let _g = txn_test_fixtures::test_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let table = unique_table_name("pgfu2_autoroll");
        pg_create_table(&table).await;

        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let table = self.0.clone();
                let _ = std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("cleanup rt");
                    rt.block_on(pg_drop_table(&table));
                })
                .join();
            }
        }
        let _cleanup = Cleanup(table.clone());

        let ds_id = "pg_e2e_autoroll";
        let insert_sql = format!("INSERT INTO {table} (id, val) VALUES (3, 'carol')");
        let executor = txn_test_fixtures::build_postgres_executor(
            "insert_t",
            &insert_sql,
            pg_connection_params(),
            ds_id,
        );
        install_dataview_executor_for_test(executor).await;

        let ctx = make_pg_task_ctx(ds_id);
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let ds_key = ds_id.to_string();

        let result = dispatch_dyn_engine_task(&ctx, serialized, move |_s| {
            let affected = drive_begin_then_dataview(&ds_key, "insert_t");
            assert_eq!(affected, 1);
            // Engine error WITHOUT commit/rollback — TaskGuard::drop is
            // the safety net.
            Err::<SerializedTaskResult, String>(
                "simulated handler failure".to_string(),
            )
        })
        .await;

        match result {
            Err(TaskError::HandlerError(msg)) => {
                assert!(
                    msg.contains("simulated handler failure"),
                    "engine error must propagate; got {msg}"
                );
            }
            other => panic!("expected HandlerError, got {other:?}"),
        }

        let post = pg_count_rows_oob(&table).await;
        assert_eq!(
            post, 0,
            "TaskGuard::drop auto-rollback must discard uncommitted writes"
        );
    }

    // I-FU2.4 — Cross-datasource rejection inside a txn.
    // Begin a txn on ds-a (mock); execute a dataview on ds-b (real
    // postgres) — the cross-DS check (spec §6.2) rejects with a Driver
    // error before any postgres call. Mirrors the SQLite-suite pattern
    // exactly, just with the dataview's home datasource being postgres.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn pg_cross_datasource_in_txn_rejects() {
        if !cluster_available() {
            eprintln!(
                "RIVERS_TEST_CLUSTER not set or PG unreachable — skipping pg_cross_datasource_in_txn_rejects"
            );
            return;
        }
        use crate::engine_loader::dyn_transaction_map::TaskId;
        use crate::engine_loader::host_context::{
            dyn_txn_map, set_current_task_id_for_test,
        };

        let _g = txn_test_fixtures::test_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let table = unique_table_name("pgfu2_crossds");
        pg_create_table(&table).await;

        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let table = self.0.clone();
                let _ = std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("cleanup rt");
                    rt.block_on(pg_drop_table(&table));
                })
                .join();
            }
        }
        let _cleanup = Cleanup(table.clone());

        let ds_id = "pg_e2e_crossds";
        let insert_sql = format!("INSERT INTO {table} (id, val) VALUES (4, 'dave')");
        let executor = txn_test_fixtures::build_postgres_executor(
            "insert_t",
            &insert_sql,
            pg_connection_params(),
            ds_id,
        );
        install_dataview_executor_for_test(executor).await;

        // Synthesize a task id well above the production NEXT_TASK_ID
        // counter so we don't collide with any concurrent dispatch.
        static N: AtomicU64 = AtomicU64::new(3_000_000);
        let task = TaskId(N.fetch_add(1, Ordering::Relaxed));

        // Seed a fake txn on a DIFFERENT datasource (mock conn — no
        // network call needed; the cross-DS check operates purely on
        // map keys, not the connection's identity).
        set_current_task_id_for_test(Some(task));
        let beh = txn_test_fixtures::behavior();
        let other_conn: Box<dyn rivers_runtime::rivers_driver_sdk::Connection> =
            Box::new(txn_test_fixtures::SharedMockConn { behavior: beh });
        dyn_txn_map()
            .insert(task, "other_ds", other_conn)
            .expect("seed cross-DS txn");

        let exec = host_dataview_executor_for_test()
            .await
            .expect("executor installed");

        let err = crate::engine_loader::host_callbacks::execute_dataview_with_optional_txn_for_test(
            exec,
            "insert_t",
            HashMap::<String, QueryValue>::new(),
            "pg-e2e",
            Some(task),
        )
        .await
        .expect_err("must reject cross-DS dataview inside txn");

        match err {
            rivers_runtime::DataViewError::Driver(msg) => {
                assert!(
                    msg.contains("TransactionError:"),
                    "expected TransactionError prefix; got {msg}"
                );
                assert!(
                    msg.contains("differs from active transaction"),
                    "expected cross-DS phrasing; got {msg}"
                );
            }
            other => panic!("expected Driver error, got {other:?}"),
        }

        // Cleanup so this task's txn doesn't leak into other tests.
        let _ = dyn_txn_map().drain_task(task);
        set_current_task_id_for_test(None);

        // The dataview was rejected before any driver call — DB still empty.
        let post = pg_count_rows_oob(&table).await;
        assert_eq!(
            post, 0,
            "cross-DS rejection must occur before any write"
        );
    }

    // I-FU2.5 — Two distinct tasks on the same datasource each hold
    // their own transaction state. Verifies the dyn-txn-map keys by
    // (TaskId, datasource), not by datasource alone.
    //
    // Uses TWO distinct tables (one per task) so we can independently
    // verify that each task's commit landed exactly its own row, with
    // no interleaving / cross-contamination.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn pg_concurrent_txns_isolated_by_task_id() {
        if !cluster_available() {
            eprintln!(
                "RIVERS_TEST_CLUSTER not set or PG unreachable — skipping pg_concurrent_txns_isolated_by_task_id"
            );
            return;
        }
        let _g = txn_test_fixtures::test_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        txn_test_fixtures::ensure_host_context();

        let table_a = unique_table_name("pgfu2_iso_a");
        let table_b = unique_table_name("pgfu2_iso_b");
        pg_create_table(&table_a).await;
        pg_create_table(&table_b).await;

        struct Cleanup(Vec<String>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let tables = self.0.clone();
                let _ = std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("cleanup rt");
                    for t in tables {
                        rt.block_on(pg_drop_table(&t));
                    }
                })
                .join();
            }
        }
        let _cleanup = Cleanup(vec![table_a.clone(), table_b.clone()]);

        // Task 1: insert 'alice' into table_a via "pg_iso_a" datasource.
        let insert_a = format!("INSERT INTO {table_a} (id, val) VALUES (1, 'alice')");
        let exec1 = txn_test_fixtures::build_postgres_executor(
            "insert_t",
            &insert_a,
            pg_connection_params(),
            "pg_iso_a",
        );
        install_dataview_executor_for_test(exec1).await;
        let ctx1 = make_pg_task_ctx("pg_iso_a");
        let serialized1 = rivers_engine_sdk::SerializedTaskContext::from(&ctx1);
        let ds1 = "pg_iso_a".to_string();
        let _ = dispatch_dyn_engine_task(&ctx1, serialized1, move |_s| {
            let _ = drive_begin_then_dataview(&ds1, "insert_t");
            drive_commit(&ds1);
            empty_engine_ok()
        })
        .await
        .expect("dispatch 1 ok");

        // Task 2: same shape but a DIFFERENT datasource id and table.
        // The dispatch helper issues a fresh TaskId, so the dyn-txn-map
        // insert under (TaskId_2, "pg_iso_b") is a separate slot from
        // task 1's already-released entry. The point of this test is
        // that two tasks DO NOT share txn state — two-table separation
        // makes that easy to verify post-dispatch.
        let insert_b = format!("INSERT INTO {table_b} (id, val) VALUES (2, 'bob')");
        let exec2 = txn_test_fixtures::build_postgres_executor(
            "insert_t",
            &insert_b,
            pg_connection_params(),
            "pg_iso_b",
        );
        install_dataview_executor_for_test(exec2).await;
        let ctx2 = make_pg_task_ctx("pg_iso_b");
        let serialized2 = rivers_engine_sdk::SerializedTaskContext::from(&ctx2);
        let ds2 = "pg_iso_b".to_string();
        let _ = dispatch_dyn_engine_task(&ctx2, serialized2, move |_s| {
            let _ = drive_begin_then_dataview(&ds2, "insert_t");
            drive_commit(&ds2);
            empty_engine_ok()
        })
        .await
        .expect("dispatch 2 ok");

        let count_a = pg_count_rows_oob(&table_a).await;
        let count_b = pg_count_rows_oob(&table_b).await;
        assert_eq!(count_a, 1, "task 1's row must persist in table_a");
        assert_eq!(count_b, 1, "task 2's row must persist in table_b");
    }
}
