//! Guard view detection and validation and CodeComponent handler dispatch.
//!
//! Per `rivers-auth-session-spec.md` §3.

use std::collections::HashMap;

use rivers_runtime::view::{ApiViewConfig, HandlerStageConfig};

use crate::process_pool::{
    Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError,
};
use crate::view_engine::ParsedRequest;

// ── Guard CodeComponent Result ────────────────────────────────

/// Result of executing the guard view's CodeComponent handler.
///
/// Per spec §3: the guard handler returns allow/redirect/reject.
#[derive(Debug, Clone)]
pub struct GuardResult {
    /// Whether to allow the request through.
    pub allow: bool,
    /// Optional redirect URL (for redirect responses).
    pub redirect_url: Option<String>,
    /// Optional session claims to set on the session.
    pub session_claims: Option<serde_json::Value>,
}

// ── Guard Error ─────────────────────────────────────────────

/// Errors from guard handler execution.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    /// The guard handler returned an error.
    #[error("guard handler error: {0}")]
    HandlerError(String),

    /// Failed to dispatch to the ProcessPool.
    #[error("guard dispatch error: {0}")]
    DispatchError(#[from] TaskError),

    /// The guard handler returned an unparseable result.
    #[error("invalid guard result: {0}")]
    InvalidResult(String),
}

// ── Guard CodeComponent Dispatch (D1) ───────────────────────

/// Execute the guard view's CodeComponent handler.
///
/// Builds a TaskContext with the request and session data, dispatches to the
/// ProcessPool, and parses the result into a GuardResult.
///
/// Per spec §3: guard handler receives the request and session, returns
/// an object with `allow`, optional `redirect_url`, and optional `session_claims`.
pub async fn execute_guard_handler(
    pool: &ProcessPoolManager,
    entrypoint: &Entrypoint,
    request: &ParsedRequest,
    session: Option<&serde_json::Value>,
    trace_id: &str,
) -> Result<GuardResult, GuardError> {
    let args = serde_json::json!({
        "request": {
            "method": request.method,
            "path": request.path,
            "query_params": request.query_params,
            "headers": request.headers,
            "body": request.body,
            "path_params": request.path_params,
        },
        "session": session,
    });

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint.clone())
        .args(args)
        .trace_id(trace_id.to_string());
    let builder = crate::task_enrichment::enrich(builder, "");
    let ctx = builder
        .build()
        .map_err(|e| GuardError::DispatchError(e))?;

    let result = pool.dispatch("default", ctx).await?;

    // Parse the TaskResult value into a GuardResult
    parse_guard_result(&result.value)
}

/// Parse a JSON value from the guard handler into a GuardResult.
fn parse_guard_result(value: &serde_json::Value) -> Result<GuardResult, GuardError> {
    let allow = value
        .get("allow")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let redirect_url = value
        .get("redirect_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let session_claims = value.get("session_claims").cloned();

    Ok(GuardResult {
        allow,
        redirect_url,
        session_claims,
    })
}

// ── Guard on_failed Handler (D2) ────────────────────────────

/// Execute the guard's on_failed CodeComponent callback.
///
/// Called when the guard handler itself fails (e.g., ProcessPool error).
/// Returns an optional custom error page HTML string.
pub async fn execute_guard_on_failed(
    pool: &ProcessPoolManager,
    entrypoint: &Entrypoint,
    error: &str,
    trace_id: &str,
) -> Result<Option<String>, GuardError> {
    let args = serde_json::json!({
        "error": error,
    });

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint.clone())
        .args(args)
        .trace_id(trace_id.to_string());
    let builder = crate::task_enrichment::enrich(builder, "");
    let ctx = builder
        .build()
        .map_err(|e| GuardError::DispatchError(e))?;

    let result = pool.dispatch("default", ctx).await?;

    // The on_failed handler may return a custom error page as a string
    let html = result
        .value
        .get("html")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(html)
}

/// Result of scanning views for guard configuration.
#[derive(Debug)]
pub struct GuardDetection {
    /// The view ID of the guard view, if found.
    pub guard_view_id: Option<String>,

    /// Validation errors encountered.
    pub errors: Vec<String>,
}

/// Scan all views and detect/validate the guard view.
///
/// Per spec §3.1: only one guard view may exist per server.
/// Per spec §10: config validation rejects a second guard declaration.
pub fn detect_guard_view(views: &HashMap<String, ApiViewConfig>) -> GuardDetection {
    let mut guard_view_id = None;
    let mut errors = Vec::new();

    for (id, config) in views {
        if config.guard {
            if let Some(ref existing) = guard_view_id {
                errors.push(format!(
                    "only one guard view is allowed per server: '{}' conflicts with '{}'",
                    id, existing
                ));
            } else {
                guard_view_id = Some(id.clone());
            }

            // Guard must use CodeComponent handler
            if matches!(config.handler, rivers_runtime::view::HandlerConfig::Dataview { .. }) {
                errors.push(format!(
                    "guard view '{}' must use a codecomponent handler, not dataview",
                    id
                ));
            }

            // Guard must have a path and method
            if config.path.is_none() {
                errors.push(format!("guard view '{}' must declare a path", id));
            }
        }
    }

    GuardDetection {
        guard_view_id,
        errors,
    }
}

