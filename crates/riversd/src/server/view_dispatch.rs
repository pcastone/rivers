//! View dispatch — route matching, REST view execution, query parsing.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::response::IntoResponse;

use crate::error_response;
use crate::view_engine;

use super::context::AppContext;
use super::handlers::{static_file_handler, services_discovery_handler};
use super::streaming::{
    execute_sse_view,
    execute_streaming_rest_view,
    execute_ws_view,
};

// ── View Dispatch ─────────────────────────────────────────────────

/// Pre-matched route data extracted under a single RwLock acquisition.
///
/// Eliminates the double-lock pattern where `combined_fallback_handler`
/// matched the route, dropped the lock, then `view_dispatch_handler`
/// re-acquired the lock and re-matched.
pub(super) struct MatchedRoute {
    pub config: rivers_runtime::view::ApiViewConfig,
    pub app_entry_point: String,
    /// Stable appId UUID from the app manifest.
    pub app_id: String,
    pub path_params: HashMap<String, String>,
    pub guard_view_path: Option<String>,
    /// View ID from the router — needed by SSE/WS/Polling to look up per-route managers.
    pub view_id: String,
}

/// Combined fallback: tries view routes first, then static files.
///
/// Per spec §3: route registration order is views before static.
/// Views are matched dynamically via `ViewRouter` against loaded app config.
/// Unmatched requests fall through to static file serving.
pub(super) async fn combined_fallback_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> axum::response::Response {
    let path = request.uri().path().to_string();

    // Phase 1: Check if any registered view matches this request.
    // Single RwLock acquire — extract all needed data in one pass (AN11.1).
    let method = request.method().to_string();
    let matched: Option<MatchedRoute> = {
        let router_guard = ctx.view_router.read().await;
        if let Some(ref router) = *router_guard {
            if let Some((route, path_params)) = router.match_route(&method, &path) {
                let config = route.config.clone();
                let app_entry_point = route.app_entry_point.clone();
                let guard_view_path = ctx.guard_view_id.as_ref().and_then(|gid| {
                    router.routes().iter()
                        .find(|r| r.view_id == *gid)
                        .map(|r| r.path_pattern.clone())
                });
                let view_id = route.view_id.clone();
                let app_id = route.app_id.clone();
                Some(MatchedRoute { config, app_entry_point, app_id, path_params, guard_view_path, view_id })
            } else {
                None
            }
        } else {
            None
        }
    }; // read lock dropped here

    if let Some(matched) = matched {
        return view_dispatch_handler(State(ctx), request, matched).await.into_response();
    }

    // Check if request path matches a failed app — return 503 with driver info
    if let Ok(failed) = ctx.failed_apps.read() {
        for (prefix, error_msg) in failed.iter() {
            if path.starts_with(prefix) {
                return error_response::service_unavailable(error_msg)
                    .into_axum_response()
                    .into_response();
            }
        }
    }

    // Phase 2: Check for /services discovery endpoint on app-main
    if path.ends_with("/services") {
        return services_discovery_handler(State(ctx), request).await.into_response();
    }

    // Phase 3: Static file fallback
    static_file_handler(State(ctx), request).await.into_response()
}

