//! Reqwest-based HTTP driver implementation.
//!
//! Implements `HttpDriver` and `HttpConnection` traits from `http_driver.rs`
//! using `reqwest` for actual HTTP/HTTPS requests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::http_driver::{
    AuthConfig, AuthState, BackoffStrategy, CircuitBreakerConfig, HttpConnection,
    HttpConnectionParams, HttpDriver, HttpDriverError, HttpMethod, HttpProtocol, HttpRequest,
    HttpResponse, HttpStreamConnection, HttpStreamEvent, RetryConfig,
};

// ── Circuit Breaker State ──────────────────────────────────────────

/// Circuit breaker states per the standard Closed -> Open -> Half-Open -> Closed model.
#[derive(Debug, Clone, PartialEq)]
enum CircuitState {
    Closed,
    Open { opened_at: Instant },
    HalfOpen { successes: u32 },
}

/// Internal circuit breaker tracking.
#[derive(Debug)]
struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: CircuitState,
    /// Recent failure timestamps within the rolling window.
    failures: Vec<Instant>,
}

impl CircuitBreaker {
    fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: CircuitState::Closed,
            failures: Vec::new(),
        }
    }

    /// Check if a request is allowed. Returns Err if circuit is open.
    fn check(&mut self) -> Result<(), HttpDriverError> {
        match &self.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open { opened_at } => {
                let elapsed = opened_at.elapsed().as_millis() as u64;
                if elapsed >= self.config.open_duration_ms {
                    // Transition to half-open
                    self.state = CircuitState::HalfOpen { successes: 0 };
                    debug!("circuit breaker transitioning to half-open");
                    Ok(())
                } else {
                    Err(HttpDriverError::CircuitOpen)
                }
            }
            CircuitState::HalfOpen { .. } => Ok(()),
        }
    }

    /// Record a successful request.
    fn record_success(&mut self) {
        match &self.state {
            CircuitState::HalfOpen { successes } => {
                let new_successes = successes + 1;
                if new_successes >= self.config.half_open_attempts {
                    debug!("circuit breaker closing after {} successful probes", new_successes);
                    self.state = CircuitState::Closed;
                    self.failures.clear();
                } else {
                    self.state = CircuitState::HalfOpen {
                        successes: new_successes,
                    };
                }
            }
            CircuitState::Closed => {
                // Prune old failures outside the window
                self.prune_old_failures();
            }
            _ => {}
        }
    }

    /// Record a failed request.
    fn record_failure(&mut self) {
        let now = Instant::now();
        match &self.state {
            CircuitState::Closed => {
                self.failures.push(now);
                self.prune_old_failures();
                if self.failures.len() as u32 >= self.config.failure_threshold {
                    warn!(
                        "circuit breaker opening after {} failures in window",
                        self.failures.len()
                    );
                    self.state = CircuitState::Open { opened_at: now };
                }
            }
            CircuitState::HalfOpen { .. } => {
                // Any failure in half-open re-opens the circuit
                warn!("circuit breaker re-opening from half-open state");
                self.state = CircuitState::Open { opened_at: now };
            }
            _ => {}
        }
    }

    fn prune_old_failures(&mut self) {
        let cutoff = Instant::now() - Duration::from_millis(self.config.window_ms);
        self.failures.retain(|t| *t > cutoff);
    }
}

// ── OAuth2 Token Cache ─────────────────────────────────────────────

/// Cached OAuth2 access token with expiry tracking.
#[derive(Debug, Clone)]
struct CachedToken {
    /// The access token value.
    access_token: String,
    /// When this token expires (absolute time).
    expires_at: Instant,
}

/// OAuth2 credentials parsed from LockBox secret.
#[derive(Debug, Clone, serde::Deserialize)]
struct OAuth2Credentials {
    client_id: String,
    client_secret: String,
    token_url: String,
    #[serde(default)]
    scope: Option<String>,
}

/// OAuth2 token endpoint response.
#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
    #[allow(dead_code)]
    token_type: Option<String>,
}

fn default_expires_in() -> u64 {
    3600
}

// ── ReqwestHttpDriver ──────────────────────────────────────────────

/// HTTP driver implementation backed by `reqwest`.
///
/// Each `connect()` call produces a `ReqwestHttpConnection` that shares
/// the underlying `reqwest::Client` connection pool.
pub struct ReqwestHttpDriver {
    /// Cached OAuth2 token (shared across connections from the same driver).
    oauth2_cache: Arc<RwLock<Option<CachedToken>>>,
}

impl ReqwestHttpDriver {
    /// Create a new ReqwestHttpDriver.
    pub fn new() -> Self {
        Self {
            oauth2_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Build a reqwest::Client from connection params.
    fn build_client(params: &HttpConnectionParams) -> Result<reqwest::Client, HttpDriverError> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(params.timeout_ms))
            .timeout(Duration::from_millis(params.timeout_ms))
            .pool_max_idle_per_host(params.pool_size as usize);

        // TLS configuration
        if !params.tls.verify {
            builder = builder.danger_accept_invalid_certs(true);
        }

        // Force HTTP/2 if requested
        if params.protocol == HttpProtocol::Http2 {
            builder = builder.http2_prior_knowledge();
        }

        builder.build().map_err(|e| {
            HttpDriverError::Connection(format!("failed to build HTTP client: {}", e))
        })
    }

