//! Integration tests for bundle manifest parsing, resources config, and bundle-level validation.

use rivers_runtime::{
    validate_bundle, validate_known_drivers, AppConfig, BundleManifest, ResourcesConfig,
};

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
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("duplicate datasource name")));
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
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("unknown appId")));
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("nonexistent-uuid")));
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
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("postrgess")));
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("unknown driver")));
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
    )
    .unwrap();

    // Create app dir with invalid app.toml
    let app_dir = bundle_dir.join("bad-app");
    std::fs::create_dir(&app_dir).unwrap();
    std::fs::write(
        app_dir.join("manifest.toml"),
        r#"
appName = "bad-app"
type = "app-service"
appId = "uuid-1"
"#,
    )
    .unwrap();
    std::fs::write(app_dir.join("resources.toml"), "").unwrap();
    std::fs::write(app_dir.join("app.toml"), "INVALID {{ TOML").unwrap();

    let result = rivers_runtime::load_bundle(bundle_dir);
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("bad-app"),
        "error should mention app name: {}",
        err_msg
    );
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
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("schema file")));
    assert!(errors
        .iter()
        .any(|e| format!("{}", e).contains("nonexistent.schema.json")));
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
    )
    .unwrap();

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
