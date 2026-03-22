use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;

use riversd::admin_auth::{
    build_signing_payload, parse_public_key, validate_timestamp, verify_admin_signature,
};

// ── Signature verification ──────────────────────────────────────

#[test]
fn sign_and_verify_roundtrip() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let method = "POST";
    let path = "/admin/reload";
    let timestamp = "1710000000";
    let body_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    let payload = build_signing_payload(method, path, timestamp, body_hash);
    let signature = signing_key.sign(&payload);

    let result = verify_admin_signature(
        &verifying_key,
        method,
        path,
        timestamp,
        body_hash,
        &signature.to_bytes(),
    );
    assert!(result.is_ok());
}

#[test]
fn verify_with_wrong_key_fails() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();

    let method = "GET";
    let path = "/admin/status";
    let timestamp = "1710000000";
    let body_hash = "abc123";

    let payload = build_signing_payload(method, path, timestamp, body_hash);
    let signature = signing_key.sign(&payload);

    let result = verify_admin_signature(
        &wrong_key,
        method,
        path,
        timestamp,
        body_hash,
        &signature.to_bytes(),
    );
    assert!(result.is_err());
}

#[test]
fn verify_with_tampered_payload_fails() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let method = "POST";
    let path = "/admin/reload";
    let timestamp = "1710000000";
    let body_hash = "original_hash";

    let payload = build_signing_payload(method, path, timestamp, body_hash);
    let signature = signing_key.sign(&payload);

    // Tamper: different path
    let result = verify_admin_signature(
        &verifying_key,
        method,
        "/admin/shutdown", // tampered
        timestamp,
        body_hash,
        &signature.to_bytes(),
    );
    assert!(result.is_err());
}

#[test]
fn invalid_signature_bytes() {
    let verifying_key = SigningKey::generate(&mut OsRng).verifying_key();
    let result = verify_admin_signature(
        &verifying_key,
        "GET",
        "/admin/status",
        "1710000000",
        "hash",
        &[0u8; 10], // wrong length
    );
    assert!(result.is_err());
}

// ── Timestamp validation ────────────────────────────────────────

#[test]
fn fresh_timestamp_passes() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let ts = now.to_string();
    assert!(validate_timestamp(&ts, 300_000).is_ok());
}

#[test]
fn expired_timestamp_fails() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // 10 minutes ago (600_000ms), max age 5 minutes (300_000ms)
    let ts = (now - 600_000).to_string();
    let result = validate_timestamp(&ts, 300_000);
    assert!(result.is_err());
}

#[test]
fn far_future_timestamp_fails() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // 2 minutes in the future (120_000ms) — exceeds 60_000ms future window
    let ts = (now + 120_000).to_string();
    let result = validate_timestamp(&ts, 300_000);
    assert!(result.is_err());
}

#[test]
fn invalid_timestamp_format() {
    assert!(validate_timestamp("not-a-number", 300_000).is_err());
}

// ── Public key parsing ──────────────────────────────────────────

#[test]
fn parse_valid_public_key() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    let hex_key = hex::encode(verifying_key.as_bytes());

    let parsed = parse_public_key(&hex_key).unwrap();
    assert_eq!(parsed.as_bytes(), verifying_key.as_bytes());
}

#[test]
fn parse_invalid_hex() {
    assert!(parse_public_key("zzzz").is_err());
}

#[test]
fn parse_wrong_length() {
    assert!(parse_public_key("abcd").is_err());
}

// ── Signing payload ─────────────────────────────────────────────

#[test]
fn signing_payload_format() {
    let payload = build_signing_payload("POST", "/admin/reload", "12345", "hash");
    assert_eq!(payload, b"POST\n/admin/reload\n12345\nhash");
}
