//! Integration tests for configuration parsing and validation.

use rivers_core_config::ServerConfig;
use rivers_runtime::{
    apply_environment_overrides, validate_app_config, validate_bundle, validate_known_drivers,
    validate_server_config, AppConfig, BundleManifest, ResourcesConfig,
};

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
    assert!(errors.iter().any(|e| format!("{}", e).contains("cors_allowed_origins")));
}

// ── App config validation ───────────────────────────────────────────

#[test]
fn validate_app_catches_unknown_datasource_ref() {
    let toml = r#"
[data.dataviews.my_view]
name = "my_view"
datasource = "nonexistent"
query = "SELECT 1"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("nonexistent")));
}

#[test]
fn validate_app_valid_config_passes() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[data.dataviews.my_view]
name = "my_view"
datasource = "mydb"
query = "SELECT * FROM users"

[api.views.list_users]
view_type = "Rest"
path = "/api/users"
method = "GET"

[api.views.list_users.handler]
type = "dataview"
dataview = "my_view"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    assert!(validate_app_config(&config).is_ok());
}

#[test]
fn validate_app_catches_unknown_dataview_in_view() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[api.views.list_users]
view_type = "Rest"
path = "/api/users"
method = "GET"

[api.views.list_users.handler]
type = "dataview"
dataview = "nonexistent_view"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("nonexistent_view")));
}

// ── Bundle manifest parsing ─────────────────────────────────────────

#[test]
fn parse_bundle_manifest() {
    let toml = r#"
bundle_name = "address-book-bundle"
bundle_version = "1.0.0"
apps = ["address-book-service", "address-book-main"]
"#;
    let manifest: BundleManifest = toml::from_str(toml).unwrap();
    assert_eq!(manifest.bundle_name, "address-book-bundle");
    assert_eq!(manifest.apps.len(), 2);
}

// ── Resources parsing ───────────────────────────────────────────────

#[test]
fn parse_resources_config() {
    let toml = r#"
[[datasources]]
name = "faker-contacts"
driver = "faker"
nopassword = true

[[services]]
name = "address-book-service"
app_id = "c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a"
"#;
    let resources: ResourcesConfig = toml::from_str(toml).unwrap();
    assert_eq!(resources.datasources.len(), 1);
    assert!(resources.datasources[0].nopassword);
    assert_eq!(resources.services.len(), 1);
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
    assert_eq!(config.storage_engine.url.as_deref(), Some("redis://prod-cache:6379"));
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
    assert_eq!(cache.dataviews.get("get_stock_price").unwrap().ttl_seconds, Some(5));
}

// ── (AX3) Config Schema Generation ───────────────────────────────────

#[test]
fn server_config_schema_is_valid_json() {
    let schema = rivers_core_config::server_config_schema();
    assert!(schema.is_object());
    assert!(schema.get("properties").is_some() || schema.get("$ref").is_some());
}

#[test]
fn app_config_schema_has_data_and_api() {
    let schema = rivers_runtime::app_config_schema();
    assert!(schema.is_object());
    // The schema should reference the AppConfig definition
    let schema_str = serde_json::to_string(&schema).unwrap();
    assert!(schema_str.contains("AppConfig") || schema_str.contains("data") || schema_str.contains("api"));
}

#[test]
fn bundle_manifest_schema_has_apps_field() {
    let schema = rivers_runtime::bundle_manifest_schema();
    let schema_str = serde_json::to_string(&schema).unwrap();
    assert!(schema_str.contains("apps") || schema_str.contains("BundleManifest"));
}

#[test]
fn server_config_schema_includes_graphql() {
    let schema = rivers_core_config::server_config_schema();
    let schema_str = serde_json::to_string(&schema).unwrap();
    assert!(schema_str.contains("graphql") || schema_str.contains("GraphqlServerConfig"));
}

// ── (C) View type validation ─────────────────────────────────────────

#[test]
fn validate_app_catches_unknown_view_type() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[api.views.bad_view]
view_type = "BadType"
path = "/api/bad"
method = "GET"

[api.views.bad_view.handler]
type = "codecomponent"
language = "javascript"
module = "handler.js"
entrypoint = "handle"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("unknown view_type")));
    assert!(errors.iter().any(|e| format!("{}", e).contains("BadType")));
}

