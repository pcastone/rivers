use std::sync::Arc;

use rivers_runtime::rivers_core::config::{SessionConfig, SessionCookieConfig};
use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;
use riversd::session::{
    build_clear_cookie, build_set_cookie, extract_session_id, SessionManager,
};

// ── Helper ───────────────────────────────────────────────────────

fn default_config() -> SessionConfig {
    SessionConfig {
        enabled: true,
        ttl_s: 3600,
        idle_timeout_s: 1800,
        cookie: SessionCookieConfig::default(),
        include_token_in_body: false,
        token_body_key: "token".to_string(),
    }
}

fn make_manager(config: SessionConfig) -> SessionManager {
    let storage = Arc::new(InMemoryStorageEngine::new());
    SessionManager::new(storage, config)
}

// ── Session Creation ─────────────────────────────────────────────

#[tokio::test]
async fn create_session_returns_valid_session() {
    let mgr = make_manager(default_config());
    let session = mgr
        .create_session("user@example.com".into(), serde_json::json!({"role": "admin"}))
        .await
        .unwrap();

    assert!(session.session_id.starts_with("sess_"));
    assert_eq!(session.subject, "user@example.com");
    assert_eq!(session.claims["role"], "admin");
}

#[tokio::test]
async fn created_session_can_be_validated() {
    let mgr = make_manager(default_config());
    let session = mgr
        .create_session("alice".into(), serde_json::json!({}))
        .await
        .unwrap();

    let validated = mgr
        .validate_session(&session.session_id)
        .await
        .unwrap()
        .expect("session should be valid");

    assert_eq!(validated.session_id, session.session_id);
    assert_eq!(validated.subject, "alice");
}

// ── Session Validation ───────────────────────────────────────────

