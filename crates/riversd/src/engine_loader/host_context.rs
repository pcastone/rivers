//! Host context — subsystem references for host callbacks, set once after server init.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use rivers_engine_sdk::HostCallbacks;

use super::dyn_transaction_map::{DynTransactionMap, TaskId};
use super::host_callbacks;

/// Maximum wall-time budget for any single async operation invoked from a
/// host callback (commit, rollback, driver `connect`, etc.). Mirrors the
/// V8-side limit per `process_pool/v8_engine/context.rs` H2 — kept in a
/// single place so V8 and the dyn-engine cdylib path can't drift.
///
/// 30 seconds is a deliberately generous budget: Postgres commit on a slow
/// link should never approach it under steady-state, but a hung driver or
/// a broken socket must not pin the worker indefinitely. Phase H2.
pub(crate) const HOST_CALLBACK_TIMEOUT_MS: u64 = 30_000;

// ── Host Context (OnceLock subsystem references) ────────────────

/// Subsystem references for host callbacks. Set once after server init.
///
/// Visibility is `pub(crate)` (not `pub(super)`) so tests in
/// `process_pool/mod.rs` can reach it via `HOST_CONTEXT_FOR_TESTS` to drive
/// the I7 dispatch lifecycle. Production callers still go through the
/// `set_host_context` setter; the struct fields remain `pub(super)`.
pub(crate) struct HostContext {
    pub(super) dataview_executor: Arc<tokio::sync::RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>>,
    pub(super) storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    pub(super) driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
    pub(super) http_client: reqwest::Client,
    pub(super) rt_handle: tokio::runtime::Handle,
}

pub(super) static HOST_CONTEXT: OnceLock<HostContext> = OnceLock::new();

/// Application keystore for dynamic engine callbacks (App Keystore feature).
/// Separate OnceLock because keystore resolution happens per-app and may
/// occur after the main host context is wired.
pub(super) static HOST_KEYSTORE: OnceLock<Arc<rivers_keystore_engine::AppKeystore>> = OnceLock::new();

/// DDL whitelist — authorizes specific database+app pairs for DDL execution.
/// Set once during server startup from `config.security.ddl_whitelist`.
pub(super) static DDL_WHITELIST: OnceLock<Vec<String>> = OnceLock::new();

/// Maps entry_point names to manifest app_id UUIDs.
/// Used by DDL whitelist check — the ProcessPool uses entry_point as app_id,
/// but the whitelist format is `{database}@{appId}` with the manifest UUID.
pub(super) static APP_ID_MAP: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();

// ── Dyn-engine transaction map (Phase I — TXN-I1.1) ─────────────
//
// Per `docs/superpowers/plans/2026-04-25-phase-i-dyn-transactions.md` and
// `changedecisionlog.md` TXN-I1.1. The cdylib (V8/WASM) host callbacks need
// per-task transaction state, but unlike the V8 in-process path the engine
// thread is shared across many tasks. Identity is supplied by riversd: each
// `dispatch_task` issues a fresh `TaskId`, binds it to the `spawn_blocking`
// worker thread via `TaskGuard`, and host callbacks read `current_task_id()`
// to find their owning entry in `DYN_TXN_MAP`.

/// Process-wide dyn-engine transaction map. Initialized lazily on first use.
static DYN_TXN_MAP: OnceLock<DynTransactionMap> = OnceLock::new();

/// Accessor — process-wide `DynTransactionMap`. Used by `host_db_begin`,
/// `host_db_commit`, `host_db_rollback`, `host_dataview_execute`, and
/// `TaskGuard::drop`.
pub(crate) fn dyn_txn_map() -> &'static DynTransactionMap {
    DYN_TXN_MAP.get_or_init(DynTransactionMap::new)
}

/// Monotonic source of `TaskId` values. Starts at 1 so `0` can be used as a
/// sentinel "no task" if ever needed.
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Bound by `TaskGuard::enter` on the `spawn_blocking` worker thread.
    /// Read by dyn-engine host callbacks to identify the owning task.
    /// `None` outside a `TaskGuard` scope (e.g. in unit tests or on the V8
    /// dispatch path, which uses `TASK_TRANSACTION` thread-local instead).
    static CURRENT_TASK_ID: Cell<Option<TaskId>> = const { Cell::new(None) };

    /// Mirrors V8's `TASK_COMMIT_FAILED` (financial-correctness gate). Set by
    /// `host_db_commit` on commit failure or commit timeout; read by
    /// `dispatch_task` after `spawn_blocking` resolves so the resulting
    /// `TaskError::HandlerError` can be upgraded to
    /// `TaskError::TransactionCommitFailed`.
    pub(crate) static DYN_TASK_COMMIT_FAILED: Cell<Option<(String, String)>> =
        const { Cell::new(None) };
}

