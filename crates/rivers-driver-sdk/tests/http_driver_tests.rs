//! HTTP driver tests — templating, response mapping, validation, config types.

use std::collections::HashMap;

use rivers_driver_sdk::http_driver::*;
use rivers_driver_sdk::QueryValue;

// ── Path Templating ────────────────────────────────────────────────

#[test]
fn path_template_single_param() {
    let params = [("user_id".to_string(), "42".to_string())]
        .into_iter()
        .collect();
    let result = resolve_path_template("/v1/users/{user_id}", &params);
    assert_eq!(result, "/v1/users/42");
}

#[test]
fn path_template_multiple_params() {
    let params = [
        ("user_id".to_string(), "42".to_string()),
        ("order_id".to_string(), "99".to_string()),
    ]
    .into_iter()
    .collect();
    let result = resolve_path_template("/v1/users/{user_id}/orders/{order_id}", &params);
    assert_eq!(result, "/v1/users/42/orders/99");
}

#[test]
fn path_template_no_params() {
    let params = HashMap::new();
    let result = resolve_path_template("/v1/health", &params);
    assert_eq!(result, "/v1/health");
}

#[test]
fn path_template_unused_param_ignored() {
    let params = [("extra".to_string(), "ignored".to_string())]
        .into_iter()
        .collect();
    let result = resolve_path_template("/v1/health", &params);
    assert_eq!(result, "/v1/health");
}

// ── Body Templating ────────────────────────────────────────────────

#[test]
fn body_template_string_substitution() {
    let template = serde_json::json!({
        "model": "gpt-4",
        "input": "{text}",
        "max_tokens": 1024
    });
    let params = [("text".to_string(), serde_json::json!("hello world"))]
        .into_iter()
        .collect();
    let result = resolve_body_template(&template, &params);
    assert_eq!(result["model"], "gpt-4");
    assert_eq!(result["input"], "hello world");
    assert_eq!(result["max_tokens"], 1024);
}

#[test]
fn body_template_preserves_types() {
    let template = serde_json::json!({
        "count": "{n}",
        "active": true,
        "name": "literal"
    });
    let params = [("n".to_string(), serde_json::json!(42))]
        .into_iter()
        .collect();
    let result = resolve_body_template(&template, &params);
    assert_eq!(result["count"], 42); // Integer, not string
    assert_eq!(result["active"], true);
    assert_eq!(result["name"], "literal");
}

#[test]
fn body_template_no_params() {
    let template = serde_json::json!({"key": "value"});
    let params = HashMap::new();
    let result = resolve_body_template(&template, &params);
    assert_eq!(result, template);
}

#[test]
fn body_template_missing_param_stays_placeholder() {
    let template = serde_json::json!({"input": "{missing}"});
    let params = HashMap::new();
    let result = resolve_body_template(&template, &params);
    assert_eq!(result["input"], "{missing}");
}

#[test]
fn body_template_nested_object() {
    let template = serde_json::json!({
        "outer": {
            "inner": "{val}"
        }
    });
    let params = [("val".to_string(), serde_json::json!("replaced"))]
        .into_iter()
        .collect();
    let result = resolve_body_template(&template, &params);
    assert_eq!(result["outer"]["inner"], "replaced");
}

#[test]
fn body_template_array_values() {
    let template = serde_json::json!({
        "items": ["{a}", "{b}"]
    });
    let params = [
        ("a".to_string(), serde_json::json!(1)),
        ("b".to_string(), serde_json::json!(2)),
    ]
    .into_iter()
    .collect();
    let result = resolve_body_template(&template, &params);
    assert_eq!(result["items"][0], 1);
    assert_eq!(result["items"][1], 2);
}

// ── Response Mapping ───────────────────────────────────────────────

#[test]
fn response_json_object_to_single_row() {
    let body = serde_json::json!({"id": 1, "name": "Alice"});
    let result = response_to_query_result(&body);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.rows[0].get("id").unwrap(), &QueryValue::Integer(1));
    assert_eq!(
        result.rows[0].get("name").unwrap(),
        &QueryValue::String("Alice".into())
    );
}

