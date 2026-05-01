//! Session propagation tests — inter-app header forwarding (spec §7.5).
//!
//! Verifies that the cross-app session propagation helpers in `riversd::session`
//! correctly build and parse the `X-Rivers-Claims` header used when `app-main`
//! proxies requests to `app-service`.
//!
//! These are compile-only unit tests. No running server is required.

use std::collections::HashMap;

use riversd::session::{build_claims_header, extract_claims_from_header};

// ── Authorization Header Forwarding ──────────────────────────────────────────

/// When app-main proxies to app-service it must forward the Authorization
/// header verbatim. This test confirms that a bearer token round-trips through
/// a claims JSON envelope without mutation.
///
/// TODO: once an explicit `forward_authorization_header` helper ships (the
/// HTTP datasource proxy layer), replace this skeleton with a direct call to
/// that function. For now we verify the primitive that produces the forwarded
/// value.
#[test]
fn authorization_header_value_survives_claims_round_trip() {
    // The Authorization header value that app-main received from the client.
    let bearer = "Bearer eyJhbGciOiJIUzI1NiJ9.test.sig";

    // Pack the token into the claims envelope that will be forwarded.
    let claims = serde_json::json!({
        "authorization": bearer,
        "username": "alice",
        "groups": ["admin"],
    });

    let header_value = build_claims_header(&claims)
        .expect("non-null claims must produce a header value");

    // Downstream service parses the header and can recover the authorization.
    let mut headers = HashMap::new();
    headers.insert("x-rivers-claims".into(), header_value);

    let recovered = extract_claims_from_header(&headers)
        .expect("valid JSON header must parse");

    assert_eq!(
        recovered["authorization"].as_str().unwrap(),
        bearer,
        "Authorization value must be preserved across the X-Rivers-Claims boundary"
    );
}

// ── X-Rivers-Claims Header Carries Claims ────────────────────────────────────

/// The X-Rivers-Claims header must carry all session claims when app-main
/// calls app-service. Verifies subject, role, and arbitrary claim fields.
#[test]
fn x_rivers_claims_header_carries_all_session_claims() {
    let session_claims = serde_json::json!({
        "username": "carol",
        "role": "editor",
        "tenant_id": "acme",
        "groups": ["editors", "viewers"],
    });

    let header_value = build_claims_header(&session_claims)
        .expect("build_claims_header must succeed for a non-null session");

    // Simulate the downstream service receiving the header.
    let mut headers = HashMap::new();
    headers.insert("x-rivers-claims".into(), header_value);

    let claims = extract_claims_from_header(&headers)
        .expect("downstream must be able to parse the X-Rivers-Claims header");

    assert_eq!(claims["username"], "carol", "username claim must propagate");
    assert_eq!(claims["role"], "editor", "role claim must propagate");
    assert_eq!(claims["tenant_id"], "acme", "tenant_id claim must propagate");

    let groups = claims["groups"].as_array().expect("groups must be an array");
    assert!(
        groups.iter().any(|g| g == "editors"),
        "group membership must propagate"
    );
}

/// A null session must NOT produce an X-Rivers-Claims header. Forwarding a
/// null session across app boundaries would leak an unauthenticated context
/// where authentication is expected.
#[test]
fn null_session_does_not_produce_claims_header() {
    let null_session = serde_json::Value::Null;
    let result = build_claims_header(&null_session);
    assert!(
        result.is_none(),
        "build_claims_header must return None for a null session to prevent \
         accidental unauthenticated cross-app calls"
    );
}

// ── Session Scope Preserved Across App Boundaries ────────────────────────────

/// When app-service receives X-Rivers-Claims it must be able to reconstruct
/// a claims object with the same scope as the originating session. This test
/// validates that all scope-bearing fields (subject, groups, custom attributes)
/// survive the encode → transmit → decode cycle.
#[test]
fn session_scope_preserved_across_encode_decode_cycle() {
    let original = serde_json::json!({
        "sub": "dave",
        "scope": "read:contacts write:contacts",
        "exp": 9999999999_u64,
        "custom": { "department": "engineering", "level": 3 },
    });

    let encoded = build_claims_header(&original)
        .expect("non-null session must encode");

    let mut headers = HashMap::new();
    headers.insert("x-rivers-claims".into(), encoded);

    let decoded = extract_claims_from_header(&headers)
        .expect("encoded claims must decode");

    assert_eq!(decoded["sub"], original["sub"], "subject must be preserved");
    assert_eq!(decoded["scope"], original["scope"], "scope must be preserved");
    assert_eq!(
        decoded["custom"]["department"], original["custom"]["department"],
        "nested custom fields must be preserved"
    );
    assert_eq!(
        decoded["custom"]["level"], original["custom"]["level"],
        "nested numeric fields must be preserved"
    );
}

/// An absent X-Rivers-Claims header must result in `None` — the service must
/// not treat a missing header as an empty or anonymous session.
#[test]
fn missing_x_rivers_claims_header_returns_none() {
    let headers: HashMap<String, String> = HashMap::new();
    let result = extract_claims_from_header(&headers);
    assert!(
        result.is_none(),
        "missing X-Rivers-Claims must return None, not an empty session"
    );
}

/// A malformed (non-JSON) X-Rivers-Claims header must be rejected gracefully.
/// The service must not panic and must not treat garbage bytes as valid claims.
#[test]
fn malformed_x_rivers_claims_header_is_rejected() {
    let mut headers = HashMap::new();
    headers.insert(
        "x-rivers-claims".into(),
        "not-json-at-all {{{}}}".into(),
    );
    let result = extract_claims_from_header(&headers);
    assert!(
        result.is_none(),
        "malformed X-Rivers-Claims must return None, not propagate invalid claims"
    );
}
