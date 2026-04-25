//! Guard view detection and validation and CodeComponent handler dispatch.
//!
//! Per `rivers-auth-session-spec.md` §3.

use std::collections::HashMap;

use rivers_runtime::view::ApiViewConfig;

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
    app_id: &str,
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
    let builder = crate::task_enrichment::enrich(
        builder,
        app_id,
        rivers_runtime::process_pool::TaskKind::SecurityHook,
    );
    let ctx = builder
        .build()
        .map_err(|e| GuardError::DispatchError(e))?;

    let result = pool.dispatch("default", ctx).await?;

    // Parse the TaskResult value into a GuardResult
    parse_guard_result(&result.value)
}

/// Parse a JSON value from the guard handler into a GuardResult.
///
/// Supports two return shapes per spec §3:
/// 1. Wrapped: `{ allow: true, session_claims: { sub: "...", ... } }`
/// 2. Flat: `{ allow: true, sub: "...", role: "..." }` — claims are the body itself
///
/// For shape 2, the body (minus control keys) becomes `session_claims`.
fn parse_guard_result(value: &serde_json::Value) -> Result<GuardResult, GuardError> {
    let allow = value
        .get("allow")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let redirect_url = value
        .get("redirect_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Prefer explicit session_claims key; fall back to flat body as claims
    let session_claims = if let Some(explicit) = value.get("session_claims") {
        Some(explicit.clone())
    } else if allow {
        // Flat claims: the handler returned IdentityClaims directly.
        // Strip control keys (allow, redirect_url) — the rest are claims.
        if let Some(obj) = value.as_object() {
            let claims: serde_json::Map<String, serde_json::Value> = obj
                .iter()
                .filter(|(k, _)| k.as_str() != "allow" && k.as_str() != "redirect_url")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if claims.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(claims))
            }
        } else {
            None
        }
    } else {
        None
    };

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
    app_id: &str,
) -> Result<Option<String>, GuardError> {
    let args = serde_json::json!({
        "error": error,
    });

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint.clone())
        .args(args)
        .trace_id(trace_id.to_string());
    let builder = crate::task_enrichment::enrich(
        builder,
        app_id,
        rivers_runtime::process_pool::TaskKind::SecurityHook,
    );
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
        let result = execute_guard_handler(&pool, &entrypoint, &request, None, "trace-1", "test-app").await;
        // Should fail with EngineUnavailable since stub workers are used
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_guard_result_flat_claims_extracted() {
        // Regression: bugreport_2026-04-06_2
        // Guard handler returns flat IdentityClaims (no session_claims wrapper).
        // parse_guard_result should extract the body as session_claims.
        let value = serde_json::json!({
            "allow": true,
            "sub": "canary-user-001",
            "role": "tester",
            "email": "canary@test.local",
            "groups": ["canary-fleet"],
        });
        let result = parse_guard_result(&value).unwrap();
        assert!(result.allow);
        let claims = result.session_claims.expect("flat claims should become session_claims");
        assert_eq!(claims["sub"], "canary-user-001");
        assert_eq!(claims["role"], "tester");
        assert_eq!(claims["email"], "canary@test.local");
        // "allow" should be stripped from claims
        assert!(claims.get("allow").is_none(), "control key 'allow' should be stripped");
    }

    #[test]
    fn test_parse_guard_result_flat_claims_not_extracted_on_reject() {
        // When allow=false, flat body should NOT become session_claims
        let value = serde_json::json!({
            "allow": false,
            "sub": "attacker",
        });
        let result = parse_guard_result(&value).unwrap();
        assert!(!result.allow);
        assert!(result.session_claims.is_none(), "rejected guard should not produce session_claims");
    }

    #[test]
    fn test_parse_guard_result_explicit_session_claims_preferred() {
        // When both explicit session_claims AND flat claims exist,
        // the explicit key takes precedence.
        let value = serde_json::json!({
            "allow": true,
            "sub": "flat-user",
            "session_claims": {"sub": "explicit-user", "role": "admin"},
        });
        let result = parse_guard_result(&value).unwrap();
        let claims = result.session_claims.unwrap();
        assert_eq!(claims["sub"], "explicit-user", "explicit session_claims should win");
    }

    #[test]
    fn test_parse_guard_result_allow_only_no_claims() {
        // Guard returns just {allow: true} with no identity info
        let value = serde_json::json!({"allow": true});
        let result = parse_guard_result(&value).unwrap();
        assert!(result.allow);
        assert!(result.session_claims.is_none(), "no claims to extract");
    }

    #[tokio::test]
    async fn test_execute_guard_on_failed_engine_unavailable() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let entrypoint = Entrypoint {
            module: "guard.js".to_string(),
            function: "on_failed".to_string(),
            language: "javascript".to_string(),
        };
        let result = execute_guard_on_failed(&pool, &entrypoint, "some error", "trace-1", "test-app").await;
        assert!(result.is_err());
    }
}
