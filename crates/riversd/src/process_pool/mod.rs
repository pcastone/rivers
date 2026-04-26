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


