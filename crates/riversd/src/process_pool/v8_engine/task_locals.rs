//! Thread-local declarations, TaskLocals guard, LockBoxContext, KeystoreContext.
//!
//! Every thread-local used by V8 host callbacks lives here. The `TaskLocals`
//! guard struct sets them on creation and clears them on Drop, making it
//! impossible to add a setup without a matching teardown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::types::*;
use rivers_runtime::process_pool::TaskKind;
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

/// Per-task state for a `DatasourceToken::Direct` entry.
///
/// The V8 worker keeps driver, resource root, and a lazily-built `Connection`
/// so typed-proxy operations can dispatch without a pool round-trip. One entry
/// per datasource name declared on the task.
pub(super) struct DirectDatasource {
    pub(super) driver: String,
    pub(super) root: std::path::PathBuf,
    pub(super) connection:
        RefCell<Option<Box<dyn rivers_runtime::rivers_driver_sdk::Connection>>>,
}

/// Per-task state for a `DatasourceToken::Broker` entry (BR-2026-04-23).
///
/// Mirrors `DirectDatasource` but for `MessageBrokerDriver::create_producer`.
/// The `BrokerProducer` is lazy-built on first `.publish()` call in this task;
/// closed in `TaskLocals::drop` using the live RT handle (same ordering
/// precedent as `auto_rollback_all`).
pub(super) struct DirectBrokerProducer {
    pub(super) driver: String,
    pub(super) params: rivers_runtime::rivers_driver_sdk::ConnectionParams,
    pub(super) producer:
        RefCell<Option<Box<dyn rivers_runtime::rivers_driver_sdk::broker::BrokerProducer>>>,
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

    /// Human-readable app name for the current task — used for per-app log routing.
    pub(super) static TASK_APP_NAME: RefCell<Option<String>> = RefCell::new(None);

    /// Dispatch-site classification (C1) — gates elevated host capabilities.
    /// `None` means "no task active". Inside a task it MUST be `Some`. Host
    /// callbacks like `ctx.ddl()` read this to enforce ApplicationInit-only
    /// access (B1.2).
    pub(crate) static TASK_KIND: RefCell<Option<TaskKind>> = RefCell::new(None);

    /// Module-identity-hash → absolute source path for the modules compiled
    /// during the current task's execute_as_module call (spec §3.6 V8
    /// resolve callback). Needed because V8's resolve callback signature is
    /// `extern "C" fn` — can't capture state via closure. The callback reads
    /// the referrer's identity hash, looks up its path here, resolves the
    /// specifier relative to that path's parent, and finds the target in
    /// `BundleModuleCache`.
    pub(crate) static TASK_MODULE_REGISTRY: RefCell<HashMap<i32, std::path::PathBuf>> = RefCell::new(HashMap::new());

    /// Namespace Object for the currently-executing module, if the source
    /// uses ES module syntax (spec §4). `call_entrypoint` reads this: Some
    /// means look up on the namespace, None means classic-script path
    /// (lookup on globalThis).
    pub(crate) static TASK_MODULE_NAMESPACE: RefCell<Option<v8::Global<v8::Object>>> = RefCell::new(None);

    /// Active handler transaction state (spec §6). `ctx.transaction(ds, fn)`
    /// populates this before invoking the JS callback; `ctx.dataview()`
    /// reads it to (a) enforce the spec §6.2 cross-datasource check and
    /// (b) route execution through the held transaction connection.
    /// Cleared in `TaskLocals::drop`. `auto_rollback_all` runs on any
    /// still-held connection when the task ends.
    pub(crate) static TASK_TRANSACTION: RefCell<Option<TaskTransactionState>> = RefCell::new(None);