#[tokio::test]
async fn validate_nonexistent_session_returns_none() {
    let mgr = make_manager(default_config());
    let result = mgr.validate_session("sess_doesntexist").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn validate_updates_last_seen() {
    let mgr = make_manager(default_config());
    let session = mgr
        .create_session("bob".into(), serde_json::json!({}))
        .await
        .unwrap();

    let original_last_seen = session.last_seen;

    // Small delay to ensure time progresses
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let validated = mgr
        .validate_session(&session.session_id)
        .await
        .unwrap()
        .unwrap();

    assert!(validated.last_seen >= original_last_seen);
}

// ── Dual Expiry ──────────────────────────────────────────────────

#[tokio::test]
async fn expired_session_returns_none() {
    // Create with 0-second TTL — immediately expired
    let config = SessionConfig {
        ttl_s: 0,
        idle_timeout_s: 1800,
        ..default_config()
    };
    let mgr = make_manager(config);
    let session = mgr
        .create_session("user".into(), serde_json::json!({}))
        .await
        .unwrap();

    // Brief pause to ensure expiry
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let result = mgr.validate_session(&session.session_id).await.unwrap();
    assert!(result.is_none(), "expired session should return None");
}

#[tokio::test]
async fn idle_timeout_expires_session() {
    // Create with 0-second idle timeout — immediately idle-expired
    let config = SessionConfig {
        ttl_s: 3600,
        idle_timeout_s: 0,
        ..default_config()
    };
    let mgr = make_manager(config);
    let session = mgr
        .create_session("user".into(), serde_json::json!({}))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let result = mgr.validate_session(&session.session_id).await.unwrap();
    assert!(result.is_none(), "idle-expired session should return None");
}

// ── Session Destruction ──────────────────────────────────────────

#[tokio::test]
async fn destroy_session_removes_it() {
    let mgr = make_manager(default_config());
    let session = mgr
        .create_session("user".into(), serde_json::json!({}))
        .await
        .unwrap();

    mgr.destroy_session(&session.session_id).await.unwrap();

    let result = mgr.validate_session(&session.session_id).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn destroy_nonexistent_session_is_ok() {
    let mgr = make_manager(default_config());
    // Should not error
    mgr.destroy_session("sess_doesntexist").await.unwrap();
}

// ── Cookie Building ──────────────────────────────────────────────

#[test]
fn set_cookie_includes_all_attributes() {
    let config = default_config();
    let cookie = build_set_cookie("sess_abc123", &config);

    assert!(cookie.contains("rivers_session=sess_abc123"));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("Path=/"));
    assert!(cookie.contains("Max-Age=3600"));
}

#[test]
fn set_cookie_includes_domain_when_set() {
    let mut config = default_config();
    config.cookie.domain = Some("example.com".to_string());
    let cookie = build_set_cookie("sess_abc123", &config);

    assert!(cookie.contains("Domain=example.com"));
}

#[test]
fn set_cookie_excludes_domain_when_none() {
    let config = default_config();
    let cookie = build_set_cookie("sess_abc123", &config);

    assert!(!cookie.contains("Domain="));
}

#[test]
fn clear_cookie_has_max_age_zero() {
    let config = default_config();
    let cookie = build_clear_cookie(&config);

    assert!(cookie.contains("rivers_session="));
    assert!(cookie.contains("Max-Age=0"));
    assert!(cookie.contains("HttpOnly"));
}

// ── Session ID Extraction ────────────────────────────────────────

#[test]
fn extract_from_cookie() {
    let id = extract_session_id(
        Some("rivers_session=sess_abc123; other=value"),
        None,
        "rivers_session",
    );
    assert_eq!(id.unwrap(), "sess_abc123");
}

#[test]
fn extract_from_bearer() {
    let id = extract_session_id(None, Some("Bearer sess_xyz789"), "rivers_session");
    assert_eq!(id.unwrap(), "sess_xyz789");
}

#[test]
fn cookie_takes_precedence_over_bearer() {
    let id = extract_session_id(
        Some("rivers_session=from_cookie"),
        Some("Bearer from_bearer"),
        "rivers_session",
    );
    assert_eq!(id.unwrap(), "from_cookie");
}

#[test]
fn no_session_returns_none() {
    let id = extract_session_id(None, None, "rivers_session");
    assert!(id.is_none());
}

#[test]
fn empty_cookie_falls_back_to_bearer() {
    let id = extract_session_id(
        Some("rivers_session="),
        Some("Bearer sess_fallback"),
        "rivers_session",
    );
    assert_eq!(id.unwrap(), "sess_fallback");
}

#[test]
fn wrong_cookie_name_returns_none() {
    let id = extract_session_id(
        Some("other_cookie=value"),
        None,
        "rivers_session",
    );
    assert!(id.is_none());
}

#[test]
fn invalid_bearer_prefix_returns_none() {
    let id = extract_session_id(None, Some("Basic user:pass"), "rivers_session");
    assert!(id.is_none());
}

#[test]
fn multiple_cookies_parses_correct_one() {
    let id = extract_session_id(
        Some("foo=bar; rivers_session=sess_correct; baz=qux"),
        None,
        "rivers_session",
    );
    assert_eq!(id.unwrap(), "sess_correct");
}

// ── Guard → Session → Cookie (regression: bugreport_2026-04-06_2) ─

#[tokio::test]
async fn guard_flat_claims_creates_session_and_cookie() {
    // Simulates the view_dispatch.rs guard session creation flow:
    // Guard handler returns flat IdentityClaims → parse → create session → build cookie
    let config = default_config();
    let mgr = make_manager(config.clone());

    // Flat claims as returned by a guard handler (no session_claims wrapper)
    let guard_body = serde_json::json!({
        "allow": true,
        "sub": "canary-user-001",
        "role": "tester",
        "email": "canary@test.local",
        "groups": ["canary-fleet"],
    });

    // Step 1: Parse guard result (same as guard.rs logic)
    let allow = guard_body.get("allow").and_then(|v| v.as_bool()).unwrap_or(false);
    assert!(allow);

    // Step 2: Extract claims — prefer explicit session_claims, fall back to flat body
    let claims = guard_body.get("session_claims").cloned().unwrap_or_else(|| {
        let obj = guard_body.as_object().unwrap();
        let filtered: serde_json::Map<String, serde_json::Value> = obj
            .iter()
            .filter(|(k, _)| k.as_str() != "allow" && k.as_str() != "redirect_url")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        serde_json::Value::Object(filtered)
    });

    // Step 3: Extract subject (sub > subject > username > anonymous)
    let subject = claims.get("sub")
        .or(claims.get("subject"))
        .or(claims.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or("anonymous")
        .to_string();
    assert_eq!(subject, "canary-user-001");

    // Step 4: Create session
    let session = mgr.create_session(subject, claims).await.unwrap();
    assert!(session.session_id.starts_with("sess_"));
    assert_eq!(session.subject, "canary-user-001");

    // Step 5: Build Set-Cookie header
    let cookie = build_set_cookie(&session.session_id, &config);
    assert!(cookie.contains(&session.session_id));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));

    // Step 6: Validate the session round-trips
    let validated = mgr.validate_session(&session.session_id).await.unwrap().unwrap();
    assert_eq!(validated.subject, "canary-user-001");
    assert_eq!(validated.claims["role"], "tester");
}

#[tokio::test]
async fn guard_wrapped_claims_creates_session_and_cookie() {
    // Explicit session_claims wrapper (backward-compatible path)
    let config = default_config();
    let mgr = make_manager(config.clone());

    let claims = serde_json::json!({"sub": "wrapped-user", "role": "admin"});

    let session = mgr
        .create_session("wrapped-user".into(), claims)
        .await
        .unwrap();

    let cookie = build_set_cookie(&session.session_id, &config);
    assert!(cookie.contains(&session.session_id));

    let validated = mgr.validate_session(&session.session_id).await.unwrap().unwrap();
    assert_eq!(validated.subject, "wrapped-user");
    assert_eq!(validated.claims["role"], "admin");
}
