use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::rivers_core::config::CsrfConfig;
use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;
use rivers_runtime::view::{ApiViewConfig, HandlerConfig};
use riversd::csrf::{
    build_csrf_cookie, is_csrf_exempt, is_state_mutating_method, CsrfManager,
};
use riversd::guard::{detect_guard_view, is_public_view, resolve_guard_redirect, GuardAction};

// ── Helper ───────────────────────────────────────────────────────

fn make_view(view_type: &str, guard: bool, auth: Option<&str>) -> ApiViewConfig {
    ApiViewConfig {
        view_type: view_type.to_string(),
        path: Some("/test".to_string()),
        method: Some("POST".to_string()),
        handler: HandlerConfig::Codecomponent {
            language: "typescript".to_string(),
            module: "auth.ts".to_string(),
            entrypoint: "authenticate".to_string(),
            resources: vec![],
        },
        handlers: None,
        max_body_mb: None,
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard,
        auth: auth.map(|s| s.to_string()),
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
        event_handlers: None,
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

fn make_dataview_view() -> ApiViewConfig {
    ApiViewConfig {
        view_type: "Rest".to_string(),
        path: Some("/api/data".to_string()),
        method: Some("GET".to_string()),
        handler: HandlerConfig::Dataview {
            dataview: "my_view".to_string(),
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
        event_handlers: None,
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

fn default_csrf_config() -> CsrfConfig {
    CsrfConfig::default()
}

fn make_csrf_manager(config: CsrfConfig) -> CsrfManager {
    let storage = Arc::new(InMemoryStorageEngine::new());
    CsrfManager::new(storage, config)
}

// ── Guard Detection ──────────────────────────────────────────────

#[test]
fn detect_single_guard() {
    let mut views = HashMap::new();
    views.insert("auth".to_string(), make_view("Rest", true, None));
    views.insert("contacts".to_string(), make_view("Rest", false, None));

    let result = detect_guard_view(&views);
    assert_eq!(result.guard_view_id, Some("auth".to_string()));
    assert!(result.errors.is_empty());
}

#[test]
fn detect_no_guard() {
    let mut views = HashMap::new();
    views.insert("contacts".to_string(), make_view("Rest", false, None));

    let result = detect_guard_view(&views);
    assert!(result.guard_view_id.is_none());
    assert!(result.errors.is_empty());
}

#[test]
fn detect_multiple_guards_rejected() {
    let mut views = HashMap::new();
    views.insert("auth1".to_string(), make_view("Rest", true, None));
    views.insert("auth2".to_string(), make_view("Rest", true, None));

    let result = detect_guard_view(&views);
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].contains("only one guard view is allowed"));
}

#[test]
fn guard_with_dataview_handler_rejected() {
    let mut views = HashMap::new();
    let mut guard = make_dataview_view();
    guard.guard = true;
    views.insert("auth".to_string(), guard);

    let result = detect_guard_view(&views);
    assert!(result.errors.iter().any(|e| e.contains("codecomponent")));
}

// ── Public View Detection ────────────────────────────────────────

#[test]
fn guard_view_is_public() {
    let view = make_view("Rest", true, None);
    assert!(is_public_view(&view));
}

#[test]
fn auth_none_is_public() {
    let view = make_view("Rest", false, Some("none"));
    assert!(is_public_view(&view));
}

#[test]
fn message_consumer_is_public() {
    let view = make_view("MessageConsumer", false, None);
    assert!(is_public_view(&view));
}

#[test]
fn default_view_is_protected() {
    let view = make_view("Rest", false, None);
    assert!(!is_public_view(&view));
}

#[test]
fn auth_session_is_protected() {
    let view = make_view("Rest", false, Some("session"));
    assert!(!is_public_view(&view));
}

// ── CSRF Token Manager ──────────────────────────────────────────

#[tokio::test]
async fn generate_and_validate_token() {
    let mgr = make_csrf_manager(default_csrf_config());
    let token = mgr.generate_token("sess_abc", 3600).await.unwrap();

    assert!(mgr.validate_token("sess_abc", &token).await.unwrap());
}

#[tokio::test]
async fn validate_wrong_token_fails() {
    let mgr = make_csrf_manager(default_csrf_config());
    let _token = mgr.generate_token("sess_abc", 3600).await.unwrap();

    assert!(!mgr.validate_token("sess_abc", "wrong_token").await.unwrap());
}

#[tokio::test]
async fn validate_missing_session_fails() {
    let mgr = make_csrf_manager(default_csrf_config());

    assert!(!mgr.validate_token("nonexistent", "any").await.unwrap());
}

#[tokio::test]
async fn delete_token_removes_it() {
    let mgr = make_csrf_manager(default_csrf_config());
    let token = mgr.generate_token("sess_abc", 3600).await.unwrap();

    mgr.delete_token("sess_abc").await.unwrap();

    assert!(!mgr.validate_token("sess_abc", &token).await.unwrap());
}

#[tokio::test]
async fn get_or_rotate_returns_same_within_interval() {
    let mgr = make_csrf_manager(default_csrf_config());
    let token1 = mgr.get_or_rotate_token("sess_abc", 3600).await.unwrap();
    let token2 = mgr.get_or_rotate_token("sess_abc", 3600).await.unwrap();

    assert_eq!(token1, token2, "token should not rotate within interval");
}

#[tokio::test]
async fn get_or_rotate_rotates_after_interval() {
    let config = CsrfConfig {
        csrf_rotation_interval_s: 0, // rotate immediately
        ..default_csrf_config()
    };
    let mgr = make_csrf_manager(config);

    let token1 = mgr.get_or_rotate_token("sess_abc", 3600).await.unwrap();
    // Brief pause to ensure time moves
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let token2 = mgr.get_or_rotate_token("sess_abc", 3600).await.unwrap();

    assert_ne!(token1, token2, "token should rotate after interval");
}

// ── CSRF Cookie ──────────────────────────────────────────────────

#[test]
fn csrf_cookie_is_not_httponly() {
    let config = default_csrf_config();
    let cookie = build_csrf_cookie("token123", &config);

    assert!(cookie.contains("rivers_csrf=token123"));
    assert!(!cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("Path=/"));
}

// ── State-Mutating Methods ───────────────────────────────────────

#[test]
fn post_is_state_mutating() {
    assert!(is_state_mutating_method("POST"));
    assert!(is_state_mutating_method("PUT"));
    assert!(is_state_mutating_method("PATCH"));
    assert!(is_state_mutating_method("DELETE"));
}

#[test]
fn safe_methods_not_state_mutating() {
    assert!(!is_state_mutating_method("GET"));
    assert!(!is_state_mutating_method("HEAD"));
    assert!(!is_state_mutating_method("OPTIONS"));
}

// ── CSRF Exemption Rules ─────────────────────────────────────────

#[test]
fn safe_method_exempt() {
    assert!(is_csrf_exempt("GET", None, false));
}

#[test]
fn bearer_token_exempt() {
    assert!(is_csrf_exempt("POST", None, true));
}

#[test]
fn auth_none_exempt() {
    assert!(is_csrf_exempt("POST", Some("none"), false));
}

#[test]
fn state_mutating_cookie_session_not_exempt() {
    assert!(!is_csrf_exempt("POST", Some("session"), false));
    assert!(!is_csrf_exempt("DELETE", None, false));
}

// ── Guard Redirect Logic ───────────────────────────────────────

#[test]
fn guard_view_with_valid_session_redirects_away() {
    let action = resolve_guard_redirect(true, true, Some("/login"), Some("/dashboard"));
    assert_eq!(action, GuardAction::Redirect("/dashboard".to_string()));
}

#[test]
fn guard_view_with_valid_session_defaults_to_root() {
    let action = resolve_guard_redirect(true, true, Some("/login"), None);
    assert_eq!(action, GuardAction::Redirect("/".to_string()));
}

#[test]
fn protected_view_without_session_redirects_to_guard() {
    let action = resolve_guard_redirect(false, false, Some("/login"), None);
    assert_eq!(action, GuardAction::RedirectToGuard("/login".to_string()));
}

#[test]
fn protected_view_without_session_rejects_when_no_guard() {
    let action = resolve_guard_redirect(false, false, None, None);
    assert_eq!(action, GuardAction::Reject);
}

#[test]
fn protected_view_with_valid_session_allowed() {
    let action = resolve_guard_redirect(false, true, Some("/login"), None);
    assert_eq!(action, GuardAction::Allow);
}

#[test]
fn guard_view_without_session_allowed() {
    let action = resolve_guard_redirect(true, false, Some("/login"), None);
    assert_eq!(action, GuardAction::Allow);
}
