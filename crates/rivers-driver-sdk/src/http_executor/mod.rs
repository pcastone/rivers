//! Reqwest-based HTTP driver implementation.
//!
//! Implements `HttpDriver` and `HttpConnection` traits from `http_driver.rs`
//! using `reqwest` for actual HTTP/HTTPS requests.

mod circuit_breaker;
mod oauth2;
mod connection;
mod sse_stream;
mod driver;

// Re-export public types at the module level so external import paths remain unchanged.
pub use connection::ReqwestHttpConnection;
pub use driver::ReqwestHttpDriver;
pub use sse_stream::SseStreamConnection;

use crate::http_driver::{
    CircuitBreakerConfig, HttpConnectionParams, HttpDriverError, RetryConfig,
};

// ── Public helper: build connection with options ───────────────────

/// Build a `ReqwestHttpConnection` with optional retry and circuit breaker configs.
///
/// This is a convenience function for callers that need to configure retry/circuit
/// breaker without going through the driver trait.
pub async fn build_connection(
    params: &HttpConnectionParams,
    retry: Option<RetryConfig>,
    circuit_breaker: Option<CircuitBreakerConfig>,
) -> Result<ReqwestHttpConnection, HttpDriverError> {
    let driver = ReqwestHttpDriver::new();
    let auth_state =
        ReqwestHttpDriver::resolve_auth(&params.auth, &driver.oauth2_cache).await?;
    let client = ReqwestHttpDriver::build_client(params)?;

    let mut conn = ReqwestHttpConnection {
        client,
        base_url: params.base_url.clone(),
        auth_state,
        retry_config: None,
        circuit_breaker: None,
        default_timeout_ms: params.timeout_ms,
        session_claims: None,
    };

    if let Some(r) = retry {
        conn = conn.with_retry(r);
    }
    if let Some(cb) = circuit_breaker {
        conn = conn.with_circuit_breaker(cb);
    }

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::circuit_breaker::{CircuitBreaker, CircuitState};
    use super::sse_stream::parse_sse_event;

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use tokio::sync::RwLock;

    use crate::http_driver::{
        AuthConfig, AuthState, BackoffStrategy, CircuitBreakerConfig,
        HttpConnectionParams, HttpDriver, HttpMethod, HttpProtocol, HttpRequest,
        RetryConfig, TlsConfig,
    };

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
    fn test_driver_construction() {
        let driver = ReqwestHttpDriver::new();
        assert_eq!(driver.name(), "http");
    }

    #[test]
    fn test_default_driver() {
        let driver = ReqwestHttpDriver::default();
        assert_eq!(driver.name(), "http");
    }

    #[test]
    fn test_build_client_default() {
        let params = default_params();
        let client = ReqwestHttpDriver::build_client(&params);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_no_tls_verify() {
        let mut params = default_params();
        params.tls.verify = false;
        let client = ReqwestHttpDriver::build_client(&params);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_http2() {
        let mut params = default_params();
        params.protocol = HttpProtocol::Http2;
        let client = ReqwestHttpDriver::build_client(&params);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_resolve_auth_none() {
        let cache = Arc::new(RwLock::new(None));
        let state = ReqwestHttpDriver::resolve_auth(&AuthConfig::None, &cache)
            .await
            .unwrap();
        assert!(matches!(state, AuthState::None));
    }

    #[tokio::test]
    async fn test_resolve_auth_bearer() {
        let cache = Arc::new(RwLock::new(None));
        let auth = AuthConfig::Bearer {
            credentials: "my-secret-token".to_string(),
        };
        let state = ReqwestHttpDriver::resolve_auth(&auth, &cache)
            .await
            .unwrap();
        match state {
            AuthState::Active {
                header_name,
                header_value,
            } => {
                assert_eq!(header_name, "Authorization");
                assert_eq!(header_value, "Bearer my-secret-token");
            }
            _ => panic!("expected Active auth state"),
        }
    }

    #[tokio::test]
    async fn test_resolve_auth_basic_json() {
        let cache = Arc::new(RwLock::new(None));
        let credentials = r#"{"username":"admin","password":"secret"}"#.to_string();
        let auth = AuthConfig::Basic { credentials };
        let state = ReqwestHttpDriver::resolve_auth(&auth, &cache)
            .await
            .unwrap();
        match state {
            AuthState::Active {
                header_name,
                header_value,
            } => {
                assert_eq!(header_name, "Authorization");
                let expected = format!("Basic {}", BASE64_STANDARD.encode("admin:secret"));
                assert_eq!(header_value, expected);
            }
            _ => panic!("expected Active auth state"),
        }
    }

    #[tokio::test]
    async fn test_resolve_auth_basic_raw() {
        let cache = Arc::new(RwLock::new(None));
        let auth = AuthConfig::Basic {
            credentials: "user:pass".to_string(),
        };
        let state = ReqwestHttpDriver::resolve_auth(&auth, &cache)
            .await
            .unwrap();
        match state {
            AuthState::Active {
                header_name,
                header_value,
            } => {
                assert_eq!(header_name, "Authorization");
                let expected = format!("Basic {}", BASE64_STANDARD.encode("user:pass"));
                assert_eq!(header_value, expected);
            }
            _ => panic!("expected Active auth state"),
        }
    }

    #[tokio::test]
    async fn test_resolve_auth_apikey() {
        let cache = Arc::new(RwLock::new(None));
        let auth = AuthConfig::ApiKey {
            credentials: "key-12345".to_string(),
            auth_header: "X-Api-Key".to_string(),
        };
        let state = ReqwestHttpDriver::resolve_auth(&auth, &cache)
            .await
            .unwrap();
        match state {
            AuthState::Active {
                header_name,
                header_value,
            } => {
                assert_eq!(header_name, "X-Api-Key");
                assert_eq!(header_value, "key-12345");
            }
            _ => panic!("expected Active auth state"),
        }
    }

    #[test]
    fn test_request_building_get() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://api.example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: None,
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        let request = HttpRequest {
            method: HttpMethod::Get,
            path: "/v1/users".to_string(),
            headers: HashMap::new(),
            query: HashMap::new(),
            body: None,
            timeout_ms: None,
        };

        let result = conn.build_request(&request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_request_building_post_with_body() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://api.example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: None,
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());

        let mut query = HashMap::new();
        query.insert("page".to_string(), "1".to_string());

        let request = HttpRequest {
            method: HttpMethod::Post,
            path: "/v1/users".to_string(),
            headers,
            query,
            body: Some(serde_json::json!({"name": "Alice"})),
            timeout_ms: Some(10000),
        };

        let result = conn.build_request(&request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_request_building_with_auth() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://api.example.com".to_string(),
            auth_state: AuthState::Active {
                header_name: "Authorization".to_string(),
                header_value: "Bearer test-token".to_string(),
            },
            retry_config: None,
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        let request = HttpRequest {
            method: HttpMethod::Get,
            path: "/secure".to_string(),
            headers: HashMap::new(),
            query: HashMap::new(),
            body: None,
            timeout_ms: None,
        };

        let result = conn.build_request(&request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_retry_delay_exponential() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig {
                attempts: 3,
                backoff: BackoffStrategy::Exponential,
                base_delay_ms: 100,
                max_delay_ms: 5000,
                retry_on_status: vec![429, 503],
                retry_on_timeout: true,
            }),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        assert_eq!(conn.retry_delay(1), Duration::from_millis(100));
        assert_eq!(conn.retry_delay(2), Duration::from_millis(200));
        assert_eq!(conn.retry_delay(3), Duration::from_millis(400));
    }

    #[test]
    fn test_retry_delay_linear() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig {
                attempts: 3,
                backoff: BackoffStrategy::Linear,
                base_delay_ms: 100,
                max_delay_ms: 5000,
                retry_on_status: vec![429],
                retry_on_timeout: true,
            }),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        assert_eq!(conn.retry_delay(1), Duration::from_millis(100));
        assert_eq!(conn.retry_delay(2), Duration::from_millis(200));
        assert_eq!(conn.retry_delay(3), Duration::from_millis(300));
    }

    #[test]
    fn test_retry_delay_none() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig {
                attempts: 3,
                backoff: BackoffStrategy::None,
                base_delay_ms: 100,
                max_delay_ms: 5000,
                retry_on_status: vec![429],
                retry_on_timeout: true,
            }),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        assert_eq!(conn.retry_delay(1), Duration::ZERO);
        assert_eq!(conn.retry_delay(2), Duration::ZERO);
    }

    #[test]
    fn test_retry_delay_max_cap() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig {
                attempts: 10,
                backoff: BackoffStrategy::Exponential,
                base_delay_ms: 1000,
                max_delay_ms: 5000,
                retry_on_status: vec![429],
                retry_on_timeout: true,
            }),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        // 1000 * 2^7 = 128000, capped at 5000
        assert_eq!(conn.retry_delay(8), Duration::from_millis(5000));
    }

    #[test]
    fn test_should_retry_status() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig::default()),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        assert!(conn.should_retry_status(429));
        assert!(conn.should_retry_status(503));
        assert!(!conn.should_retry_status(200));
        assert!(!conn.should_retry_status(404));
    }

    #[test]
    fn test_should_retry_timeout() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig::default()),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        assert!(conn.should_retry_timeout());
    }

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_opens_on_threshold() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            window_ms: 60000,
            open_duration_ms: 30000,
            half_open_attempts: 1,
        });

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed);

        cb.record_failure();
        assert!(matches!(cb.state, CircuitState::Open { .. }));
    }

    #[test]
    fn test_circuit_breaker_rejects_when_open() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            window_ms: 60000,
            open_duration_ms: 30000,
            half_open_attempts: 1,
        });

        cb.record_failure();
        assert!(cb.check().is_err());
    }

    #[test]
    fn test_circuit_breaker_success_resets_half_open() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            window_ms: 60000,
            open_duration_ms: 0, // Immediate transition for testing
            half_open_attempts: 1,
        });

        cb.record_failure();
        assert!(matches!(cb.state, CircuitState::Open { .. }));

        // With 0ms open_duration, check() should transition to half-open
        cb.check().unwrap();
        assert!(matches!(cb.state, CircuitState::HalfOpen { .. }));

        cb.record_success();
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn test_retry_config_validation() {
        let config = RetryConfig {
            attempts: 0,
            ..Default::default()
        };
        let errors = crate::http_driver::validate_retry_config(&config);
        assert!(!errors.is_empty());
        assert!(errors[0].contains("at least 1"));
    }

    #[test]
    fn test_all_http_methods_mapped() {
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: None,
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        let methods = vec![
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
            HttpMethod::Head,
        ];

        for method in methods {
            let request = HttpRequest {
                method,
                path: "/test".to_string(),
                headers: HashMap::new(),
                query: HashMap::new(),
                body: None,
                timeout_ms: None,
            };
            assert!(conn.build_request(&request).is_ok());
        }
    }

    #[tokio::test]
    async fn test_connect_stream_bad_url_returns_error() {
        let driver = ReqwestHttpDriver::new();
        let mut params = default_params();
        params.base_url = "http://127.0.0.1:1/nonexistent-sse".to_string();
        params.timeout_ms = 500;
        let result = driver.connect_stream(&params).await;
        assert!(result.is_err(), "expected connection error for bad URL");
    }

    #[test]
    fn test_parse_sse_event_basic() {
        let mut buffer = "event: message\ndata: {\"hello\":\"world\"}\nid: 1\n\n".to_string();
        let event = parse_sse_event(&mut buffer).unwrap();
        assert_eq!(event.event_type.as_deref(), Some("message"));
        assert_eq!(event.data, serde_json::json!({"hello": "world"}));
        assert_eq!(event.id.as_deref(), Some("1"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_parse_sse_event_data_only() {
        let mut buffer = "data: plain text\n\n".to_string();
        let event = parse_sse_event(&mut buffer).unwrap();
        assert!(event.event_type.is_none());
        assert_eq!(event.data, serde_json::Value::String("plain text".into()));
        assert!(event.id.is_none());
    }

    #[test]
    fn test_parse_sse_event_multiline_data() {
        let mut buffer = "data: line1\ndata: line2\n\n".to_string();
        let event = parse_sse_event(&mut buffer).unwrap();
        assert_eq!(event.data, serde_json::Value::String("line1\nline2".into()));
    }

    #[test]
    fn test_parse_sse_event_incomplete() {
        let mut buffer = "data: partial".to_string();
        assert!(parse_sse_event(&mut buffer).is_none());
        assert_eq!(buffer, "data: partial"); // buffer unchanged
    }

    #[test]
    fn test_parse_sse_event_preserves_remainder() {
        let mut buffer = "data: first\n\ndata: second\n\n".to_string();
        let event = parse_sse_event(&mut buffer).unwrap();
        assert_eq!(event.data, serde_json::Value::String("first".into()));
        assert_eq!(buffer, "data: second\n\n");
    }

    #[tokio::test]
    async fn test_connect_produces_connection() {
        let driver = ReqwestHttpDriver::new();
        let params = default_params();
        let result = driver.connect(&params).await;
        assert!(result.is_ok());
    }

    // ── RW1.1.d — saturating retry backoff arithmetic ────────────

    #[test]
    fn test_retry_backoff_64_retries_converges_to_max() {
        // 64 retries with base=1s and 2× factor — without saturating_mul
        // this would overflow u64 long before max_delay_ms is applied.
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig {
                attempts: 64,
                backoff: BackoffStrategy::Exponential,
                base_delay_ms: 1_000,
                max_delay_ms: 30_000,
                retry_on_status: vec![503],
                retry_on_timeout: true,
            }),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };

        // Every attempt from attempt 6 onward should be capped at max_delay_ms.
        for attempt in 1u32..=64 {
            let d = conn.retry_delay(attempt);
            assert!(
                d <= Duration::from_millis(30_000),
                "attempt {attempt}: delay {d:?} exceeded max_delay_ms"
            );
        }

        // High attempts must converge to exactly max_delay_ms.
        assert_eq!(conn.retry_delay(64), Duration::from_millis(30_000));
    }

    #[test]
    fn test_retry_backoff_no_overflow_panic() {
        // Verifies u64::MAX-level exponents do not panic (saturating_pow).
        let conn = ReqwestHttpConnection {
            client: reqwest::Client::new(),
            base_url: "https://example.com".to_string(),
            auth_state: AuthState::None,
            retry_config: Some(RetryConfig {
                attempts: u32::MAX,
                backoff: BackoffStrategy::Exponential,
                base_delay_ms: u64::MAX,
                max_delay_ms: 5_000,
                retry_on_status: vec![],
                retry_on_timeout: false,
            }),
            circuit_breaker: None,
            default_timeout_ms: 5000,
            session_claims: None,
        };
        // Should not panic, must return max_delay_ms.
        assert_eq!(conn.retry_delay(u32::MAX), Duration::from_millis(5_000));
    }
}
