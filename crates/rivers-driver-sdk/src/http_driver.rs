//! HTTP Driver — first-class datasource driver for HTTP/HTTP2/SSE/WebSocket.
//!
//! Per `rivers-http-driver-spec.md` §1-§13.
//!
//! The HTTP driver does not fit `DatabaseDriver` or `MessageBrokerDriver`:
//! - `DatabaseDriver` models a query against a static endpoint — HTTP DataViews
//!   have parameterized paths, methods, headers, and bodies.
//! - `MessageBrokerDriver` models continuous inbound streams with ack/nack —
//!   SSE/WebSocket are read-only push delivery with no ack semantics.
//!
//! `HttpDriver` is a purpose-built trait that owns path templating, auth token
//! lifecycle, protocol negotiation, and dual activation (request/response or streaming).

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{QueryResult, QueryValue};

// ── Error ──────────────────────────────────────────────────────────

/// Errors specific to the HTTP driver.
#[derive(Debug, Error)]
pub enum HttpDriverError {
    /// Connection establishment failed.
    #[error("HTTP connection failed: {0}")]
    Connection(String),

    /// Request execution failed (network, timeout, etc.).
    #[error("HTTP request failed: {0}")]
    Request(String),

    /// Response status code not in `success_status` list.
    #[error("unexpected HTTP status {status}: {body}")]
    UnexpectedStatus {
        /// The HTTP status code received.
        status: u16,
        /// Response body (truncated for error display).
        body: String,
    },

    /// Request timed out.
    #[error("HTTP request timed out after {0}ms")]
    Timeout(u64),

    /// Circuit breaker is open — requests rejected immediately.
    #[error("circuit breaker is open for datasource")]
    CircuitOpen,

    /// Unsupported WebSocket binary frame received.
    #[error("unsupported WebSocket frame type: binary frames are rejected")]
    UnsupportedFrameType,

    /// Auth token refresh failed.
    #[error("auth refresh failed: {0}")]
    AuthRefresh(String),

    /// Configuration validation error.
    #[error("HTTP driver config error: {0}")]
    Config(String),

    /// Internal driver error.
    #[error("HTTP driver internal error: {0}")]
    Internal(String),
}

// ── Traits ─────────────────────────────────────────────────────────

/// HTTP datasource driver.
///
/// Per spec §2. Registers alongside `DatabaseDriver` and `MessageBrokerDriver`
/// in `DriverRegistrar`. Built-in — no plugin crate needed.
#[async_trait]
pub trait HttpDriver: Send + Sync {
    /// Driver name — always "http".
    fn name(&self) -> &str;

    /// Build and return a connection for request/response use.
    async fn connect(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<Box<dyn HttpConnection>, HttpDriverError>;

    /// Build and return a persistent stream connection (SSE or WebSocket).
    async fn connect_stream(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<Box<dyn HttpStreamConnection>, HttpDriverError>;

    /// Refresh auth credentials if needed.
    ///
    /// Called by the pool manager on a background interval for
    /// `oauth2_client_credentials`.
    async fn refresh_auth(
        &self,
        params: &HttpConnectionParams,
    ) -> Result<AuthState, HttpDriverError>;
}

/// HTTP connection for request/response operations.
///
/// Per spec §2. Acquired from a connection pool, executes a single
/// HTTP request and returns the response.
#[async_trait]
pub trait HttpConnection: Send + Sync {
    /// Execute an HTTP request and return the response.
    async fn execute(
        &mut self,
        request: &HttpRequest,
    ) -> Result<HttpResponse, HttpDriverError>;
}

/// Persistent stream connection for SSE or WebSocket.
///
/// Per spec §2. The `BrokerConsumerBridge` drives the stream,
/// routing events to `MessageConsumer` views via the EventBus.
#[async_trait]
pub trait HttpStreamConnection: Send + Sync {
    /// Receive the next event from the stream.
    ///
    /// Returns `None` when the stream is closed by the remote.
    async fn next(&mut self) -> Result<Option<HttpStreamEvent>, HttpDriverError>;

    /// Close the stream connection.
    async fn close(&mut self) -> Result<(), HttpDriverError>;
}

// ── Connection Params ──────────────────────────────────────────────

/// Parameters for establishing an HTTP connection.
///
/// Per spec §2.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConnectionParams {
    /// Base URL for the upstream (e.g. `"https://api.openai.com"`).
    pub base_url: String,
    /// Protocol to use.
    pub protocol: HttpProtocol,
    /// Auth configuration.
    pub auth: AuthConfig,
    /// TLS configuration.
    pub tls: TlsConfig,
    /// Connection establishment timeout in milliseconds.
    pub timeout_ms: u64,
    /// Maximum number of pooled connections.
    pub pool_size: u32,
}

/// HTTP protocol selection.
///
/// Per spec §3. Determines whether the driver uses request/response
/// or streaming activation path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HttpProtocol {
    /// HTTP/1.1 request/response.
    #[default]
    Http,
    /// HTTP/2 request/response (multiplexed).
    Http2,
    /// Server-Sent Events (persistent inbound stream).
    Sse,
    /// WebSocket (persistent bidirectional stream).
    WebSocket,
}

