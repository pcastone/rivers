//! HTTP server integration tests.
//!
//! Tests for the HTTPD core: router, middleware, health endpoints, shutdown.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http::header::HeaderValue;
use tower::ServiceExt; // for oneshot()

use rivers_runtime::rivers_core::ServerConfig;
use riversd::server::{build_admin_router, build_main_router, AppContext};
use riversd::shutdown::ShutdownCoordinator;

// ── Helpers ───────────────────────────────────────────────────────

fn test_ctx() -> AppContext {
    let mut config = ServerConfig::default();
    // Disable admin auth for unit tests (no keypair available)
    config.base.admin_api.no_auth = Some(true);
    AppContext::new(config, Arc::new(ShutdownCoordinator::new()))
}

fn json_get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .body(Body::empty())
        .unwrap()
}

fn json_post(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .body(Body::empty())
        .unwrap()
}

// ── Health Endpoint ───────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200_ok() {
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_get("/health")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn health_verbose_returns_server_info() {
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_get("/health/verbose")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["draining"], false);
    // inflight is 1 because the current request is tracked by shutdown_guard_middleware
    assert_eq!(json["inflight_requests"], 1);
    assert_eq!(json["service"], "riversd");
    assert!(json["uptime_seconds"].is_number());
}

// ── Gossip Endpoint ───────────────────────────────────────────────

#[tokio::test]
async fn gossip_receive_returns_200() {
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_post("/gossip/receive")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// ── Trace ID Middleware ───────────────────────────────────────────

#[tokio::test]
async fn trace_id_generated_when_not_provided() {
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_get("/health")).await.unwrap();

    let trace_id = response.headers().get("x-trace-id");
    assert!(trace_id.is_some(), "should generate x-trace-id header");
    let trace_str = trace_id.unwrap().to_str().unwrap();
    // Should be a UUID
    assert_eq!(trace_str.len(), 36);
    assert!(trace_str.contains('-'));
}

#[tokio::test]
async fn trace_id_preserved_when_provided() {
    let router = build_main_router(test_ctx());
    let request = Request::builder()
        .uri("/health")
        .header("x-trace-id", "my-custom-trace-123")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    let trace_id = response
        .headers()
        .get("x-trace-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(trace_id, "my-custom-trace-123");
}

// ── Security Headers Middleware ───────────────────────────────────

#[tokio::test]
async fn security_headers_present() {
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_get("/health")).await.unwrap();

    let headers = response.headers();
    assert_eq!(
        headers.get("x-content-type-options").unwrap(),
        &HeaderValue::from_static("nosniff")
    );
    assert_eq!(
        headers.get("x-frame-options").unwrap(),
        &HeaderValue::from_static("DENY")
    );
    assert_eq!(
        headers.get("x-xss-protection").unwrap(),
        &HeaderValue::from_static("1; mode=block")
    );
    assert_eq!(
        headers.get("referrer-policy").unwrap(),
        &HeaderValue::from_static("strict-origin-when-cross-origin")
    );
    // CSP is not injected by default — operator's responsibility
    assert!(headers.get("content-security-policy").is_none());
}

// ── Shutdown Guard Middleware ─────────────────────────────────────

#[tokio::test]
async fn shutdown_guard_rejects_when_draining() {
    let shutdown = Arc::new(ShutdownCoordinator::new());
    shutdown.mark_draining();
    let ctx = AppContext::new(ServerConfig::default(), shutdown);

    let router = build_main_router(ctx);
    let response = router.oneshot(json_get("/health")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["message"], "server is shutting down");
    assert_eq!(json["code"], 503);
}

