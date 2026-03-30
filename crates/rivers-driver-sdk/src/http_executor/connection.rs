//! HTTP connection implementation backed by `reqwest`.
//!
//! Handles request building, auth header injection, retry logic,
//! and circuit breaker integration.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::http_driver::{
    AuthState, BackoffStrategy, CircuitBreakerConfig, HttpConnection, HttpDriverError,
    HttpMethod, HttpRequest, HttpResponse, RetryConfig,
};

use super::circuit_breaker::CircuitBreaker;

/// HTTP connection backed by a `reqwest::Client`.
///
/// Handles request building, auth header injection, retry logic,
/// and circuit breaker integration.
pub struct ReqwestHttpConnection {
    pub(crate) client: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) auth_state: AuthState,
    pub(crate) retry_config: Option<RetryConfig>,
    pub(crate) circuit_breaker: Option<Arc<RwLock<CircuitBreaker>>>,
    pub(crate) default_timeout_ms: u64,
    /// Session claims to forward on inter-service calls (X-Rivers-Claims header).
    ///
    /// Per spec section 7.5: cross-app session propagation.
    pub session_claims: Option<String>,
}

impl ReqwestHttpConnection {
    /// Set retry configuration for this connection.
    pub fn with_retry(mut self, config: RetryConfig) -> Self {
        self.retry_config = Some(config);
        self
    }

    /// Set circuit breaker configuration for this connection.
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = Some(Arc::new(RwLock::new(CircuitBreaker::new(config))));
        self
    }

    /// Build a reqwest::RequestBuilder from an HttpRequest.
    pub(crate) fn build_request(&self, request: &HttpRequest) -> Result<reqwest::RequestBuilder, HttpDriverError> {
        let url = format!("{}{}", self.base_url, request.path);

        let method = match &request.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
            HttpMethod::Head => reqwest::Method::HEAD,
        };

        let mut builder = self.client.request(method, &url);

        // Set timeout
        let timeout_ms = request.timeout_ms.unwrap_or(self.default_timeout_ms);
        builder = builder.timeout(Duration::from_millis(timeout_ms));

        // Add query parameters
        if !request.query.is_empty() {
            builder = builder.query(&request.query.iter().collect::<Vec<_>>());
        }

        // Add headers
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        // Inject auth header
        if let AuthState::Active {
            ref header_name,
            ref header_value,
        } = self.auth_state
        {
            builder = builder.header(header_name.as_str(), header_value.as_str());
        }

        // Inject session claims header for inter-service calls (section 7.5)
        if let Some(ref claims) = self.session_claims {
            builder = builder.header("X-Rivers-Claims", claims.as_str());
        }

        // Set body
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }

        Ok(builder)
    }

    /// Execute a single request attempt (no retry).
    async fn execute_once(&self, request: &HttpRequest) -> Result<HttpResponse, HttpDriverError> {
        let builder = self.build_request(request)?;

        let response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                let timeout_ms = request.timeout_ms.unwrap_or(self.default_timeout_ms);
                HttpDriverError::Timeout(timeout_ms)
            } else if e.is_connect() {
                HttpDriverError::Connection(e.to_string())
            } else {
                HttpDriverError::Request(e.to_string())
            }
        })?;

        let status = response.status().as_u16();

        // Collect response headers
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();

        // Parse body as JSON, falling back to string wrapping
        let content_type = headers
            .get("content-type")
            .cloned()
            .unwrap_or_default();
        let body_text = response.text().await.map_err(|e| {
            HttpDriverError::Request(format!("failed to read response body: {}", e))
        })?;

        let body = if content_type.contains("application/json") || body_text.starts_with('{') || body_text.starts_with('[') {
            serde_json::from_str(&body_text).unwrap_or_else(|_| {
                crate::http_driver::wrap_non_json_response(&body_text, &content_type)
            })
        } else if body_text.is_empty() {
            serde_json::Value::Null
        } else {
            crate::http_driver::wrap_non_json_response(&body_text, &content_type)
        };

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Determine if a response status code should be retried.
    pub(crate) fn should_retry_status(&self, status: u16) -> bool {
        if let Some(ref config) = self.retry_config {
            config.retry_on_status.contains(&status)
        } else {
            false
        }
    }

    /// Determine if a timeout error should be retried.
    pub(crate) fn should_retry_timeout(&self) -> bool {
        if let Some(ref config) = self.retry_config {
            config.retry_on_timeout
        } else {
            false
        }
    }

    /// Calculate delay for a retry attempt.
    pub(crate) fn retry_delay(&self, attempt: u32) -> Duration {
        if let Some(ref config) = self.retry_config {
            let delay_ms = match config.backoff {
                BackoffStrategy::None => 0,
                BackoffStrategy::Linear => config.base_delay_ms * (attempt as u64),
                BackoffStrategy::Exponential => {
                    config.base_delay_ms * 2u64.pow(attempt.saturating_sub(1))
                }
            };
            Duration::from_millis(delay_ms.min(config.max_delay_ms))
        } else {
            Duration::ZERO
        }
    }
}

#[async_trait]
impl HttpConnection for ReqwestHttpConnection {
    async fn execute(
        &mut self,
        request: &HttpRequest,
    ) -> Result<HttpResponse, HttpDriverError> {
        // Check circuit breaker
        if let Some(ref cb) = self.circuit_breaker {
            let mut cb = cb.write().await;
            cb.check()?;
        }

        let max_attempts = self
            .retry_config
            .as_ref()
            .map(|c| c.attempts)
            .unwrap_or(1);

        let mut last_result: Result<HttpResponse, HttpDriverError> =
            Err(HttpDriverError::Internal("no attempts made".to_string()));

        for attempt in 1..=max_attempts {
            if attempt > 1 {
                let delay = self.retry_delay(attempt);
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }

            match self.execute_once(request).await {
                Ok(resp) => {
                    if self.should_retry_status(resp.status) && attempt < max_attempts {
                        debug!(
                            status = resp.status,
                            attempt,
                            max_attempts,
                            "retryable status, will retry"
                        );
                        last_result = Ok(resp);
                        continue;
                    }
                    // Record success with circuit breaker
                    if let Some(ref cb) = self.circuit_breaker {
                        let mut cb = cb.write().await;
                        cb.record_success();
                    }
                    return Ok(resp);
                }
                Err(HttpDriverError::Timeout(ms)) if self.should_retry_timeout() && attempt < max_attempts => {
                    debug!(attempt, max_attempts, "timeout, will retry");
                    last_result = Err(HttpDriverError::Timeout(ms));
                    continue;
                }
                Err(e) => {
                    // Record failure with circuit breaker
                    if let Some(ref cb) = self.circuit_breaker {
                        let mut cb = cb.write().await;
                        cb.record_failure();
                    }
                    if attempt < max_attempts {
                        debug!(attempt, max_attempts, error = %e, "request failed, will retry");
                        last_result = Err(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        // If we exhausted retries on a retryable status, record failure and return
        if let Some(ref cb) = self.circuit_breaker {
            let mut cb = cb.write().await;
            cb.record_failure();
        }
        last_result
    }
}
