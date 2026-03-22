//! Integration tests for the reqwest-based HTTP driver.
//!
//! These tests exercise construction and config mapping logic
//! without making actual HTTP calls.

use rivers_driver_sdk::http_driver::{
    AuthConfig, AuthState, CircuitBreakerConfig, HttpConnectionParams, HttpDriver, HttpProtocol,
    RetryConfig, TlsConfig,
};
use rivers_driver_sdk::http_executor::{build_connection, ReqwestHttpDriver};

fn default_params() -> HttpConnectionParams {
    HttpConnectionParams {
        base_url: "https://httpbin.org".to_string(),
        protocol: HttpProtocol::Http,
        auth: AuthConfig::None,
        tls: TlsConfig::default(),
        timeout_ms: 5000,
        pool_size: 4,
    }
}

#[test]
fn test_driver_name() {
    let driver = ReqwestHttpDriver::new();
    assert_eq!(driver.name(), "http");
}

#[tokio::test]
async fn test_connect_returns_connection() {
    let driver = ReqwestHttpDriver::new();
    let params = default_params();
    let conn = driver.connect(&params).await;
    assert!(conn.is_ok(), "connect() should succeed with default params");
}

#[tokio::test]
async fn test_connect_with_bearer_auth() {
    let driver = ReqwestHttpDriver::new();
    let mut params = default_params();
    params.auth = AuthConfig::Bearer {
        credentials: "test-token-123".to_string(),
    };
    let conn = driver.connect(&params).await;
    assert!(conn.is_ok(), "connect() with bearer auth should succeed");
}

#[tokio::test]
async fn test_connect_with_basic_auth() {
    let driver = ReqwestHttpDriver::new();
    let mut params = default_params();
    params.auth = AuthConfig::Basic {
        credentials: r#"{"username":"user","password":"pass"}"#.to_string(),
    };
    let conn = driver.connect(&params).await;
    assert!(conn.is_ok(), "connect() with basic auth should succeed");
}

#[tokio::test]
async fn test_connect_with_apikey_auth() {
    let driver = ReqwestHttpDriver::new();
    let mut params = default_params();
    params.auth = AuthConfig::ApiKey {
        credentials: "my-api-key".to_string(),
        auth_header: "X-Api-Key".to_string(),
    };
    let conn = driver.connect(&params).await;
    assert!(conn.is_ok(), "connect() with API key auth should succeed");
}

#[tokio::test]
async fn test_connect_stream_fails_without_server() {
    let driver = ReqwestHttpDriver::new();
    let mut params = default_params();
    params.base_url = "http://127.0.0.1:1".to_string(); // unreachable
    let result = driver.connect_stream(&params).await;
    assert!(result.is_err(), "connect_stream should fail without a running server");
}

#[tokio::test]
async fn test_refresh_auth_none() {
    let driver = ReqwestHttpDriver::new();
    let params = default_params();
    let state = driver.refresh_auth(&params).await.unwrap();
    assert!(matches!(state, AuthState::None));
}

#[tokio::test]
async fn test_refresh_auth_bearer() {
    let driver = ReqwestHttpDriver::new();
    let mut params = default_params();
    params.auth = AuthConfig::Bearer {
        credentials: "refresh-token".to_string(),
    };
    let state = driver.refresh_auth(&params).await.unwrap();
    match state {
        AuthState::Active {
            header_name,
            header_value,
        } => {
            assert_eq!(header_name, "Authorization");
            assert_eq!(header_value, "Bearer refresh-token");
        }
        _ => panic!("expected Active state"),
    }
}

#[tokio::test]
async fn test_build_connection_with_retry() {
    let params = default_params();
    let retry = RetryConfig::default();
    let conn = build_connection(&params, Some(retry), None).await;
    assert!(conn.is_ok(), "build_connection with retry should succeed");
}

#[tokio::test]
async fn test_build_connection_with_circuit_breaker() {
    let params = default_params();
    let cb = CircuitBreakerConfig::default();
    let conn = build_connection(&params, None, Some(cb)).await;
    assert!(
        conn.is_ok(),
        "build_connection with circuit breaker should succeed"
    );
}

#[tokio::test]
async fn test_build_connection_with_both() {
    let params = default_params();
    let retry = RetryConfig::default();
    let cb = CircuitBreakerConfig::default();
    let conn = build_connection(&params, Some(retry), Some(cb)).await;
    assert!(
        conn.is_ok(),
        "build_connection with retry + circuit breaker should succeed"
    );
}

#[tokio::test]
async fn test_oauth2_invalid_credentials() {
    let driver = ReqwestHttpDriver::new();
    let mut params = default_params();
    params.auth = AuthConfig::OAuth2ClientCredentials {
        credentials: "not-valid-json".to_string(),
        refresh_buffer_s: 60,
        auth_retry_attempts: 1,
    };
    let result = driver.connect(&params).await;
    match result {
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("parse"),
                "expected parse error, got: {}",
                err_msg
            );
        }
        Ok(_) => panic!("expected error for invalid OAuth2 credentials"),
    }
}