#[tokio::test]
async fn shutdown_guard_allows_when_not_draining() {
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_get("/health")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// ── ShutdownCoordinator ───────────────────────────────────────────

#[test]
fn shutdown_coordinator_initial_state() {
    let coord = ShutdownCoordinator::new();
    assert!(!coord.is_draining());
    assert_eq!(coord.inflight_count(), 0);
}

#[test]
fn shutdown_coordinator_mark_draining() {
    let coord = ShutdownCoordinator::new();
    coord.mark_draining();
    assert!(coord.is_draining());
}

#[test]
fn shutdown_coordinator_inflight_tracking() {
    let coord = ShutdownCoordinator::new();
    assert_eq!(coord.enter(), 1);
    assert_eq!(coord.enter(), 2);
    assert_eq!(coord.inflight_count(), 2);
    coord.exit();
    assert_eq!(coord.inflight_count(), 1);
    coord.exit();
    assert_eq!(coord.inflight_count(), 0);
}

#[tokio::test]
async fn shutdown_coordinator_drain_completes() {
    let coord = Arc::new(ShutdownCoordinator::new());
    coord.mark_draining();
    // No inflight — drain should complete immediately
    coord.wait_for_drain().await;
    assert_eq!(coord.inflight_count(), 0);
}

#[tokio::test]
async fn shutdown_coordinator_drain_waits_for_inflight() {
    let coord = Arc::new(ShutdownCoordinator::new());
    coord.enter();
    coord.mark_draining();

    let coord2 = coord.clone();
    let drain_handle = tokio::spawn(async move {
        coord2.wait_for_drain().await;
    });

    // Give drain a moment to start waiting
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(!drain_handle.is_finished());

    // Complete the inflight request
    coord.exit();

    // Drain should now complete
    tokio::time::timeout(std::time::Duration::from_secs(1), drain_handle)
        .await
        .expect("drain timed out")
        .expect("drain task panicked");
}

// ── HTTP/2 Config Validation ──────────────────────────────────────

#[tokio::test]
async fn http2_without_tls_rejected() {
    let mut config = ServerConfig::default();
    config.base.http2.enabled = true;
    // No TLS certs set (base.tls is None)

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);

    let result =
        riversd::server::run_server_with_listener_with_control(config, listener, rx).await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    // Accept either the generic TLS-required message or the HTTP/2-specific one
    assert!(
        msg.contains("TLS is required") || msg.contains("HTTP/2 requires TLS"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn tls_config_present_passes_validation() {
    use rivers_runtime::rivers_core::config::TlsConfig;

    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, SELF_SIGNED_CERT).unwrap();
    std::fs::write(&key_path, SELF_SIGNED_KEY).unwrap();

    let mut config = ServerConfig::default();
    config.base.http2.enabled = true;
    config.base.tls = Some(TlsConfig {
        cert: Some(cert_path.to_str().unwrap().to_string()),
        key: Some(key_path.to_str().unwrap().to_string()),
        ..TlsConfig::default()
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Trigger immediate shutdown
    tx.send(true).unwrap();

    // Server should start and shutdown cleanly with valid TLS certs
    let result =
        riversd::server::run_server_with_listener_with_control(config, listener, rx).await;
    assert!(result.is_ok(), "should pass validation: {:?}", result);
}

// ── Admin Server Router ───────────────────────────────────────────

#[tokio::test]
async fn admin_status_returns_200() {
    let router = build_admin_router(test_ctx());
    let response = router.oneshot(json_get("/admin/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_drivers_returns_200() {
    let router = build_admin_router(test_ctx());
    let response = router.oneshot(json_get("/admin/drivers")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_datasources_returns_200() {
    let router = build_admin_router(test_ctx());
    let response = router
        .oneshot(json_get("/admin/datasources"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_datasources_no_bundle_returns_empty() {
    let router = build_admin_router(test_ctx());
    let response = router
        .oneshot(json_get("/admin/datasources"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // No bundle deployed → empty list with note
    assert_eq!(json["count"], 0);
    assert!(json["datasources"].as_array().unwrap().is_empty());
}

// ── Admin Log Endpoints ──────────────────────────────────────────

#[tokio::test]
async fn admin_log_levels_returns_current_level() {
    let router = build_admin_router(test_ctx());
    let response = router.oneshot(json_get("/admin/log/levels")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["levels"]["global"].is_string());
}

#[tokio::test]
async fn admin_log_set_invalid_level_returns_error() {
    let router = build_admin_router(test_ctx());
    let body = serde_json::json!({"target": "global", "level": "nonsense"});
    let request = Request::builder()
        .uri("/admin/log/set")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST); // AV6: proper status code
    let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], 400);
}

#[tokio::test]
async fn admin_log_set_no_controller_returns_unavailable() {
    // test_ctx() has no log_controller
    let router = build_admin_router(test_ctx());
    let body = serde_json::json!({"target": "global", "level": "debug"});
    let request = Request::builder()
        .uri("/admin/log/set")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE); // AV6: proper status code
    let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], 503);
}

#[tokio::test]
async fn admin_log_reset_no_controller_returns_unavailable() {
    let router = build_admin_router(test_ctx());
    let response = router.oneshot(json_post("/admin/log/reset")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE); // AV6: proper status code
    let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], 503);
}

#[tokio::test]
async fn admin_log_set_with_controller_updates_level() {
    use riversd::server::LogController;
    use std::sync::{Arc, Mutex};

    let applied = Arc::new(Mutex::new(String::new()));
    let applied_clone = Arc::clone(&applied);
    let controller = Arc::new(LogController::new("info", move |filter: &str| {
        *applied_clone.lock().unwrap() = filter.to_string();
        Ok(())
    }));

    let mut ctx = test_ctx();
    ctx.log_controller = Some(controller);

    let router = build_admin_router(ctx);
    let body = serde_json::json!({"target": "global", "level": "debug"});
    let request = Request::builder()
        .uri("/admin/log/set")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "updated");
    assert_eq!(*applied.lock().unwrap(), "debug");
}

#[tokio::test]
async fn admin_log_reset_with_controller_resets_to_initial() {
    use riversd::server::LogController;
    use std::sync::{Arc, Mutex};

    let applied = Arc::new(Mutex::new(String::new()));
    let applied_clone = Arc::clone(&applied);
    let controller = Arc::new(LogController::new("warn", move |filter: &str| {
        *applied_clone.lock().unwrap() = filter.to_string();
        Ok(())
    }));

    // Simulate a previous level change
    controller.set("debug").unwrap();

    let mut ctx = test_ctx();
    ctx.log_controller = Some(controller);

    let router = build_admin_router(ctx);
    let response = router.oneshot(json_post("/admin/log/reset")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "reset");
    assert_eq!(*applied.lock().unwrap(), "warn");
}

// ── Full Server Lifecycle ─────────────────────────────────────────

#[tokio::test]
async fn server_starts_and_stops_via_watch_channel() {
    use rivers_runtime::rivers_core::config::TlsConfig;

    // TLS is mandatory — provide cert/key so validation passes.
    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, SELF_SIGNED_CERT).unwrap();
    std::fs::write(&key_path, SELF_SIGNED_KEY).unwrap();

    let mut config = ServerConfig::default();
    config.base.tls = Some(TlsConfig {
        cert: Some(cert_path.to_str().unwrap().to_string()),
        key: Some(key_path.to_str().unwrap().to_string()),
        ..TlsConfig::default()
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let _addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Trigger shutdown immediately so the TLS accept loop exits right away.
    tx.send(true).unwrap();

    let result = riversd::server::run_server_with_listener_with_control(config, listener, rx).await;

    assert!(result.is_ok(), "server should shut down cleanly: {:?}", result);
}

// ── 404 for Unknown Routes ────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let router = build_main_router(test_ctx());
    let response = router
        .oneshot(json_get("/nonexistent"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// Self-signed test certificate and key (EC P-256, CN=localhost).
const SELF_SIGNED_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBfDCCASOgAwIBAgIUGKqgyLPX914To9tCqmTbtV1E5o0wCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMxNTE3MjgwM1oXDTI3MDMxNTE3
MjgwM1owFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0D
AQcDQgAEbr93Pd5EQ/VtOHFZhnTY6DRH7eEO/phoFoZU9Na7iUm45S/raAUOCY9U
HE8ySzI2FG4BHw23i0FyvRUQg3yTVqNTMFEwHQYDVR0OBBYEFPDcwEVTgrDhLWed
0r3IqRFKoTHVMB8GA1UdIwQYMBaAFPDcwEVTgrDhLWed0r3IqRFKoTHVMA8GA1Ud
EwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDRwAwRAIgfdb4OSUr3CvGjmjv2jzbvBwj
LftrRBsea2WRSi1bw+MCIA3SlHBRGSdQvJg6LOnvjaGAnc+z7ddBC3tqLwTgeb1w
-----END CERTIFICATE-----";

const SELF_SIGNED_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg3qxcuMg60U6Xe/Li
7sesr6xvsXBKRKbmy/ULL4Ls+FShRANCAARuv3c93kRD9W04cVmGdNjoNEft4Q7+
mGgWhlT01ruJSbjlL+toBQ4Jj1QcTzJLMjYUbgEfDbeLQXK9FRCDfJNW
-----END PRIVATE KEY-----";

// ── Request Observer Middleware ──────────────────────────────────

#[tokio::test]
async fn request_observer_runs_on_request() {
    // The observer logs via tracing::debug — verify the request completes normally
    // and response still has correct status + trace_id (observer must not break the chain).
    let router = build_main_router(test_ctx());
    let response = router.oneshot(json_get("/health")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // trace_id should still be present (observer sits between timeout and trace_id)
    assert!(response.headers().get("x-trace-id").is_some());
}

// ── Hot Reload State ────────────────────────────────────────────

#[tokio::test]
async fn hot_reload_state_swap_updates_config() {
    use riversd::hot_reload::HotReloadState;

    let config = ServerConfig::default();
    let state = HotReloadState::new(config.clone(), None);
    assert_eq!(state.version(), 0);

    // Swap to a new config
    let mut new_config = config.clone();
    new_config.base.request_timeout_seconds = 99;
    state.swap(new_config).await.unwrap();

    assert_eq!(state.version(), 1);
    let current = state.current_config().await;
    assert_eq!(current.base.request_timeout_seconds, 99);
}