/// Issue a fresh `TaskId`. Called by `dispatch_task` immediately before
/// `spawn_blocking` so the closure body can be wrapped in a `TaskGuard`.
pub(crate) fn next_task_id() -> TaskId {
    TaskId(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed))
}

/// Read the current task id. Returns `None` when called outside a
/// `TaskGuard` scope.
pub(crate) fn current_task_id() -> Option<TaskId> {
    CURRENT_TASK_ID.with(|c| c.get())
}

/// Test-only setter for `CURRENT_TASK_ID`. Lets unit tests bind a task id
/// without a full `TaskGuard` (which would also schedule auto-rollback on
/// drop — undesirable inside tokio tests because `Drop` calls `block_on`).
#[cfg(test)]
pub(crate) fn set_current_task_id_for_test(id: Option<TaskId>) {
    CURRENT_TASK_ID.with(|c| c.set(id));
}

/// Test-only accessor for the `HostContext` `OnceLock`. Used by the I7
/// dispatch tests (in `process_pool/mod.rs`) so the closure-driven engine
/// runner can reach the `HostContext` without crossing the private
/// `pub(super) HOST_CONTEXT` visibility boundary.
#[cfg(test)]
pub(crate) static HOST_CONTEXT_FOR_TESTS: &OnceLock<HostContext> = &HOST_CONTEXT;

/// Test-only check whether `TASK_DS_CONFIGS` has an entry for
/// `(task_id, namespaced_ds)`. Used by I7 dispatch tests to verify
/// `TaskGuard::drop` cleared the per-task stash.
#[cfg(test)]
pub(crate) fn lookup_task_ds_for_test(
    task_id: TaskId,
    namespaced_ds: &str,
) -> Option<(String, rivers_runtime::rivers_driver_sdk::ConnectionParams)> {
    lookup_task_ds(task_id, namespaced_ds)
}

/// Test-only sync accessor for the runtime handle stashed in `HOST_CONTEXT`.
/// Phase I8 e2e tests run inside `dispatch_dyn_engine_task` closures (on
/// `spawn_blocking` workers) and need a runtime handle to `block_on` the
/// `execute_dataview_with_optional_txn` future without deadlocking.
#[cfg(test)]
pub(crate) fn host_rt_handle_for_test() -> tokio::runtime::Handle {
    HOST_CONTEXT
        .get()
        .expect("host_rt_handle_for_test: HOST_CONTEXT must be set first")
        .rt_handle
        .clone()
}

/// Test-only async accessor for the installed `DataViewExecutor`.
/// Phase I8 e2e tests need to grab the executor handle so they can drive
/// `execute_dataview_with_optional_txn_for_test` directly.
#[cfg(test)]
pub(crate) async fn host_dataview_executor_for_test()
-> Option<Arc<rivers_runtime::DataViewExecutor>> {
    let ctx = HOST_CONTEXT
        .get()
        .expect("host_dataview_executor_for_test: HOST_CONTEXT must be set first");
    ctx.dataview_executor.read().await.clone()
}

/// Test-only async installer for the DataViewExecutor inside `HOST_CONTEXT`.
/// Phase I8 e2e tests need a real executor wired into the (already-set)
/// `HostContext.dataview_executor` `RwLock` so `host_dataview_execute`
/// (and its internal `execute_dataview_with_optional_txn` helper) hit
/// real driver code instead of returning "DataViewExecutor not initialized".
///
/// The fixture calls `set_host_context(...)` first with
/// `Arc::new(RwLock::new(None))`; this helper writes `Some(executor)` into
/// that same lock so subsequent host-callback invocations resolve it.
/// Idempotent — last writer wins.
#[cfg(test)]
pub(crate) async fn install_dataview_executor_for_test(
    executor: Arc<rivers_runtime::DataViewExecutor>,
) {
    let ctx = HOST_CONTEXT.get().expect(
        "install_dataview_executor_for_test: HOST_CONTEXT must be set first \
         (call txn_test_fixtures::ensure_host_context())",
    );
    *ctx.dataview_executor.write().await = Some(executor);
}


/// Setter for the dyn-engine commit-failure thread-local. Used by
/// `host_db_commit` (I4) when `commit_transaction()` errors or times out.
pub(crate) fn signal_commit_failed(ds_name: String, reason: String) {
    DYN_TASK_COMMIT_FAILED.with(|c| c.set(Some((ds_name, reason))));
}

/// Take-and-clear the dyn-engine commit-failure thread-local. Called by
/// `dispatch_task` (I7) after `spawn_blocking` resolves.
pub(crate) fn take_commit_failed() -> Option<(String, String)> {
    DYN_TASK_COMMIT_FAILED.with(|c| c.take())
}