#[test]
fn response_json_array_to_multiple_rows() {
    let body = serde_json::json!([
        {"id": 1, "name": "Alice"},
        {"id": 2, "name": "Bob"}
    ]);
    let result = response_to_query_result(&body);
    assert_eq!(result.rows.len(), 2);
    assert_eq!(result.affected_rows, 2);
    assert_eq!(result.rows[1].get("name").unwrap(), &QueryValue::String("Bob".into()));
}

#[test]
fn response_null_to_empty() {
    let body = serde_json::Value::Null;
    let result = response_to_query_result(&body);
    assert!(result.rows.is_empty());
    assert_eq!(result.affected_rows, 0);
}

#[test]
fn response_empty_array_to_empty() {
    let body = serde_json::json!([]);
    let result = response_to_query_result(&body);
    assert!(result.rows.is_empty());
    assert_eq!(result.affected_rows, 0);
}

#[test]
fn response_scalar_array_wrapped() {
    let body = serde_json::json!([1, 2, 3]);
    let result = response_to_query_result(&body);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(result.rows[0].get("value").unwrap(), &QueryValue::Integer(1));
}

#[test]
fn response_nested_json_preserved() {
    let body = serde_json::json!({"data": {"nested": true}});
    let result = response_to_query_result(&body);
    assert_eq!(result.rows.len(), 1);
    match result.rows[0].get("data").unwrap() {
        QueryValue::Json(v) => assert_eq!(v["nested"], true),
        other => panic!("expected Json, got: {:?}", other),
    }
}

#[test]
fn wrap_non_json() {
    let result = wrap_non_json_response("<html>hello</html>", "text/html");
    assert_eq!(result["raw"], "<html>hello</html>");
    assert_eq!(result["content_type"], "text/html");
}

// ── Validation ─────────────────────────────────────────────────────

#[test]
fn validate_valid_dataview() {
    let config = HttpDataViewConfig {
        datasource: "openai".into(),
        method: HttpMethod::Post,
        path: "/v1/users/{user_id}".into(),
        headers: HashMap::new(),
        query_params: HashMap::new(),
        body_template: None,
        parameters: vec![HttpDataViewParam {
            name: "user_id".into(),
            location: ParamLocation::Path,
            required: true,
            default: None,
        }],
        success_status: vec![200],
        return_schema: None,
        timeout_ms: None,
    };
    let errors = validate_http_dataview(&config);
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
}

#[test]
fn validate_empty_success_status() {
    let config = HttpDataViewConfig {
        datasource: "test".into(),
        method: HttpMethod::Get,
        path: "/health".into(),
        headers: HashMap::new(),
        query_params: HashMap::new(),
        body_template: None,
        parameters: vec![],
        success_status: vec![],
        return_schema: None,
        timeout_ms: None,
    };
    let errors = validate_http_dataview(&config);
    assert!(errors.iter().any(|e| e.contains("success_status")));
}

#[test]
fn validate_path_param_not_in_template() {
    let config = HttpDataViewConfig {
        datasource: "test".into(),
        method: HttpMethod::Get,
        path: "/v1/users".into(),
        headers: HashMap::new(),
        query_params: HashMap::new(),
        body_template: None,
        parameters: vec![HttpDataViewParam {
            name: "user_id".into(),
            location: ParamLocation::Path,
            required: true,
            default: None,
        }],
        success_status: vec![200],
        return_schema: None,
        timeout_ms: None,
    };
    let errors = validate_http_dataview(&config);
    assert!(errors.iter().any(|e| e.contains("path parameter 'user_id'")));
}

#[test]
fn validate_undeclared_path_placeholder() {
    let config = HttpDataViewConfig {
        datasource: "test".into(),
        method: HttpMethod::Get,
        path: "/v1/users/{unknown}".into(),
        headers: HashMap::new(),
        query_params: HashMap::new(),
        body_template: None,
        parameters: vec![],
        success_status: vec![200],
        return_schema: None,
        timeout_ms: None,
    };
    let errors = validate_http_dataview(&config);
    assert!(errors.iter().any(|e| e.contains("undeclared parameter 'unknown'")));
}

