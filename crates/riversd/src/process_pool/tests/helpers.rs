//! Shared test helpers for process pool engine tests.

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
