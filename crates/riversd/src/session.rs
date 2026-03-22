//! Session management — cookie-based sessions backed by StorageEngine.
//!
//! Per `rivers-auth-session-spec.md` §4-8, `rivers-httpd-spec.md` §12.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use rivers_runtime::rivers_core::config::SessionConfig;
use rivers_runtime::rivers_core::storage::StorageEngine;

/// StorageEngine namespace for sessions.
const SESSION_NAMESPACE: &str = "session";

/// A Rivers session.
///
/// Per spec §4.1: session_id, subject, claims, created_at, expires_at, last_seen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub subject: String,
    pub claims: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Session manager — creates, validates, and destroys sessions.
///
/// All session data is stored in StorageEngine under namespace "session",
/// key = session_id.
pub struct SessionManager {
    storage: Arc<dyn StorageEngine>,
    config: SessionConfig,
}

impl SessionManager {
    pub fn new(storage: Arc<dyn StorageEngine>, config: SessionConfig) -> Self {
        Self { storage, config }
    }

    /// Create a new session.
    ///
    /// Per spec §4.1: generate random ID, store in StorageEngine with TTL.
    pub async fn create_session(
        &self,
        subject: String,
        claims: serde_json::Value,
    ) -> Result<Session, SessionError> {
        let session_id = generate_session_id();
        let now = Utc::now();
        let expires_at = now
            + Duration::try_seconds(self.config.ttl_s as i64)
                .unwrap_or_else(|| Duration::seconds(3600));

        let session = Session {
            session_id: session_id.clone(),
            subject,
            claims,
            created_at: now,
            expires_at,
            last_seen: now,
        };

        let value = serde_json::to_vec(&session)
            .map_err(|e| SessionError::Internal(e.to_string()))?;

        let ttl_ms = self.config.ttl_s.saturating_mul(1000);

        self.storage
            .set(SESSION_NAMESPACE, &session_id, value, Some(ttl_ms))
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        Ok(session)
    }

    /// Validate and return a session by ID.
    ///
    /// Per spec §4.2: lookup → check expiry → check idle → update last_seen.
    /// Returns None if session is invalid/expired.
    pub async fn validate_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Session>, SessionError> {
        let data = match self
            .storage
            .get(SESSION_NAMESPACE, session_id)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?
        {
            Some(bytes) => bytes,
            None => return Ok(None),
        };

        let session: Session = serde_json::from_slice(&data)
            .map_err(|e| SessionError::Internal(format!("corrupt session data: {}", e)))?;

        let now = Utc::now();

        // Check absolute expiry (ttl_s from created_at)
        if now > session.expires_at {
            let _ = self.storage.delete(SESSION_NAMESPACE, session_id).await;
            return Ok(None);
        }

        // Check idle timeout (idle_timeout_s from last_seen)
        let idle_limit = session.last_seen
            + Duration::try_seconds(self.config.idle_timeout_s as i64)
                .unwrap_or_else(|| Duration::seconds(1800));
        if now > idle_limit {
            let _ = self.storage.delete(SESSION_NAMESPACE, session_id).await;
            return Ok(None);
        }

        // Update last_seen
        let mut updated = session;
        updated.last_seen = now;
        let value = serde_json::to_vec(&updated)
            .map_err(|e| SessionError::Internal(e.to_string()))?;

        // Rewrite with remaining TTL
        let remaining_ms = (updated.expires_at - now).num_milliseconds().max(0) as u64;
        let _ = self
            .storage
            .set(SESSION_NAMESPACE, session_id, value, Some(remaining_ms))
            .await;

        Ok(Some(updated))
    }

    /// Destroy a session.
    ///
    /// Per spec §4.4: delete from StorageEngine.
    pub async fn destroy_session(&self, session_id: &str) -> Result<(), SessionError> {
        self.storage
            .delete(SESSION_NAMESPACE, session_id)
            .await
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Get the session config.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }
}

