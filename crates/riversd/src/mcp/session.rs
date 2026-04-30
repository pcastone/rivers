//! MCP session management — Mcp-Session-Id lifecycle with StorageEngine.

use rivers_runtime::rivers_core::storage::StorageEngine;
use std::sync::Arc;

const MCP_SESSION_NAMESPACE: &str = "mcp";

/// Create a new MCP session and store it in StorageEngine.
///
/// `auth_context` is the caller identity resolved at `initialize` time (e.g. parsed
/// Bearer token). It is stored in the session payload and returned on every subsequent
/// `validate_session` call so handlers receive it as `ctx.session`.
pub async fn create_session(
    storage: &Arc<dyn StorageEngine>,
    ttl_seconds: u64,
    auth_context: Option<serde_json::Value>,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut data = serde_json::json!({
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(auth) = auth_context {
        data["auth"] = auth;
    }
    let value = serde_json::to_vec(&data)
        .map_err(|e| format!("session serialize: {e}"))?;
    let ttl_ms = Some(ttl_seconds * 1000);

    storage.set(MCP_SESSION_NAMESPACE, &session_id, value, ttl_ms)
        .await
        .map_err(|e| format!("session create: {e}"))?;

    Ok(session_id)
}

/// Validate an MCP session — returns the stored session payload if valid, refreshes TTL.
///
/// Returns `None` when the session does not exist or has expired.
/// The returned value contains at minimum `{"created_at": "..."}` and optionally
/// `{"auth": {"kind": "bearer", "token": "<key>"}}` when auth was established at init.
pub async fn validate_session(
    storage: &Arc<dyn StorageEngine>,
    session_id: &str,
    ttl_seconds: u64,
) -> Option<serde_json::Value> {
    match storage.get(MCP_SESSION_NAMESPACE, session_id).await {
        Ok(Some(data)) => {
            // Refresh TTL (sliding expiration) by re-setting
            let ttl_ms = Some(ttl_seconds * 1000);
            let _ = storage.set(MCP_SESSION_NAMESPACE, session_id, data.clone(), ttl_ms).await;
            serde_json::from_slice::<serde_json::Value>(&data).ok()
        }
        _ => None,
    }
}

/// Parse an HTTP `Authorization` header into an auth context value.
///
/// `Bearer <token>` → `{"kind": "bearer", "token": "<token>"}`
/// Other schemes   → `{"kind": "raw", "value": "<full-header>"}`
/// Missing header  → `None`
pub fn parse_auth_header(header: Option<&str>) -> Option<serde_json::Value> {
    let header = header?;
    if let Some(token) = header.strip_prefix("Bearer ").or_else(|| header.strip_prefix("bearer ")) {
        Some(serde_json::json!({ "kind": "bearer", "token": token }))
    } else {
        Some(serde_json::json!({ "kind": "raw", "value": header }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;

    fn storage() -> Arc<dyn StorageEngine> {
        Arc::new(InMemoryStorageEngine::new())
    }

    #[tokio::test]
    async fn create_and_validate_without_auth() {
        let s = storage();
        let sid = create_session(&s, 3600, None).await.unwrap();
        let data = validate_session(&s, &sid, 3600).await.unwrap();
        assert!(data.get("created_at").is_some());
        assert!(data.get("auth").is_none());
    }

    #[tokio::test]
    async fn create_and_validate_with_bearer_auth() {
        let s = storage();
        let auth = parse_auth_header(Some("Bearer my-api-key-abc"));
        let sid = create_session(&s, 3600, auth).await.unwrap();
        let data = validate_session(&s, &sid, 3600).await.unwrap();
        let auth_obj = data.get("auth").unwrap();
        assert_eq!(auth_obj["kind"], "bearer");
        assert_eq!(auth_obj["token"], "my-api-key-abc");
    }

    #[tokio::test]
    async fn validate_missing_session_returns_none() {
        let s = storage();
        let result = validate_session(&s, "nonexistent-id", 3600).await;
        assert!(result.is_none());
    }

    #[test]
    fn parse_auth_header_bearer() {
        let v = parse_auth_header(Some("Bearer tok123")).unwrap();
        assert_eq!(v["kind"], "bearer");
        assert_eq!(v["token"], "tok123");
    }

    #[test]
    fn parse_auth_header_bearer_lowercase() {
        let v = parse_auth_header(Some("bearer tok456")).unwrap();
        assert_eq!(v["kind"], "bearer");
        assert_eq!(v["token"], "tok456");
    }

    #[test]
    fn parse_auth_header_other_scheme() {
        let v = parse_auth_header(Some("Basic dXNlcjpwYXNz")).unwrap();
        assert_eq!(v["kind"], "raw");
        assert!(v["value"].as_str().unwrap().contains("Basic"));
    }

    #[test]
    fn parse_auth_header_missing() {
        assert!(parse_auth_header(None).is_none());
    }
}
