//! Security pipeline tests (P0-1 — fail closed when session manager is absent).
//!
//! Verifies that `run_security_pipeline` rejects requests to non-public views
//! when `AppContext::session_manager` is `None`. Without the fix, such a
//! configuration would silently let requests through.

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;

use rivers_runtime::rivers_core::ServerConfig;
use rivers_runtime::view::ApiViewConfig;
use riversd::security_pipeline::run_security_pipeline;
use riversd::server::AppContext;
use riversd::shutdown::ShutdownCoordinator;

fn protected_view() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "Rest",
        "path": "/api/protected",
        "method": "GET",
        "handler": { "type": "dataview", "dataview": "noop" }
    }))
    .expect("valid ApiViewConfig")
}

fn public_view() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "Rest",
        "path": "/api/public",
        "method": "GET",
        "auth": "none",
        "handler": { "type": "dataview", "dataview": "noop" }
    }))
    .expect("valid ApiViewConfig")
}

fn ctx_without_session_manager() -> AppContext {
    let mut config = ServerConfig::default();
    config.base.admin_api.no_auth = Some(true);
    AppContext::new(config, Arc::new(ShutdownCoordinator::new()))
    // .session_manager left as None — the misconfig we test.
}

/// **P0-1 / A2.3**: a non-public view + missing session manager must fail closed.
#[tokio::test(flavor = "current_thread")]
async fn protected_view_without_session_manager_fails_closed() {
    let ctx = ctx_without_session_manager();
    assert!(ctx.session_manager.is_none(), "fixture sanity");

    let view = protected_view();
    let headers: HashMap<String, String> = HashMap::new();

    let result = run_security_pipeline(
        &ctx,
        &view,
        &headers,
        "GET",
        "trace-fail-closed-1",
        None,
        "test-app",
    )
    .await;

    let resp = match result {
        Ok(_) => panic!("pipeline must reject when session_manager is None"),
        Err(r) => r,
    };
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "fail-closed returns 500 (misconfig), not 401/302"
    );
}

/// Public views must remain reachable even with no session manager — they
/// don't require authentication, so the misconfig doesn't apply.
#[tokio::test(flavor = "current_thread")]
async fn public_view_without_session_manager_still_works() {
    let ctx = ctx_without_session_manager();
    let view = public_view();
    let headers: HashMap<String, String> = HashMap::new();

    let result = run_security_pipeline(
        &ctx,
        &view,
        &headers,
        "GET",
        "trace-public-ok",
        None,
        "test-app",
    )
    .await;

    assert!(
        result.is_ok(),
        "public view should pass through even with no session manager"
    );
}