/// Build a Set-Cookie header value for a session.
///
/// Per spec §8.1: HttpOnly, SameSite, Path, Secure, Domain.
pub fn build_set_cookie(session_id: &str, config: &SessionConfig) -> String {
    let cookie = &config.cookie;
    let mut parts = vec![format!("{}={}", cookie.name, session_id)];

    if cookie.http_only {
        parts.push("HttpOnly".to_string());
    }
    if cookie.secure {
        parts.push("Secure".to_string());
    }
    parts.push(format!("SameSite={}", cookie.same_site));
    parts.push(format!("Path={}", cookie.path));

    if let Some(ref domain) = cookie.domain {
        if !domain.is_empty() {
            parts.push(format!("Domain={}", domain));
        }
    }

    parts.push(format!("Max-Age={}", config.ttl_s));

    parts.join("; ")
}

/// Build a Set-Cookie header that clears the session cookie.
///
/// Per spec §4.4 and §12.1: clear_cookie → Max-Age=0.
pub fn build_clear_cookie(config: &SessionConfig) -> String {
    let cookie = &config.cookie;
    let mut parts = vec![format!("{}=", cookie.name)];

    parts.push("Max-Age=0".to_string());
    if cookie.http_only {
        parts.push("HttpOnly".to_string());
    }
    if cookie.secure {
        parts.push("Secure".to_string());
    }
    parts.push(format!("SameSite={}", cookie.same_site));
    parts.push(format!("Path={}", cookie.path));

    if let Some(ref domain) = cookie.domain {
        if !domain.is_empty() {
            parts.push(format!("Domain={}", domain));
        }
    }

    parts.join("; ")
}

/// Extract session ID from request cookies or Authorization Bearer header.
///
/// Per spec §8.2: cookie takes precedence over Bearer.
pub fn extract_session_id(
    cookie_header: Option<&str>,
    auth_header: Option<&str>,
    cookie_name: &str,
) -> Option<String> {
    // Try cookie first (takes precedence)
    if let Some(cookies) = cookie_header {
        if let Some(id) = parse_cookie_value(cookies, cookie_name) {
            if !id.is_empty() {
                return Some(id);
            }
        }
    }

    // Fall back to Bearer token
    if let Some(auth) = auth_header {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }

    None
}

/// Parse a specific cookie value from a Cookie header string.
fn parse_cookie_value(cookies: &str, name: &str) -> Option<String> {
    for pair in cookies.split(';') {
        let pair = pair.trim();
        if let Some((key, value)) = pair.split_once('=') {
            if key.trim() == name {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Generate a cryptographically random session ID.
fn generate_session_id() -> String {
    format!("sess_{}", uuid::Uuid::new_v4().as_simple())
}

// ── Cross-App Session Propagation (§7.5) ────────────────────────

/// Extract session claims from the X-Rivers-Claims header (inter-service calls).
///
/// Per spec §7.5: claims carried in X-Rivers-Claims header for cross-app propagation.
pub fn extract_claims_from_header(headers: &std::collections::HashMap<String, String>) -> Option<serde_json::Value> {
    headers.get("x-rivers-claims")
        .and_then(|claims_str| serde_json::from_str(claims_str).ok())
}

/// Build an X-Rivers-Claims header value from a session.
pub fn build_claims_header(session: &serde_json::Value) -> Option<String> {
    if session.is_null() {
        return None;
    }
    serde_json::to_string(session).ok()
}

/// Session management errors.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_claims_from_header_valid() {
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "x-rivers-claims".into(),
            r#"{"username":"alice","groups":["admin"]}"#.into(),
        );
        let claims = extract_claims_from_header(&headers).unwrap();
        assert_eq!(claims["username"], "alice");
    }

    #[test]
    fn extract_claims_from_header_missing() {
        let headers = std::collections::HashMap::new();
        assert!(extract_claims_from_header(&headers).is_none());
    }

    #[test]
    fn extract_claims_from_header_invalid_json() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("x-rivers-claims".into(), "not-valid-json".into());
        assert!(extract_claims_from_header(&headers).is_none());
    }

    #[test]
    fn build_claims_header_from_session() {
        let session = serde_json::json!({"username": "alice"});
        let header = build_claims_header(&session).unwrap();
        assert!(header.contains("alice"));
    }

    #[test]
    fn build_claims_header_null_session() {
        let session = serde_json::Value::Null;
        assert!(build_claims_header(&session).is_none());
    }
}
