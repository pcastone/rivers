//! Security pipeline extracted from view_dispatch_handler (AN13.3).
//!
//! Runs session validation, guard redirect, and CSRF checks in order.
//! Returns `Ok(SecurityOutcome)` on success or `Err(Response)` on failure.

use std::collections::HashMap;

use axum::response::IntoResponse;
use rivers_runtime::view::ApiViewConfig;

use crate::error_response;
use crate::server::AppContext;

/// Outcome of a successful security pipeline run.
pub struct SecurityOutcome {
    /// Resolved session data (if any).
    pub session: Option<serde_json::Value>,
    /// Session ID extracted from cookie/header.
    pub session_id: Option<String>,
    /// Whether to clear the session cookie (invalid session detected).
    pub clear_cookie: bool,
}

/// Run the security pipeline: session validation -> guard redirect -> CSRF.
///
/// **CRITICAL:** The check order is: session validation, guard redirect, CSRF.
///
/// On success: returns `Ok(SecurityOutcome)` with session data.
/// On failure (redirect, 401, 403): returns `Err(Response)` to be returned immediately.
pub async fn run_security_pipeline(
    ctx: &AppContext,
    config: &ApiViewConfig,
    headers: &HashMap<String, String>,
    method: &str,
    trace_id: &str,
    guard_view_path: Option<&str>,
) -> Result<SecurityOutcome, axum::response::Response> {
    let mut session: Option<serde_json::Value> = None;
    let mut clear_session_cookie = false;

    // ── Step 1: Extract session ID ──────────────────────────
    let cookie_name = &ctx.config.security.session.cookie.name;
    let session_id = crate::session::extract_session_id(
        headers.get("cookie").map(|s| s.as_str()),
        headers.get("authorization").map(|s| s.as_str()),
        cookie_name,
    );

    // ── Step 2: Session validation for protected views ──────
    if !crate::guard::is_public_view(config) {
        if let Some(ref mgr) = ctx.session_manager {
            match &session_id {
                Some(sid) => {
                    match mgr.validate_session(sid).await {
                        Ok(Some(sess)) => {
                            session = serde_json::to_value(&sess).ok();
                        }
                        _ => {
                            clear_session_cookie = true;
                            // Fall through to guard redirect
                        }
                    }
                }
                None => {} // No session — fall through to guard redirect
            }

            // If no valid session on a protected view, redirect or reject
            if session.is_none() {
                let on_auth = config.guard_config.as_ref()
                    .and_then(|gc| gc.valid_session_url.as_deref());
                let action = crate::guard::resolve_guard_redirect(
                    config.guard,
                    false,
                    guard_view_path,
                    on_auth,
                );
                match action {
                    crate::guard::GuardAction::Redirect(url) => {
                        let mut resp = axum::response::Redirect::temporary(&url).into_response();
                        if clear_session_cookie {
                            resp.headers_mut().insert(
                                axum::http::header::SET_COOKIE,
                                crate::session::build_clear_cookie(&ctx.config.security.session)
                                    .parse().unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
                            );
                        }
                        return Err(resp);
                    }
                    crate::guard::GuardAction::RedirectToGuard(path) => {
                        let mut resp = axum::response::Redirect::temporary(&path).into_response();
                        if clear_session_cookie {
                            resp.headers_mut().insert(
                                axum::http::header::SET_COOKIE,
                                crate::session::build_clear_cookie(&ctx.config.security.session)
                                    .parse().unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
                            );
                        }
                        return Err(resp);
                    }
                    crate::guard::GuardAction::Reject => {
                        return Err(
                            error_response::unauthorized("authentication required")
                                .with_trace_id(trace_id.to_string())
                                .into_axum_response()
                        );
                    }
                    crate::guard::GuardAction::Allow => {} // Guard view itself
                }
            }
        }
    }

    // For guard views with a valid session, check for redirect
    if config.guard && session.is_some() {
        let on_auth = config.guard_config.as_ref()
            .and_then(|gc| gc.valid_session_url.as_deref());
        let action = crate::guard::resolve_guard_redirect(
            true,
            true,
            guard_view_path,
            on_auth,
        );
        if let crate::guard::GuardAction::Redirect(url) = action {
            return Err(axum::response::Redirect::temporary(&url).into_response());
        }
    }

    // ── Step 3: CSRF validation on mutating requests ────────
    if let (Some(ref csrf_mgr), Some(ref sid)) = (&ctx.csrf_manager, &session_id) {
        if session.is_some() {
            let has_bearer = headers.get("authorization")
                .map(|v| v.starts_with("Bearer ")).unwrap_or(false);
            if !crate::csrf::is_csrf_exempt(method, config.auth.as_deref(), has_bearer) {
                let csrf_header = ctx.config.security.csrf.header_name.to_lowercase();
                let csrf_token = headers.get(&csrf_header)
                    .or(headers.get("x-csrf-token"))
                    .map(|s| s.as_str())
                    .unwrap_or("");
                match csrf_mgr.validate_token(sid, csrf_token).await {
                    Ok(true) => {}
                    _ => {
                        return Err(
                            error_response::forbidden("CSRF validation failed")
                                .with_trace_id(trace_id.to_string())
                                .into_axum_response()
                        );
                    }
                }
            }
        }
    }

    Ok(SecurityOutcome {
        session,
        session_id,
        clear_cookie: clear_session_cookie,
    })
}
