//! HTTP driver implementation backed by `reqwest`.
//!
//! Each `connect()` call produces a `ReqwestHttpConnection` that shares
//! the underlying `reqwest::Client` connection pool.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use tokio::sync::RwLock;

use crate::http_driver::{
    AuthConfig, AuthState, HttpConnection, HttpConnectionParams, HttpDriver, HttpDriverError,
    HttpProtocol, HttpStreamConnection,
};

use super::connection::ReqwestHttpConnection;
use super::oauth2::{fetch_oauth2_token, CachedToken, OAuth2Credentials};
use super::sse_stream::SseStreamConnection;

/// HTTP driver implementation backed by `reqwest`.
///
/// Each `connect()` call produces a `ReqwestHttpConnection` that shares
/// the underlying `reqwest::Client` connection pool.
pub struct ReqwestHttpDriver {
    /// Cached OAuth2 token (shared across connections from the same driver).
    pub(super) oauth2_cache: Arc<RwLock<Option<CachedToken>>>,
}

impl ReqwestHttpDriver {
    /// Create a new ReqwestHttpDriver.
    pub fn new() -> Self {
        Self {
            oauth2_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Build a reqwest::Client from connection params.
    pub(crate) fn build_client(params: &HttpConnectionParams) -> Result<reqwest::Client, HttpDriverError> {
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
    pub(crate) async fn resolve_auth(
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