#[test]
fn validate_app_accepts_all_known_view_types() {
    for vt in &["Rest", "Websocket", "ServerSentEvents", "MessageConsumer"] {
        let toml = format!(
            r#"
[api.views.v]
view_type = "{}"
path = "/api/v"
method = "GET"

[api.views.v.handler]
type = "codecomponent"
language = "javascript"
module = "handler.js"
entrypoint = "handle"
"#,
            vt
        );
        let config: AppConfig = toml::from_str(&toml).unwrap();
        assert!(
            validate_app_config(&config).is_ok(),
            "view_type '{}' should be valid",
            vt
        );
    }
}

// ── (E) Invalidates target validation ────────────────────────────────

#[test]
fn validate_app_catches_invalid_invalidates_target() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[data.dataviews.create_contact]
name = "create_contact"
datasource = "mydb"
post_query = "INSERT INTO contacts"
invalidates = ["list_contacts_typo"]
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    let errors = validate_app_config(&config).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("invalidates target")));
    assert!(errors.iter().any(|e| format!("{}", e).contains("list_contacts_typo")));
}

#[test]
fn validate_app_allows_valid_invalidates_target() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"

[data.dataviews.list_contacts]
name = "list_contacts"
datasource = "mydb"
query = "SELECT * FROM contacts"

[data.dataviews.create_contact]
name = "create_contact"
datasource = "mydb"
post_query = "INSERT INTO contacts"
invalidates = ["list_contacts"]
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();
    assert!(validate_app_config(&config).is_ok());
}

// ── (H) Duplicate resource names ─────────────────────────────────────

#[test]
fn validate_catches_duplicate_resource_datasource_names() {
    let toml = r#"
[[datasources]]
name = "mydb"
driver = "postgres"

[[datasources]]
name = "mydb"
driver = "mysql"
"#;
    let resources: ResourcesConfig = toml::from_str(toml).unwrap();

    // Build a minimal bundle to test via validate_bundle
    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![rivers_runtime::LoadedApp {
            manifest: rivers_runtime::AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "id-1".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
            },
            resources,
            config: AppConfig::default(),
            app_dir: std::path::PathBuf::from("/tmp/nonexistent"),
        }],
    };

    let errors = validate_bundle(&bundle).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("duplicate datasource name")));
}

// ── (K) Cross-app service reference check ────────────────────────────

#[test]
fn validate_bundle_catches_unknown_service_app_id() {
    let resources = ResourcesConfig {
        datasources: vec![],
        keystores: vec![],
        services: vec![rivers_runtime::bundle::ServiceDependency {
            name: "backend-api".into(),
            app_id: "nonexistent-uuid".into(),
            required: true,
        }],
    };

    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![rivers_runtime::LoadedApp {
            manifest: rivers_runtime::AppManifest {
                app_name: "frontend".into(),
                description: None,
                version: None,
                app_type: "app-main".into(),
                app_id: "frontend-uuid".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
            },
            resources,
            config: AppConfig::default(),
            app_dir: std::path::PathBuf::from("/tmp/nonexistent"),
        }],
    };

    let errors = validate_bundle(&bundle).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("unknown appId")));
    assert!(errors.iter().any(|e| format!("{}", e).contains("nonexistent-uuid")));
}

#[test]
fn validate_bundle_allows_valid_service_reference() {
    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![
            rivers_runtime::LoadedApp {
                manifest: rivers_runtime::AppManifest {
                    app_name: "backend".into(),
                    description: None,
                    version: None,
                    app_type: "app-service".into(),
                    app_id: "backend-uuid".into(),
                    entry_point: None,
                    app_entry_point: None,
                    source: None,
                    spa: None,
                },
                resources: ResourcesConfig::default(),
                config: AppConfig::default(),
                app_dir: std::path::PathBuf::from("/tmp/nonexistent"),
            },
            rivers_runtime::LoadedApp {
                manifest: rivers_runtime::AppManifest {
                    app_name: "frontend".into(),
                    description: None,
                    version: None,
                    app_type: "app-main".into(),
                    app_id: "frontend-uuid".into(),
                    entry_point: None,
                    app_entry_point: None,
                    source: None,
                    spa: None,
                },
                resources: ResourcesConfig {
                    datasources: vec![],
                    keystores: vec![],
                    services: vec![rivers_runtime::bundle::ServiceDependency {
                        name: "backend".into(),
                        app_id: "backend-uuid".into(),
                        required: true,
                    }],
                },
                config: AppConfig::default(),
                app_dir: std::path::PathBuf::from("/tmp/nonexistent"),
            },
        ],
    };

    assert!(validate_bundle(&bundle).is_ok());
}

// ── (D) Known driver name validation ─────────────────────────────────

