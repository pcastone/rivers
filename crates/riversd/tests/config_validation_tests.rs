//! Config validation tests — verify ServerConfig parsing and defaults.
//!
//! Feature 17 of the Rivers feature inventory.
//! Only 5 tests existed before — these add coverage for critical fields.

use rivers_runtime::rivers_core::config::ServerConfig;

#[test]
fn default_config_parses() {
    let config = ServerConfig::default();
    assert_eq!(config.base.host, "0.0.0.0");
    assert_eq!(config.storage_engine.backend, "memory");
    assert!(!config.graphql.enabled);
}

#[test]
fn storage_engine_memory_backend_default() {
    let config = ServerConfig::default();
    assert_eq!(config.storage_engine.backend, "memory");
    assert!(config.storage_engine.url.is_none());
    assert!(config.storage_engine.path.is_none());
}

#[test]
fn security_defaults_sane() {
    let config = ServerConfig::default();
    // Session should default to enabled (for guard views)
    // CORS should be configurable
    assert!(config.security.cors_allowed_origins.is_empty() || !config.security.cors_enabled);
}

#[test]
fn session_cookie_http_only_validation() {
    let config = ServerConfig::default();
    // The session cookie should have http_only = true by default
    assert!(
        config.security.session.cookie.http_only,
        "Session cookie http_only should default to true"
    );
}

#[test]
fn ddl_whitelist_default_empty() {
    let config = ServerConfig::default();
    assert!(
        config.security.ddl_whitelist.is_empty(),
        "DDL whitelist should default to empty (deny all)"
    );
}

#[test]
fn canary_config_parses() {
    let toml_str = include_str!("../../../canary-bundle/riversd-canary.toml");
    let config: ServerConfig = toml::from_str(toml_str)
        .expect("canary config should parse as valid ServerConfig");

    assert_eq!(config.base.port, 8090);
    assert_eq!(config.storage_engine.backend, "memory");
    assert!(config.security.csrf.enabled);
    // ddl_whitelist is set in [security] section
    assert!(
        !config.security.ddl_whitelist.is_empty(),
        "canary config ddl_whitelist should not be empty, got: {:?}",
        config.security.ddl_whitelist
    );
}

#[test]
fn bundle_path_optional() {
    let config = ServerConfig::default();
    assert!(
        config.bundle_path.is_none(),
        "bundle_path should be None by default"
    );
}

#[test]
fn request_timeout_has_default() {
    let config = ServerConfig::default();
    assert!(
        config.base.request_timeout_seconds > 0,
        "request_timeout_seconds should have a positive default"
    );
}
