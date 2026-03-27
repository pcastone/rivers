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
pub mod wasm_engine;
#[cfg(feature = "static-engines")]
pub mod wasm_config;
#[cfg(test)]
#[cfg(feature = "static-engines")]
mod engine_tests;

// Re-export all public types from submodules
pub use types::*;
#[cfg(feature = "static-engines")]
pub use v8_config::*;
#[cfg(feature = "static-engines")]
pub use wasm_config::*;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::execute_js_task;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::clear_script_cache;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::ensure_v8_initialized;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::DEFAULT_HEAP_LIMIT;
#[cfg(feature = "static-engines")]
pub(crate) use v8_engine::SCRIPT_CACHE;
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

static SHARED_KEYSTORE_RESOLVER: std::sync::OnceLock<Arc<crate::keystore::KeystoreResolver>> =
    std::sync::OnceLock::new();

/// Set the shared keystore resolver. Called once at startup after bundle loading.
pub fn set_keystore_resolver(resolver: Arc<crate::keystore::KeystoreResolver>) {
    let _ = SHARED_KEYSTORE_RESOLVER.set(resolver);
}

/// Get the shared keystore resolver. Returns None if no keystores are configured.
pub fn get_keystore_resolver() -> Option<&'static Arc<crate::keystore::KeystoreResolver>> {
    SHARED_KEYSTORE_RESOLVER.get()
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
        // Dynamic engine path — serialize context, call through C-ABI
        let serialized = rivers_engine_sdk::SerializedTaskContext::from(&ctx);
        let engine_name = engine_key.to_string();

        let result = tokio::task::spawn_blocking(move || {
            crate::engine_loader::execute_on_engine(&engine_name, &serialized)
        })
        .await
        .map_err(|e| TaskError::WorkerCrash(format!("engine task panicked: {e}")))?;

        return result
            .map(|r| r.into())
            .map_err(|e| TaskError::HandlerError(e));
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

