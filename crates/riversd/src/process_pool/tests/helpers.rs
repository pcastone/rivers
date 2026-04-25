//! Shared test helpers for process pool engine tests.

use std::sync::Arc;

use rivers_runtime::rivers_core::storage::{InMemoryStorageEngine, StorageEngine};

use super::*;

pub(super) fn make_js_task(source: &str, function: &str) -> TaskContext {
    TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: function.into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({ "_source": source }))
        .trace_id("test-trace".into())
        .app_id("test-app".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap()
}

/// Like `make_js_task` but attaches a fresh `InMemoryStorageEngine`. Required
/// for any test that calls `ctx.store.{set,get,del}` — B2 (P1-5) made the
/// callbacks throw a JS exception when no StorageEngine is configured (no
/// silent in-memory fallback). Tests that need to exercise the no-storage
/// throw path should keep using `make_js_task`.
pub(super) fn make_js_task_with_storage(source: &str, function: &str) -> TaskContext {
    let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
    TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: function.into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({ "_source": source }))
        .trace_id("test-trace".into())
        .app_id("test-app".into())
        .task_kind(TaskKind::Rest)
        .storage(storage)
        .build()
        .unwrap()
}

/// Helper: create a JS task with HTTP capability enabled.
pub(super) fn make_http_js_task(source: &str, function: &str) -> TaskContext {
    TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: function.into(),
            language: "javascript".into(),
        })
        .http(HttpToken)
        .args(serde_json::json!({ "_source": source }))
        .trace_id("test-http".into())
        .app_id("test-app".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap()
}
