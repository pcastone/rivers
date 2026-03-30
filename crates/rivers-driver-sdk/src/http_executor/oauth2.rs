//! OAuth2 client-credentials token caching and fetching.
//!
//! Manages cached access tokens with expiry tracking and retries
//! against the token endpoint.

use std::time::{Duration, Instant};

use tracing::debug;

use crate::http_driver::HttpDriverError;

/// Cached OAuth2 access token with expiry tracking.
#[derive(Debug, Clone)]
pub(crate) struct CachedToken {
    /// The access token value.
    pub(crate) access_token: String,
    /// When this token expires (absolute time).
    pub(crate) expires_at: Instant,
}

/// OAuth2 credentials parsed from LockBox secret.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct OAuth2Credentials {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    pub(crate) token_url: String,
    #[serde(default)]
    pub(crate) scope: Option<String>,
}

/// OAuth2 token endpoint response.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    #[serde(default = "default_expires_in")]
    pub(crate) expires_in: u64,
    #[allow(dead_code)]
    pub(crate) token_type: Option<String>,
}

fn default_expires_in() -> u64 {
    3600
}

/// Fetch an OAuth2 token from the token endpoint with retry.
pub(crate) async fn fetch_oauth2_token(
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

        debug!(attempt, max_attempts, "fetching OAuth2 token");

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
