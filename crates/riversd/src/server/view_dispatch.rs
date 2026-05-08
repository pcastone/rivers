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
        // CB-P1.11: clone the static response_headers config before `matched`
        // is moved into the handler, then apply them to the materialized
        // response. Single intercept point covers REST + MCP + SSE/WS — every
        // view type goes through this branch.
        let static_headers = matched.config.response_headers.clone();
        let mut response = view_dispatch_handler(State(ctx), request, matched)
            .await
            .into_response();
        crate::view_engine::apply_static_response_headers(&mut response, static_headers.as_ref());
        return response;
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
        "Mcp" => {
            return execute_mcp_view(ctx, request, matched).await;
        }
        _ => {
            // Rest (default) or streaming Rest — falls through to body extraction below
        }
    }

    // ── REST path: extract method, headers, body ──
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // QP-3: Max query string length (default 8192 bytes)
    if let Some(query_str) = request.uri().query() {
        if query_str.len() > 8192 {
            return error_response::uri_too_long(
                format!("query string exceeds maximum length ({} > 8192 bytes)", query_str.len())
            ).into_response();
        }
    }

    let (query, query_all): (HashMap<String, String>, HashMap<String, Vec<String>>) = request
        .uri()
        .query()
        .map(|q| parse_query_string_multi(q))
        .unwrap_or_default();
    let headers: HashMap<String, String> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.to_string(), val.to_string())))
        .collect();

    // Check if this is a streaming REST view before consuming the body
    let is_streaming = matched.config.streaming.unwrap_or(false);

    // Extract body for non-GET/HEAD requests.
    // P1.6: when Content-Type is application/x-protobuf, transcode to JSON first.
    let is_protobuf = headers.get("content-type")
        .map(|ct| ct.starts_with("application/x-protobuf"))
        .unwrap_or(false);

    let body = if method != "GET" && method != "HEAD" {
        let bytes = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await;
        match bytes {
            Ok(b) if !b.is_empty() => {
                if is_protobuf {
                    match crate::otlp_transcoder::transcode_otlp_protobuf(&path, &b) {
                        Ok(json_bytes) => serde_json::from_slice(&json_bytes)
                            .unwrap_or(serde_json::Value::Null),
                        Err(crate::otlp_transcoder::TranscodeError::UnknownSignal(_)) => {
                            // Not an OTLP path — pass bytes through as-is
                            serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null)
                        }
                        Err(crate::otlp_transcoder::TranscodeError::DecodeFailed { reason, .. }) => {
                            return axum::http::Response::builder()
                                .status(415)
                                .header("content-type", "application/json")
                                .body(axum::body::Body::from(
                                    format!(r#"{{"error":"protobuf decode failed: {reason}"}}"#)
                                ))
                                .unwrap_or_else(|_| axum::http::Response::new(axum::body::Body::empty()))
                                .into_response();
                        }
                    }
                } else {
                    serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null)
                }
            }
            _ => serde_json::Value::Null,
        }
    } else {
        serde_json::Value::Null
    };

    let trace_id = uuid::Uuid::new_v4().to_string();

    // Use pre-matched route data — no second RwLock acquire needed (AN11.1)
    let view_id_for_audit = matched.view_id.clone();
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
        query_all,
        headers,
        body,
        path_params,
    };

    // ── Streaming REST dispatch ──
    if is_streaming {
        return execute_streaming_rest_view(&ctx, parsed, &config, &trace_id, &matched.app_id).await;
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
        &app_entry_point,
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
    let handler_start = std::time::Instant::now();
    let view_result = {
        use tracing::Instrument;
        let span = tracing::info_span!(
            "handler",
            handler = %matched.view_id,
            app = %manifest_app_id,
            method = %method,
        );
        view_engine::execute_rest_view(&mut view_ctx, &config, Some(&ctx.pool), dv_ref)
            .instrument(span)
            .await
    };
    let handler_duration_ms = handler_start.elapsed().as_millis() as u64;
    tracing::debug!(
        handler = %matched.view_id,
        duration_ms = handler_duration_ms,
        status = if view_result.is_ok() { "ok" } else { "err" },
        "handler complete"
    );
    drop(dv_guard);

    // Emit audit event for this handler invocation (P2.8)
    if let Some(ref bus) = ctx.audit_bus {
        let status: u16 = if view_result.is_ok() { 200 } else { 500 };
        let _ = bus.send(crate::audit::AuditEvent::HandlerInvoked {
            app_id: manifest_app_id.clone(),
            view: view_id_for_audit,
            method: method.clone(),
            path: path.clone(),
            duration_ms: handler_duration_ms,
            status,
        });
    }

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
                        let app_id_owned = manifest_app_id.clone();
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
            // Spec §5.3: look up the matched app's `[base] debug` flag so
            // `HandlerWithStack` errors surface the remapped stack in the
            // response envelope when enabled. Falls back to `false` if the
            // bundle/app lookup misses — `map_view_error` OR's in
            // `cfg!(debug_assertions)` for dev-build convenience.
            // `matched.app_id` was moved into `manifest_app_id` earlier;
            // reuse that binding.
            let debug_enabled = ctx
                .loaded_bundle
                .as_ref()
                .and_then(|b| {
                    b.apps
                        .iter()
                        .find(|a| a.manifest.app_id == manifest_app_id)
                        .map(|a| a.config.base.debug)
                })
                .unwrap_or(false);
            error_response::map_view_error(&e, Some(&trace_id), debug_enabled)
                .into_axum_response()
        }
    }
}

