//! Integration tests for the guard login -> session cookie -> protected endpoint flow.
//!
//! Tests the security pipeline end-to-end via HTTP through the Axum router:
//! session creation, cookie validation, protected endpoint access, session
//! expiry, and CSRF enforcement.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use rivers_runtime::rivers_core::config::{CsrfConfig, SessionConfig, SessionCookieConfig};
use rivers_runtime::rivers_core::{InMemoryStorageEngine, ServerConfig};
use rivers_runtime::view::ApiViewConfig;
use riversd::csrf::CsrfManager;
use riversd::server::{build_main_router, AppContext};
use riversd::session::{build_set_cookie, SessionManager};
use riversd::shutdown::ShutdownCoordinator;
use riversd::view_engine::ViewRouter;

// ── Helpers ───────────────────────────────────────────────────────

/// Build an AppContext wired with SessionManager and CsrfManager.
fn session_ctx() -> (AppContext, Arc<SessionManager>, Arc<CsrfManager>) {
    let storage = Arc::new(InMemoryStorageEngine::new());

    let session_config = SessionConfig {
        enabled: true,
        ttl_s: 3600,
        idle_timeout_s: 1800,
        cookie: SessionCookieConfig::default(),
        include_token_in_body: false,
        token_body_key: "token".to_string(),
    };
    let session_mgr = Arc::new(SessionManager::new(storage.clone(), session_config));

    let csrf_config = CsrfConfig::default();
    let csrf_mgr = Arc::new(CsrfManager::new(storage.clone(), csrf_config));

    let mut config = ServerConfig::default();
    config.base.admin_api.no_auth = Some(true);

    let mut ctx = AppContext::new(config, Arc::new(ShutdownCoordinator::new()));
    ctx.session_manager = Some(session_mgr.clone());
    ctx.csrf_manager = Some(csrf_mgr.clone());
    ctx.storage_engine = Some(storage);

    (ctx, session_mgr, csrf_mgr)
}

/// Build a protected REST view config (auth = "session", the default).
fn protected_view_config() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "Rest",
        "path": "/api/protected",
        "method": "GET",
        "handler": { "type": "dataview", "dataview": "test_dv" }
    }))
    .expect("valid ApiViewConfig")
}

/// Build a guard view config.
fn guard_view_config() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "Rest",
        "path": "/auth/login",
        "method": "POST",
        "guard": true,
        "handler": { "type": "codecomponent", "language": "javascript", "module": "guard.js", "entrypoint": "handle" }
    }))
    .expect("valid ApiViewConfig")
}

/// Build a public view config (auth = "none").
fn public_view_config() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "Rest",
        "path": "/api/public",
        "method": "GET",
        "auth": "none",
        "handler": { "type": "dataview", "dataview": "public_dv" }
    }))
    .expect("valid ApiViewConfig")
}

/// Build a protected POST view for CSRF testing.
fn protected_post_view_config() -> ApiViewConfig {
    serde_json::from_value(serde_json::json!({
        "view_type": "Rest",
        "path": "/api/submit",
        "method": "POST",
        "handler": { "type": "dataview", "dataview": "submit_dv" }
    }))
    .expect("valid ApiViewConfig")
}

// ── Test 1: Session creation and cookie validation ────────────────

#[tokio::test]
async fn session_create_and_validate_roundtrip() {
    let (_ctx, session_mgr, _csrf_mgr) = session_ctx();

    // Create a session
    let session = session_mgr
        .create_session(
            "testuser".to_string(),
            serde_json::json!({"role": "admin"}),
        )
        .await
        .expect("session creation should succeed");

    assert!(session.session_id.starts_with("sess_"));
    assert_eq!(session.subject, "testuser");

    // Build the Set-Cookie header
    let cookie = build_set_cookie(&session.session_id, session_mgr.config());
    assert!(
        cookie.contains(&format!("rivers_session={}", session.session_id)),
        "cookie should contain session ID: {}",
        cookie
    );
    assert!(cookie.contains("HttpOnly"), "cookie should be HttpOnly");
    assert!(cookie.contains("SameSite=Lax"), "cookie should have SameSite");

    // Parse the session ID back from the cookie
    let session_id = cookie
        .split(';')
        .next()
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap();
    assert_eq!(session_id, session.session_id);

    // Validate the session round-trips
    let validated = session_mgr
        .validate_session(session_id)
        .await
        .expect("validation should not error")
        .expect("session should be valid");
    assert_eq!(validated.subject, "testuser");
    assert_eq!(validated.claims["role"], "admin");
}

// ── Test 2: Protected endpoint returns 401 without session ────────

#[tokio::test]
async fn protected_endpoint_returns_401_without_session() {
    let (ctx, _session_mgr, _csrf_mgr) = session_ctx();

    // Set up views: only a protected view (no guard).
    // Without a guard view, unauthenticated requests get 401 (GuardAction::Reject).
    let mut views = HashMap::new();
    views.insert("protected".to_string(), protected_view_config());

    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/protected")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "protected endpoint without session should return 401"
    );
}

// ── Test 3: Protected endpoint returns 200-range with valid session ──

#[tokio::test]
async fn protected_endpoint_accessible_with_valid_session() {
    let (ctx, session_mgr, _csrf_mgr) = session_ctx();

    let mut views = HashMap::new();
    views.insert("protected".to_string(), protected_view_config());

    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    // Create a session
    let session = session_mgr
        .create_session(
            "alice".to_string(),
            serde_json::json!({"role": "user"}),
        )
        .await
        .expect("session creation should succeed");

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/protected")
        .method("GET")
        .header(
            "cookie",
            format!("rivers_session={}", session.session_id),
        )
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // The view uses a dataview handler which will fail (no DataViewExecutor wired),
    // but it should NOT be 401 — the security pipeline should pass.
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "valid session should not get 401"
    );
    assert_ne!(
        response.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "valid session should not get redirected"
    );
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "GET with valid session should not get 403"
    );
}

