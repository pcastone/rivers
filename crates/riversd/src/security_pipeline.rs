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
    app_id: &str,
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
        // P0-1 fail-closed: a protected view MUST have a session manager.
        // If `session_manager` is None (misconfig), the original code silently
        // skipped validation and let the request through. Now we reject.
        if ctx.session_manager.is_none() {
            tracing::error!(
                trace_id = %trace_id,
                view_type = %config.view_type,
                method = %method,
                "protected view dispatched without session_manager configured \u{2014} rejecting (fail-closed)"
            );
            return Err(
                error_response::internal_error(
                    "session manager not configured; protected view cannot be served"
                )
                .with_trace_id(trace_id.to_string())
                .into_axum_response()
            );
        }
        if let Some(ref mgr) = ctx.session_manager {
            match &session_id {
                Some(sid) => {
                    match mgr.validate_session(sid).await {
                        Ok(Some(sess)) => {
                            session = serde_json::to_value(&sess).ok();

                            // Fire on_session_valid lifecycle hook (fire-and-forget)
                            if let Some(ref hooks) = config.guard_config.as_ref().and_then(|gc| gc.lifecycle_hooks.as_ref()) {
                                if let Some(ref hook) = hooks.on_session_valid {
                                    let pool = ctx.pool.clone();
                                    let hook = hook.clone();
                                    let session_clone = session.clone();
                                    let trace = trace_id.to_string();
                                    let app_id_owned = app_id.to_string();
                                    tokio::spawn(async move {
                                        let entrypoint = crate::process_pool::Entrypoint {
                                            module: hook.module.clone(),
                                            function: hook.entrypoint.clone(),
                                            language: "javascript".to_string(),
                                        };
                                        let args = serde_json::json!({ "session": session_clone });
                                        let builder = crate::process_pool::TaskContextBuilder::new()
                                            .entrypoint(entrypoint)
                                            .args(args)
                                            .trace_id(trace);
                                        let builder = crate::task_enrichment::enrich(
                                            builder,
                                            &app_id_owned,
                                            rivers_runtime::process_pool::TaskKind::SecurityHook,
                                        );
                                        if let Ok(task_ctx) = builder.build() {
                                            let _ = pool.dispatch("default", task_ctx).await;
                                        }
                                    });
                                }
                            }
                        }
                        _ => {
                            clear_session_cookie = true;

                            // Fire on_invalid_session lifecycle hook (fire-and-forget)
                            if let Some(ref hooks) = config.guard_config.as_ref().and_then(|gc| gc.lifecycle_hooks.as_ref()) {
                                if let Some(ref hook) = hooks.on_invalid_session {
                                    let pool = ctx.pool.clone();
                                    let hook = hook.clone();
                                    let trace = trace_id.to_string();
                                    let sid_clone = sid.clone();
                                    let app_id_owned = app_id.to_string();
                                    tokio::spawn(async move {
                                        let entrypoint = crate::process_pool::Entrypoint {
                                            module: hook.module.clone(),
                                            function: hook.entrypoint.clone(),
                                            language: "javascript".to_string(),
                                        };
                                        let args = serde_json::json!({ "session_id": sid_clone });
                                        let builder = crate::process_pool::TaskContextBuilder::new()
                                            .entrypoint(entrypoint)
                                            .args(args)
                                            .trace_id(trace);
                                        let builder = crate::task_enrichment::enrich(
                                            builder,
                                            &app_id_owned,
                                            rivers_runtime::process_pool::TaskKind::SecurityHook,
                                        );
                                        if let Ok(task_ctx) = builder.build() {
                                            let _ = pool.dispatch("default", task_ctx).await;
                                        }
                                    });
                                }
                            }
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

// ── Named guard pre-flight (CB-P1.10 Plan H) ─────────────────────────

/// Run a per-view named guard's codecomponent before the view's
/// primary handler dispatches.
///
/// Honoured uniformly on every view type that flows through
/// `view_dispatch_handler` — REST, streaming REST, MCP, WebSocket, SSE.
/// The guard handler receives a `ParsedRequest` with the original
/// method, path, and headers; the view's body has not been consumed
/// yet (auth-shape decisions only).
///
/// Returns `Ok(())` if the guard returned `{ allow: true }`. Returns
/// `Err(Response)` with HTTP 401 when the guard rejected, the named
/// view is missing at runtime (X014 should have caught this; defensive
/// fallback), the named view is not a codecomponent, or the guard
/// dispatcher itself errored.
pub async fn run_named_guard_preflight(
    ctx: &crate::server::AppContext,
    app_entry_point: &str,
    path_params: &HashMap<String, String>,
    method: String,
    path: String,
    headers_map: HashMap<String, String>,
    guard_view_name: &str,
) -> Result<(), axum::response::Response> {
    use rivers_runtime::view::HandlerConfig;
    use crate::error_response;

    let dv_namespace = app_entry_point;
    let entrypoint = {
        let bundle = match ctx.loaded_bundle.as_ref() {
            Some(b) => b,
            None => {
                tracing::error!(
                    guard_view = %guard_view_name,
                    "named guard pre-flight: no bundle loaded"
                );
                return Err(error_response::internal_error(
                    "named guard pre-flight: bundle not loaded",
                )
                .into_axum_response()
                .into_response());
            }
        };
        let app = bundle.apps.iter().find(|a| {
            a.manifest.entry_point.as_deref() == Some(dv_namespace)
                || a.manifest.app_entry_point.as_deref() == Some(dv_namespace)
        });
        let Some(app) = app else {
            return Err(error_response::internal_error(
                "named guard pre-flight: app not found in bundle",
            )
            .into_axum_response()
            .into_response());
        };
        let Some(view_config) = app.config.api.views.get(guard_view_name) else {
            tracing::error!(
                guard_view = %guard_view_name,
                app = %dv_namespace,
                "named guard pre-flight: guard view not found at runtime",
            );
            return Err(error_response::unauthorized("named guard not configured")
                .into_axum_response()
                .into_response());
        };
        match &view_config.handler {
            HandlerConfig::Codecomponent { language, module, entrypoint, .. } => {
                crate::process_pool::Entrypoint {
                    language: language.clone(),
                    module: module.clone(),
                    function: entrypoint.clone(),
                }
            }
            _ => {
                tracing::error!(
                    guard_view = %guard_view_name,
                    "named guard pre-flight: target is not a codecomponent (X014 should have caught this)",
                );
                return Err(error_response::unauthorized("named guard misconfigured")
                    .into_axum_response()
                    .into_response());
            }
        }
    };

    let parsed = crate::view_engine::ParsedRequest {
        method,
        path,
        query_params: HashMap::new(),
        query_all: HashMap::new(),
        headers: headers_map,
        body: serde_json::Value::Null,
        path_params: path_params.clone(),
    };

    let trace_id = uuid::Uuid::new_v4().to_string();
    match crate::guard::execute_guard_handler(
        &ctx.pool,
        &entrypoint,
        &parsed,
        None,
        &trace_id,
        dv_namespace,
    )
    .await
    {
        Ok(result) if result.allow => Ok(()),
        Ok(_) => {
            tracing::info!(
                guard_view = %guard_view_name,
                "named guard pre-flight rejected request"
            );
            Err(error_response::unauthorized("guard rejected the request")
                .with_trace_id(trace_id)
                .into_axum_response()
                .into_response())
        }
        Err(e) => {
            tracing::error!(
                guard_view = %guard_view_name,
                error = %e,
                "named guard pre-flight dispatch failed"
            );
            Err(error_response::unauthorized("guard dispatch failed")
                .with_trace_id(trace_id)
                .into_axum_response()
                .into_response())
        }
    }
}
