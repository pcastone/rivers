//! CORS and security header tests.

use axum::http::{HeaderMap, Method};
use riversd::cors::*;

// ── CORS Config Validation ────────────────────────────────────────

#[test]
fn cors_wildcard_with_credentials_rejected() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["*".to_string()],
        allow_credentials: true,
        ..Default::default()
    };
    assert!(validate_cors_config(&config).is_err());
}

#[test]
fn cors_wildcard_without_credentials_ok() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["*".to_string()],
        allow_credentials: false,
        ..Default::default()
    };
    assert!(validate_cors_config(&config).is_ok());
}

#[test]
fn cors_specific_origin_with_credentials_ok() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["https://app.example.com".to_string()],
        allow_credentials: true,
        ..Default::default()
    };
    assert!(validate_cors_config(&config).is_ok());
}

// ── CORS Header Resolution ────────────────────────────────────────

#[test]
fn cors_disabled_returns_none() {
    let config = CorsConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(resolve_cors_headers(&config, Some("https://example.com"), None).is_none());
}

#[test]
fn cors_no_origin_returns_none() {
    let config = CorsConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(resolve_cors_headers(&config, None, None).is_none());
}

#[test]
fn cors_wildcard_matches_any_origin() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["*".to_string()],
        ..Default::default()
    };
    let headers = resolve_cors_headers(&config, Some("https://any.com"), None).unwrap();
    assert_eq!(headers.allow_origin, "*");
}

#[test]
fn cors_specific_origin_matches() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["https://app.example.com".to_string()],
        ..Default::default()
    };
    let headers = resolve_cors_headers(
        &config,
        Some("https://app.example.com"),
        None,
    )
    .unwrap();
    assert_eq!(headers.allow_origin, "https://app.example.com");
}

#[test]
fn cors_non_matching_origin_returns_none() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["https://app.example.com".to_string()],
        ..Default::default()
    };
    assert!(resolve_cors_headers(&config, Some("https://evil.com"), None).is_none());
}

#[test]
fn cors_preflight_sets_max_age() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["*".to_string()],
        ..Default::default()
    };
    let headers = resolve_cors_headers(
        &config,
        Some("https://example.com"),
        Some(&Method::OPTIONS),
    )
    .unwrap();
    assert!(headers.is_preflight);
}

#[test]
fn cors_credentials_included_when_enabled() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["https://app.example.com".to_string()],
        allow_credentials: true,
        ..Default::default()
    };
    let headers = resolve_cors_headers(
        &config,
        Some("https://app.example.com"),
        None,
    )
    .unwrap();
    assert!(headers.allow_credentials);
}

// ── CORS Header Application ──────────────────────────────────────

#[test]
fn cors_headers_applied_to_header_map() {
    let config = CorsConfig {
        enabled: true,
        allowed_origins: vec!["*".to_string()],
        allowed_methods: vec!["GET".to_string(), "POST".to_string()],
        allowed_headers: vec!["Content-Type".to_string()],
        allow_credentials: false,
    };
    let headers = resolve_cors_headers(&config, Some("https://x.com"), None).unwrap();

    let mut map = HeaderMap::new();
    headers.apply(&mut map);

    assert_eq!(
        map.get("access-control-allow-origin").unwrap().to_str().unwrap(),
        "*"
    );
    assert_eq!(
        map.get("access-control-allow-methods").unwrap().to_str().unwrap(),
        "GET, POST"
    );
    assert_eq!(
        map.get("access-control-allow-headers").unwrap().to_str().unwrap(),
        "Content-Type"
    );
    assert!(map.get("access-control-allow-credentials").is_none());
}

// ── Header Blocklist ──────────────────────────────────────────────

#[test]
fn blocked_headers_stripped() {
    let mut headers = HeaderMap::new();
    headers.insert("set-cookie", "session=abc".parse().unwrap());
    headers.insert("access-control-allow-origin", "*".parse().unwrap());
    headers.insert("x-custom", "keep-me".parse().unwrap());
    headers.insert("content-type", "application/json".parse().unwrap());

    strip_blocked_headers(&mut headers);

    assert!(headers.get("set-cookie").is_none());
    assert!(headers.get("access-control-allow-origin").is_none());
    assert!(headers.get("x-custom").is_some(), "non-blocked headers preserved");
    assert!(headers.get("content-type").is_some(), "content-type preserved");
}

#[test]
fn all_blocked_headers_in_list() {
    // Verify the blocklist has the expected count from spec
    assert!(BLOCKED_HEADERS.len() >= 18, "should have all blocked headers from spec");
}

#[test]
fn blocked_headers_includes_security_headers() {
    assert!(BLOCKED_HEADERS.contains(&"x-content-type-options"));
    assert!(BLOCKED_HEADERS.contains(&"x-frame-options"));
    assert!(BLOCKED_HEADERS.contains(&"strict-transport-security"));
    assert!(BLOCKED_HEADERS.contains(&"content-security-policy"));
}
