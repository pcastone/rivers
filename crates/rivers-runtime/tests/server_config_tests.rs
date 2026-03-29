//! Integration tests for ServerConfig parsing, validation, environment overrides, and cache config.

use rivers_core_config::ServerConfig;
use rivers_runtime::{apply_environment_overrides, validate_server_config};

// ── ServerConfig parsing ────────────────────────────────────────────

#[test]
fn parse_minimal_server_config() {
    let toml = "";
    let config: ServerConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.base.host, "0.0.0.0");
    assert_eq!(config.base.port, 8080);
    assert_eq!(config.base.request_timeout_seconds, 30);
    assert!(config.base.backpressure.enabled);
    assert_eq!(config.base.backpressure.queue_depth, 512);
}

#[test]
fn parse_full_server_config() {
    let toml = r#"
[base]
host = "127.0.0.1"
port = 9090
workers = 4
request_timeout_seconds = 60

[base.backpressure]
enabled = false
queue_depth = 1024
queue_timeout_ms = 200

[base.http2]
enabled = true
tls_cert = "/etc/certs/cert.pem"
tls_key = "/etc/certs/key.pem"

[base.admin_api]
enabled = true
host = "127.0.0.1"
port = 9091

[security]
cors_enabled = true
cors_allowed_origins = ["https://example.com"]
cors_allowed_methods = ["GET", "POST"]
rate_limit_per_minute = 60
rate_limit_burst_size = 30

[static_files]
enabled = true
root_path = "/var/www"
index_file = "index.html"
spa_fallback = true
max_age = 3600

[storage_engine]
backend = "sqlite"
path = "/var/data/rivers.db"
retention_ms = 172800000
"#;
    let config: ServerConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.base.host, "127.0.0.1");
    assert_eq!(config.base.port, 9090);
    assert_eq!(config.base.workers, Some(4));
    assert!(!config.base.backpressure.enabled);
    assert!(config.base.http2.enabled);
    assert!(config.base.admin_api.enabled);
    assert_eq!(config.base.admin_api.port, Some(9091));
    assert!(config.security.cors_enabled);
    assert_eq!(config.security.cors_allowed_origins.len(), 1);
    assert_eq!(config.security.rate_limit_per_minute, 60);
    assert!(config.static_files.enabled);
    assert!(config.static_files.spa_fallback);
    assert_eq!(config.storage_engine.backend, "sqlite");
}

// ── Validation ──────────────────────────────────────────────────────

#[test]
fn validate_default_config_passes() {
    let config = ServerConfig::default();
    assert!(validate_server_config(&config).is_ok());
}

#[test]
fn validate_catches_port_zero() {
    let toml = r#"
[base]
port = 0
"#;
    let config: ServerConfig = toml::from_str(toml).unwrap();
    let errors = validate_server_config(&config).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("port")));
}

#[test]
fn validate_catches_admin_port_conflict() {
    let toml = r#"
[base]
port = 8080

[base.admin_api]
enabled = true
port = 8080
"#;
    let config: ServerConfig = toml::from_str(toml).unwrap();
    let errors = validate_server_config(&config).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("conflicts")));
}

#[test]
fn validate_catches_cors_without_origins() {
    let toml = r#"
[security]
cors_enabled = true
"#;
    let config: ServerConfig = toml::from_str(toml).unwrap();
    let errors = validate_server_config(&config).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("cors_allowed_origins")));
}

// ── Environment overrides ───────────────────────────────────────────

#[test]
fn apply_env_overrides() {
    let toml = r#"
[base]
host = "0.0.0.0"
port = 8080

[environment_overrides.prod.base]
port = 443
workers = 8

[environment_overrides.prod.security]
rate_limit_per_minute = 300

[environment_overrides.prod.storage_engine]
backend = "redis"
url = "redis://prod-cache:6379"
"#;
    let mut config: ServerConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.base.port, 8080);

    apply_environment_overrides(&mut config, "prod");
    assert_eq!(config.base.port, 443);
    assert_eq!(config.base.workers, Some(8));
    assert_eq!(config.security.rate_limit_per_minute, 300);
    assert_eq!(config.storage_engine.backend, "redis");
    assert_eq!(
        config.storage_engine.url.as_deref(),
        Some("redis://prod-cache:6379")
    );
}

#[test]
fn apply_nonexistent_env_is_noop() {
    let mut config = ServerConfig::default();
    let port_before = config.base.port;
    apply_environment_overrides(&mut config, "staging");
    assert_eq!(config.base.port, port_before);
}

// ── Cache Config Ownership (Phase J) ─────────────────────────────

#[test]
fn storage_engine_cache_config_parses() {
    let toml_str = r#"
        [base]
        host = "0.0.0.0"
        port = 8080

        [storage_engine]
        backend = "memory"

        [storage_engine.cache.datasources.orders_db]
        enabled = true
        ttl_seconds = 120

        [storage_engine.cache.dataviews.get_stock_price]
        ttl_seconds = 5
    "#;
    let config: rivers_core_config::config::ServerConfig = toml::from_str(toml_str).unwrap();
    let cache = &config.storage_engine.cache;
    assert_eq!(cache.datasources.len(), 1);
    assert!(cache.datasources.get("orders_db").unwrap().enabled);
    assert_eq!(cache.datasources.get("orders_db").unwrap().ttl_seconds, 120);
    assert_eq!(cache.dataviews.len(), 1);
    assert_eq!(
        cache.dataviews.get("get_stock_price").unwrap().ttl_seconds,
        Some(5)
    );
}
