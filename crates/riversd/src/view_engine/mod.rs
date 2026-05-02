//! View Layer — REST view routing, handler pipeline, and response serialization.
//!
//! Per `rivers-view-layer-spec.md` §1-5, §12-13.

mod pipeline;
mod router;
mod types;
mod validation;

pub use pipeline::*;
pub use router::*;
pub use types::*;
pub use validation::*;

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::view::{HandlerStageConfig, ViewEventHandlers};
    use std::collections::HashMap;

    fn make_none_handler_view(with_pipeline: bool) -> rivers_runtime::view::ApiViewConfig {
        use rivers_runtime::view::{ApiViewConfig, HandlerConfig};

        let event_handlers = if with_pipeline {
            Some(ViewEventHandlers {
                pre_process: vec![],
                handlers: vec![HandlerStageConfig {
                    module: "my_module".into(),
                    entrypoint: "handle".into(),
                    key: None,

                    on_failure: None,
                }],
                post_process: vec![],
                on_error: vec![],
            })
        } else {
            None
        };
        ApiViewConfig {
            view_type: "Rest".into(),
            path: Some("/api/computed".into()),
            method: Some("GET".into()),
            handler: HandlerConfig::None {},
            parameter_mapping: None,
            dataviews: vec![],
            primary: None,
            streaming: None,
            streaming_format: None,
            stream_timeout_ms: None,
            guard: false,
            auth: None,
            guard_config: None,
            allow_outbound_http: false,
            rate_limit_per_minute: None,
            rate_limit_burst_size: None,
            websocket_mode: None,
            max_connections: None,
            sse_tick_interval_ms: None,
            sse_trigger_events: vec![],
            sse_event_buffer_size: None,
            session_revalidation_interval_s: None,
            event_handlers,
            on_stream: None,
            ws_hooks: None,
            on_event: None,
            polling: None,
            tools: HashMap::new(),
            resources: HashMap::new(),
            prompts: HashMap::new(),
            instructions: None,
            session: None,
            federation: vec![],
        }
    }

    #[tokio::test]
    async fn test_null_datasource_returns_null_primary() {
        let config = make_none_handler_view(true);
        let request = ParsedRequest::new("GET", "/api/computed");
        let mut ctx = ViewContext::new(
            request,
            "trace-1".into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );

        let result = execute_rest_view(&mut ctx, &config, None, None).await.unwrap();

        // Primary source should be null (no DataView executed)
        assert_eq!(result.body, serde_json::Value::Null);
        assert_eq!(result.status, 200);
        // resdata should be Null
        assert_eq!(ctx.resdata, serde_json::Value::Null);
    }

    #[test]
    fn test_null_handler_validation_requires_pipeline() {
        let mut views = HashMap::new();

        // View with None handler but no pipeline stages => error
        views.insert("no_pipeline".into(), make_none_handler_view(false));
        // View with None handler and pipeline stages => ok
        views.insert("with_pipeline".into(), make_none_handler_view(true));

        let errors = validate_views(&views, &[]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("no_pipeline"));
        assert!(errors[0].contains("handler type 'none'"));
    }

    #[test]
    fn test_null_handler_not_counted_as_unknown_dataview() {
        let mut views = HashMap::new();
        views.insert("computed".into(), make_none_handler_view(true));

        // No available dataviews — should not produce "unknown dataview" error
        let errors = validate_views(&views, &[]);
        assert!(errors.is_empty());
    }

    // ── D5: on_error tests ────────────────────────────────────

    #[tokio::test]
    async fn test_on_error_handlers_returns_none_when_pool_unavailable() {
        use crate::process_pool::ProcessPoolManager;

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let handlers = vec![HandlerStageConfig {
            module: "error_handler.js".into(),
            entrypoint: "on_error".into(),
            key: None,
            on_failure: None,
        }];
        let ctx = ViewContext::new(
            ParsedRequest::new("GET", "/test"),
            "trace-1".into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
        let error = ViewError::Handler("test error".into());

        // Pool returns EngineUnavailable, so handler returns None
        let result = execute_on_error_handlers(&pool, &handlers, &ctx, &error).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_on_error_handlers_empty_list() {
        use crate::process_pool::ProcessPoolManager;

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let ctx = ViewContext::new(
            ParsedRequest::new("GET", "/test"),
            "trace-1".into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
        let error = ViewError::Handler("test error".into());

        let result = execute_on_error_handlers(&pool, &[], &ctx, &error).await;
        assert!(result.is_none());
    }

    // ── D6: on_session_valid tests ──────────────────────────

    #[tokio::test]
    async fn test_on_session_valid_engine_unavailable() {
        use crate::process_pool::ProcessPoolManager;

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let handler = HandlerStageConfig {
            module: "session.js".into(),
            entrypoint: "validate".into(),
            key: None,
            on_failure: None,
        };
        let session = serde_json::json!({"user_id": "u-1"});

        let result = execute_on_session_valid(&pool, &handler, &session, "trace-1", "test-app").await;
        // Should fail because the engine is unavailable
        assert!(result.is_err());
    }

    // ── parse_handler_view_result tests ─────────────────────

    #[test]
    fn test_parse_handler_view_result_valid() {
        let value = serde_json::json!({
            "status": 503,
            "headers": {"x-custom": "val"},
            "body": {"message": "service unavailable"},
        });
        let result = parse_handler_view_result(&value).unwrap();
        assert_eq!(result.status, 503);
        assert_eq!(result.headers.get("x-custom").unwrap(), "val");
        assert_eq!(result.body, serde_json::json!({"message": "service unavailable"}));
    }

    #[test]
    fn test_parse_handler_view_result_missing_status() {
        let value = serde_json::json!({"body": "hello"});
        assert!(parse_handler_view_result(&value).is_none());
    }

    #[test]
    fn test_parse_handler_view_result_minimal() {
        let value = serde_json::json!({"status": 200});
        let result = parse_handler_view_result(&value).unwrap();
        assert_eq!(result.status, 200);
        assert_eq!(result.body, serde_json::Value::Null);
        assert!(result.headers.is_empty());
    }

    // ── F4: handler-result status & header validation ──────────

    #[test]
    fn test_parse_handler_view_result_rejects_status_999() {
        let value = serde_json::json!({"status": 999, "body": "nope"});
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
        // Sanitized body explains the failure.
        assert_eq!(
            result.body.get("error").and_then(|v| v.as_str()),
            Some("invalid_handler_response")
        );
        assert!(result.headers.is_empty());
    }

    #[test]
    fn test_parse_handler_view_result_rejects_status_below_100() {
        let value = serde_json::json!({"status": 99, "body": "nope"});
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
    }

    #[test]
    fn test_parse_handler_view_result_rejects_status_zero() {
        let value = serde_json::json!({"status": 0});
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
    }

    #[test]
    fn test_parse_handler_view_result_accepts_boundary_statuses() {
        for s in &[100u64, 200, 404, 500, 599] {
            let value = serde_json::json!({"status": s});
            let result = parse_handler_view_result(&value).unwrap();
            assert_eq!(result.status, *s as u16, "boundary status {} should be accepted", s);
        }
    }

    #[test]
    fn test_parse_handler_view_result_rejects_crlf_in_header_value() {
        // Classic header-smuggling vector: CRLF in a header value would let
        // the handler inject a second header (e.g. "Set-Cookie: ...").
        let value = serde_json::json!({
            "status": 200,
            "headers": {"X-Bad": "foo\r\nInjection: yes"},
            "body": "ok",
        });
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
        assert_eq!(
            result.body.get("error").and_then(|v| v.as_str()),
            Some("invalid_handler_response")
        );
        // The original X-Bad header must not have leaked into the response.
        assert!(result.headers.is_empty());
    }

    #[test]
    fn test_parse_handler_view_result_rejects_lf_in_header_value() {
        let value = serde_json::json!({
            "status": 200,
            "headers": {"X-Bad": "foo\nInjection: yes"},
        });
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
    }

    #[test]
    fn test_parse_handler_view_result_rejects_nul_in_header_value() {
        let value = serde_json::json!({
            "status": 200,
            "headers": {"X-Bad": "foo\u{0000}bar"},
        });
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
    }

    #[test]
    fn test_parse_handler_view_result_rejects_empty_header_name() {
        let value = serde_json::json!({
            "status": 200,
            "headers": {"": "value"},
        });
        let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
        assert_eq!(result.status, 500);
    }

    #[test]
    fn test_parse_handler_view_result_rejects_invalid_header_name_chars() {
        // Spaces and colons are not in the RFC 7230 token grammar.
        for name in &["X Bad", "X:Bad", "X\nBad", "Bad Header"] {
            let value = serde_json::json!({
                "status": 200,
                "headers": {*name: "value"},
            });
            let result = parse_handler_view_result(&value).expect("returns sanitized envelope");
            assert_eq!(
                result.status, 500,
                "header name {:?} should have been rejected",
                name
            );
        }
    }

    #[test]
    fn test_parse_handler_view_result_allows_security_headers() {
        // F4.3: handler-set security headers (CSP, HSTS, etc.) must NOT be
        // blocked. Apps can set them at will; we only enforce grammar.
        let value = serde_json::json!({
            "status": 200,
            "headers": {
                "Content-Security-Policy": "default-src 'self'",
                "Strict-Transport-Security": "max-age=31536000; includeSubDomains",
                "X-Frame-Options": "DENY",
            },
            "body": "ok",
        });
        let result = parse_handler_view_result(&value).expect("envelope");
        assert_eq!(result.status, 200);
        assert_eq!(
            result.headers.get("Content-Security-Policy").map(String::as_str),
            Some("default-src 'self'")
        );
        assert_eq!(
            result.headers.get("Strict-Transport-Security").map(String::as_str),
            Some("max-age=31536000; includeSubDomains")
        );
        assert_eq!(
            result.headers.get("X-Frame-Options").map(String::as_str),
            Some("DENY")
        );
    }

    #[test]
    fn test_parse_handler_view_result_skips_non_string_header_values() {
        // Mirrors prior behaviour: non-string values are silently skipped
        // rather than rejected. (They couldn't possibly be valid anyway.)
        let value = serde_json::json!({
            "status": 200,
            "headers": {
                "X-Number": 42,
                "X-Bool": true,
                "X-Real": "kept",
            },
        });
        let result = parse_handler_view_result(&value).unwrap();
        assert_eq!(result.status, 200);
        assert_eq!(result.headers.len(), 1);
        assert_eq!(result.headers.get("X-Real").map(String::as_str), Some("kept"));
    }

    // ── StoreHandle tests ───────────────────────────────────────

    #[test]
    fn store_handle_reserved_key_detection() {
        assert!(StoreHandle::is_reserved_key("session:abc"));
        assert!(StoreHandle::is_reserved_key("csrf:token-123"));
        assert!(StoreHandle::is_reserved_key("cache:views:orders"));
        assert!(StoreHandle::is_reserved_key("raft:state"));
        assert!(StoreHandle::is_reserved_key("rivers:internal"));
        assert!(!StoreHandle::is_reserved_key("user:prefs:123"));
        assert!(!StoreHandle::is_reserved_key("mykey"));
    }

    // ── S12: Request-time validation tests ───────────────────────

    fn make_test_schema() -> rivers_runtime::rivers_driver_sdk::SchemaDefinition {
        rivers_runtime::rivers_driver_sdk::SchemaDefinition {
            driver: "postgresql".into(),
            schema_type: "object".into(),
            description: String::new(),
            fields: vec![rivers_runtime::rivers_driver_sdk::SchemaFieldDef {
                name: "name".into(),
                field_type: "string".into(),
                required: true,
                constraints: std::collections::HashMap::new(),
            }],
            extra: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn validate_input_passes_valid_data() {
        let schema = make_test_schema();
        let data = serde_json::json!({"name": "alice"});
        assert!(
            validate_input(&data, Some(&schema), rivers_runtime::rivers_driver_sdk::ValidationDirection::Input)
                .is_ok()
        );
    }

    #[test]
    fn validate_input_rejects_missing_required() {
        let schema = make_test_schema();
        let data = serde_json::json!({});
        assert!(
            validate_input(&data, Some(&schema), rivers_runtime::rivers_driver_sdk::ValidationDirection::Input)
                .is_err()
        );
    }

    #[test]
    fn validate_output_warns_on_failure() {
        let schema = make_test_schema();
        let data = serde_json::json!({});
        let warning = validate_output(&data, Some(&schema));
        assert!(warning.is_some());
    }

    #[test]
    fn validate_output_none_on_success() {
        let schema = make_test_schema();
        let data = serde_json::json!({"name": "alice"});
        let warning = validate_output(&data, Some(&schema));
        assert!(warning.is_none());
    }

    #[test]
    fn validate_input_none_schema_passes() {
        let data = serde_json::json!({"anything": true});
        assert!(
            validate_input(&data, None, rivers_runtime::rivers_driver_sdk::ValidationDirection::Input).is_ok()
        );
    }

    #[test]
    fn validate_output_none_schema_passes() {
        let data = serde_json::json!({"anything": true});
        assert!(validate_output(&data, None).is_none());
    }

    // ── Namespaced path tests (AF4) ─────────────────────────────────

    #[test]
    fn build_namespaced_path_no_prefix() {
        assert_eq!(
            build_namespaced_path(None, "address-book", "service", "contacts"),
            "/address-book/service/contacts"
        );
    }

    #[test]
    fn build_namespaced_path_with_prefix() {
        assert_eq!(
            build_namespaced_path(Some("v1"), "address-book", "service", "contacts/{id}"),
            "/v1/address-book/service/contacts/{id}"
        );
    }

    #[test]
    fn build_namespaced_path_strips_leading_slash() {
        assert_eq!(
            build_namespaced_path(None, "myapp", "api", "/users"),
            "/myapp/api/users"
        );
    }

    #[test]
    fn build_namespaced_path_empty_view() {
        assert_eq!(
            build_namespaced_path(None, "address-book", "main", ""),
            "/address-book/main"
        );
    }

    #[test]
    fn build_namespaced_path_empty_prefix_treated_as_none() {
        assert_eq!(
            build_namespaced_path(Some(""), "address-book", "service", "contacts"),
            "/address-book/service/contacts"
        );
    }
}
