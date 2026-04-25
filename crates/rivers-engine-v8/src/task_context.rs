//! Per-task thread-local storage for V8 engine execution.
//!
//! Each task gets its own set of thread-locals (env, store, trace ID, etc.)
//! that are set up before execution and cleared after.

use std::cell::RefCell;
use std::collections::HashMap;

use rivers_engine_sdk::{SerializedTaskContext, TaskKind};

// ── Per-Task Thread-Locals ──────────────────────────────────────

thread_local! {
    /// Environment variables for the current task.
    pub(crate) static TASK_ENV: RefCell<Option<HashMap<String, String>>> = RefCell::new(None);

    /// Per-task key-value store (in-memory fallback).
    pub(crate) static TASK_STORE: RefCell<HashMap<String, serde_json::Value>> = RefCell::new(HashMap::new());

    /// Trace ID for the current task.
    pub(crate) static TASK_TRACE_ID: RefCell<Option<String>> = RefCell::new(None);

    /// Whether outbound HTTP is allowed.
    pub(crate) static TASK_HTTP_ENABLED: RefCell<bool> = RefCell::new(false);

    /// Store namespace prefix.
    pub(crate) static TASK_STORE_NAMESPACE: RefCell<Option<String>> = RefCell::new(None);

    /// App ID for the current task.
    pub(crate) static TASK_APP_ID: RefCell<Option<String>> = RefCell::new(None);

    /// Node ID.
    pub(crate) static TASK_NODE_ID: RefCell<Option<String>> = RefCell::new(None);

    /// Runtime env.
    pub(crate) static TASK_RUNTIME_ENV: RefCell<Option<String>> = RefCell::new(None);

    /// Dispatch-site classification — gates ctx.ddl() and similar capabilities.
    pub(crate) static TASK_KIND: RefCell<Option<TaskKind>> = RefCell::new(None);
}

/// Set up thread-locals from a serialized task context.
pub(crate) fn setup_task_locals(ctx: &SerializedTaskContext) {
    TASK_ENV.with(|e| *e.borrow_mut() = Some(ctx.env.clone()));
    TASK_STORE.with(|s| s.borrow_mut().clear());
    TASK_TRACE_ID.with(|t| *t.borrow_mut() = Some(ctx.trace_id.clone()));
    TASK_HTTP_ENABLED.with(|h| *h.borrow_mut() = ctx.http_enabled);
    TASK_STORE_NAMESPACE.with(|n| *n.borrow_mut() = ctx.store_namespace.clone());
    TASK_APP_ID.with(|a| *a.borrow_mut() = Some(ctx.app_id.clone()));
    TASK_NODE_ID.with(|n| *n.borrow_mut() = Some(ctx.node_id.clone()));
    TASK_RUNTIME_ENV.with(|r| *r.borrow_mut() = Some(ctx.runtime_env.clone()));
    TASK_KIND.with(|k| *k.borrow_mut() = ctx.task_kind);
}

/// Clear thread-locals after task execution.
pub(crate) fn clear_task_locals() {
    TASK_ENV.with(|e| *e.borrow_mut() = None);
    TASK_STORE.with(|s| s.borrow_mut().clear());
    TASK_TRACE_ID.with(|t| *t.borrow_mut() = None);
    TASK_HTTP_ENABLED.with(|h| *h.borrow_mut() = false);
    TASK_STORE_NAMESPACE.with(|n| *n.borrow_mut() = None);
    TASK_APP_ID.with(|a| *a.borrow_mut() = None);
    TASK_NODE_ID.with(|n| *n.borrow_mut() = None);
    TASK_RUNTIME_ENV.with(|r| *r.borrow_mut() = None);
    TASK_KIND.with(|k| *k.borrow_mut() = None);
}
