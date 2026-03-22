use riversd::error_response::{
    bad_request, forbidden, internal_error, map_error_code, map_view_error, method_not_allowed,
    not_found, rate_limited, request_timeout, service_unavailable, unauthorized, validation_error,
    ErrorCategory, ErrorResponse,
};
use riversd::view_engine::ViewError;

// ── ErrorResponse ───────────────────────────────────────────────

#[test]
fn error_response_basic() {
    let resp = ErrorResponse::new(404, "not found");
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["code"], 404);
    assert_eq!(json["message"], "not found");
    assert!(json["details"].is_null());
    assert!(json["trace_id"].is_null());
    // SHAPE-2: flat envelope — no "error" wrapper key
    assert!(json.get("error").is_none());
}

#[test]
fn error_response_with_details() {
    let resp = ErrorResponse::new(422, "validation failed")
        .with_details(serde_json::json!({"field": "email", "reason": "invalid"}));
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["details"]["field"], "email");
}

#[test]
fn error_response_with_trace_id() {
    let resp = ErrorResponse::new(500, "internal error").with_trace_id("trace-123".into());
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["trace_id"], "trace-123");
}

#[test]
fn error_response_skips_none_fields() {
    let resp = ErrorResponse::new(400, "bad request");
    let json_str = serde_json::to_string(&resp).unwrap();
    assert!(!json_str.contains("details"));
    assert!(!json_str.contains("trace_id"));
}

// ── Status Code Mapping ─────────────────────────────────────────

#[test]
fn map_all_categories() {
    assert_eq!(map_error_code(ErrorCategory::BadRequest), 400);
    assert_eq!(map_error_code(ErrorCategory::Unauthorized), 401);
    assert_eq!(map_error_code(ErrorCategory::Forbidden), 403);
    assert_eq!(map_error_code(ErrorCategory::NotFound), 404);
    assert_eq!(map_error_code(ErrorCategory::MethodNotAllowed), 405);
    assert_eq!(map_error_code(ErrorCategory::Timeout), 408);
    assert_eq!(map_error_code(ErrorCategory::Conflict), 409);
    assert_eq!(map_error_code(ErrorCategory::ValidationError), 422);
    assert_eq!(map_error_code(ErrorCategory::RateLimited), 429);
    assert_eq!(map_error_code(ErrorCategory::InternalError), 500);
    assert_eq!(map_error_code(ErrorCategory::ServiceUnavailable), 503);
    assert_eq!(map_error_code(ErrorCategory::GatewayTimeout), 504);
}

// ── Convenience Constructors ────────────────────────────────────

#[test]
fn convenience_bad_request() {
    let resp = bad_request("invalid input");
    assert_eq!(resp.code, 400);
    assert_eq!(resp.message, "invalid input");
}

#[test]
fn convenience_unauthorized() {
    let resp = unauthorized("no session");
    assert_eq!(resp.code, 401);
}

#[test]
fn convenience_forbidden() {
    let resp = forbidden("access denied");
    assert_eq!(resp.code, 403);
}

#[test]
fn convenience_not_found() {
    let resp = not_found("page not found");
    assert_eq!(resp.code, 404);
}

#[test]
fn convenience_method_not_allowed() {
    let resp = method_not_allowed("POST not allowed");
    assert_eq!(resp.code, 405);
}

#[test]
fn convenience_request_timeout() {
    let resp = request_timeout("took too long");
    assert_eq!(resp.code, 408);
}

#[test]
fn convenience_validation_error() {
    let resp = validation_error("invalid email");
    assert_eq!(resp.code, 422);
}

#[test]
fn convenience_rate_limited() {
    let resp = rate_limited("too many requests");
    assert_eq!(resp.code, 429);
}

#[test]
fn convenience_internal_error() {
    let resp = internal_error("unexpected failure");
    assert_eq!(resp.code, 500);
}

#[test]
fn convenience_service_unavailable() {
    let resp = service_unavailable("shutting down");
    assert_eq!(resp.code, 503);
}

// ── View Error Mapping ──────────────────────────────────────────

#[test]
fn map_view_not_found() {
    let err = ViewError::NotFound("no route".into());
    let resp = map_view_error(&err, None);
    assert_eq!(resp.code, 404);
}

#[test]
fn map_view_method_not_allowed() {
    let err = ViewError::MethodNotAllowed("POST not supported".into());
    let resp = map_view_error(&err, None);
    assert_eq!(resp.code, 405);
}

#[test]
fn map_view_handler_error() {
    let err = ViewError::Handler("handler crashed".into());
    let resp = map_view_error(&err, None);
    assert_eq!(resp.code, 500);
}

#[test]
fn map_view_validation_error() {
    let err = ViewError::Validation("invalid params".into());
    let resp = map_view_error(&err, None);
    assert_eq!(resp.code, 422);
}

#[test]
fn map_view_internal_error() {
    let err = ViewError::Internal("unexpected".into());
    let resp = map_view_error(&err, None);
    assert_eq!(resp.code, 500);
}

#[test]
fn map_view_error_includes_trace_id() {
    let err = ViewError::NotFound("missing".into());
    let resp = map_view_error(&err, Some("abc-123"));
    assert_eq!(resp.code, 404);
    assert_eq!(resp.trace_id.as_deref(), Some("abc-123"));
}

#[test]
fn error_response_trace_id_on_new() {
    let resp = ErrorResponse::new(500, "test").with_trace_id("trace-xyz".to_string());
    assert_eq!(resp.trace_id.as_deref(), Some("trace-xyz"));
}