/// Determine guard redirect for a request.
///
/// Per spec §3.3-3.4, §5.7:
/// - User hits guard view with valid session → redirect to `on_authenticated` URL (default "/")
/// - User hits protected view without session → redirect to guard view path
/// - User hits protected view with invalid session → redirect to guard view path + clear cookie
pub fn resolve_guard_redirect(
    is_guard_view: bool,
    has_valid_session: bool,
    guard_path: Option<&str>,
    on_authenticated: Option<&str>,
) -> GuardAction {
    if is_guard_view && has_valid_session {
        // Already logged in, redirect away from login page
        GuardAction::Redirect(on_authenticated.unwrap_or("/").to_string())
    } else if !is_guard_view && !has_valid_session {
        // Not authenticated, redirect to guard (login) view
        match guard_path {
            Some(path) => GuardAction::RedirectToGuard(path.to_string()),
            None => GuardAction::Reject, // No guard configured — 401
        }
    } else {
        GuardAction::Allow
    }
}

/// Action to take based on guard logic.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardAction {
    /// Allow the request through.
    Allow,
    /// Redirect to a URL (e.g., authenticated user hitting login page).
    Redirect(String),
    /// Redirect to the guard view (unauthenticated user hitting protected view).
    RedirectToGuard(String),
    /// Reject with 401 (no guard configured).
    Reject,
}

/// Check if a view is public (auth = "none").
///
/// Per spec §5.1: all views are protected by default.
/// Guard view is implicitly public.
pub fn is_public_view(config: &ApiViewConfig) -> bool {
    if config.guard {
        return true; // Guard is implicitly public
    }

    if let Some(ref auth) = config.auth {
        return auth == "none";
    }

    // MessageConsumer views are auto-exempt per spec §5.4
    if config.view_type == "MessageConsumer" {
        return true;
    }

    false // Protected by default
}

// ── Guard Lifecycle Hooks ────────────────────────────────────

/// Guard lifecycle hooks — all optional, all side-effects only.
///
/// Per technology-path-spec §9.5: hooks cannot influence auth flow.
/// TOML is authoritative for all routing decisions.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GuardLifecycleHooks {
    /// Fires when session already exists and is valid.
    pub on_session_valid: Option<HandlerStageConfig>,

    /// Fires on invalid or expired session.
    pub on_invalid_session: Option<HandlerStageConfig>,

    /// Fires on credential validation failure.
    pub on_failed: Option<HandlerStageConfig>,
}

/// Execute a guard lifecycle hook (fire-and-forget, side effects only).
///
/// Returns Ok(()) regardless of handler outcome — hooks cannot influence auth flow.
pub async fn execute_guard_lifecycle_hook(
    pool: &ProcessPoolManager,
    hook: &HandlerStageConfig,
    session: Option<&serde_json::Value>,
    request: Option<&ParsedRequest>,
    trace_id: &str,
) -> Result<(), GuardError> {
    let entrypoint = Entrypoint {
        module: hook.module.clone(),
        function: hook.entrypoint.clone(),
        language: "javascript".to_string(),
    };

    let args = serde_json::json!({
        "session": session,
        "request": request.map(|r| serde_json::json!({
            "method": r.method,
            "path": r.path,
        })),
    });

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(trace_id.to_string());
    let builder = crate::task_enrichment::enrich(builder, "");
    let ctx = builder
        .build()
        .map_err(|e| GuardError::DispatchError(e))?;

    // Fire and forget — result is ignored per spec
    let _ = pool.dispatch("default", ctx).await;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_guard_result_allow() {
        let value = serde_json::json!({
            "allow": true,
            "session_claims": {"user_id": "u-123"},
        });
        let result = parse_guard_result(&value).unwrap();
        assert!(result.allow);
        assert!(result.redirect_url.is_none());
        assert!(result.session_claims.is_some());
    }

    #[test]
    fn test_parse_guard_result_reject_with_redirect() {
        let value = serde_json::json!({
            "allow": false,
            "redirect_url": "/login",
        });
        let result = parse_guard_result(&value).unwrap();
        assert!(!result.allow);
        assert_eq!(result.redirect_url, Some("/login".to_string()));
        assert!(result.session_claims.is_none());
    }

    #[test]
    fn test_parse_guard_result_defaults_to_reject() {
        let value = serde_json::json!({});
        let result = parse_guard_result(&value).unwrap();
        assert!(!result.allow);
    }

    #[tokio::test]
    async fn test_execute_guard_handler_engine_unavailable() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "guard.js".to_string(),
            function: "handle".to_string(),
            language: "javascript".to_string(),
        };
        let request = ParsedRequest::new("POST", "/auth/login");
        let result = execute_guard_handler(&pool, &entrypoint, &request, None, "trace-1").await;
        // Should fail with EngineUnavailable since stub workers are used
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_guard_on_failed_engine_unavailable() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "guard.js".to_string(),
            function: "on_failed".to_string(),
            language: "javascript".to_string(),
        };
        let result = execute_guard_on_failed(&pool, &entrypoint, "some error", "trace-1").await;
        assert!(result.is_err());
    }
}