/// Snapshot of datasource configs needed by `host_db_begin` to look up
/// `(driver_name, ConnectionParams)` without an extra FFI roundtrip.
/// Populated by `dispatch_task` before `spawn_blocking`, cleared by
/// `TaskGuard::drop`. Q1 design decision (option A) per TXN-I1.1.
#[derive(Debug, Clone)]
pub(crate) struct DatasourceConfigsSnapshot {
    /// Map from "{entry_point}:{ds_name}" (the dyn-engine namespacing
    /// convention) → (driver_name, ConnectionParams). Mirrors the keys
    /// produced by `SerializedTaskContext::from(&ctx)`.
    pub configs: HashMap<
        String,
        (String, rivers_runtime::rivers_driver_sdk::ConnectionParams),
    >,
}

/// Per-task datasource configs stash. `RwLock` because reads (host_db_begin
/// per-task lookup) vastly outnumber writes (dispatch start / TaskGuard drop).
static TASK_DS_CONFIGS: RwLock<
    Option<HashMap<TaskId, DatasourceConfigsSnapshot>>,
> = RwLock::new(None);

fn task_ds_configs_with_init<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<TaskId, DatasourceConfigsSnapshot>) -> R,
{
    let mut guard = TASK_DS_CONFIGS.write().expect("TASK_DS_CONFIGS poisoned");
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    f(guard.as_mut().expect("just initialized"))
}

/// Stash a per-task datasource configs snapshot. Called by `dispatch_task`
/// (I7) before `spawn_blocking`.
pub(crate) fn store_task_ds_configs(task_id: TaskId, snapshot: DatasourceConfigsSnapshot) {
    task_ds_configs_with_init(|map| {
        map.insert(task_id, snapshot);
    });
}

/// Look up a `(driver_name, ConnectionParams)` pair for a namespaced
/// datasource on the current task. Used by `host_db_begin` (I3).
///
/// `namespaced_ds` is the same form used by `SerializedTaskContext` —
/// typically `"{entry_point}:{ds_name}"`.
pub(crate) fn lookup_task_ds(
    task_id: TaskId,
    namespaced_ds: &str,
) -> Option<(String, rivers_runtime::rivers_driver_sdk::ConnectionParams)> {
    let guard = TASK_DS_CONFIGS.read().expect("TASK_DS_CONFIGS poisoned");
    guard
        .as_ref()?
        .get(&task_id)?
        .configs
        .get(namespaced_ds)
        .cloned()
}

/// Drop the per-task datasource configs entry. Called from `TaskGuard::drop`.
pub(crate) fn clear_task_ds_configs(task_id: TaskId) {
    let mut guard = TASK_DS_CONFIGS.write().expect("TASK_DS_CONFIGS poisoned");
    if let Some(map) = guard.as_mut() {
        map.remove(&task_id);
    }
}

/// RAII guard that binds a `TaskId` to the current `spawn_blocking` worker
/// thread for the duration of one cdylib task. `Drop` runs the auto-rollback
/// hook for any transactions left in `DYN_TXN_MAP` and clears the per-task
/// datasource configs stash.
///
/// Must be constructed from inside a `spawn_blocking` closure, **not** from
/// a tokio runtime worker thread — `Drop` calls `rt_handle.block_on(...)` to
/// drive the async rollback, which would deadlock on a runtime worker.
pub(crate) struct TaskGuard {
    task_id: TaskId,
    rt_handle: tokio::runtime::Handle,
}

impl TaskGuard {
    /// Bind `task_id` to the current thread. Captures the current tokio
    /// runtime handle so `Drop` can drive async rollback synchronously.
    pub(crate) fn enter(task_id: TaskId, rt_handle: tokio::runtime::Handle) -> Self {
        CURRENT_TASK_ID.with(|c| c.set(Some(task_id)));
        Self { task_id, rt_handle }
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        // Auto-rollback every transaction left in the dyn-txn map for this
        // task. `block_on` is safe here because TaskGuard is constructed on
        // a `spawn_blocking` worker (not a tokio runtime worker).
        let leftover = dyn_txn_map().drain_task(self.task_id);
        if !leftover.is_empty() {
            tracing::warn!(
                target: "rivers.handler",
                task_id = ?self.task_id,
                count = leftover.len(),
                "TaskGuard: rolling back leftover dyn-engine transactions"
            );
            // Each rollback is independent — a failure or panic in one must
            // not prevent the others. Spawn each rollback as its own tokio
            // task so panics are caught by the runtime and surfaced as
            // `JoinError`s rather than aborting the loop.
            self.rt_handle.block_on(async {
                for (ds_name, mut conn) in leftover {
                    let join = tokio::spawn(async move {
                        conn.rollback_transaction().await
                    })
                    .await;
                    match join {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::warn!(
                            target: "rivers.handler",
                            ds_name = %ds_name,
                            error = %e,
                            "auto-rollback failed; connection abandoned"
                        ),
                        Err(join_err) => tracing::warn!(
                            target: "rivers.handler",
                            ds_name = %ds_name,
                            error = %join_err,
                            "auto-rollback panicked; connection abandoned"
                        ),
                    }
                }
            });
        }
        clear_task_ds_configs(self.task_id);
        CURRENT_TASK_ID.with(|c| c.set(None));
    }
}

