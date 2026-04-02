//! CORS configuration and header injection.
//!
//! Per `rivers-httpd-spec.md` §9.

use axum::http::{HeaderValue, Method};

/// CORS configuration for a view or global scope.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    /// Whether CORS handling is enabled.
    pub enabled: bool,
    /// Allowed origin URLs (use `"*"` for wildcard).
    pub allowed_origins: Vec<String>,
    /// Allowed HTTP methods.
    pub allowed_methods: Vec<String>,
    /// Allowed request headers.
    pub allowed_headers: Vec<String>,
    /// Whether to include credentials (cookies, auth headers).
    pub allow_credentials: bool,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_origins: vec!["*".to_string()],
            allowed_methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "PATCH".to_string(),
                "DELETE".to_string(),
                "OPTIONS".to_string(),
            ],
            allowed_headers: vec!["Content-Type".to_string(), "Authorization".to_string()],
            allow_credentials: false,
        }
    }
}

/// Validate CORS config.
///
/// Per spec §9.2: wildcard `*` is incompatible with `allow_credentials = true`.
pub fn validate_cors_config(config: &CorsConfig) -> Result<(), String> {
    if config.allow_credentials
        && config.allowed_origins.iter().any(|o| o == "*")
    {
        return Err(
            "CORS: cors_allow_credentials=true is incompatible with wildcard origin '*'"
                .to_string(),
        );
    }
    Ok(())
}

/// Resolve CORS headers for a response.
///
/// Per spec §9.1-9.2: match Origin header against allowed origins,
/// return headers to set. Returns None if CORS is disabled or origin doesn't match.
pub fn resolve_cors_headers(
    config: &CorsConfig,
    request_origin: Option<&str>,
    request_method: Option<&Method>,
) -> Option<CorsHeaders> {
    if !config.enabled {
        return None;
    }

    let origin = request_origin?;

    // Origin matching
    let allow_origin = if config.allowed_origins.iter().any(|o| o == "*") {
        "*".to_string()
    } else if config.allowed_origins.iter().any(|o| o == origin) {
        origin.to_string()
    } else {
        // No match — no CORS headers
        return None;
    };

    let is_preflight = request_method == Some(&Method::OPTIONS);

    Some(CorsHeaders {
        allow_origin,
        allow_methods: config.allowed_methods.join(", "),
        allow_headers: config.allowed_headers.join(", "),
        allow_credentials: config.allow_credentials,
        is_preflight,
    })
}

/// Resolved CORS headers to inject into a response.
pub struct CorsHeaders {
    /// Matched origin or `"*"`.
    pub allow_origin: String,
    /// Comma-separated allowed methods.
    pub allow_methods: String,
    /// Comma-separated allowed headers.
    pub allow_headers: String,
    /// Whether credentials are allowed.
    pub allow_credentials: bool,
    /// Whether this is an OPTIONS preflight response.
    pub is_preflight: bool,
}

impl CorsHeaders {
    /// Apply CORS headers to a response.
    pub fn apply(&self, headers: &mut axum::http::HeaderMap) {
        if let Ok(val) = HeaderValue::from_str(&self.allow_origin) {
            headers.insert("access-control-allow-origin", val);
        }
        if let Ok(val) = HeaderValue::from_str(&self.allow_methods) {
            headers.insert("access-control-allow-methods", val);
        }
        if let Ok(val) = HeaderValue::from_str(&self.allow_headers) {
            headers.insert("access-control-allow-headers", val);
        }
        if self.allow_credentials {
            headers.insert(
                "access-control-allow-credentials",
                HeaderValue::from_static("true"),
            );
        }
        if self.is_preflight {
            headers.insert("access-control-max-age", HeaderValue::from_static("86400"));
        }
        // Vary: Origin required when origin is dynamically reflected (not *)
        if self.allow_origin != "*" {
            headers.insert("vary", HeaderValue::from_static("Origin"));
        }
    }
}

/// Header blocklist — headers that CodeComponent handlers cannot set.
///
/// Per spec §17 (SEC-8).
pub const BLOCKED_HEADERS: &[&str] = &[
    "set-cookie",
    "access-control-allow-origin",
    "access-control-allow-credentials",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-expose-headers",
    "access-control-max-age",
    "host",
    "transfer-encoding",
    "connection",
    "upgrade",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-content-type-options",
    "x-frame-options",
    "strict-transport-security",
    "content-security-policy",
];

/// Strip blocked headers from a handler's response.
///
/// Per spec §17 (SEC-8).
pub fn strip_blocked_headers(headers: &mut axum::http::HeaderMap) {
    for &name in BLOCKED_HEADERS {
        headers.remove(name);
    }
}
