//! Server configuration types.
//!
//! Maps to `riversd.conf` — the top-level TOML config for a riversd instance.
//! Per `rivers-httpd-spec.md` §19.

/// Runtime, process pool, environment overrides, logging, engines, plugins, GraphQL, and static files.
pub mod runtime;
/// Security, CORS, rate limiting, CSRF, and session configuration.
pub mod security;
/// Root server config, base settings, backpressure, and HTTP/2.
pub mod server;
/// Storage engine and cache configuration.
pub mod storage;
/// TLS, admin API, cluster, and session store configuration.
pub mod tls;
/// OTel span export configuration.
pub mod telemetry;
/// Unknown-key detection for riversd.toml at startup.
pub mod validate_config;
/// MCP server-level subscription limits.
pub mod mcp;

pub use mcp::McpConfig;
pub use runtime::*;
pub use security::*;
pub use server::*;
pub use storage::*;
pub use telemetry::*;
pub use tls::*;

#[cfg(test)]
mod tls_config_tests {
    use super::*;

    #[test]
    fn tls_config_parses_with_cert_paths() {
        let toml = r#"
            [base.tls]
            cert = "/etc/rivers/server.crt"
            key = "/etc/rivers/server.key"
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let tls = cfg.base.tls.unwrap();
        assert_eq!(tls.cert.unwrap(), "/etc/rivers/server.crt");
        assert_eq!(tls.key.unwrap(), "/etc/rivers/server.key");
    }

    #[test]
    fn tls_config_parses_without_cert_paths() {
        let toml = r#"
            [base.tls]
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let tls = cfg.base.tls.unwrap();
        assert!(tls.cert.is_none());
        assert!(tls.key.is_none());
        assert_eq!(tls.redirect_port, 80);
        assert!(tls.redirect);
    }

    #[test]
    fn tls_config_x509_defaults() {
        let toml = r#"
            [base.tls]
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let tls = cfg.base.tls.unwrap();
        assert_eq!(tls.x509.san, vec!["localhost", "127.0.0.1"]);
        assert_eq!(tls.x509.days, 365);
    }

    #[test]
    fn admin_tls_config_optional_fields() {
        let toml = r#"
            [base.admin_api.tls]
            require_client_cert = false
        "#;
        let cfg: ServerConfig = toml::from_str(toml).unwrap();
        let admin_tls = cfg.base.admin_api.tls.unwrap();
        assert!(admin_tls.server_cert.is_none());
        assert!(admin_tls.server_key.is_none());
        assert!(admin_tls.ca_cert.is_none());
        assert!(!admin_tls.require_client_cert);
    }

    #[test]
    fn http2_config_has_no_tls_fields() {
        // Exhaustive destructuring — compile error if any new field (e.g. tls_cert) is added
        let Http2Config { enabled: _, initial_window_size: _, max_concurrent_streams: _ } = Http2Config::default();
    }
}