/// Wire host subsystem references so callbacks can reach DataViewExecutor,
/// StorageEngine, DriverFactory, and HTTP client. Called once during server
/// startup after all subsystems are initialized.
pub fn set_host_context(
    dataview_executor: Arc<tokio::sync::RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>>,
    storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
) {
    let _ = HOST_CONTEXT.set(HostContext {
        dataview_executor,
        storage_engine,
        driver_factory,
        // Phase H7 / T2-7: timeout-bounded shared client. Without it, a
        // stalled upstream pinned the dynamic engine worker indefinitely
        // because `host_http_request` blocks on `recv()` for the spawned
        // request future. Same policy as the V8 path (H6).
        http_client: crate::http_client::outbound_client().clone(),
        rt_handle: tokio::runtime::Handle::current(),
    });
}

/// Set the application keystore for dynamic engine callbacks.
/// Called after `set_host_context` when an app has [[keystores]] declared.
pub fn set_host_keystore(keystore: Arc<rivers_keystore_engine::AppKeystore>) {
    let _ = HOST_KEYSTORE.set(keystore);
}

/// Set the DDL whitelist for host callback gating.
/// Called once during server startup alongside `set_host_context`.
pub fn set_ddl_whitelist(whitelist: Vec<String>) {
    let _ = DDL_WHITELIST.set(whitelist);
}

/// Set the entry_point → manifest app_id (UUID) mapping.
/// Called once during bundle loading so DDL whitelist checks can
/// resolve the ProcessPool's entry_point-based app_id to the UUID
/// used in whitelist entries.
pub fn set_app_id_map(map: std::collections::HashMap<String, String>) {
    let _ = APP_ID_MAP.set(map);
}

/// Read the configured DDL whitelist, if one was set during startup.
///
/// Returns `None` when `set_ddl_whitelist` has not been called (e.g. tests
/// that don't wire one). Returns `Some(vec)` otherwise — the vec may be
/// empty when the operator configured no entries.
///
/// Mirrors the read pattern in `host_callbacks::host_ddl_execute` so the V8
/// in-process callback (`ctx.ddl()`) and the dynamic-engine callback share
/// a single source of whitelist state — there must not be two stores.
pub fn ddl_whitelist() -> Option<Vec<String>> {
    DDL_WHITELIST.get().cloned()
}

/// Resolve a ProcessPool entry_point name to the manifest app_id (UUID).
///
/// The ProcessPool dispatches with entry_point as `app_id`, but the DDL
/// whitelist is keyed by the manifest UUID (`database@uuid`). When no
/// mapping is configured, callers should fall back to the entry_point
/// itself — same behavior as `host_callbacks::host_ddl_execute`.
pub fn app_id_for_entry_point(entry_point: &str) -> Option<String> {
    APP_ID_MAP.get().and_then(|m| m.get(entry_point).cloned())
}

// ── Host Callback Implementations ───────────────────────────────
//
// NOTE: The callbacks below return JSON over FFI boundaries using `{"error": ...}`
// format. This is an FFI protocol contract with cdylib engine plugins (V8, WASM).
// Do NOT replace with ErrorResponse — changing the shape would break dynamic
// engine plugins that parse these responses.

/// Build the `HostCallbacks` table with all callback functions wired.
pub fn build_host_callbacks() -> HostCallbacks {
    HostCallbacks {
        dataview_execute: Some(host_callbacks::host_dataview_execute),
        store_get: Some(host_callbacks::host_store_get),
        store_set: Some(host_callbacks::host_store_set),
        store_del: Some(host_callbacks::host_store_del),
        datasource_build: Some(host_callbacks::host_datasource_build),
        http_request: Some(host_callbacks::host_http_request),
        log_message: Some(host_callbacks::host_log_message),
        free_buffer: Some(host_callbacks::host_free_buffer),
        keystore_has: Some(host_callbacks::host_keystore_has),
        keystore_info: Some(host_callbacks::host_keystore_info),
        crypto_encrypt: Some(host_callbacks::host_crypto_encrypt),
        crypto_decrypt: Some(host_callbacks::host_crypto_decrypt),
        ddl_execute: Some(host_callbacks::host_ddl_execute),
        db_begin: Some(host_callbacks::host_db_begin),
        db_commit: Some(host_callbacks::host_db_commit),
        db_rollback: Some(host_callbacks::host_db_rollback),
        db_batch: Some(host_callbacks::host_db_batch),
    }
}
