//! Driver validation tests — failed_apps tracking, path matching, 503 response.
//!
//! Validates that apps blocked due to missing drivers:
//!   1. Start with an empty failed_apps map.
//!   2. Can be populated with error entries.
//!   3. Use path-prefix matching to identify blocked requests.
//!   4. Return 503 with a JSON body for requests to blocked paths.
//!   5. Do not affect unrelated routes (e.g., /health).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for oneshot()

use rivers_runtime::rivers_core::ServerConfig;
use riversd::server::{build_main_router, AppContext};
use riversd::shutdown::ShutdownCoordinator;

// ── Helpers ───────────────────────────────────────────────────────

fn test_ctx() -> AppContext {
    let mut config = ServerConfig::default();
    config.base.admin_api.no_auth = Some(true);
    AppContext::new(config, Arc::new(ShutdownCoordinator::new()))
}

// ── Test 1: failed_apps starts empty ─────────────────────────────

#[tokio::test]
async fn failed_apps_starts_empty() {
    let ctx = AppContext::new(
        ServerConfig::default(),
        Arc::new(ShutdownCoordinator::new()),
    );
    let failed = ctx.failed_apps.read().unwrap();
    assert!(failed.is_empty(), "failed_apps should be empty on init");
}

// ── Test 2: failed_apps can be populated ─────────────────────────

#[tokio::test]
async fn failed_apps_can_be_populated() {
    let ctx = test_ctx();
    {
        let mut failed = ctx.failed_apps.write().unwrap();
        failed.insert(
            "/my-bundle/my-app".to_string(),
            "app 'my-app' blocked: missing drivers: mongodb".to_string(),
        );
    }
    let failed = ctx.failed_apps.read().unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed.contains_key("/my-bundle/my-app"));
    assert!(
        failed["/my-bundle/my-app"].contains("missing drivers"),
        "error message should mention missing drivers"
    );
}

// ── Test 3: path prefix matching ─────────────────────────────────

#[tokio::test]
async fn failed_apps_path_prefix_matching() {
    let ctx = test_ctx();
    {
        let mut failed = ctx.failed_apps.write().unwrap();
        failed.insert(
            "/canary-fleet/nosql".to_string(),
            "app 'nosql' blocked: missing drivers: mongodb".to_string(),
        );
    }

    let failed = ctx.failed_apps.read().unwrap();

    // A deep sub-path under the blocked prefix should match.
    let matching_path = "/canary-fleet/nosql/canary/nosql/mongo/ping";
    let matched = failed
        .iter()
        .any(|(prefix, _)| matching_path.starts_with(prefix.as_str()));
    assert!(matched, "path '{matching_path}' should match prefix '/canary-fleet/nosql'");

    // A sibling app path should NOT match.
    let sibling_path = "/canary-fleet/sql/canary/sql/pg/select";
    let should_not_match = failed
        .iter()
        .any(|(prefix, _)| sibling_path.starts_with(prefix.as_str()));
    assert!(
        !should_not_match,
        "path '{sibling_path}' should NOT match prefix '/canary-fleet/nosql'"
    );
}

// ── Test 4: failed app returns 503 ───────────────────────────────

#[tokio::test]
async fn failed_app_returns_503() {
    let ctx = test_ctx();
    {
        let mut failed = ctx.failed_apps.write().unwrap();
        failed.insert(
            "/test-bundle/broken-app".to_string(),
            "app 'broken-app' blocked: missing drivers: mongodb".to_string(),
        );
    }

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/test-bundle/broken-app/some/endpoint")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "blocked app path should return 503"
    );

    let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 503, "JSON body should contain code: 503");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or("")
            .contains("missing drivers"),
        "JSON message should reference missing drivers, got: {}",
        json["message"]
    );
}

// ── Test 5: healthy app not affected by failed app ────────────────

#[tokio::test]
async fn healthy_app_not_affected_by_failed_app() {
    let ctx = test_ctx();
    {
        let mut failed = ctx.failed_apps.write().unwrap();
        failed.insert(
            "/test-bundle/broken-app".to_string(),
            "app 'broken-app' blocked: missing drivers: mongodb".to_string(),
        );
    }

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "/health should return 200 even when another app is blocked"
    );

    let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}
