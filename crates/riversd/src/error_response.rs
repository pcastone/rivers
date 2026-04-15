//! Consistent JSON error response format.
//!
//! Per `rivers-httpd-spec.md` §18.
//!
//! All server-generated errors use a consistent JSON envelope.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

// ── Error Envelope ──────────────────────────────────────────────

/// Standard JSON error response — flat envelope.
///
/// Per spec §18 / SHAPE-2: consistent flat format `{code, message, details?, trace_id?}`
/// across all server-generated errors.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    /// HTTP status code.
    pub code: u16,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Optional distributed trace ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl ErrorResponse {
    /// Create a simple error response.
    pub fn new(code: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
            trace_id: None,
        }
    }

    /// Add details to the error response.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Add trace ID to the error response.
    pub fn with_trace_id(mut self, trace_id: String) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Convert to an axum Response with appropriate status code.
    pub fn into_axum_response(self) -> Response {
        let status = StatusCode::from_u16(self.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }

    // ── Named Constructors ──────────────────────────────────────

    /// 400 Bad Request
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(400, msg)
    }

    /// 401 Unauthorized
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::new(401, msg)
    }

    /// 403 Forbidden
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(403, msg)
    }

    /// 404 Not Found
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(404, msg)
    }

    /// 500 Internal Server Error
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(500, msg)
    }

    /// 503 Service Unavailable
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::new(503, msg)
    }
}

// ── IntoResponse ───────────────────────────────────────────────

/// Allows `ErrorResponse` to be returned directly from axum handlers.
///
/// Equivalent to calling `into_axum_response()` — sets the HTTP status
/// code from `self.code` and serializes the envelope as JSON.
impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        self.into_axum_response()
    }
}

// ── Status Code Mapping ─────────────────────────────────────────

/// Map a runtime error category to an HTTP status code.
///
/// Per spec §18.2.
pub fn map_error_code(category: ErrorCategory) -> u16 {
    match category {
        ErrorCategory::BadRequest => 400,
        ErrorCategory::Unauthorized => 401,
        ErrorCategory::Forbidden => 403,
        ErrorCategory::NotFound => 404,
        ErrorCategory::MethodNotAllowed => 405,
        ErrorCategory::Timeout => 408,
        ErrorCategory::Conflict => 409,
        ErrorCategory::ValidationError => 422,
        ErrorCategory::RateLimited => 429,
        ErrorCategory::InternalError => 500,
        ErrorCategory::ServiceUnavailable => 503,
        ErrorCategory::GatewayTimeout => 504,
    }
}

/// Error categories for status code mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 400 Bad Request.
    BadRequest,
    /// 401 Unauthorized.
    Unauthorized,
    /// 403 Forbidden.
    Forbidden,
    /// 404 Not Found.
    NotFound,
    /// 405 Method Not Allowed.
    MethodNotAllowed,
    /// 408 Request Timeout.
    Timeout,
    /// 409 Conflict.
    Conflict,
    /// 422 Unprocessable Entity.
    ValidationError,
    /// 429 Too Many Requests.
    RateLimited,
    /// 500 Internal Server Error.
    InternalError,
    /// 503 Service Unavailable.
    ServiceUnavailable,
    /// 504 Gateway Timeout.
    GatewayTimeout,
}

// ── Convenience Constructors ────────────────────────────────────

/// 400 Bad Request
pub fn bad_request(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(400, message)
}

/// 401 Unauthorized
pub fn unauthorized(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(401, message)
}

/// 403 Forbidden
pub fn forbidden(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(403, message)
}

/// 404 Not Found
pub fn not_found(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(404, message)
}

/// 405 Method Not Allowed
pub fn method_not_allowed(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(405, message)
}

/// 408 Request Timeout
pub fn request_timeout(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(408, message)
}

/// 422 Unprocessable Entity
pub fn validation_error(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(422, message)
}

/// 429 Too Many Requests
pub fn rate_limited(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(429, message)
}

/// 500 Internal Server Error
pub fn internal_error(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(500, message)
}

/// 503 Service Unavailable
pub fn service_unavailable(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(503, message)
}

/// 414 URI Too Long
pub fn uri_too_long(message: impl Into<String>) -> ErrorResponse {
    ErrorResponse::new(414, message)
}

// ── Runtime Error Mapping ───────────────────────────────────────

/// Map a view error to an error response, optionally including trace_id.
///
/// Internal/handler/pipeline errors are sanitized — the full error is logged
/// server-side but only a generic message is returned to the client.
/// In debug builds, the full error is included for development convenience.
pub fn map_view_error(err: &crate::view_engine::ViewError, trace_id: Option<&str>) -> ErrorResponse {
    let mut resp = match err {
        crate::view_engine::ViewError::NotFound(msg) => not_found(msg.clone()),
        crate::view_engine::ViewError::MethodNotAllowed(msg) => method_not_allowed(msg.clone()),
        crate::view_engine::ViewError::Validation(msg) => validation_error(msg.clone()),
        // Sanitize internal errors — don't leak driver/infra details to clients
        crate::view_engine::ViewError::Handler(msg) => {
            tracing::error!(error = %msg, "handler error");
            if cfg!(debug_assertions) {
                internal_error(msg.clone())
            } else {
                internal_error("internal server error")
            }
        }
        crate::view_engine::ViewError::Pipeline(msg) => {
            tracing::error!(error = %msg, "pipeline error");
            if cfg!(debug_assertions) {
                internal_error(msg.clone())
            } else {
                internal_error("internal server error")
            }
        }
        crate::view_engine::ViewError::Internal(msg) => {
            tracing::error!(error = %msg, "internal error");
            if cfg!(debug_assertions) {
                internal_error(msg.clone())
            } else {
                internal_error("internal server error")
            }
        }
    };
    if let Some(id) = trace_id {
        resp = resp.with_trace_id(id.to_string());
    }
    resp
}