/// TLS configuration for HTTP connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Whether to verify the upstream's TLS certificate (default: true).
    #[serde(default = "default_tls_verify")]
    pub verify: bool,
    /// Optional CA certificate path for custom CAs.
    pub ca_cert: Option<String>,
    /// Optional client certificate path for mTLS.
    pub client_cert: Option<String>,
    /// Optional client key path for mTLS.
    pub client_key: Option<String>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            verify: true,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        }
    }
}

fn default_tls_verify() -> bool {
    true
}

// ── Auth ────────────────────────────────────────────────────────────

/// Auth configuration for HTTP datasources.
///
/// Per spec §4. Credentials always come from LockBox.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// No authentication.
    #[default]
    None,

    /// Static bearer token. Injected as `Authorization: Bearer <token>`.
    ///
    /// Per spec §4.1.
    Bearer {
        /// LockBox URI for the token.
        credentials: String,
    },

    /// HTTP Basic auth. LockBox secret is `{ "username": "...", "password": "..." }`.
    ///
    /// Per spec §4.2.
    Basic {
        /// LockBox URI for the credentials JSON.
        credentials: String,
    },

    /// API key injected as a named header.
    ///
    /// Per spec §4.3. `auth_header` specifies the header name.
    ApiKey {
        /// LockBox URI for the API key.
        credentials: String,
        /// Header name to inject the key into.
        auth_header: String,
    },

    /// OAuth2 client credentials flow with automatic token refresh.
    ///
    /// Per spec §4.4. LockBox secret is
    /// `{ "client_id": "...", "client_secret": "...", "token_url": "...", "scope": "..." }`.
    OAuth2ClientCredentials {
        /// LockBox URI for the OAuth2 credentials JSON.
        credentials: String,
        /// Seconds before expiry to trigger refresh. Default: 60.
        #[serde(default = "default_refresh_buffer")]
        refresh_buffer_s: u64,
        /// Retry attempts on token refresh failure. Default: 3.
        #[serde(default = "default_auth_retry_attempts")]
        auth_retry_attempts: u32,
    },
}

fn default_refresh_buffer() -> u64 {
    60
}

fn default_auth_retry_attempts() -> u32 {
    3
}

/// Current auth state, managed by the driver.
#[derive(Debug, Clone)]
pub enum AuthState {
    /// No auth needed.
    None,
    /// Auth is active with a resolved token/header.
    Active {
        /// The header name (e.g. "Authorization", "X-Api-Key").
        header_name: String,
        /// The header value (e.g. `"Bearer <token>"`).
        header_value: String,
    },
    /// Auth token has expired and needs refresh.
    Expired,
}

// ── Request / Response ─────────────────────────────────────────────

/// HTTP request to execute.
///
/// Per spec §2.2. Path is fully resolved — `{params}` already substituted.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Fully resolved path (e.g. "/v1/users/42/orders").
    pub path: String,
    /// Request headers (merged from static + auth + per-request).
    pub headers: HashMap<String, String>,
    /// Query string parameters.
    pub query: HashMap<String, String>,
    /// Request body (JSON).
    pub body: Option<serde_json::Value>,
    /// Per-request timeout override in milliseconds.
    pub timeout_ms: Option<u64>,
}

