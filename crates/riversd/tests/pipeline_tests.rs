//! Pipeline stage tests — SHAPE-12 sequential execution order and stage behavior.
//!
//! Exercises the 4-stage REST view pipeline:
//!   pre_process → DataView/CodeComponent → handlers → post_process + on_error
//!
//! These tests compile and run without a live server or ProcessPool:
//! - Structural tests verify `ViewEventHandlers` stage ordering via config inspection.
//! - Execution tests use `pool = None` (stub mode) to verify resdata propagation,
//!   None-handler null-init, and on_error not-reached on success.
//!
//! Spec ref: rivers-view-layer-spec.md §4, rivers-technology-path-spec.md SHAPE-12.
//!
//! Run with: cargo test -p riversd --test pipeline_tests

use std::collections::HashMap;

use rivers_runtime::view::{
    ApiViewConfig, HandlerConfig, HandlerStageConfig, ViewEventHandlers,
};
use riversd::view_engine::{execute_rest_view, ParsedRequest, ViewContext};

// ── Helpers ───────────────────────────────────────────────────────

fn make_stage(module: &str, entrypoint: &str) -> HandlerStageConfig {
    HandlerStageConfig {
        module: module.to_string(),
        entrypoint: entrypoint.to_string(),
        key: None,
        on_failure: None,
    }
}

/// Build a minimal REST view backed by a DataView handler with optional
/// pipeline event_handlers.
fn rest_view_with_handlers(
    dataview: &str,
    event_handlers: Option<ViewEventHandlers>,
) -> ApiViewConfig {
    ApiViewConfig {
        view_type: "Rest".to_string(),
        path: Some("/api/test".to_string()),
        method: Some("GET".to_string()),
        handler: HandlerConfig::Dataview {
            dataview: dataview.to_string(),
        },
        handlers: None,
        max_body_mb: None,
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        event_handlers,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
        polling: None,
        tools: HashMap::new(),
        resources: HashMap::new(),
        prompts: HashMap::new(),
        instructions: None,
        session: None,
        federation: vec![],
        response_headers: None,
        schedule: None,
        interval_seconds: None,
        overlap_policy: None,
        max_concurrent: None,
            guard_view: None,
    }
}

/// Build a minimal REST view using the None handler (no primary datasource).
fn none_handler_view(event_handlers: Option<ViewEventHandlers>) -> ApiViewConfig {
    ApiViewConfig {
        view_type: "Rest".to_string(),
        path: Some("/api/noop".to_string()),
        method: Some("GET".to_string()),
        handler: HandlerConfig::None {},
        handlers: None,
        max_body_mb: None,
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        event_handlers,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
        polling: None,
        tools: HashMap::new(),
        resources: HashMap::new(),
        prompts: HashMap::new(),
        instructions: None,
        session: None,
        federation: vec![],
        response_headers: None,
        schedule: None,
        interval_seconds: None,
        overlap_policy: None,
        max_concurrent: None,
            guard_view: None,
    }
}

fn make_ctx() -> ViewContext {
    let req = ParsedRequest::new("GET", "/api/test");
    ViewContext::new(
        req,
        "trace-pipeline-1".to_string(),
        "test-app-id".to_string(),
        "test-app".to_string(),
        "node-1".to_string(),
        "dev".to_string(),
    )
}

// ── Test 1: SHAPE-12 — pipeline stage definition order ──────────
//
// Confirms that `ViewEventHandlers` exposes pre_process, handlers,
// post_process, and on_error in the documented sequential order.
// Without a running ProcessPool these are structural assertions that
// verify the config type carries stage identity correctly.

#[test]
fn pipeline_stage_order_is_pre_process_handlers_post_process_on_error() {
    let handlers = ViewEventHandlers {
        pre_process: vec![make_stage("pre.js", "onPreProcess")],
        handlers: vec![
            make_stage("handler_a.js", "onRequest"),
            make_stage("handler_b.js", "onRequest"),
        ],
        post_process: vec![make_stage("post.js", "onPostProcess")],
        on_error: vec![make_stage("err.js", "onError")],
    };

    // pre_process is first
    assert_eq!(handlers.pre_process.len(), 1);
    assert_eq!(handlers.pre_process[0].module, "pre.js");
    assert_eq!(handlers.pre_process[0].entrypoint, "onPreProcess");

    // handlers (ordered chain) comes second; multiple stages allowed
    assert_eq!(handlers.handlers.len(), 2);
    assert_eq!(handlers.handlers[0].module, "handler_a.js");
    assert_eq!(handlers.handlers[1].module, "handler_b.js");

    // post_process is third
    assert_eq!(handlers.post_process.len(), 1);
    assert_eq!(handlers.post_process[0].module, "post.js");

    // on_error is last (not part of the happy-path chain)
    assert_eq!(handlers.on_error.len(), 1);
    assert_eq!(handlers.on_error[0].module, "err.js");
}