/// View dispatch handler — routes API requests to the ViewEngine.
///
/// Per spec §3: view routes are registered before static file fallback.
/// Accepts pre-matched route data from `combined_fallback_handler` to
/// avoid a second RwLock acquisition and re-match (AN11.1).
///
/// Branches on `view_type` BEFORE body extraction so that WebSocket views
/// preserve the raw request for upgrade, and SSE views skip body parsing.
async fn view_dispatch_handler(
    State(ctx): State<AppContext>,
    request: Request,
    matched: MatchedRoute,
) -> impl IntoResponse {
    let view_type = matched.config.view_type.as_str();

    // ── Per-view rate limiting ──────────────────────────────────────
    if let Some(rpm) = matched.config.rate_limit_per_minute {
        if rpm > 0 {
            use std::sync::LazyLock;
            use tokio::sync::Mutex;

            static VIEW_LIMITERS: LazyLock<Mutex<HashMap<String, Arc<crate::rate_limit::RateLimiter>>>> =
                LazyLock::new(|| Mutex::new(HashMap::new()));

            let view_key = matched.view_id.clone();
            let limiter = {
                let mut map = VIEW_LIMITERS.lock().await;
                map.entry(view_key).or_insert_with(|| {
                    Arc::new(crate::rate_limit::RateLimiter::new(&crate::rate_limit::RateLimitConfig {
                        requests_per_minute: rpm,
                        burst_size: matched.config.rate_limit_burst_size.unwrap_or(rpm / 2).max(1),
                        strategy: crate::rate_limit::RateLimitStrategy::Ip,
                    }))
                }).clone()
            };

            // Resolve client IP — respects X-Forwarded-For behind trusted proxies
            let direct_ip = request
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|ci| ci.0.ip().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let xff = request.headers().get("x-forwarded-for")
                .and_then(|v| v.to_str().ok());
            let trusted_proxies = &ctx.config.security.trusted_proxies;
            let client_ip = crate::rate_limit::resolve_client_ip(&direct_ip, xff, trusted_proxies);

            if let crate::rate_limit::RateLimitResult::Limited { retry_after_secs } = limiter.check(&client_ip).await {
                let mut resp = error_response::rate_limited("rate limit exceeded").into_axum_response();
                if let Ok(val) = axum::http::HeaderValue::from_str(&retry_after_secs.to_string()) {
                    resp.headers_mut().insert("retry-after", val);
                }
                return resp.into_response();
            }
        }
    }

    // ── Dispatch switch: branch by view_type before body extraction ──
    match view_type {
        "ServerSentEvents" => {
            return execute_sse_view(ctx, request, matched).await;
        }
        "Websocket" => {
            return execute_ws_view(ctx, request, matched).await;
        }
        _ => {
            // Rest (default) or streaming Rest — falls through to body extraction below
        }
    }

    // ── REST path: extract method, headers, body ──
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query: HashMap<String, String> = request
        .uri()
        .query()
        .map(|q| parse_query_string(q))
        .unwrap_or_default();
    let headers: HashMap<String, String> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.to_string(), val.to_string())))
        .collect();

    // Check if this is a streaming REST view before consuming the body
    let is_streaming = matched.config.streaming.unwrap_or(false);

    // Extract body for non-GET/HEAD requests
    let body = if method != "GET" && method != "HEAD" {
        let bytes = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await;
        match bytes {
            Ok(b) if !b.is_empty() => serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        }
    } else {
        serde_json::Value::Null
    };

    let trace_id = uuid::Uuid::new_v4().to_string();

    // Use pre-matched route data — no second RwLock acquire needed (AN11.1)
    let mut config = matched.config;
    let app_entry_point = matched.app_entry_point;
    let path_params = matched.path_params;
    let guard_view_path = matched.guard_view_path;

    // Namespace the dataview reference so it resolves to the correct app's dataview
    if !app_entry_point.is_empty() {
        if let rivers_runtime::view::HandlerConfig::Dataview { ref mut dataview } = config.handler {
            *dataview = format!("{}:{}", app_entry_point, dataview);
        }
    }

    let parsed = view_engine::ParsedRequest {
        method: method.clone(),
        path: path.clone(),
        query_params: query,
        headers,
        body,
        path_params,
    };

    // ── Streaming REST dispatch ──
    if is_streaming {
        return execute_streaming_rest_view(&ctx, parsed, &config, &trace_id).await;
    }

    // ── Standard REST dispatch ──
    let manifest_app_id = matched.app_id;
    let dv_namespace = app_entry_point.clone();
    let node_id = ctx.config.app_id.clone().unwrap_or_else(|| "node-0".to_string());

    let mut view_ctx = view_engine::ViewContext::new(
        parsed,
        trace_id.clone(),
        manifest_app_id.clone(), // stable appId UUID from manifest
        dv_namespace,            // entry point slug for DataView namespacing
        node_id,
        "dev".to_string(),
    );

    // ── Steps 1-3: Security pipeline (AN13.3) ──────────────
    let security = crate::security_pipeline::run_security_pipeline(
        &ctx, &config,
        &view_ctx.request.headers,
        &method,
        &trace_id,
        guard_view_path.as_deref(),
    ).await;
    let session_id = match security {
        Ok(outcome) => {
            view_ctx.session = outcome.session;
            outcome.session_id
        }
        Err(resp) => return resp,
    };

    // ── Circuit breaker check (circuit-breaker-spec §4) ──────────────
    if let rivers_runtime::view::HandlerConfig::Dataview { ref dataview } = config.handler {
        // Resolve the circuit_breaker_id (if any) while holding the read lock,
        // then drop the lock before the async is_open call.
        let breaker_id_opt: Option<String> = {
            let dv_guard_cb = ctx.dataview_executor.read().await;
            dv_guard_cb
                .as_ref()
                .and_then(|executor| executor.get_dataview_config(dataview))
                .and_then(|dv_config| dv_config.circuit_breaker_id.clone())
        };
        if let Some(breaker_id) = breaker_id_opt {
            if ctx.circuit_breaker_registry.is_open(&manifest_app_id, &breaker_id).await {
                let body = serde_json::json!({
                    "error": format!("circuit breaker '{}' is open", breaker_id),
                    "breakerId": breaker_id,
                    "retryable": true
                });
                return axum::response::Response::builder()
                    .status(503)
                    .header("content-type", "application/json")
                    .header("retry-after", "30")
                    .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap();
            }
        }
    }

    // ── Execute the view with the ProcessPool and DataViewExecutor ──
    let dv_guard = ctx.dataview_executor.read().await;
    let dv_ref = dv_guard.as_deref();
    #[cfg(feature = "metrics")]
    let exec_start = std::time::Instant::now();
    let view_result = view_engine::execute_rest_view(&mut view_ctx, &config, Some(&ctx.pool), dv_ref).await;
    drop(dv_guard);

    #[cfg(feature = "metrics")]
    {
        let exec_duration = exec_start.elapsed().as_secs_f64() * 1000.0;
        let success = view_result.is_ok();
        let engine_label = match &config.handler {
            rivers_runtime::view::HandlerConfig::Codecomponent { .. } => "v8",
            rivers_runtime::view::HandlerConfig::Dataview { .. } => "dataview",
            _ => "none",
        };
        crate::server::metrics::record_engine_execution(engine_label, exec_duration, success);
    }

    // ── Step 4: Build response with session/CSRF cookies ────
    let mut set_cookies: Vec<String> = Vec::new();

    // If this is a guard view and the result succeeded, check for session creation
    if config.guard {
        if let Ok(ref result) = view_result {
            // Guard view returned allow=true with session_claims → create session
            if result.body.get("allow").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(claims) = result.body.get("session_claims").cloned() {
                    if let Some(ref mgr) = ctx.session_manager {
                        let subject = claims.get("sub")
                            .or(claims.get("subject"))
                            .or(claims.get("username"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("anonymous")
                            .to_string();
                        match mgr.create_session(subject, claims.clone()).await {
                            Ok(session) => {
                                // Set session cookie
                                set_cookies.push(
                                    crate::session::build_set_cookie(
                                        &session.session_id,
                                        &ctx.config.security.session,
                                    ),
                                );
                                // Generate CSRF token for the new session
                                if let Some(ref csrf_mgr) = ctx.csrf_manager {
                                    if let Ok(csrf_token) = csrf_mgr.get_or_rotate_token(
                                        &session.session_id,
                                        ctx.config.security.session.ttl_s,
                                    ).await {
                                        set_cookies.push(
                                            crate::csrf::build_csrf_cookie(
                                                &csrf_token,
                                                &ctx.config.security.csrf,
                                            ),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "failed to create session from guard result");
                            }
                        }
                    }
                }
            } else {
                // Guard handler returned allow=false — credential validation failed.
                // Fire on_failed lifecycle hook (fire-and-forget).
                if let Some(ref hooks) = config.guard_config.as_ref().and_then(|gc| gc.lifecycle_hooks.as_ref()) {
                    if let Some(ref hook) = hooks.on_failed {
                        let pool = ctx.pool.clone();
                        let hook = hook.clone();
                        let trace = trace_id.clone();
                        tokio::spawn(async move {
                            let entrypoint = crate::process_pool::Entrypoint {
                                module: hook.module.clone(),
                                function: hook.entrypoint.clone(),
                                language: "javascript".to_string(),
                            };
                            let args = serde_json::json!({ "reason": "credential_validation_failed" });
                            let builder = crate::process_pool::TaskContextBuilder::new()
                                .entrypoint(entrypoint)
                                .args(args)
                                .trace_id(trace);
                            let builder = crate::task_enrichment::enrich(builder, "");
                            if let Ok(task_ctx) = builder.build() {
                                let _ = pool.dispatch("default", task_ctx).await;
                            }
                        });
                    }
                }
            }
        }
    }

    // For all responses with an active session, rotate CSRF cookie
    if set_cookies.is_empty() {
        if let (Some(ref csrf_mgr), Some(ref sid)) = (&ctx.csrf_manager, &session_id) {
            if view_ctx.session.is_some() {
                if let Ok(csrf_token) = csrf_mgr.get_or_rotate_token(
                    sid,
                    ctx.config.security.session.ttl_s,
                ).await {
                    set_cookies.push(
                        crate::csrf::build_csrf_cookie(
                            &csrf_token,
                            &ctx.config.security.csrf,
                        ),
                    );
                }
            }
        }
    }

    // Build the final HTTP response
    match view_result {
        Ok(result) => {
            let (status, resp_headers, body_str) =
                view_engine::serialize_view_result(&result);
            let mut builder = axum::response::Response::builder().status(status);
            for (k, v) in &resp_headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            for cookie in &set_cookies {
                builder = builder.header("set-cookie", cookie.as_str());
            }
            builder
                .body(axum::body::Body::from(body_str))
                .unwrap_or_else(|_| {
                    error_response::internal_error("response body construction failed")
                        .with_trace_id(trace_id.clone())
                        .into_axum_response()
                })
                .into_response()
        }
        Err(e) => {
            error_response::map_view_error(&e, Some(&trace_id))
                .into_axum_response()
        }
    }
}

/// Parse a URL query string into key-value pairs.
pub(super) fn parse_query_string(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?;
            let value = parts.next().unwrap_or("");
            let key = percent_encoding::percent_decode_str(key)
                .decode_utf8_lossy()
                .into_owned();
            let value = percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned();
            Some((key, value))
        })
        .collect()
}
