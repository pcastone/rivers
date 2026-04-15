//! MCP session management — Mcp-Session-Id lifecycle with StorageEngine.

use rivers_runtime::rivers_core::storage::StorageEngine;
use std::sync::Arc;

const MCP_SESSION_NAMESPACE: &str = "mcp";

/// Create a new MCP session and store it in StorageEngine.
pub async fn create_session(
    storage: &Arc<dyn StorageEngine>,
    ttl_seconds: u64,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let data = serde_json::json!({
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    let value = serde_json::to_vec(&data)
        .map_err(|e| format!("session serialize: {e}"))?;
    let ttl_ms = Some(ttl_seconds * 1000);

    storage.set(MCP_SESSION_NAMESPACE, &session_id, value, ttl_ms)
        .await
        .map_err(|e| format!("session create: {e}"))?;

    Ok(session_id)
}

/// Validate an MCP session — returns true if valid, refreshes TTL (sliding expiration).
pub async fn validate_session(
    storage: &Arc<dyn StorageEngine>,
    session_id: &str,
    ttl_seconds: u64,
) -> bool {
    match storage.get(MCP_SESSION_NAMESPACE, session_id).await {
        Ok(Some(data)) => {
            // Refresh TTL (sliding expiration) by re-setting
            let ttl_ms = Some(ttl_seconds * 1000);
            let _ = storage.set(MCP_SESSION_NAMESPACE, session_id, data, ttl_ms).await;
            true
        }
        _ => false,
    }
}