#[test]
fn validate_api_key_empty_header() {
    let auth = AuthConfig::ApiKey {
        credentials: "lockbox://key".into(),
        auth_header: "".into(),
    };
    let errors = validate_http_auth(&auth);
    assert!(errors.iter().any(|e| e.contains("auth_header")));
}

#[test]
fn validate_api_key_valid() {
    let auth = AuthConfig::ApiKey {
        credentials: "lockbox://key".into(),
        auth_header: "X-Api-Key".into(),
    };
    let errors = validate_http_auth(&auth);
    assert!(errors.is_empty());
}

#[test]
fn validate_retry_zero_attempts() {
    let config = RetryConfig {
        attempts: 0,
        ..Default::default()
    };
    let errors = validate_retry_config(&config);
    assert!(errors.iter().any(|e| e.contains("at least 1")));
}

#[test]
fn validate_retry_valid() {
    let errors = validate_retry_config(&RetryConfig::default());
    assert!(errors.is_empty());
}

#[test]
fn validate_cb_zero_threshold() {
    let config = CircuitBreakerConfig {
        failure_threshold: 0,
        ..Default::default()
    };
    let errors = validate_circuit_breaker_config(&config);
    assert!(errors.iter().any(|e| e.contains("at least 1")));
}

#[test]
fn validate_cb_valid() {
    let errors = validate_circuit_breaker_config(&CircuitBreakerConfig::default());
    assert!(errors.is_empty());
}

// ── Config Defaults ────────────────────────────────────────────────

#[test]
fn retry_config_defaults() {
    let config = RetryConfig::default();
    assert_eq!(config.attempts, 3);
    assert_eq!(config.backoff, BackoffStrategy::Exponential);
    assert_eq!(config.base_delay_ms, 100);
    assert_eq!(config.max_delay_ms, 5000);
    assert_eq!(config.retry_on_status, vec![429, 502, 503, 504]);
    assert!(config.retry_on_timeout);
}

#[test]
fn circuit_breaker_defaults() {
    let config = CircuitBreakerConfig::default();
    assert_eq!(config.failure_threshold, 5);
    assert_eq!(config.window_ms, 10000);
    assert_eq!(config.open_duration_ms, 30000);
    assert_eq!(config.half_open_attempts, 1);
}

#[test]
fn http_protocol_default() {
    assert_eq!(HttpProtocol::default(), HttpProtocol::Http);
}

#[test]
fn http_method_default() {
    assert_eq!(HttpMethod::default(), HttpMethod::Get);
}

#[test]
fn success_status_defaults() {
    let config = HttpDataViewConfig {
        datasource: "test".into(),
        method: HttpMethod::Get,
        path: "/".into(),
        headers: HashMap::new(),
        query_params: HashMap::new(),
        body_template: None,
        parameters: vec![],
        success_status: vec![200, 201, 202, 204],
        return_schema: None,
        timeout_ms: None,
    };
    assert_eq!(config.success_status, vec![200, 201, 202, 204]);
}

// ── Error Types ────────────────────────────────────────────────────

#[test]
fn http_driver_error_display() {
    let err = HttpDriverError::UnexpectedStatus {
        status: 404,
        body: "not found".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("404"));
    assert!(msg.contains("not found"));
}

#[test]
fn http_driver_error_circuit_open() {
    let err = HttpDriverError::CircuitOpen;
    assert!(err.to_string().contains("circuit breaker"));
}

#[test]
fn http_driver_error_timeout() {
    let err = HttpDriverError::Timeout(5000);
    assert!(err.to_string().contains("5000"));
}

// ── Auth Config ────────────────────────────────────────────────────

#[test]
fn auth_config_none_default() {
    let auth = AuthConfig::default();
    matches!(auth, AuthConfig::None);
}

#[test]
fn auth_state_active() {
    let state = AuthState::Active {
        header_name: "Authorization".into(),
        header_value: "Bearer token123".into(),
    };
    match state {
        AuthState::Active {
            header_name,
            header_value,
        } => {
            assert_eq!(header_name, "Authorization");
            assert_eq!(header_value, "Bearer token123");
        }
        _ => panic!("expected Active"),
    }
}
