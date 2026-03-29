//! Security, CSRF, and session configuration types.

use schemars::JsonSchema;
use serde::Deserialize;

use super::tls::default_cookie_name;

// ── [security] ──────────────────────────────────────────────────────

/// `[security]` -- CORS, rate limiting, IP allowlists.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SecurityConfig {
    #[serde(default)]
    pub cors_enabled: bool,

    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,

    #[serde(default)]
    pub cors_allowed_methods: Vec<String>,

    #[serde(default)]
    pub cors_allowed_headers: Vec<String>,

    #[serde(default)]
    pub cors_allow_credentials: bool,

    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u32,

    #[serde(default = "default_burst_size")]
    pub rate_limit_burst_size: u32,

    #[serde(default = "default_rate_strategy")]
    pub rate_limit_strategy: String,

    pub rate_limit_custom_header: Option<String>,

    #[serde(default)]
    pub admin_ip_allowlist: Vec<String>,

    #[serde(default)]
    pub session: SessionConfig,

    #[serde(default)]
    pub csrf: CsrfConfig,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            cors_enabled: false,
            cors_allowed_origins: Vec::new(),
            cors_allowed_methods: Vec::new(),
            cors_allowed_headers: Vec::new(),
            cors_allow_credentials: false,
            rate_limit_per_minute: default_rate_limit(),
            rate_limit_burst_size: default_burst_size(),
            rate_limit_strategy: default_rate_strategy(),
            rate_limit_custom_header: None,
            admin_ip_allowlist: Vec::new(),
            session: SessionConfig::default(),
            csrf: CsrfConfig::default(),
        }
    }
}

// ── [security.csrf] ─────────────────────────────────────────────────

/// `[security.csrf]` -- CSRF protection configuration.
/// Per `rivers-auth-session-spec.md` S9.5.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CsrfConfig {
    /// Enable CSRF protection (default: true).
    pub enabled: bool,

    /// Minimum seconds between token rotations (default: 300).
    pub csrf_rotation_interval_s: u64,

    /// CSRF cookie name (default: "rivers_csrf").
    pub cookie_name: String,

    /// CSRF header name (default: "X-CSRF-Token").
    pub header_name: String,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            csrf_rotation_interval_s: 300,
            cookie_name: "rivers_csrf".to_string(),
            header_name: "X-CSRF-Token".to_string(),
        }
    }
}

// ── [security.session] ──────────────────────────────────────────────

/// `[security.session]` -- session management configuration.
/// Per `rivers-auth-session-spec.md` S4.3, S8.1.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SessionConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Absolute session lifetime in seconds from creation (default: 3600).
    #[serde(default = "default_session_ttl")]
    pub ttl_s: u64,

    /// Inactivity timeout in seconds from last_seen (default: 1800).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_s: u64,

    #[serde(default)]
    pub cookie: SessionCookieConfig,

    /// When true, the session token is included in the JSON response body
    /// of the guard handler's success response. Useful for SPAs that store
    /// tokens in memory rather than relying solely on cookies.
    /// Default: false.
    #[serde(default)]
    pub include_token_in_body: bool,

    /// JSON key name for the session token when `include_token_in_body` is true.
    /// Default: "token".
    #[serde(default = "default_token_body_key")]
    pub token_body_key: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_s: default_session_ttl(),
            idle_timeout_s: default_idle_timeout(),
            cookie: SessionCookieConfig::default(),
            include_token_in_body: false,
            token_body_key: default_token_body_key(),
        }
    }
}

fn default_token_body_key() -> String {
    "token".to_string()
}

fn default_session_ttl() -> u64 {
    3600
}

fn default_idle_timeout() -> u64 {
    1800
}

/// `[security.session.cookie]` -- session cookie attributes.
/// Per spec S8.1: http_only=true is enforced and not configurable to false.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SessionCookieConfig {
    pub name: String,

    /// Always true -- enforced. Config validation rejects false.
    pub http_only: bool,

    /// HTTPS only. Default true, can be false for local dev (emits warning).
    pub secure: bool,

    /// "Strict" | "Lax" | "None". Default: "Lax".
    pub same_site: String,

    pub path: String,

    /// Not set by default (current domain only).
    pub domain: Option<String>,
}

impl SessionCookieConfig {
    /// Validate session cookie security invariants.
    ///
    /// Per spec S8.1: http_only=true is mandatory. Setting it to false is a
    /// configuration error -- session cookies must never be readable by JavaScript.
    pub fn validate(&self) -> Result<(), String> {
        if !self.http_only {
            return Err(
                "session cookie http_only must be true — setting http_only=false is a security violation".into(),
            );
        }
        Ok(())
    }
}

impl Default for SessionCookieConfig {
    fn default() -> Self {
        Self {
            name: default_cookie_name(),
            http_only: true,
            secure: true,
            same_site: "Lax".to_string(),
            path: "/".to_string(),
            domain: None,
        }
    }
}

fn default_rate_limit() -> u32 {
    120
}

fn default_burst_size() -> u32 {
    60
}

fn default_rate_strategy() -> String {
    "ip".to_string()
}