/// HTTP method.
///
/// Per spec §2.2.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// HTTP GET (default).
    #[default]
    Get,
    /// HTTP POST.
    Post,
    /// HTTP PUT.
    Put,
    /// HTTP PATCH.
    Patch,
    /// HTTP DELETE.
    Delete,
    /// HTTP HEAD.
    Head,
}

/// HTTP response from the upstream.
///
/// Per spec §2.3. Non-2xx responses are not automatically errors —
/// the DataView config declares acceptable status codes via `success_status`.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
    /// Response body (deserialized as JSON).
    pub body: serde_json::Value,
}

/// Event from an SSE or WebSocket stream.
///
/// Per spec §2.4.
#[derive(Debug, Clone)]
pub struct HttpStreamEvent {
    /// SSE "event:" field — `None` for data-only events.
    pub event_type: Option<String>,
    /// Event data (deserialized as JSON).
    pub data: serde_json::Value,
    /// SSE "id:" field.
    pub id: Option<String>,
}

// ── DataView Config ────────────────────────────────────────────────

/// HTTP DataView configuration.
///
/// Per spec §6. The "query" is a full HTTP request template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpDataViewConfig {
    /// Datasource name to use.
    pub datasource: String,
    /// HTTP method.
    #[serde(default)]
    pub method: HttpMethod,
    /// Path template with `{param}` placeholders.
    pub path: String,
    /// Static headers merged with auth headers.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Static query parameters.
    #[serde(default)]
    pub query_params: HashMap<String, String>,
    /// Body template with `{param}` placeholders.
    pub body_template: Option<serde_json::Value>,
    /// Parameter declarations.
    #[serde(default)]
    pub parameters: Vec<HttpDataViewParam>,
    /// Acceptable response status codes. Default: [200, 201, 202, 204].
    #[serde(default = "default_success_status")]
    pub success_status: Vec<u16>,
    /// Optional return schema name for validation.
    pub return_schema: Option<String>,
    /// Per-DataView timeout override in milliseconds.
    pub timeout_ms: Option<u64>,
}

fn default_success_status() -> Vec<u16> {
    vec![200, 201, 202, 204]
}

/// Parameter declaration for an HTTP DataView.
///
/// Per spec §6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpDataViewParam {
    /// Parameter name.
    pub name: String,
    /// Where the parameter is injected.
    pub location: ParamLocation,
    /// Whether the parameter is required.
    #[serde(default)]
    pub required: bool,
    /// Default value if not provided.
    pub default: Option<serde_json::Value>,
}

/// Where an HTTP DataView parameter is injected.
///
/// Per spec §6.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    /// Substituted into path template `{param}`.
    Path,
    /// Appended to query string.
    Query,
    /// Substituted into body template.
    Body,
    /// Injected as request header.
    Header,
}

// ── Retry & Circuit Breaker ────────────────────────────────────────

/// Retry configuration for HTTP datasources.
///
/// Per spec §9.1. Applied on request/response path only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Total attempts including first. Default: 3.
    #[serde(default = "default_retry_attempts")]
    pub attempts: u32,
    /// Backoff strategy.
    #[serde(default)]
    pub backoff: BackoffStrategy,
    /// Initial delay in milliseconds. Default: 100.
    #[serde(default = "default_base_delay")]
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds. Default: 5000.
    #[serde(default = "default_max_delay")]
    pub max_delay_ms: u64,
    /// HTTP status codes to retry on. Default: [429, 502, 503, 504].
    #[serde(default = "default_retry_on_status")]
    pub retry_on_status: Vec<u16>,
    /// Whether to retry on request timeout. Default: true.
    #[serde(default = "default_true")]
    pub retry_on_timeout: bool,
}

fn default_retry_attempts() -> u32 {
    3
}
fn default_base_delay() -> u64 {
    100
}
fn default_max_delay() -> u64 {
    5000
}
fn default_retry_on_status() -> Vec<u16> {
    vec![429, 502, 503, 504]
}
fn default_true() -> bool {
    true
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: default_retry_attempts(),
            backoff: BackoffStrategy::default(),
            base_delay_ms: default_base_delay(),
            max_delay_ms: default_max_delay(),
            retry_on_status: default_retry_on_status(),
            retry_on_timeout: true,
        }
    }
}

/// Backoff strategy for retries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackoffStrategy {
    /// No delay between retries.
    None,
    /// Linear backoff: delay = base_delay_ms * attempt.
    Linear,
    /// Exponential backoff: delay = base_delay_ms * 2^(attempt-1).
    #[default]
    Exponential,
}