    /// Set by `ctx_transaction_callback` when the post-callback
    /// `commit_transaction()` call fails. `execute_js_task` checks this
    /// after `call_entrypoint` returns an error and upgrades the error
    /// from the generic `HandlerErrorWithStack` (indistinguishable from a
    /// handler throw) to the distinct `TransactionCommitFailed` variant.
    /// Stores (datasource, driver-error-message).
    ///
    /// Why this matters: for financial workloads, "handler threw" and
    /// "commit failed after handler returned" have different retry
    /// semantics. Without this disambiguation the client sees the same
    /// HTTP 500 for both cases.
    pub(crate) static TASK_COMMIT_FAILED: RefCell<Option<(String, String)>> = RefCell::new(None);

    /// `DatasourceToken::Direct` entries declared by the current task.
    /// The typed-proxy host fn (`__rivers_direct_dispatch`) reads from this
    /// map to build/reuse a `Connection` and run operations in-thread.
    pub(super) static TASK_DIRECT_DATASOURCES:
        RefCell<HashMap<String, DirectDatasource>> = RefCell::new(HashMap::new());

    /// `DatasourceToken::Broker` entries declared by the current task
    /// (BR-2026-04-23). The broker-dispatch host fn
    /// (`Rivers.__brokerPublish`) reads from this map to build/reuse a
    /// `BrokerProducer` and run `publish` operations in-thread.
    pub(super) static TASK_DIRECT_BROKER_PRODUCERS:
        RefCell<HashMap<String, DirectBrokerProducer>> = RefCell::new(HashMap::new());

    /// Elicitation request channel sender for the current task (P2.6).
    ///
    /// When `Some`, the `Rivers.__elicit` host callback posts an
    /// `ElicitationRequest` here; the relay task spawned by
    /// `dispatch_codecomponent_tool` reads it, sends the
    /// `elicitation/create` SSE notification to the MCP client, and
    /// registers the `oneshot::Sender` in `ElicitationRegistry`.
    ///
    /// `None` outside an MCP tool call — `Rivers.__elicit` throws
    /// "elicitation not available" in that case.
    pub(crate) static TASK_ELICITATION_TX:
        RefCell<Option<tokio::sync::mpsc::UnboundedSender<crate::mcp::elicitation::ElicitationRequest>>> =
        RefCell::new(None);
}

/// Active transaction state for the current task.
pub(super) struct TaskTransactionState {
    /// The TransactionMap that holds the connection for commit/rollback.
    pub(super) map: Arc<crate::transaction::TransactionMap>,
    /// The single datasource this transaction is scoped to (spec §6.2).
    pub(super) datasource: String,
}

/// Active `Rivers.db.tx` transaction state (TXN spec §4–6).
///
/// Holds the connection (via TransactionMap), the datasource name, and the
/// accumulated results keyed by DataView name. Multiple calls to the same
/// DataView append to the Vec — ordering is preserved (RM-2).
pub(crate) struct TxHandleState {
    /// The TransactionMap that holds the checked-out connection.
    pub(crate) map: Arc<crate::transaction::TransactionMap>,
    /// Datasource this transaction is scoped to (CD-1).
    pub(crate) datasource: String,
    /// Accumulated results from `tx.query()` calls: name → ordered Vec.
    pub(crate) results: HashMap<String, Vec<rivers_runtime::rivers_driver_sdk::QueryResult>>,
}