#[test]
fn validate_known_drivers_catches_bad_driver() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postrgess"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();

    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![rivers_runtime::LoadedApp {
            manifest: rivers_runtime::AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "id-1".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
            },
            resources: ResourcesConfig::default(),
            config,
            app_dir: std::path::PathBuf::from("/tmp/nonexistent"),
        }],
    };

    let known = &["faker", "postgres", "mysql", "sqlite", "redis"];
    let errors = validate_known_drivers(&bundle, known);
    assert!(!errors.is_empty());
    assert!(errors.iter().any(|e| format!("{}", e).contains("postrgess")));
    assert!(errors.iter().any(|e| format!("{}", e).contains("unknown driver")));
}

#[test]
fn validate_known_drivers_passes_valid_driver() {
    let toml = r#"
[data.datasources.mydb]
name = "mydb"
driver = "postgres"
"#;
    let config: AppConfig = toml::from_str(toml).unwrap();

    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![rivers_runtime::LoadedApp {
            manifest: rivers_runtime::AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "id-1".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
            },
            resources: ResourcesConfig::default(),
            config,
            app_dir: std::path::PathBuf::from("/tmp/nonexistent"),
        }],
    };

    let known = &["faker", "postgres", "mysql", "sqlite", "redis"];
    let errors = validate_known_drivers(&bundle, known);
    assert!(errors.is_empty());
}

// ── (I) TOML parse error context ─────────────────────────────────────

#[test]
fn load_bundle_parse_error_includes_app_context() {
    let dir = tempfile::tempdir().unwrap();
    let bundle_dir = dir.path();

    // Write valid bundle manifest
    std::fs::write(
        bundle_dir.join("manifest.toml"),
        r#"bundle_name = "test"
bundle_version = "1.0"
apps = ["bad-app"]
"#,
    ).unwrap();

    // Create app dir with invalid app.toml
    let app_dir = bundle_dir.join("bad-app");
    std::fs::create_dir(&app_dir).unwrap();
    std::fs::write(app_dir.join("manifest.toml"), r#"
appName = "bad-app"
type = "app-service"
appId = "uuid-1"
"#).unwrap();
    std::fs::write(app_dir.join("resources.toml"), "").unwrap();
    std::fs::write(app_dir.join("app.toml"), "INVALID {{ TOML").unwrap();

    let result = rivers_runtime::load_bundle(bundle_dir);
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("bad-app"), "error should mention app name: {}", err_msg);
}

// ── (F) Schema file existence ────────────────────────────────────────

#[test]
fn validate_catches_missing_schema_file() {
    let dir = tempfile::tempdir().unwrap();
    let app_dir = dir.path().to_path_buf();

    let toml_str = r#"
[data.datasources.mydb]
name = "mydb"
driver = "faker"

[data.dataviews.list_contacts]
name = "list_contacts"
datasource = "mydb"
return_schema = "schemas/nonexistent.schema.json"
"#;
    let config: AppConfig = toml::from_str(toml_str).unwrap();

    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![rivers_runtime::LoadedApp {
            manifest: rivers_runtime::AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "id-1".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
            },
            resources: ResourcesConfig::default(),
            config,
            app_dir,
        }],
    };

    let errors = validate_bundle(&bundle).unwrap_err();
    assert!(errors.iter().any(|e| format!("{}", e).contains("schema file")));
    assert!(errors.iter().any(|e| format!("{}", e).contains("nonexistent.schema.json")));
}

#[test]
fn validate_passes_when_schema_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let app_dir = dir.path().to_path_buf();

    // Create schema file
    std::fs::create_dir(app_dir.join("schemas")).unwrap();
    std::fs::write(
        app_dir.join("schemas/contact.schema.json"),
        r#"{"type":"object","properties":{"id":{"type":"integer"}}}"#,
    ).unwrap();

    let toml_str = r#"
[data.datasources.mydb]
name = "mydb"
driver = "faker"

[data.dataviews.list_contacts]
name = "list_contacts"
datasource = "mydb"
return_schema = "schemas/contact.schema.json"
"#;
    let config: AppConfig = toml::from_str(toml_str).unwrap();

    let bundle = rivers_runtime::LoadedBundle {
        manifest: BundleManifest {
            bundle_name: "test".into(),
            bundle_version: "1.0".into(),
            source: None,
            apps: vec![],
        },
        apps: vec![rivers_runtime::LoadedApp {
            manifest: rivers_runtime::AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "id-1".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
            },
            resources: ResourcesConfig::default(),
            config,
            app_dir,
        }],
    };

    assert!(validate_bundle(&bundle).is_ok());
}