/// Handle an MCP view — JSON-RPC 2.0 dispatch over HTTP POST.
///
/// GET requests to `{mcp_path}/instructions` are routed here via a synthetic
/// route registered in the router (spec MCP-15). They return the compiled
/// instructions document as `text/markdown`.
async fn execute_mcp_view(
    ctx: AppContext,
    request: axum::http::Request<axum::body::Body>,
    matched: MatchedRoute,
) -> axum::response::Response {
    // P1.1.1.a — GET + Accept: text/event-stream → SSE notification stream
    // A valid Mcp-Session-Id is required; the stream stays open until the client
    // disconnects, at which point the session is detached from the registry.
    if request.method() == axum::http::Method::GET
        && request
            .headers()
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false)
    {
        let session_id = request
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let Some(sid) = session_id else {
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        };

        let app_id = matched.app_id.clone();
        let registry = ctx.subscription_registry.clone();
        let rx = registry.attach_sse(&sid, &app_id).await;

        use tokio_stream::{wrappers::ReceiverStream, StreamExt as _};
        let stream = ReceiverStream::new(rx)
            .map(|event| Ok::<_, std::convert::Infallible>(event));

        // P1.1.1.b — 30-second keepalive comment frames so proxies don't close idle streams.
        // Cleanup: when the SSE response body is dropped (client disconnect), the mpsc
        // channel sender held by the registry will see TrySendError::Closed on the next
        // notify_changed call. A periodic registry sweep (P1.1.3) removes dead sessions.
        return axum::response::sse::Sse::new(stream)
            .keep_alive(
                axum::response::sse::KeepAlive::new()
                    .interval(std::time::Duration::from_secs(30))
                    .text(""),
            )
            .into_response();
    }

    // GET /mcp/.../instructions — serve compiled instructions as text/markdown (spec MCP-15)
    if request.method() == axum::http::Method::GET {
        let tools = &matched.config.tools;
        let resources = &matched.config.resources;
        let prompts = &matched.config.prompts;
        let dv_namespace = &matched.app_entry_point;
        let static_instructions = matched.config.instructions.as_deref();

        let app_dir_buf = ctx.loaded_bundle.as_ref()
            .and_then(|b| b.apps.iter().find(|a| {
                a.manifest.entry_point.as_deref() == Some(dv_namespace.as_str())
                    || a.manifest.app_entry_point.as_deref() == Some(dv_namespace.as_str())
            }))
            .map(|a| a.app_dir.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let app_dir = app_dir_buf.as_path();

        let dv_guard = ctx.dataview_executor.read().await;
        let doc = crate::mcp::instructions::compile_instructions(
            static_instructions,
            app_dir,
            tools, resources, prompts,
            &|dv_name, method| {
                let namespaced = format!("{}:{}", dv_namespace, dv_name);
                dv_guard.as_ref()
                    .and_then(|e| e.get_dataview_config(&namespaced))
                    .map(|dv| dv.parameters_for_method(method).to_vec())
                    .unwrap_or_default()
            },
        );
        drop(dv_guard);

        return axum::response::Response::builder()
            .status(200)
            .header("content-type", "text/markdown; charset=utf-8")
            .body(axum::body::Body::from(doc))
            .unwrap();
    }

    // Extract headers BEFORE consuming body (into_body() moves the request)
    let session_id = request.headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let auth_header = request.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // ── CB-P1.10: per-view named guard pre-flight ──────────────
    // If `guard_view = "name"` is set, dispatch the named view's
    // codecomponent before parsing the JSON-RPC body. The guard handler
    // sees only request metadata (method, headers, path_params) — the body
    // is not yet consumed. `{ allow: true }` proceeds; anything else
    // rejects with HTTP 401 (MCP-27 — auth failures map to HTTP, not
    // JSON-RPC, so the client sees the same shape as REST guard failures).
    if let Some(guard_view_name) = matched.config.guard_view.clone() {
        // Snapshot what the guard needs before we move into the dispatch
        // body — the request body type is !Sync, so a &Request held
        // across an await would make the Future !Send.
        let mut guard_headers: HashMap<String, String> = HashMap::new();
        for (k, v) in request.headers() {
            if let Ok(s) = v.to_str() {
                guard_headers.insert(k.as_str().to_string(), s.to_string());
            }
        }
        let guard_method = request.method().to_string();
        let guard_path = request.uri().path().to_string();
        let outcome = run_mcp_named_guard_preflight(
            &ctx,
            &matched,
            guard_method,
            guard_path,
            guard_headers,
            &guard_view_name,
        )
        .await;
        if let Err(rejection) = outcome {
            return rejection;
        }
    }

    // Read POST body (max 16 MiB)
    let bytes = match axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            let resp = crate::mcp::jsonrpc::JsonRpcResponse::parse_error();
            return axum::Json(resp).into_response();
        }
    };

    // Parse JSON body
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            let resp = crate::mcp::jsonrpc::JsonRpcResponse::parse_error();
            return axum::Json(resp).into_response();
        }
    };

    let tools = &matched.config.tools;
    let resources = &matched.config.resources;
    let prompts = &matched.config.prompts;
    let federation = &matched.config.federation;
    let instructions = matched.config.instructions.as_deref();
    let app_id = &matched.app_id;
    let dv_namespace = &matched.app_entry_point;

    // Resolve app_dir from the loaded bundle using the app entry point slug
    let app_dir_buf = ctx.loaded_bundle.as_ref()
        .and_then(|b| b.apps.iter().find(|a| {
            a.manifest.entry_point.as_deref() == Some(dv_namespace.as_str())
                || a.manifest.app_entry_point.as_deref() == Some(dv_namespace.as_str())
        }))
        .map(|a| a.app_dir.clone())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let app_dir = app_dir_buf.as_path();

    // Session TTL from config (default 3600s)
    let session_ttl = matched.config.session
        .as_ref()
        .map(|s| s.ttl_seconds)
        .unwrap_or(3600);

    // Handle batch or single request
    // Parse auth header once — used at initialize time to store identity in the session,
    // and threaded into dispatch for every tool call that reaches a codecomponent handler.
    let init_auth_context = crate::mcp::session::parse_auth_header(auth_header.as_deref());

    if let Some(batch) = body.as_array() {
        let mut responses = Vec::new();
        for item in batch {
            match serde_json::from_value::<crate::mcp::jsonrpc::JsonRpcRequest>(item.clone()) {
                Ok(req) => {
                    if req.id.is_some() {
                        // Session validation for non-initialize methods in batch
                        let auth_context = if req.method != "initialize" && req.method != "ping" {
                            if let Some(ref storage) = ctx.storage_engine {
                                match &session_id {
                                    Some(sid) => {
                                        match crate::mcp::session::validate_session(storage, sid, session_ttl).await {
                                            Some(data) => data.get("auth").cloned(),
                                            None => {
                                                let resp = crate::mcp::jsonrpc::JsonRpcResponse::session_required(req.id.clone());
                                                responses.push(serde_json::to_value(&resp).unwrap_or_default());
                                                continue;
                                            }
                                        }
                                    }
                                    None => {
                                        let resp = crate::mcp::jsonrpc::JsonRpcResponse::session_required(req.id.clone());
                                        responses.push(serde_json::to_value(&resp).unwrap_or_default());
                                        continue;
                                    }
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let resp = crate::mcp::dispatch::dispatch(
                            &ctx, &req, tools, resources, prompts, app_id, dv_namespace, app_dir, instructions,
                            auth_context.as_ref(), session_id.as_deref(), federation, &matched.path_params,
                        ).await;
                        responses.push(serde_json::to_value(&resp).unwrap_or_default());
                    }
                    // Notifications (no id) produce no response per JSON-RPC 2.0
                }
                Err(_) => {
                    let resp = crate::mcp::jsonrpc::JsonRpcResponse::invalid_request(None);
                    responses.push(serde_json::to_value(&resp).unwrap_or_default());
                }
            }
        }
        axum::Json(serde_json::Value::Array(responses)).into_response()
    } else {
        match serde_json::from_value::<crate::mcp::jsonrpc::JsonRpcRequest>(body) {
            Ok(req) => {
                if req.id.is_none() {
                    // Notification — no response per JSON-RPC 2.0 spec
                    return axum::http::StatusCode::NO_CONTENT.into_response();
                }

                // For non-initialize methods: validate session and extract stored auth context
                let auth_context = if req.method != "initialize" && req.method != "ping" {
                    if let Some(ref storage) = ctx.storage_engine {
                        match &session_id {
                            Some(sid) => {
                                match crate::mcp::session::validate_session(storage, sid, session_ttl).await {
                                    Some(data) => data.get("auth").cloned(),
                                    None => {
                                        let resp = crate::mcp::jsonrpc::JsonRpcResponse::session_required(req.id.clone());
                                        return axum::Json(resp).into_response();
                                    }
                                }
                            }
                            None => {
                                let resp = crate::mcp::jsonrpc::JsonRpcResponse::session_required(req.id.clone());
                                return axum::Json(resp).into_response();
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let resp = crate::mcp::dispatch::dispatch(
                    &ctx, &req, tools, resources, prompts, app_id, dv_namespace, app_dir, instructions,
                    auth_context.as_ref(), session_id.as_deref(), federation, &matched.path_params,
                ).await;

                // For initialize: create session (storing auth identity) and attach Mcp-Session-Id header
                if req.method == "initialize" {
                    if let Some(ref storage) = ctx.storage_engine {
                        match crate::mcp::session::create_session(storage, session_ttl, init_auth_context).await {
                            Ok(new_sid) => {
                                let body_str = serde_json::to_string(&resp).unwrap_or_default();
                                let mut response = axum::response::Response::builder()
                                    .status(200)
                                    .header("content-type", "application/json");
                                if let Ok(hv) = axum::http::HeaderValue::from_str(&new_sid) {
                                    response = response.header("mcp-session-id", hv);
                                }
                                return response
                                    .body(axum::body::Body::from(body_str))
                                    .unwrap_or_else(|_| axum::Json(resp).into_response());
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to create MCP session");
                                // Return response without session header rather than failing
                            }
                        }
                    }
                }

                axum::Json(resp).into_response()
            }
            Err(_) => {
                let resp = crate::mcp::jsonrpc::JsonRpcResponse::invalid_request(None);
                axum::Json(resp).into_response()
            }
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

/// Parse query string preserving all values per key (for duplicate keys).
pub(super) fn parse_query_string_multi(query: &str) -> (HashMap<String, String>, HashMap<String, Vec<String>>) {
    let mut first: HashMap<String, String> = HashMap::new();
    let mut all: HashMap<String, Vec<String>> = HashMap::new();

    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let mut parts = pair.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => percent_encoding::percent_decode_str(k)
                .decode_utf8_lossy()
                .into_owned(),
            None => continue,
        };
        let value = parts.next()
            .map(|v| percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned())
            .unwrap_or_default();

        first.entry(key.clone()).or_insert_with(|| value.clone());
        all.entry(key).or_default().push(value);
    }

    (first, all)
}

/// CB-P1.10 — Run a per-view named guard's codecomponent before the MCP
/// JSON-RPC dispatcher.
///
/// Returns `Ok(())` if the guard returned `{ allow: true }`. Returns
/// `Err(Response)` (HTTP 401) when the guard rejected, the named view is
/// missing, the named view is not a codecomponent (already caught by
/// X014 at validate time, but defensive at runtime), or the guard
/// dispatcher itself errored.
///
/// The guard handler receives a `ParsedRequest` with the original
/// method, path, and headers — the JSON-RPC body has not been consumed
/// yet. This matches the contract `execute_guard_handler` already uses
/// for the server-wide guard.
async fn run_mcp_named_guard_preflight(
    ctx: &AppContext,
    matched: &MatchedRoute,
    method: String,
    path: String,
    headers_map: HashMap<String, String>,
    guard_view_name: &str,
) -> Result<(), axum::response::Response> {
    use rivers_runtime::view::HandlerConfig;

    // Resolve the named guard view in the same app.
    let dv_namespace = &matched.app_entry_point;
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
            a.manifest.entry_point.as_deref() == Some(dv_namespace.as_str())
                || a.manifest.app_entry_point.as_deref() == Some(dv_namespace.as_str())
        });
        let Some(app) = app else {
            return Err(error_response::internal_error(
                "named guard pre-flight: app not found in bundle",
            )
            .into_axum_response()
            .into_response());
        };
        let Some(view_config) = app.config.api.views.get(guard_view_name) else {
            // Should be impossible after X014 validation, but keep the path
            // safe at runtime — reject with 401 rather than crashing.
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

    // Build a ParsedRequest from the snapshot taken before the body was
    // consumed. The body is Null — the guard runs before JSON-RPC parse.
    let parsed = crate::view_engine::ParsedRequest {
        method,
        path,
        query_params: HashMap::new(),
        query_all: HashMap::new(),
        headers: headers_map,
        body: serde_json::Value::Null,
        path_params: matched.path_params.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_preserves_duplicates() {
        let (first, all) = parse_query_string_multi("tag=a&tag=b&tag=c");
        assert_eq!(first.get("tag"), Some(&"a".to_string()));
        assert_eq!(all.get("tag").unwrap(), &vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn multi_single_value_is_array_of_one() {
        let (_, all) = parse_query_string_multi("limit=10");
        assert_eq!(all.get("limit").unwrap(), &vec!["10".to_string()]);
    }

    #[test]
    fn multi_empty_value() {
        let (first, _) = parse_query_string_multi("key=");
        assert_eq!(first.get("key"), Some(&"".to_string()));
    }

    #[test]
    fn multi_bare_key() {
        let (first, _) = parse_query_string_multi("key");
        assert_eq!(first.get("key"), Some(&"".to_string()));
    }

    #[test]
    fn multi_percent_encoded() {
        let (first, _) = parse_query_string_multi("name=John%20Doe");
        assert_eq!(first.get("name"), Some(&"John Doe".to_string()));
    }

    #[test]
    fn multi_empty_string() {
        let (first, all) = parse_query_string_multi("");
        assert!(first.is_empty());
        assert!(all.is_empty());
    }
}
