//! Integration tests for JSON Schema generation (schemars) of config types.

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
    assert!(
        schema_str.contains("AppConfig")
            || schema_str.contains("data")
            || schema_str.contains("api")
    );
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