thread_local! {
    /// Active `Rivers.db.tx` transaction handle for the current V8 task.
    /// Set by `tx_begin_callback`, cleared by commit/rollback/auto-rollback.
    pub(crate) static TASK_TX_HANDLE: RefCell<Option<TxHandleState>> = RefCell::new(None);
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
    ///
    /// **C1.3:** Rejects empty `app_id` with a hard error. The previous
    /// behavior — silently substituting `"app:default"` as the store namespace —
    /// hid a class of bugs where a MessageConsumer's `ctx.store.set("k","v")`
    /// landed in a different namespace from the same-app HTTP handler. The
    /// dispatch site is required to plumb the real app_id; if it can't, that
    /// is a bug at the dispatch site, not something to paper over here.
    pub(super) fn set(
        ctx: &TaskContext,
        rt_handle: tokio::runtime::Handle,
    ) -> Result<Self, TaskError> {
        if ctx.app_id.is_empty() {
            tracing::error!(
                trace_id = %ctx.trace_id,
                task_kind = %ctx.task_kind.as_str(),
                entrypoint_module = %ctx.entrypoint.module,
                entrypoint_function = %ctx.entrypoint.function,
                "dispatch rejected: empty app_id (C1)"
            );
            return Err(TaskError::Internal(format!(
                "dispatch rejected: empty app_id (task_kind={}, entrypoint={}::{}, trace_id={})",
                ctx.task_kind.as_str(),
                ctx.entrypoint.module,
                ctx.entrypoint.function,
                ctx.trace_id,
            )));
        }
        RT_HANDLE.with(|h| *h.borrow_mut() = Some(rt_handle));
        TASK_ENV.with(|e| *e.borrow_mut() = Some(ctx.env.clone()));
        TASK_STORE.with(|s| s.borrow_mut().clear());
        TASK_TRACE_ID.with(|t| *t.borrow_mut() = Some(ctx.trace_id.clone()));
        TASK_APP_NAME.with(|n| *n.borrow_mut() = Some(ctx.app_id.clone()));
        TASK_HTTP_ENABLED.with(|h| *h.borrow_mut() = ctx.http.is_some());
        TASK_STORAGE.with(|s| *s.borrow_mut() = ctx.storage.clone());
        TASK_KIND.with(|k| *k.borrow_mut() = Some(ctx.task_kind));
        let store_ns = format!("app:{}", ctx.app_id);
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
        TASK_DIRECT_DATASOURCES.with(|m| {
            let mut map = m.borrow_mut();
            map.clear();
            for (name, token) in &ctx.datasources {
                if let DatasourceToken::Direct { driver, root } = token {
                    map.insert(
                        name.clone(),
                        DirectDatasource {
                            driver: driver.clone(),
                            root: root.clone(),
                            connection: RefCell::new(None),
                        },
                    );
                }
            }
        });
        // BR-2026-04-23 — broker producer cache. ConnectionParams come
        // from the parallel ctx.datasource_configs map (TaskContext
        // populates both at build time).
        TASK_DIRECT_BROKER_PRODUCERS.with(|m| {
            let mut map = m.borrow_mut();
            map.clear();
            for (name, token) in &ctx.datasources {
                if let DatasourceToken::Broker { driver } = token {
                    // Missing datasource_configs entry is a plumbing bug
                    // (TaskContextBuilder should populate both maps);
                    // surface it at publish-time via the V8 callback
                    // rather than panicking here.
                    let params = ctx.datasource_configs
                        .get(name)
                        .map(|rd| rd.params.clone())
                        .unwrap_or_else(|| rivers_runtime::rivers_driver_sdk::ConnectionParams {
                            host: String::new(),
                            port: 0,
                            database: String::new(),
                            username: String::new(),
                            password: String::new(),
                            options: HashMap::new(),
                        });
                    map.insert(
                        name.clone(),
                        DirectBrokerProducer {
                            driver: driver.clone(),
                            params,
                            producer: RefCell::new(None),
                        },
                    );
                }
            }
        });
        // P2.6: install the elicitation sender if one was registered for this trace_id.
        // `take_elicitation_tx` removes the entry from the global map atomically so
        // no two workers can claim the same sender.
        let elicitation_tx = take_elicitation_tx(&ctx.trace_id);
        TASK_ELICITATION_TX.with(|t| *t.borrow_mut() = elicitation_tx);
        Ok(TaskLocals)
    }
}