/// Circuit breaker configuration for HTTP datasources.
///
/// Per spec §9.2. Standard Closed → Open → Half-Open → Closed model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Failures in window before circuit opens. Default: 5.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// Rolling window for failure counting in milliseconds. Default: 10000.
    #[serde(default = "default_window_ms")]
    pub window_ms: u64,
    /// How long circuit stays open in milliseconds. Default: 30000.
    #[serde(default = "default_open_duration")]
    pub open_duration_ms: u64,
    /// Probe attempts before closing circuit. Default: 1.
    #[serde(default = "default_half_open_attempts")]
    pub half_open_attempts: u32,
}

fn default_failure_threshold() -> u32 {
    5
}
fn default_window_ms() -> u64 {
    10000
}
fn default_open_duration() -> u64 {
    30000
}
fn default_half_open_attempts() -> u32 {
    1
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_failure_threshold(),
            window_ms: default_window_ms(),
            open_duration_ms: default_open_duration(),
            half_open_attempts: default_half_open_attempts(),
        }
    }
}

// ── Path Templating ────────────────────────────────────────────────

/// Resolve `{param}` placeholders in a path template.
///
/// Per spec §6.1. All declared path parameters must appear in the template.
///
/// # Examples
///
/// ```ignore
/// let path = resolve_path_template(
///     "/v1/users/{user_id}/orders/{order_id}",
///     &[("user_id", "42"), ("order_id", "99")].into_iter()
///         .map(|(k, v)| (k.to_string(), v.to_string()))
///         .collect(),
/// );
/// assert_eq!(path, "/v1/users/42/orders/99");
/// ```
pub fn resolve_path_template(
    template: &str,
    params: &HashMap<String, String>,
) -> String {
    let mut result = template.to_string();
    for (name, value) in params {
        let placeholder = format!("{{{}}}", name);
        result = result.replace(&placeholder, value);
    }
    result
}

// ── Body Templating ────────────────────────────────────────────────

/// Resolve `{param}` placeholders in a JSON body template.
///
/// Per spec §6.2. Only string values containing a single `{param}` placeholder
/// are substituted. Non-string values pass through unchanged. String values
/// with no placeholder pass through as literal strings.
///
/// The substituted value preserves JSON types from the `params` map.
pub fn resolve_body_template(
    template: &serde_json::Value,
    params: &HashMap<String, serde_json::Value>,
) -> serde_json::Value {
    match template {
        serde_json::Value::Object(obj) => {
            let mut result = serde_json::Map::new();
            for (key, value) in obj {
                result.insert(key.clone(), resolve_body_template(value, params));
            }
            serde_json::Value::Object(result)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(
                arr.iter().map(|v| resolve_body_template(v, params)).collect(),
            )
        }
        serde_json::Value::String(s) => {
            // Check if this is a single `{param}` placeholder
            if s.starts_with('{') && s.ends_with('}') && s.len() > 2 {
                let param_name = &s[1..s.len() - 1];
                // Only substitute if there are no other braces inside
                if !param_name.contains('{') && !param_name.contains('}') {
                    if let Some(value) = params.get(param_name) {
                        return value.clone();
                    }
                }
            }
            template.clone()
        }
        // Non-string values pass through unchanged
        _ => template.clone(),
    }
}

// ── Response Mapping ───────────────────────────────────────────────

/// Map an HTTP response body to a `QueryResult`.
///
/// Per spec §10:
/// - JSON object → single row (each key→value becomes a column)
/// - JSON array → one row per element (each element must be an object)
/// - Null/empty → empty QueryResult
/// - Non-JSON → wrapped as `{ "raw": "...", "content_type": "..." }`
pub fn response_to_query_result(body: &serde_json::Value) -> QueryResult {
    match body {
        serde_json::Value::Object(obj) => {
            let row = json_object_to_row(obj);
            let affected = if row.is_empty() { 0 } else { 1 };
            QueryResult {
                rows: vec![row],
                affected_rows: affected,
                last_insert_id: None,
                column_names: None,
            }
        }
        serde_json::Value::Array(arr) => {
            let rows: Vec<HashMap<String, QueryValue>> = arr
                .iter()
                .map(|v| {
                    if let serde_json::Value::Object(obj) = v {
                        json_object_to_row(obj)
                    } else {
                        // Non-object array elements wrapped as { "value": ... }
                        let mut row = HashMap::new();
                        row.insert("value".to_string(), json_to_query_value(v));
                        row
                    }
                })
                .collect();
            let affected = rows.len() as u64;
            QueryResult {
                rows,
                affected_rows: affected,
                last_insert_id: None,
                column_names: None,
            }
        }
        serde_json::Value::Null => QueryResult::empty(),
        // Scalar values (string, number, bool) — wrap as single row with "value" key
        other => {
            let mut row = HashMap::new();
            row.insert("value".to_string(), json_to_query_value(other));
            QueryResult {
                rows: vec![row],
                affected_rows: 1,
                last_insert_id: None,
                column_names: None,
            }
        }
    }
}

