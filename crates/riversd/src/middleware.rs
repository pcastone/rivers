//! HTTP middleware stack.
//!
//! Per `rivers-httpd-spec.md` §4.
//!
//! Main server middleware order (outermost to innermost):
//! 1. compression
//! 2. body_limit (16 MiB)
//! 3. trace_id (moved early so all error responses include trace_id)
//! 4. security_headers
//! 5. session (stub)
//! 6. rate_limit
//! 7. shutdown_guard
//! 8. backpressure
//! 9. timeout
//! 10. request_observer (stub)

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

use crate::cors::{CorsConfig, resolve_cors_headers};
use crate::error_response;
use crate::rate_limit::{RateLimitResult, RateLimiter};
use crate::session::{self, SessionManager};
use crate::shutdown::ShutdownCoordinator;

// ── Trace ID Middleware ───────────────────────────────────────────

/// Extract or generate a trace ID, store in request extensions and response header.
///
/// Per spec §4 step 10: extract/generate trace_id, inject headers.
pub async fn trace_id_middleware(mut request: Request<Body>, next: Next) -> Response {
    // Check for existing trace ID header
    let trace_id = request
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Store in extensions for downstream handlers
    request.extensions_mut().insert(TraceId(trace_id.clone()));

    let mut response = next.run(request).await;

    // Inject trace ID in response
    if let Ok(val) = HeaderValue::from_str(&trace_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-trace-id"), val);
    }

    response
}

/// Trace ID extracted from request or generated.
#[derive(Debug, Clone)]
pub struct TraceId(pub String);

/// Extract trace ID from request extensions, if available.
pub fn extract_trace_id(request: &Request<Body>) -> Option<String> {
    request
        .extensions()
        .get::<TraceId>()
        .map(|t| t.0.clone())
}

// ── Timeout Middleware ────────────────────────────────────────────

/// Per-request timeout.
///
/// Per spec §4 step 8: default 30s, configurable via `base.request_timeout_seconds`.
pub async fn timeout_middleware(
    State(timeout_secs): State<u64>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let trace_id = extract_trace_id(&request);
    let timeout = std::time::Duration::from_secs(timeout_secs);
    match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_elapsed) => {
            let mut err = error_response::request_timeout("request timeout");
            if let Some(id) = trace_id {
                err = err.with_trace_id(id);
            }
            err.into_axum_response()
        }
    }
}

// ── Shutdown Guard Middleware ─────────────────────────────────────

/// Reject new requests when the server is draining.
///
/// Per spec §13.3.
pub async fn shutdown_guard_middleware(
    State(coordinator): State<Arc<ShutdownCoordinator>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if coordinator.is_draining() {
        let mut err = error_response::service_unavailable("server is shutting down");
        if let Some(id) = extract_trace_id(&request) {
            err = err.with_trace_id(id);
        }
        return err.into_axum_response();
    }

    coordinator.enter();

    // RAII guard ensures exit() is called even if the handler panics,
    // preventing graceful shutdown from hanging on a leaked inflight counter.
    struct InflightGuard(Arc<ShutdownCoordinator>);
    impl Drop for InflightGuard {
        fn drop(&mut self) {
            self.0.exit();
        }
    }
    let _guard = InflightGuard(coordinator);

    next.run(request).await
}

// ── Security Headers Middleware ───────────────────────────────────

/// Inject standard security response headers.
///
/// Per spec §4 step 3 (SEC-12).
pub async fn security_headers_middleware(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;

    let headers = response.headers_mut();
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "x-xss-protection",
        HeaderValue::from_static("1; mode=block"),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    // CSP is the operator's responsibility — not injected by default.

    response
}

// ── Session Middleware ───────────────────────────────────────────