impl Drop for TaskLocals {
    fn drop(&mut self) {
        // Auto-rollback any transaction the handler left open — BEFORE
        // clearing RT_HANDLE, because auto_rollback_all is async and needs
        // the runtime. Spec §6: timeout or panic must not leave a
        // connection holding a transaction in the pool.
        if let Some(state) = TASK_TRANSACTION.with(|t| t.borrow_mut().take()) {
            if let Some(rt) = RT_HANDLE.with(|h| h.borrow().clone()) {
                rt.block_on(state.map.auto_rollback_all());
            }
        }
        // AR-1/AR-2 (TXN spec §8): auto-rollback any Rivers.db.tx transaction
        // the handler left open (no commit or rollback called).
        if let Some(state) = TASK_TX_HANDLE.with(|t| t.borrow_mut().take()) {
            if let Some(rt) = RT_HANDLE.with(|h| h.borrow().clone()) {
                let ds = state.datasource.clone();
                let outcome = rt.block_on(state.map.rollback(&ds));
                // Collect trace_id for the log message (best-effort).
                let trace_id = TASK_TRACE_ID.with(|t| {
                    t.borrow().clone().unwrap_or_else(|| "<unknown>".into())
                });
                match outcome {
                    Ok(()) => {
                        tracing::warn!(
                            target: "rivers.handler",
                            datasource = %ds,
                            trace_id = %trace_id,
                            "transaction auto-rolled back: handler exited without commit or rollback"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            target: "rivers.handler",
                            datasource = %ds,
                            trace_id = %trace_id,
                            error = %e,
                            "transaction auto-rollback FAILED — connection discarded"
                        );
                    }
                }
            }
        }
        // BR-2026-04-23: close any broker producers opened this task.
        // Must run BEFORE RT_HANDLE is cleared because producer.close()
        // is async. Best-effort — log on error, don't block drop.
        if let Some(rt) = RT_HANDLE.with(|h| h.borrow().clone()) {
            TASK_DIRECT_BROKER_PRODUCERS.with(|m| {
                let mut map = m.borrow_mut();
                for (name, entry) in map.drain() {
                    if let Some(mut prod) = entry.producer.borrow_mut().take() {
                        if let Err(e) = rt.block_on(prod.close()) {
                            tracing::warn!(
                                target: "rivers.broker",
                                datasource = %name,
                                driver = %entry.driver,
                                error = %e,
                                "broker producer close failed on TaskLocals drop"
                            );
                        }
                    }
                }
            });
        }
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
        TASK_APP_NAME.with(|n| *n.borrow_mut() = None);
        TASK_KIND.with(|k| *k.borrow_mut() = None);
        TASK_MODULE_REGISTRY.with(|r| r.borrow_mut().clear());
        TASK_MODULE_NAMESPACE.with(|n| *n.borrow_mut() = None);
        // TASK_TRANSACTION and TASK_TX_HANDLE were drained above, before RT_HANDLE was cleared.
        TASK_COMMIT_FAILED.with(|c| *c.borrow_mut() = None);
        TASK_TX_HANDLE.with(|t| *t.borrow_mut() = None);
        TASK_DIRECT_DATASOURCES.with(|m| m.borrow_mut().clear());
        TASK_DIRECT_BROKER_PRODUCERS.with(|m| m.borrow_mut().clear());
        // P2.6: drop the elicitation sender so the relay task's receiver sees EOF
        // and terminates cleanly. Any pending elicitations in the registry were
        // already resolved (or timed out) before the task completes.
        TASK_ELICITATION_TX.with(|t| *t.borrow_mut() = None);
    }
}

// ── P2.6 global registry: trace_id → ElicitationRequest sender ──────────
//
// Because `TaskContext` lives in `rivers-runtime` (which we must not modify),
// we cannot add the elicitation sender as a `TaskContext` field. Instead, we
// use a process-level static map keyed by `trace_id`. The lifecycle is:
//
//   1. `dispatch_codecomponent_tool` (async, on the tokio thread) creates
//      the channel and calls `register_elicitation_tx(trace_id, tx)`.
//   2. `TaskLocals::set` (on the spawn_blocking thread) reads the sender
//      via `take_elicitation_tx(trace_id)` and installs it in the thread-local.
//   3. `TaskLocals::drop` clears the thread-local (the sender is dropped,
//      which closes the relay channel cleanly).

use std::collections::HashMap as ElicitHashMap;
use std::sync::Mutex as ElicitMutex;
use tokio::sync::mpsc::UnboundedSender as ElicitTx;
use crate::mcp::elicitation::ElicitationRequest;