/// Wrap a non-JSON response body as a JSON object.
///
/// Per spec §10.2.
pub fn wrap_non_json_response(raw_body: &str, content_type: &str) -> serde_json::Value {
    serde_json::json!({
        "raw": raw_body,
        "content_type": content_type,
    })
}

/// Convert a JSON object to a row of `QueryValue` entries.
fn json_object_to_row(obj: &serde_json::Map<String, serde_json::Value>) -> HashMap<String, QueryValue> {
    obj.iter()
        .map(|(k, v)| (k.clone(), json_to_query_value(v)))
        .collect()
}

/// Convert a `serde_json::Value` to a `QueryValue`.
fn json_to_query_value(value: &serde_json::Value) -> QueryValue {
    match value {
        serde_json::Value::Null => QueryValue::Null,
        serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                QueryValue::Float(f)
            } else {
                QueryValue::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => QueryValue::String(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            QueryValue::Json(value.clone())
        }
    }
}

// ── Validation ─────────────────────────────────────────────────────

/// Validate an HTTP DataView configuration.
///
/// Per spec §11. Returns a list of validation errors (empty = valid).
pub fn validate_http_dataview(config: &HttpDataViewConfig) -> Vec<String> {
    let mut errors = Vec::new();

    // Check success_status is non-empty
    if config.success_status.is_empty() {
        errors.push("success_status must declare at least one status code".to_string());
    }

    // Check path parameters are declared and present in template
    let path_params: Vec<&HttpDataViewParam> = config
        .parameters
        .iter()
        .filter(|p| p.location == ParamLocation::Path)
        .collect();

    for param in &path_params {
        let placeholder = format!("{{{}}}", param.name);
        if !config.path.contains(&placeholder) {
            errors.push(format!(
                "path parameter '{}' not found in path template",
                param.name
            ));
        }
    }

    // Check path template placeholders have declared parameters
    let mut remaining = config.path.as_str();
    while let Some(start) = remaining.find('{') {
        if let Some(end) = remaining[start..].find('}') {
            let param_name = &remaining[start + 1..start + end];
            if !config
                .parameters
                .iter()
                .any(|p| p.name == param_name && p.location == ParamLocation::Path)
            {
                errors.push(format!(
                    "path template references undeclared parameter '{}'",
                    param_name
                ));
            }
            remaining = &remaining[start + end + 1..];
        } else {
            break;
        }
    }

    errors
}

/// Validate HTTP datasource-level auth config.
///
/// Per spec §11.
pub fn validate_http_auth(auth: &AuthConfig) -> Vec<String> {
    let mut errors = Vec::new();
    // api_key requires auth_header — but that's enforced by the enum structure
    // (ApiKey variant has auth_header field). So we just validate it's non-empty.
    if let AuthConfig::ApiKey { auth_header, .. } = auth {
        if auth_header.is_empty() {
            errors.push("auth_header is required when auth = api_key".to_string());
        }
    }
    errors
}

/// Validate retry configuration.
///
/// Per spec §11.
pub fn validate_retry_config(config: &RetryConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if config.attempts == 0 {
        errors.push("retry.attempts must be at least 1".to_string());
    }
    errors
}

/// Validate circuit breaker configuration.
///
/// Per spec §11.
pub fn validate_circuit_breaker_config(config: &CircuitBreakerConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if config.failure_threshold == 0 {
        errors.push("failure_threshold must be at least 1".to_string());
    }
    errors
}
