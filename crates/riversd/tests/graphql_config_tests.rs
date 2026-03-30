use riversd::graphql::{validate_graphql_config, GraphqlConfig, GraphqlFieldType};

// ── GraphqlConfig ───────────────────────────────────────────────

#[test]
fn default_config() {
    let config = GraphqlConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.path, "/graphql");
    assert!(config.introspection);
    assert_eq!(config.max_depth, 10);
    assert_eq!(config.max_complexity, 1000);
}

// ── Validation ──────────────────────────────────────────────────

#[test]
fn validate_disabled_passes() {
    let config = GraphqlConfig::default(); // disabled
    let errors = validate_graphql_config(&config);
    assert!(errors.is_empty());
}

#[test]
fn validate_valid_enabled() {
    let config = GraphqlConfig {
        enabled: true,
        ..Default::default()
    };
    let errors = validate_graphql_config(&config);
    assert!(errors.is_empty());
}

#[test]
fn validate_empty_path() {
    let config = GraphqlConfig {
        enabled: true,
        path: "".into(),
        ..Default::default()
    };
    let errors = validate_graphql_config(&config);
    assert!(errors.iter().any(|e| e.contains("path must not be empty")));
}

#[test]
fn validate_path_no_slash() {
    let config = GraphqlConfig {
        enabled: true,
        path: "graphql".into(),
        ..Default::default()
    };
    let errors = validate_graphql_config(&config);
    assert!(errors.iter().any(|e| e.contains("must start with '/'")));
}

#[test]
fn validate_zero_depth() {
    let config = GraphqlConfig {
        enabled: true,
        max_depth: 0,
        ..Default::default()
    };
    let errors = validate_graphql_config(&config);
    assert!(errors.iter().any(|e| e.contains("max_depth must be > 0")));
}

#[test]
fn validate_zero_complexity() {
    let config = GraphqlConfig {
        enabled: true,
        max_complexity: 0,
        ..Default::default()
    };
    let errors = validate_graphql_config(&config);
    assert!(errors.iter().any(|e| e.contains("max_complexity must be > 0")));
}

#[test]
fn validate_graphql_config_accumulates_all_errors() {
    let config = GraphqlConfig {
        enabled: true,
        path: "no-slash".into(),
        max_depth: 0,
        max_complexity: 0,
        ..Default::default()
    };
    let errors = validate_graphql_config(&config);
    assert!(errors.len() >= 3, "should have path + depth + complexity errors: {:?}", errors);
}

// ── GraphqlFieldType ────────────────────────────────────────────

#[test]
fn field_type_from_json_schema() {
    assert_eq!(
        GraphqlFieldType::from_json_schema_type("string"),
        GraphqlFieldType::String
    );
    assert_eq!(
        GraphqlFieldType::from_json_schema_type("integer"),
        GraphqlFieldType::Int
    );
    assert_eq!(
        GraphqlFieldType::from_json_schema_type("number"),
        GraphqlFieldType::Float
    );
    assert_eq!(
        GraphqlFieldType::from_json_schema_type("boolean"),
        GraphqlFieldType::Boolean
    );
    assert_eq!(
        GraphqlFieldType::from_json_schema_type("unknown"),
        GraphqlFieldType::String
    );
}

// ── GraphqlServerConfig conversion ──────────────────────────────

#[test]
fn graphql_server_config_converts_to_graphql_config() {
    let server_cfg = rivers_runtime::rivers_core::GraphqlServerConfig {
        enabled: true,
        path: "/api/graphql".into(),
        introspection: false,
        max_depth: 5,
        max_complexity: 500,
    };
    let config = GraphqlConfig::from(&server_cfg);
    assert!(config.enabled);
    assert_eq!(config.path, "/api/graphql");
    assert!(!config.introspection);
    assert_eq!(config.max_depth, 5);
    assert_eq!(config.max_complexity, 500);
}

// ── GraphQL config deserialization from TOML ────────────────────

#[test]
fn graphql_server_config_defaults_disabled() {
    let toml_str = "";
    let config: rivers_runtime::rivers_core::config::GraphqlServerConfig = toml::from_str(toml_str).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.path, "/graphql");
}

#[test]
fn graphql_server_config_toml_roundtrip() {
    let toml_str = r#"
        enabled = true
        path = "/gql"
        max_depth = 15
    "#;
    let config: rivers_runtime::rivers_core::config::GraphqlServerConfig = toml::from_str(toml_str).unwrap();
    assert!(config.enabled);
    assert_eq!(config.path, "/gql");
    assert_eq!(config.max_depth, 15);
    assert_eq!(config.max_complexity, 1000); // default
}