/// Session middleware — parse cookie/Bearer, lookup, validate, inject into extensions.
///
/// Per spec §12.1: sessions are never auto-created. Session creation is the
/// exclusive responsibility of guard view CodeComponent handlers.
pub async fn session_middleware(
    State(manager): State<Arc<SessionManager>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let cookie_name = &manager.config().cookie.name;

    // Extract session ID from cookie or Authorization Bearer header
    let cookie_header = request
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let session_id =
        session::extract_session_id(cookie_header.as_deref(), auth_header.as_deref(), cookie_name);

    let mut clear_cookie = false;

    if let Some(ref id) = session_id {
        match manager.validate_session(id).await {
            Ok(Some(session)) => {
                // Valid session — inject into request extensions
                request.extensions_mut().insert(session);
            }
            Ok(None) => {
                // Expired or not found — mark for cookie clearing
                clear_cookie = true;
            }
            Err(_) => {
                // Storage error — treat as no session
                clear_cookie = true;
            }
        }
    }

    let mut response = next.run(request).await;

    // Clear cookie if session was invalid
    if clear_cookie {
        let clear = session::build_clear_cookie(manager.config());
        if let Ok(val) = HeaderValue::from_str(&clear) {
            response.headers_mut().insert("set-cookie", val);
        }
    }

    response
}

// ── Rate Limit Middleware ─────────────────────────────────────────

/// Token bucket rate limiting middleware.
///
/// Per spec §10.1, §10.5.
/// Supports IP (default) and custom header strategies.
pub async fn rate_limit_middleware(
    State((limiter, strategy, trusted_proxies)): State<(Arc<RateLimiter>, crate::rate_limit::RateLimitStrategy, Vec<String>)>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Extract direct connection IP
    let direct_ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Resolve real client IP (respects trusted proxies + X-Forwarded-For)
    let xff = request.headers().get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    let client_ip = crate::rate_limit::resolve_client_ip(&direct_ip, xff, &trusted_proxies);

    // Extract key based on strategy
    let key = match &strategy {
        crate::rate_limit::RateLimitStrategy::CustomHeader(header_name) => {
            // Only trust custom header if request comes from a trusted proxy
            if trusted_proxies.is_empty() || !crate::rate_limit::resolve_client_ip(&direct_ip, None, &trusted_proxies).eq(&direct_ip) {
                // Direct connection is from a trusted proxy — trust the header
                request
                    .headers()
                    .get(header_name.as_str())
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
                    .unwrap_or(client_ip.clone())
            } else {
                // Direct connection is NOT from a trusted proxy — ignore custom header
                client_ip.clone()
            }
        }
        crate::rate_limit::RateLimitStrategy::Ip => client_ip,
    };

    match limiter.check(&key).await {
        RateLimitResult::Allowed => next.run(request).await,
        RateLimitResult::Limited { retry_after_secs } => {
            let mut err = error_response::rate_limited("rate limit exceeded");
            if let Some(id) = extract_trace_id(&request) {
                err = err.with_trace_id(id);
            }
            let mut response = err.into_axum_response();
            if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                response.headers_mut().insert("retry-after", val);
            }
            response
        }
    }
}

// ── Request Observer Middleware ──────────────────────────────

/// Publish a `RequestCompleted` event for every processed request.
///
/// Per spec §4 step 9: records method, path, status, duration, trace_id.
pub async fn request_observer_middleware(request: Request<Body>, next: Next) -> Response {
    #[cfg(feature = "metrics")]
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(feature = "metrics")]
    static ACTIVE_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let trace_id = extract_trace_id(&request)
        .unwrap_or_default();
    let start = std::time::Instant::now();

    #[cfg(feature = "metrics")]
    {
        let count = ACTIVE_CONNECTIONS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::server::metrics::set_active_connections(count);
    }

    let response = next.run(request).await;

    #[cfg(feature = "metrics")]
    {
        let count = ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        crate::server::metrics::set_active_connections(count);
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    let status_code = response.status().as_u16();

    tracing::debug!(
        method = %method,
        path = %path,
        status = status_code,
        duration_ms = duration_ms,
        trace_id = %trace_id,
        "RequestCompleted"
    );

    #[cfg(feature = "metrics")]
    crate::server::metrics::record_request(&method, status_code, duration_ms as f64);

    response
}

// ── CORS Middleware ──────────────────────────────────────────────

/// CORS middleware — injects CORS headers on all responses, including errors.
///
/// Per spec §9 and B1.5: CORS headers must appear on error responses too,
/// so browsers can read error payloads from cross-origin requests.
pub async fn cors_middleware(
    State(cors_config): State<Arc<CorsConfig>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let method = request.method().clone();

    let mut response = next.run(request).await;

    if let Some(cors_headers) = resolve_cors_headers(
        &cors_config,
        origin.as_deref(),
        Some(&method),
    ) {
        cors_headers.apply(response.headers_mut());
    }

    response
}
