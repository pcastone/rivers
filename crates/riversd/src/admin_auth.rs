//! Ed25519 admin API authentication.
//!
//! Per `rivers-auth-session-spec.md` §6: admin requests are authenticated
//! via Ed25519 signature over method + path + timestamp + body hash.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

// ── Error Types ─────────────────────────────────────────────────

/// Admin authentication errors.
#[derive(Debug, thiserror::Error)]
pub enum AdminAuthError {
    #[error("invalid public key: {0}")]
    InvalidPublicKey(String),

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    #[error("signature verification failed")]
    VerificationFailed,

    #[error("timestamp expired: {0}")]
    TimestampExpired(String),

    #[error("invalid timestamp format: {0}")]
    InvalidTimestamp(String),
}

// ── Public API ──────────────────────────────────────────────────

/// Build the signing payload for admin requests.
///
/// Per spec §6: the signature covers `method + path + timestamp + body_hash`,
/// concatenated with newline separators.
pub fn build_signing_payload(
    method: &str,
    path: &str,
    timestamp: &str,
    body_hash: &str,
) -> Vec<u8> {
    format!("{}\n{}\n{}\n{}", method, path, timestamp, body_hash).into_bytes()
}

/// Verify an Ed25519 signature on an admin API request.
///
/// Per spec §6: the signature covers method + path + timestamp + body hash.
pub fn verify_admin_signature(
    public_key: &VerifyingKey,
    method: &str,
    path: &str,
    timestamp: &str,
    body_hash: &str,
    signature: &[u8],
) -> Result<(), AdminAuthError> {
    let sig = Signature::from_slice(signature)
        .map_err(|e| AdminAuthError::InvalidSignature(e.to_string()))?;

    let payload = build_signing_payload(method, path, timestamp, body_hash);

    public_key
        .verify(&payload, &sig)
        .map_err(|_| AdminAuthError::VerificationFailed)
}

/// Parse a hex-encoded Ed25519 public key.
pub fn parse_public_key(hex_key: &str) -> Result<VerifyingKey, AdminAuthError> {
    let bytes = hex::decode(hex_key)
        .map_err(|e| AdminAuthError::InvalidPublicKey(format!("invalid hex: {}", e)))?;

    if bytes.len() != 32 {
        return Err(AdminAuthError::InvalidPublicKey(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&bytes);

    VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| AdminAuthError::InvalidPublicKey(e.to_string()))
}

/// Validate timestamp freshness (reject if older than `max_age_ms`).
///
/// Expects `timestamp` as a Unix epoch **milliseconds** string (e.g. "1710000000000").
pub fn validate_timestamp(
    timestamp: &str,
    max_age_ms: u64,
) -> Result<(), AdminAuthError> {
    let ts: u64 = timestamp
        .parse()
        .map_err(|e| AdminAuthError::InvalidTimestamp(format!("not a valid integer: {}", e)))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let age = now.saturating_sub(ts);
    if age > max_age_ms {
        return Err(AdminAuthError::TimestampExpired(format!(
            "timestamp is {}ms old, max allowed is {}ms",
            age, max_age_ms
        )));
    }

    // Also reject timestamps in the future (by more than 60_000 ms)
    if ts > now + 60_000 {
        return Err(AdminAuthError::TimestampExpired(format!(
            "timestamp is {}ms in the future",
            ts - now
        )));
    }

    Ok(())
}