// ── Test 2: pre_process fires before DataView (stub mode) ────────
//
// With pool=None, CodeComponent stages are not dispatched, but the
// DataView handler still runs (stub path). This confirms the pipeline
// reaches the DataView stage and produces a stub resdata even when
// a pre_process stage is configured.

#[tokio::test]
async fn pre_process_configured_does_not_block_dataview_stub_execution() {
    let handlers = ViewEventHandlers {
        pre_process: vec![make_stage("pre.js", "onPreProcess")],
        handlers: vec![],
        post_process: vec![],
        on_error: vec![],
    };

    let config = rest_view_with_handlers("my_dataview", Some(handlers));
    let mut ctx = make_ctx();

    // pool=None → CodeComponent stages are no-ops; DataView still stubs.
    let result = execute_rest_view(&mut ctx, &config, None, None)
        .await
        .expect("pipeline must succeed");

    assert_eq!(result.status, 200);
    // DataView stub path sets _stub + _dataview in resdata
    assert_eq!(
        result.body.get("_stub"),
        Some(&serde_json::Value::Bool(true)),
        "DataView stub marker must be present after pre_process stage"
    );
    assert_eq!(
        result.body["_dataview"],
        serde_json::Value::String("my_dataview".into()),
        "stub must record the dataview name"
    );
}

// ── Test 3: post_process is side-effect-only, does not alter resdata ─
//
// After the DataView runs, any post_process stages fire (observer pattern).
// With pool=None they are skipped, but the pipeline must still return
// the correct DataView resdata unchanged.  This verifies that post_process
// is downstream of the primary execution and cannot overwrite resdata in
// stub mode.

#[tokio::test]
async fn post_process_does_not_alter_resdata_in_stub_mode() {
    let handlers = ViewEventHandlers {
        pre_process: vec![],
        handlers: vec![],
        post_process: vec![make_stage("post.js", "onPostProcess")],
        on_error: vec![],
    };

    let config = rest_view_with_handlers("contacts", Some(handlers));
    let mut ctx = make_ctx();

    let result = execute_rest_view(&mut ctx, &config, None, None)
        .await
        .expect("pipeline must succeed");

    // DataView stub result is still present — post_process didn't erase it
    assert_eq!(result.status, 200);
    assert_eq!(result.body["_dataview"], "contacts");
}

// ── Test 4: on_error does not fire when pipeline succeeds ────────
//
// The on_error handlers are wired only when the inner pipeline returns Err.
// On a clean DataView stub run, on_error stages must NOT interfere with the
// successful ViewResult.  Verified by asserting the stub body is intact.

#[tokio::test]
async fn on_error_does_not_fire_on_successful_pipeline() {
    let handlers = ViewEventHandlers {
        pre_process: vec![],
        handlers: vec![],
        post_process: vec![],
        on_error: vec![make_stage("err.js", "onError")],
    };

    let config = rest_view_with_handlers("orders", Some(handlers));
    let mut ctx = make_ctx();

    let result = execute_rest_view(&mut ctx, &config, None, None)
        .await
        .expect("on_error must not convert a success into an error");

    assert_eq!(result.status, 200);
    // The DataView stub body must be the result — not an error payload
    assert_eq!(
        result.body.get("_stub"),
        Some(&serde_json::Value::Bool(true)),
        "on_error must not fire on success; DataView stub must be returned"
    );
    assert_eq!(result.body["_dataview"], "orders");
}

// ── Test 5: None handler initialises resdata to null ────────────
//
// The `HandlerConfig::None` pattern (datasource = "none") is used for views
// that run only CodeComponent pipeline stages.  resdata MUST start as null
// so that the first handler stage owns the payload rather than inheriting
// stale DataView output.

#[tokio::test]
async fn none_handler_initialises_resdata_to_null() {
    let config = none_handler_view(None);
    let mut ctx = make_ctx();
    // Pre-seed resdata to confirm the None handler resets it
    ctx.resdata = serde_json::json!({"stale": true});

    let result = execute_rest_view(&mut ctx, &config, None, None)
        .await
        .expect("None handler pipeline must succeed");

    assert_eq!(result.status, 200);
    // None handler sets resdata = null; body should be null
    assert!(
        result.body.is_null(),
        "None handler must initialise resdata to null, got: {:?}",
        result.body
    );
}

// ── Test 6: all pipeline stages empty — plain DataView stub ──────
//
// Sanity check: a view with no event_handlers at all must complete
// successfully with just the DataView stub output.

#[tokio::test]
async fn no_pipeline_stages_succeeds_with_dataview_stub() {
    let config = rest_view_with_handlers("inventory", None);
    let mut ctx = make_ctx();

    let result = execute_rest_view(&mut ctx, &config, None, None)
        .await
        .expect("baseline: view with no pipeline stages must succeed");

    assert_eq!(result.status, 200);
    assert_eq!(result.body["_dataview"], "inventory");
}