    /// Resolve auth config into an AuthState with resolved header name/value.
    ///
    /// For OAuth2, this checks the cache and fetches a new token if needed.
    async fn resolve_auth(
        auth: &AuthConfig,
        oauth2_cache: &Arc<RwLock<Option<CachedToken>>>,
    ) -> Result<AuthState, HttpDriverError> {
        match auth {
            AuthConfig::None => Ok(AuthState::None),
            AuthConfig::Bearer { credentials } => Ok(AuthState::Active {
                header_name: "Authorization".to_string(),
                header_value: format!("Bearer {}", credentials),
            }),
            AuthConfig::Basic { credentials } => {
                // credentials is expected to be "username:password" or a base64-encoded value.
                // Per spec, LockBox secret is JSON: { "username": "...", "password": "..." }
                // We'll try to parse as JSON first, fall back to raw string.
                let encoded = if let Ok(parsed) =
                    serde_json::from_str::<serde_json::Value>(credentials)
                {
                    let user = parsed
                        .get("username")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let pass = parsed
                        .get("password")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    BASE64_STANDARD.encode(format!("{}:{}", user, pass))
                } else {
                    BASE64_STANDARD.encode(credentials.as_bytes())
                };
                Ok(AuthState::Active {
                    header_name: "Authorization".to_string(),
                    header_value: format!("Basic {}", encoded),
                })
            }
            AuthConfig::ApiKey {
                credentials,
                auth_header,
            } => Ok(AuthState::Active {
                header_name: auth_header.clone(),
                header_value: credentials.clone(),
            }),
            AuthConfig::OAuth2ClientCredentials {
                credentials,
                refresh_buffer_s,
                auth_retry_attempts,
            } => {
                // Check cache first
                {
                    let cache = oauth2_cache.read().await;
                    if let Some(cached) = &*cache {
                        let buffer = Duration::from_secs(*refresh_buffer_s);
                        if cached.expires_at > Instant::now() + buffer {
                            return Ok(AuthState::Active {
                                header_name: "Authorization".to_string(),
                                header_value: format!("Bearer {}", cached.access_token),
                            });
                        }
                    }
                }

                // Need to fetch a new token
                let oauth2_creds: OAuth2Credentials =
                    serde_json::from_str(credentials).map_err(|e| {
                        HttpDriverError::AuthRefresh(format!(
                            "failed to parse OAuth2 credentials: {}",
                            e
                        ))
                    })?;

                let token =
                    fetch_oauth2_token(&oauth2_creds, *auth_retry_attempts).await?;

                let cached = CachedToken {
                    access_token: token.access_token.clone(),
                    expires_at: Instant::now() + Duration::from_secs(token.expires_in),
                };

                // Update cache
                {
                    let mut cache = oauth2_cache.write().await;
                    *cache = Some(cached);
                }

                Ok(AuthState::Active {
                    header_name: "Authorization".to_string(),
                    header_value: format!("Bearer {}", token.access_token),
                })
            }
        }
    }
}

impl Default for ReqwestHttpDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch an OAuth2 token from the token endpoint with retry.
async fn fetch_oauth2_token(
    creds: &OAuth2Credentials,
    max_attempts: u32,
) -> Result<TokenResponse, HttpDriverError> {
    let client = reqwest::Client::new();
    let mut last_err = String::new();

    for attempt in 0..max_attempts {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 * 2u64.pow(attempt - 1))).await;
        }

        let mut form = vec![
            ("grant_type", "client_credentials".to_string()),
            ("client_id", creds.client_id.clone()),
            ("client_secret", creds.client_secret.clone()),
        ];
        if let Some(scope) = &creds.scope {
            form.push(("scope", scope.clone()));
        }

        match client.post(&creds.token_url).form(&form).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<TokenResponse>().await {
                        Ok(token) => return Ok(token),
                        Err(e) => {
                            last_err = format!("failed to parse token response: {}", e);
                        }
                    }
                } else {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    last_err = format!("token endpoint returned {}: {}", status, body);
                }
            }
            Err(e) => {
                last_err = format!("token request failed: {}", e);
            }
        }
    }

    Err(HttpDriverError::AuthRefresh(last_err))
}

#[async_trait]
impl HttpDriver for ReqwestHttpDriver {
    fn name(&self) -> &str {
        "http"
    }