// ── Test 4: Expired session is rejected ───────────────────────────

#[tokio::test]
async fn expired_session_rejected() {
    let storage = Arc::new(InMemoryStorageEngine::new());

    // Create a session manager with TTL = 0 (immediately expired)
    let session_config = SessionConfig {
        enabled: true,
        ttl_s: 0,
        idle_timeout_s: 0,
        cookie: SessionCookieConfig::default(),
        include_token_in_body: false,
        token_body_key: "token".to_string(),
    };
    let session_mgr = Arc::new(SessionManager::new(storage.clone(), session_config));
    let csrf_mgr = Arc::new(CsrfManager::new(storage.clone(), CsrfConfig::default()));

    let mut config = ServerConfig::default();
    config.base.admin_api.no_auth = Some(true);
    let mut ctx = AppContext::new(config, Arc::new(ShutdownCoordinator::new()));
    ctx.session_manager = Some(session_mgr.clone());
    ctx.csrf_manager = Some(csrf_mgr);
    ctx.storage_engine = Some(storage);

    // Install a protected view
    let mut views = HashMap::new();
    views.insert("protected".to_string(), protected_view_config());
    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    // Create a session (with TTL=0, it expires immediately)
    let session = session_mgr
        .create_session("bob".to_string(), serde_json::json!({}))
        .await
        .expect("session creation should succeed");

    // Small delay to ensure expiry
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/protected")
        .method("GET")
        .header(
            "cookie",
            format!("rivers_session={}", session.session_id),
        )
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    // Should be rejected — either 401 (no guard configured) or redirect
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "expired session should return 401 (no guard configured)"
    );
}

// ── Test 5: CSRF validation — POST without token returns 403 ──────

#[tokio::test]
async fn csrf_post_without_token_returns_403() {
    let (ctx, session_mgr, csrf_mgr) = session_ctx();

    let mut views = HashMap::new();
    views.insert("submit".to_string(), protected_post_view_config());
    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    // Create a valid session
    let session = session_mgr
        .create_session("charlie".to_string(), serde_json::json!({}))
        .await
        .expect("session creation should succeed");

    // Generate a CSRF token (so the session has one — but we won't send it)
    csrf_mgr
        .generate_token(&session.session_id, 3600)
        .await
        .expect("csrf token generation should succeed");

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/submit")
        .method("POST")
        .header(
            "cookie",
            format!("rivers_session={}", session.session_id),
        )
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "POST without CSRF token should return 403"
    );

    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["message"]
            .as_str()
            .unwrap_or("")
            .contains("CSRF"),
        "error should mention CSRF: {:?}",
        json
    );
}

// ── Test 6: CSRF validation — POST with valid token succeeds ──────

#[tokio::test]
async fn csrf_post_with_valid_token_passes() {
    let (ctx, session_mgr, csrf_mgr) = session_ctx();

    let mut views = HashMap::new();
    views.insert("submit".to_string(), protected_post_view_config());
    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    // Create a valid session
    let session = session_mgr
        .create_session("charlie".to_string(), serde_json::json!({}))
        .await
        .expect("session creation should succeed");

    // Generate CSRF token
    let csrf_token = csrf_mgr
        .generate_token(&session.session_id, 3600)
        .await
        .expect("csrf token generation should succeed");

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/submit")
        .method("POST")
        .header(
            "cookie",
            format!("rivers_session={}", session.session_id),
        )
        .header("x-csrf-token", &csrf_token)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // Should NOT be 403 — CSRF is valid. May be 500 (no DataViewExecutor) but not 403.
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "POST with valid CSRF token should not get 403"
    );
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "POST with valid session should not get 401"
    );
}

// ── Test 7: Protected endpoint with guard redirects to login ──────

#[tokio::test]
async fn protected_endpoint_redirects_to_guard_when_configured() {
    let (mut ctx, _session_mgr, _csrf_mgr) = session_ctx();

    let mut views = HashMap::new();
    views.insert("guard".to_string(), guard_view_config());
    views.insert("protected".to_string(), protected_view_config());

    let detection = riversd::guard::detect_guard_view(&views);
    assert!(detection.errors.is_empty(), "guard detection errors: {:?}", detection.errors);
    ctx.guard_view_id = detection.guard_view_id;

    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/protected")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // With a guard view configured, unauthenticated access should redirect
    assert_eq!(
        response.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "unauthenticated request should redirect to guard view"
    );

    let location = response
        .headers()
        .get("location")
        .expect("redirect should have location header")
        .to_str()
        .unwrap();
    assert_eq!(
        location, "/auth/login",
        "should redirect to guard view path"
    );
}

// ── Test 8: Public view (auth=none) accessible without session ────

#[tokio::test]
async fn public_view_accessible_without_session() {
    let (ctx, _session_mgr, _csrf_mgr) = session_ctx();

    let mut views = HashMap::new();
    views.insert("public".to_string(), public_view_config());
    let router_obj = ViewRouter::from_views(&views);
    {
        let mut vr = ctx.view_router.write().await;
        *vr = Some(router_obj);
    }

    let router = build_main_router(ctx);
    let request = Request::builder()
        .uri("/api/public")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // Public view should NOT return 401 or redirect
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "public view should not return 401"
    );
    assert_ne!(
        response.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "public view should not redirect"
    );
}
