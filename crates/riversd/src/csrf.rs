//! CSRF protection — double-submit cookie pattern.
//!
//! Per `rivers-auth-session-spec.md` §9.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rivers_runtime::rivers_core::config::CsrfConfig;
use rivers_runtime::rivers_core::storage::StorageEngine;

/// CSRF namespace in StorageEngine.
const CSRF_NAMESPACE: &str = "csrf";

/// CSRF token manager — generates, validates, and rotates CSRF tokens.
///
/// Tokens are stored in StorageEngine under `csrf:{session_id}`.
pub struct CsrfManager {
    storage: Arc<dyn StorageEngine>,
    config: CsrfConfig,
}

/// A CSRF token entry stored in StorageEngine.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CsrfEntry {
    token: String,
    created_at_epoch_s: u64,
}

impl CsrfManager {
    /// Create a new CSRF manager backed by the given storage engine.
    pub fn new(storage: Arc<dyn StorageEngine>, config: CsrfConfig) -> Self {
        Self { storage, config }
    }

    /// Generate a new CSRF token for a session.
    ///
    /// Per spec §9.2: token generated at session creation time, stored under `csrf:{session_id}`.
    pub async fn generate_token(
        &self,
        session_id: &str,
        ttl_s: u64,
    ) -> Result<String, CsrfError> {
        let token = generate_csrf_token();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = CsrfEntry {
            token: token.clone(),
            created_at_epoch_s: now,
        };
        let value =
            serde_json::to_vec(&entry).map_err(|e| CsrfError::Internal(e.to_string()))?;

        self.storage
            .set(
                CSRF_NAMESPACE,
                session_id,
                value,
                Some(ttl_s * 1000), // StorageEngine TTL is in ms
            )
            .await
            .map_err(|e| CsrfError::Storage(e.to_string()))?;

        Ok(token)
    }

    /// Validate a CSRF token for a session.
    ///
    /// Per spec §9.3: compare X-CSRF-Token header value against stored token.
    pub async fn validate_token(
        &self,
        session_id: &str,
        provided_token: &str,
    ) -> Result<bool, CsrfError> {
        let data = match self
            .storage
            .get(CSRF_NAMESPACE, session_id)
            .await
            .map_err(|e| CsrfError::Storage(e.to_string()))?
        {
            Some(bytes) => bytes,
            None => return Ok(false),
        };

        let entry: CsrfEntry =
            serde_json::from_slice(&data).map_err(|e| CsrfError::Internal(e.to_string()))?;

        Ok(constant_time_eq(entry.token.as_bytes(), provided_token.as_bytes()))
    }

    /// Get or rotate the CSRF token for a session.
    ///
    /// Per spec §9.2: token rotated at most once per `csrf_rotation_interval_s`.
    pub async fn get_or_rotate_token(
        &self,
        session_id: &str,
        ttl_s: u64,
    ) -> Result<String, CsrfError> {
        if let Some(data) = self
            .storage
            .get(CSRF_NAMESPACE, session_id)
            .await
            .map_err(|e| CsrfError::Storage(e.to_string()))?
        {
            if let Ok(entry) = serde_json::from_slice::<CsrfEntry>(&data) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let age = now.saturating_sub(entry.created_at_epoch_s);
                if age < self.config.csrf_rotation_interval_s {
                    // Not time to rotate yet
                    return Ok(entry.token);
                }
            }
        }

        // Generate new token (rotate)
        self.generate_token(session_id, ttl_s).await
    }

    /// Delete the CSRF token for a session.
    ///
    /// Per spec §9.2: on session destroy, delete csrf entry.
    pub async fn delete_token(&self, session_id: &str) -> Result<(), CsrfError> {
        self.storage
            .delete(CSRF_NAMESPACE, session_id)
            .await
            .map_err(|e| CsrfError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Get the CSRF config.
    pub fn config(&self) -> &CsrfConfig {
        &self.config
    }
}

/// Build the CSRF cookie header value.
///
/// Per spec §9.1: NOT HttpOnly (readable by JavaScript), SameSite=Lax, Path=/.
pub fn build_csrf_cookie(token: &str, config: &CsrfConfig) -> String {
    format!(
        "{}={}; SameSite=Lax; Path=/",
        config.cookie_name, token
    )
}

/// Check if a request method is state-mutating (requires CSRF validation).
///
/// Per spec §9.3: POST, PUT, PATCH, DELETE are state-mutating.
pub fn is_state_mutating_method(method: &str) -> bool {
    matches!(
        method.to_uppercase().as_str(),
        "POST" | "PUT" | "PATCH" | "DELETE"
    )
}

/// Check if a request is exempt from CSRF validation.
///
/// Per spec §9.3 validation matrix.
pub fn is_csrf_exempt(method: &str, auth_mode: Option<&str>, has_bearer: bool) -> bool {
    // Safe methods are exempt
    if !is_state_mutating_method(method) {
        return true;
    }

    // Bearer token sessions are exempt
    if has_bearer {
        return true;
    }

    // auth = "none" views are exempt
    if let Some(auth) = auth_mode {
        if auth == "none" {
            return true;
        }
    }

    false
}

/// Generate a cryptographically random CSRF token.
fn generate_csrf_token() -> String {
    uuid::Uuid::new_v4().as_simple().to_string()
}

/// Constant-time comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// CSRF errors.
#[derive(Debug, thiserror::Error)]
pub enum CsrfError {
    /// StorageEngine read/write failure.
    #[error("storage error: {0}")]
    Storage(String),

    /// Internal serialization or logic error.
    #[error("internal error: {0}")]
    Internal(String),
}
