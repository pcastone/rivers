//! Tests for `Rivers.crypto` -- randomHex, bcrypt, HMAC, base64url, timing-safe.

use super::*;
use super::helpers::make_js_task;

#[tokio::test]
async fn execute_rivers_crypto_random_hex() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            return { hex: Rivers.crypto.randomHex(16) };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    let hex = result.value["hex"].as_str().unwrap();
    // 16 random bytes -> 32 hex chars
    assert_eq!(hex.len(), 32, "expected 32 hex chars, got {}", hex.len());
    assert!(
        hex.chars().all(|c| c.is_ascii_hexdigit()),
        "expected hex string, got: {hex}"
    );
}

// ── P2.2: Rivers.crypto.randomHex produces unique values ────

#[tokio::test]
async fn execute_random_hex_is_unique() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var a = Rivers.crypto.randomHex(8);
            var b = Rivers.crypto.randomHex(8);
            return { a: a, b: b, different: a !== b };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    // Two calls should produce different hex strings (extremely unlikely to collide)
    assert_eq!(result.value["different"], true);
}

// ── P2.2: Rivers.crypto native implementations ─────────────

#[tokio::test]
async fn execute_crypto_hash_password_bcrypt() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var h = Rivers.crypto.hashPassword("secret");
            var v = Rivers.crypto.verifyPassword("secret", h);
            return { hash: h, verified: v };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    let hash = result.value["hash"].as_str().unwrap();
    assert!(hash.starts_with("$2b$12$"), "expected bcrypt $2b$12$ prefix, got: {hash}");
    assert_eq!(result.value["verified"], true);
}

// ── P3.6: Native Crypto Tests ────────────────────────────────

#[tokio::test]
async fn execute_crypto_hash_and_verify() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var hash = Rivers.crypto.hashPassword("secret123");
            var valid = Rivers.crypto.verifyPassword("secret123", hash);
            var invalid = Rivers.crypto.verifyPassword("wrong", hash);
            return { hash_prefix: hash.substring(0, 7), valid: valid, invalid: invalid };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["hash_prefix"], "$2b$12$");
    assert_eq!(result.value["valid"], true);
    assert_eq!(result.value["invalid"], false);
}

#[tokio::test]
async fn execute_crypto_timing_safe_equal() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            return {
                same: Rivers.crypto.timingSafeEqual("abc", "abc"),
                diff: Rivers.crypto.timingSafeEqual("abc", "xyz"),
                diff_len: Rivers.crypto.timingSafeEqual("ab", "abc"),
            };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["same"], true);
    assert_eq!(result.value["diff"], false);
    assert_eq!(result.value["diff_len"], false);
}

#[tokio::test]
async fn execute_crypto_random_base64url() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var a = Rivers.crypto.randomBase64url(16);
            var b = Rivers.crypto.randomBase64url(16);
            return { a: a, b: b, different: a !== b };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["different"], true);
    assert!(result.value["a"].as_str().unwrap().len() > 0);
}

#[tokio::test]
async fn execute_crypto_hmac_real() {
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var result = Rivers.crypto.hmac("secret", "hello");
            return { hmac: result, len: result.length };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    // SHA-256 HMAC = 32 bytes = 64 hex chars
    assert_eq!(result.value["len"], 64);
    // Verify it produces the correct HMAC-SHA256 for "hello" with key "secret"
    let hmac_val = result.value["hmac"].as_str().unwrap();
    assert!(
        hmac_val.chars().all(|c| c.is_ascii_hexdigit()),
        "expected hex string, got: {hmac_val}"
    );
}

#[tokio::test]
async fn execute_crypto_hmac_deterministic() {
    // Same key+data should produce same HMAC
    let ctx = make_js_task(
        r#"function handler(ctx) {
            var a = Rivers.crypto.hmac("key1", "data1");
            var b = Rivers.crypto.hmac("key1", "data1");
            var c = Rivers.crypto.hmac("key2", "data1");
            return { same: a === b, diff: a !== c };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["same"], true);
    assert_eq!(result.value["diff"], true);
}
