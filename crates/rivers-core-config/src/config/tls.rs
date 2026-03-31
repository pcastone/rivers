//! TLS, admin API, cluster, and session store configuration types.

use schemars::JsonSchema;
use serde::Deserialize;

// ── [base.tls] ──────────────────────────────────────────────────────

/// `[base.tls]` -- TLS configuration. Mandatory on the main server.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TlsConfig {
    /// Path to the PEM certificate file.
    pub cert: Option<String>,
    /// Path to the PEM private key file.
    pub key: Option<String>,

    /// Redirect HTTP to HTTPS (default: `true`).
    #[serde(default = "default_true")]
    pub redirect: bool,

    /// HTTP port to redirect from (default: `80`).
    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,

    /// X.509 fields for auto-generated and CLI-generated certificates.
    #[serde(default)]
    pub x509: TlsX509Config,

    /// Cipher suite and TLS version constraints.
    #[serde(default)]
    pub engine: TlsEngineConfig,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            cert: None,
            key: None,
            redirect: true,
            redirect_port: default_redirect_port(),
            x509: TlsX509Config::default(),
            engine: TlsEngineConfig::default(),
        }
    }
}

fn default_redirect_port() -> u16 {
    80
}

/// `[base.tls.x509]` -- x509 fields used for auto-gen and riversctl tls gen/request.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TlsX509Config {
    /// Certificate Common Name (default: `"localhost"`).
    #[serde(default = "default_cn")]
    pub common_name: String,

    /// Organization name for the certificate subject.
    #[serde(default)]
    pub organization: Option<String>,

    /// Country code (ISO 3166-1 alpha-2).
    #[serde(default)]
    pub country: Option<String>,

    /// State or province name.
    #[serde(default)]
    pub state: Option<String>,

    /// City or locality name.
    #[serde(default)]
    pub locality: Option<String>,

    /// Subject Alternative Names (default: `["localhost", "127.0.0.1"]`).
    #[serde(default = "default_san")]
    pub san: Vec<String>,

    /// Certificate validity in days (default: `365`).
    #[serde(default = "default_days")]
    pub days: u32,
}

impl Default for TlsX509Config {
    fn default() -> Self {
        Self {
            common_name: default_cn(),
            organization: None,
            country: None,
            state: None,
            locality: None,
            san: default_san(),
            days: default_days(),
        }
    }
}

fn default_cn() -> String {
    "localhost".to_string()
}

fn default_san() -> Vec<String> {
    vec!["localhost".to_string(), "127.0.0.1".to_string()]
}

fn default_days() -> u32 {
    365
}

/// `[base.tls.engine]` -- cipher suites and TLS version.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TlsEngineConfig {
    /// Minimum TLS version: `"tls12"` or `"tls13"` (default: `"tls12"`).
    #[serde(default = "default_min_version")]
    pub min_version: String,

    /// Allowed cipher suites (empty = system defaults).
    #[serde(default)]
    pub ciphers: Vec<String>,
}

impl Default for TlsEngineConfig {
    fn default() -> Self {
        Self {
            min_version: default_min_version(),
            ciphers: Vec::new(),
        }
    }
}

fn default_min_version() -> String {
    "tls12".to_string()
}

// ── [base.admin_api] ────────────────────────────────────────────────

/// `[base.admin_api]` -- operational admin server on a separate socket.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AdminApiConfig {
    /// Enable the admin API server (default: `false`).
    #[serde(default)]
    pub enabled: bool,

    /// Admin API listen address (default: `"127.0.0.1"`).
    #[serde(default = "default_admin_host")]
    pub host: String,

    /// Admin API listen port.
    pub port: Option<u16>,
    /// Ed25519 public key for verifying admin API request signatures (hex-encoded 32-byte seed).
    pub public_key: Option<String>,
    /// Ed25519 private key -- used by riversctl for signing requests, NOT used by riversd.
    /// Included in config for tool integration (riversctl reads this when RIVERS_ADMIN_KEY is not set).
    pub private_key: Option<String>,

    /// Skip Ed25519 signature verification (development only).
    ///
    /// Per spec S15.3: `--no-admin-auth` CLI flag maps to this field.
    #[serde(default)]
    pub no_auth: Option<bool>,

    /// TLS settings for the admin API socket.
    #[serde(default)]
    pub tls: Option<AdminTlsConfig>,

    /// Role-based access control for admin endpoints.
    #[serde(default)]
    pub rbac: Option<RbacConfig>,
}

fn default_admin_host() -> String {
    "127.0.0.1".to_string()
}

/// TLS config for the admin API.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AdminTlsConfig {
    /// CA certificate for client cert verification.
    pub ca_cert: Option<String>,
    /// Server certificate for the admin socket.
    pub server_cert: Option<String>,
    /// Server private key for the admin socket.
    pub server_key: Option<String>,
    /// Require mutual TLS (client certificate) for admin connections.
    #[serde(default)]
    pub require_client_cert: bool,
}

impl Default for AdminTlsConfig {
    fn default() -> Self {
        Self {
            ca_cert: None,
            server_cert: None,
            server_key: None,
            require_client_cert: false,
        }
    }
}

/// RBAC config for the admin API.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RbacConfig {
    /// Role definitions: role name → list of permitted admin API actions.
    #[serde(default)]
    pub roles: std::collections::HashMap<String, Vec<String>>,
    /// Key bindings: public key hex → role name.
    #[serde(default)]
    pub bindings: std::collections::HashMap<String, String>,
}

// ── [base.cluster] ──────────────────────────────────────────────────

/// `[base.cluster]` -- clustering and session store settings.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ClusterConfig {
    /// Session persistence backend.
    #[serde(default)]
    pub session_store: SessionStoreConfig,
}

/// `[base.cluster.session_store]` -- session persistence.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SessionStoreConfig {
    /// Enable persistent session storage (default: `false`).
    #[serde(default)]
    pub enabled: bool,

    /// Session cookie name (default: `"rivers_session"`).
    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,
}

impl Default for SessionStoreConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cookie_name: default_cookie_name(),
        }
    }
}

pub(super) fn default_cookie_name() -> String {
    "rivers_session".to_string()
}

fn default_true() -> bool {
    true
}