static ELICITATION_GLOBAL: ElicitMutex<
    Option<ElicitHashMap<String, ElicitTx<ElicitationRequest>>>
> = ElicitMutex::new(None);

fn elicitation_global_map() -> std::sync::MutexGuard<'static, Option<ElicitHashMap<String, ElicitTx<ElicitationRequest>>>> {
    ELICITATION_GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Store an elicitation sender for a task, keyed by trace_id.
///
/// Called by `dispatch_codecomponent_tool` before dispatching the task to the pool.
pub(crate) fn register_elicitation_tx(
    trace_id: &str,
    tx: ElicitTx<ElicitationRequest>,
) {
    let mut guard = elicitation_global_map();
    if guard.is_none() {
        *guard = Some(ElicitHashMap::new());
    }
    guard.as_mut().unwrap().insert(trace_id.to_string(), tx);
}

/// Move the elicitation sender for a task into the thread-local.
///
/// Called by `TaskLocals::set` to wire the sender into the V8 worker thread.
/// Returns `None` when no elicitation channel was registered for this trace_id
/// (i.e., the task was not dispatched via MCP).
fn take_elicitation_tx(trace_id: &str) -> Option<ElicitTx<ElicitationRequest>> {
    let mut guard = elicitation_global_map();
    guard.as_mut().and_then(|m| m.remove(trace_id))
}

#[cfg(test)]
mod direct_datasource_tests {
    use super::*;
    use crate::process_pool::{Entrypoint, TaskContextBuilder};

    fn task_with_datasources(entries: Vec<(&str, DatasourceToken)>) -> TaskContext {
        let mut b = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "run".into(),
                language: "javascript".into(),
            })
            .app_id("test-app".into())
            .task_kind(TaskKind::Rest);
        for (name, token) in entries {
            b = b.datasource(name.into(), token);
        }
        b.build().unwrap()
    }

    #[test]
    fn direct_tokens_populate_thread_local() {
        let ctx = task_with_datasources(vec![
            (
                "fs",
                DatasourceToken::direct("filesystem", std::path::PathBuf::from("/tmp/root")),
            ),
            ("db", DatasourceToken::pooled("postgres:db")),
        ]);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let guard = TaskLocals::set(&ctx, rt.handle().clone()).unwrap();

        TASK_DIRECT_DATASOURCES.with(|m| {
            let map = m.borrow();
            assert_eq!(map.len(), 1, "only Direct tokens should populate");
            let fs = map.get("fs").expect("fs entry");
            assert_eq!(fs.driver, "filesystem");
            assert_eq!(fs.root, std::path::PathBuf::from("/tmp/root"));
            assert!(fs.connection.borrow().is_none(), "connection is lazy");
        });

        drop(guard);

        TASK_DIRECT_DATASOURCES.with(|m| {
            assert!(m.borrow().is_empty(), "Drop clears the map");
        });
    }

    #[test]
    fn pooled_only_leaves_map_empty() {
        let ctx = task_with_datasources(vec![("db", DatasourceToken::pooled("postgres:db"))]);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = TaskLocals::set(&ctx, rt.handle().clone()).unwrap();
        TASK_DIRECT_DATASOURCES.with(|m| assert!(m.borrow().is_empty()));
    }

    #[test]
    fn empty_app_id_is_rejected() {
        // C1.3: building a TaskContext with no app_id and trying to set
        // TaskLocals must produce a hard TaskError::Internal so the dispatch
        // does not silently land in the wrong store namespace.
        let ctx = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "inline".into(),
                function: "run".into(),
                language: "javascript".into(),
            })
            .task_kind(TaskKind::Rest)
            .build()
            .unwrap();
        // app_id stays empty (no .app_id() call).
        assert_eq!(ctx.app_id, "");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = TaskLocals::set(&ctx, rt.handle().clone());
        assert!(res.is_err(), "empty app_id must be rejected");
        let err_msg = res.err().unwrap().to_string();
        assert!(
            err_msg.contains("empty app_id"),
            "error message should mention empty app_id, got: {err_msg}",
        );
    }
}