    async fn connect(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<Box<dyn HttpConnection>, HttpDriverError> {
        let client = Self::build_client(params)?;
        let auth_state =
            Self::resolve_auth(&params.auth, &self.oauth2_cache).await?;

        Ok(Box::new(ReqwestHttpConnection {
            client,
            base_url: params.base_url.clone(),
            auth_state,
            retry_config: None,
            circuit_breaker: None,
            default_timeout_ms: params.timeout_ms,
            session_claims: None,
        }))
    }

    async fn connect_stream(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<Box<dyn HttpStreamConnection>, HttpDriverError> {
        // WebSocket streaming requires a different transport — return an explicit
        // error instead of silently falling through to an SSE connection.
        if params.protocol == HttpProtocol::WebSocket {
            return Err(HttpDriverError::Config(
                "WebSocket streaming not yet implemented".into(),
            ));
        }

        let client = Self::build_client(params)?;
        let auth_state =
            Self::resolve_auth(&params.auth, &self.oauth2_cache).await?;

        let mut request = client.get(&params.base_url)
            .header("Accept", "text/event-stream");

        // Inject auth header
        if let AuthState::Active {
            ref header_name,
            ref header_value,
        } = auth_state
        {
            request = request.header(header_name.as_str(), header_value.as_str());
        }

        let response = request
            .send()
            .await
            .map_err(|e| HttpDriverError::Connection(format!("stream connect: {e}")))?;

        if !response.status().is_success() {
            return Err(HttpDriverError::Request(format!(
                "stream connect failed: HTTP {}",
                response.status()
            )));
        }

        Ok(Box::new(SseStreamConnection {
            response,
            buffer: String::new(),
        }))
    }

    async fn refresh_auth(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<AuthState, HttpDriverError> {
        // Invalidate cache first for OAuth2
        if matches!(params.auth, AuthConfig::OAuth2ClientCredentials { .. }) {
            let mut cache = self.oauth2_cache.write().await;
            *cache = None;
        }
        Self::resolve_auth(&params.auth, &self.oauth2_cache).await
    }
}

// ── ReqwestHttpConnection ──────────────────────────────────────────

/// HTTP connection backed by a `reqwest::Client`.
///
/// Handles request building, auth header injection, retry logic,
/// and circuit breaker integration.
pub struct ReqwestHttpConnection {
    client: reqwest::Client,
    base_url: String,
    auth_state: AuthState,
    retry_config: Option<RetryConfig>,
    circuit_breaker: Option<Arc<RwLock<CircuitBreaker>>>,
    default_timeout_ms: u64,
    /// Session claims to forward on inter-service calls (X-Rivers-Claims header).
    ///
    /// Per spec §7.5: cross-app session propagation.
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
    fn build_request(&self, request: &HttpRequest) -> Result<reqwest::RequestBuilder, HttpDriverError> {
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

        // Inject session claims header for inter-service calls (§7.5)
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
    fn should_retry_status(&self, status: u16) -> bool {
        if let Some(ref config) = self.retry_config {
            config.retry_on_status.contains(&status)
        } else {
            false
        }
    }

    /// Determine if a timeout error should be retried.
    fn should_retry_timeout(&self) -> bool {
        if let Some(ref config) = self.retry_config {
            config.retry_on_timeout
        } else {
            false
        }
    }

    /// Calculate delay for a retry attempt.
    fn retry_delay(&self, attempt: u32) -> Duration {
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

// ── SSE Stream Connection ──────────────────────────────────────────

/// SSE (Server-Sent Events) stream connection backed by a reqwest streaming response.
///
/// Reads chunked data from the upstream, parses SSE wire format
/// (`event:`, `data:`, `id:` lines delimited by double newlines),
/// and yields `HttpStreamEvent` values.
pub struct SseStreamConnection {
    response: reqwest::Response,
    buffer: String,
}

#[async_trait]
impl HttpStreamConnection for SseStreamConnection {
    async fn next(&mut self) -> Result<Option<HttpStreamEvent>, HttpDriverError> {
        loop {
            // Try to parse a complete SSE event from the buffer first
            if let Some(event) = parse_sse_event(&mut self.buffer) {
                return Ok(Some(event));
            }

            // Read more data from the streaming response
            let chunk = self
                .response
                .chunk()
                .await
                .map_err(|e| HttpDriverError::Request(format!("stream read: {e}")))?;

            match chunk {
                None => return Ok(None), // Stream ended
                Some(bytes) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), HttpDriverError> {
        // Drop the response to close the connection
        Ok(())
    }
}

/// Parse a single SSE event from the buffer.
///
/// SSE events are delimited by a double newline (`\n\n`). Each event
/// consists of lines prefixed with `event: `, `data: `, or `id: `.
fn parse_sse_event(buffer: &mut String) -> Option<HttpStreamEvent> {
    // SSE events are delimited by double newline
    if let Some(pos) = buffer.find("\n\n") {
        let event_text = buffer[..pos].to_string();
        *buffer = buffer[pos + 2..].to_string();

        let mut event_type = None;
        let mut data = String::new();
        let mut id = None;

        for line in event_text.lines() {
            if let Some(val) = line.strip_prefix("event: ") {
                event_type = Some(val.to_string());
            } else if let Some(val) = line.strip_prefix("data: ") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(val);
            } else if let Some(val) = line.strip_prefix("id: ") {
                id = Some(val.to_string());
            }
        }

        if !data.is_empty() || event_type.is_some() {
            return Some(HttpStreamEvent {
                event_type,
                data: serde_json::from_str(&data)
                    .unwrap_or(serde_json::Value::String(data)),
                id,
            });
        }
    }
    None
}

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
    use crate::http_driver::TlsConfig;

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
}
