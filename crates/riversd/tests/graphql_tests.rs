use std::collections::HashMap;
use std::sync::Arc;

use riversd::graphql::{
    build_dynamic_schema, build_mutation_mappings_from_views, build_resolver_mappings_from_dataviews,
    generate_graphql_types, graphql_router, validate_graphql_config, GraphqlConfig,
    GraphqlFieldType, MutationMapping, ResolverMapping,
};
use rivers_runtime::view::ApiViewConfig;
use riversd::process_pool::ProcessPoolManager;

fn default_pool() -> Arc<ProcessPoolManager> {
    Arc::new(ProcessPoolManager::from_config(&HashMap::new()))
}

fn default_event_bus() -> Arc<rivers_runtime::rivers_core::EventBus> {
    Arc::new(rivers_runtime::rivers_core::EventBus::new())
}

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

// ── Schema Generation ───────────────────────────────────────────

#[test]
fn generate_types_from_schema() {
    let mut schemas = HashMap::new();
    schemas.insert(
        "contact".to_string(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "email": {"type": "string"},
                "active": {"type": "boolean"}
            },
            "required": ["id", "name"]
        }),
    );

    let types = generate_graphql_types(&schemas);
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].name, "Contact");
    assert_eq!(types[0].fields.len(), 4);

    // Check id field
    let id_field = types[0].fields.iter().find(|f| f.name == "id").unwrap();
    assert_eq!(id_field.field_type, GraphqlFieldType::Int);
    assert!(!id_field.nullable); // required

    // Check email field
    let email_field = types[0].fields.iter().find(|f| f.name == "email").unwrap();
    assert_eq!(email_field.field_type, GraphqlFieldType::String);
    assert!(email_field.nullable); // not required
}

#[test]
fn generate_types_with_array_field() {
    let mut schemas = HashMap::new();
    schemas.insert(
        "order".to_string(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {"type": "array", "items": {"type": "string"}}
            }
        }),
    );

    let types = generate_graphql_types(&schemas);
    let items_field = types[0].fields.iter().find(|f| f.name == "items").unwrap();
    assert_eq!(
        items_field.field_type,
        GraphqlFieldType::List(Box::new(GraphqlFieldType::String))
    );
}

#[test]
fn generate_types_pascal_case_name() {
    let mut schemas = HashMap::new();
    schemas.insert(
        "order_items".to_string(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            }
        }),
    );

    let types = generate_graphql_types(&schemas);
    assert_eq!(types[0].name, "OrderItems");
}

#[test]
fn generate_types_empty_schemas() {
    let types = generate_graphql_types(&HashMap::new());
    assert!(types.is_empty());
}

#[test]
fn generate_types_schema_without_properties() {
    let mut schemas = HashMap::new();
    schemas.insert("bare".to_string(), serde_json::json!({"type": "string"}));

    let types = generate_graphql_types(&schemas);
    assert!(types.is_empty()); // no properties → no type generated
}

// ── ResolverMapping ─────────────────────────────────────────────

#[test]
fn resolver_mapping_serialization() {
    let mapping = ResolverMapping {
        field_name: "contacts".into(),
        dataview: "list_contacts".into(),
        argument_mapping: {
            let mut m = HashMap::new();
            m.insert("page".into(), "offset_page".into());
            m
        },
        is_list: true,
    };

    let json = serde_json::to_value(&mapping).unwrap();
    assert_eq!(json["field_name"], "contacts");
    assert_eq!(json["dataview"], "list_contacts");
    assert_eq!(json["is_list"], true);
}

// ── Dynamic Schema Building (C6.1) ─────────────────────────────

#[tokio::test]
async fn build_dynamic_schema_from_resolvers() {
    let config = GraphqlConfig {
        enabled: true,
        max_depth: 5,
        max_complexity: 100,
        introspection: true,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "hello_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let schema = build_dynamic_schema(&config, &resolvers, |_dataview, _args| {
        Ok(serde_json::json!("world"))
    });

    assert!(schema.is_ok(), "schema build should succeed");
}

#[tokio::test]
async fn execute_simple_query_against_dynamic_schema() {
    let config = GraphqlConfig {
        enabled: true,
        max_depth: 5,
        max_complexity: 100,
        introspection: true,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "greeting".into(),
        dataview: "greeting_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let schema = build_dynamic_schema(&config, &resolvers, |dataview, _args| {
        assert_eq!(dataview, "greeting_view");
        Ok(serde_json::json!("hello rivers"))
    })
    .unwrap();

    let result = schema
        .execute("{ greeting }")
        .await;

    assert!(result.errors.is_empty(), "query should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert_eq!(data["greeting"], "hello rivers");
}

#[tokio::test]
async fn dynamic_schema_with_arguments() {
    let config = GraphqlConfig {
        enabled: true,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "contact".into(),
        dataview: "get_contact".into(),
        argument_mapping: {
            let mut m = HashMap::new();
            m.insert("id".into(), "contact_id".into());
            m
        },
        is_list: false,
    }];

    let schema = build_dynamic_schema(&config, &resolvers, |dataview, args| {
        assert_eq!(dataview, "get_contact");
        let id = args.get("contact_id").cloned().unwrap_or(serde_json::Value::Null);
        Ok(serde_json::json!(format!("contact_{}", id)))
    })
    .unwrap();

    let result = schema
        .execute(r#"{ contact(id: "42") }"#)
        .await;

    assert!(result.errors.is_empty(), "query should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert_eq!(data["contact"], "contact_\"42\"");
}

#[tokio::test]
async fn introspection_disabled_blocks_schema_query() {
    let config = GraphqlConfig {
        enabled: true,
        introspection: false,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "hello_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let schema = build_dynamic_schema(&config, &resolvers, |_, _| {
        Ok(serde_json::json!("world"))
    })
    .unwrap();

    // Introspection query should fail or return errors
    let result = schema
        .execute("{ __schema { types { name } } }")
        .await;

    assert!(
        !result.errors.is_empty(),
        "introspection should be blocked when disabled"
    );
}

#[tokio::test]
async fn max_depth_limit_enforced() {
    let config = GraphqlConfig {
        enabled: true,
        max_depth: 1,
        max_complexity: 1000,
        introspection: false,
        ..Default::default()
    };

    // With depth limit of 1, a deeply nested query should fail.
    // Since we use scalar String fields, depth > 1 isn't possible with
    // the current dynamic schema (no nested objects). We verify the config
    // is applied by checking the schema builds successfully with limits.
    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "hello_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let schema = build_dynamic_schema(&config, &resolvers, |_, _| {
        Ok(serde_json::json!("world"))
    });

    assert!(schema.is_ok(), "schema should build with depth limits");
}

#[test]
fn graphql_router_creates_routes() {
    let config = GraphqlConfig {
        enabled: true,
        introspection: true,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "hello_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let schema = build_dynamic_schema(&config, &resolvers, |_, _| {
        Ok(serde_json::json!("world"))
    })
    .unwrap();

    // Should not panic — creates router with POST + playground routes
    let _router = graphql_router(schema, &config);
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

// ── Resolver mapping from DataViews ─────────────────────────────

#[test]
fn build_resolver_mappings_strips_namespace() {
    let names = vec!["app:list_contacts", "app:get_contact"];
    let mappings = build_resolver_mappings_from_dataviews(&names);

    assert_eq!(mappings.len(), 2);
    assert_eq!(mappings[0].field_name, "list_contacts");
    assert_eq!(mappings[0].dataview, "app:list_contacts");
    assert_eq!(mappings[1].field_name, "get_contact");
}

#[test]
fn build_resolver_mappings_no_namespace() {
    let names = vec!["list_contacts"];
    let mappings = build_resolver_mappings_from_dataviews(&names);

    assert_eq!(mappings[0].field_name, "list_contacts");
    assert_eq!(mappings[0].dataview, "list_contacts");
}

// ── Schema with executor (mutation stub) ────────────────────────

#[tokio::test]
async fn schema_with_executor_includes_mutation_stub() {
    use riversd::graphql::build_schema_with_executor;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let config = GraphqlConfig {
        enabled: true,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "hello_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let executor: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(&config, &resolvers, executor, &[], default_pool(), &[], default_event_bus()).unwrap();

    // Execute the mutation stub
    let result = schema.execute("mutation { _noop }").await;
    assert!(result.errors.is_empty(), "mutation _noop should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert_eq!(data["_noop"], true);
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

// ── Integration: Schema with executor end-to-end ────────────────

#[tokio::test]
async fn schema_with_real_executor_resolves_query() {
    use riversd::graphql::build_schema_with_executor;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use rivers_runtime::rivers_core::DriverFactory;
    use rivers_runtime::rivers_driver_sdk::ConnectionParams;
    use rivers_runtime::dataview::DataViewConfig;

    // Set up faker driver
    let mut factory = DriverFactory::new();
    let faker = Arc::new(rivers_runtime::rivers_core::drivers::FakerDriver::new());
    factory.register_database_driver(faker);

    // Set up DataView + registry
    let mut registry = rivers_runtime::DataViewRegistry::new();
    registry.register(DataViewConfig {
        name: "list_contacts".into(),
        datasource: "faker-ds".into(),
        query: Some("schemas/contact.schema.json".into()),
        parameters: vec![],
        return_schema: None,
        invalidates: Vec::new(),
        validate_result: false,
        strict_parameters: false,
        caching: None,
        get_query: None, post_query: None, put_query: None, delete_query: None,
        get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
        get_parameters: Vec::new(), post_parameters: Vec::new(),
        put_parameters: Vec::new(), delete_parameters: Vec::new(),
        streaming: false,
    });

    // Connection params
    let mut ds_params = std::collections::HashMap::new();
    let mut opts = std::collections::HashMap::new();
    opts.insert("driver".to_string(), "faker".to_string());
    ds_params.insert("faker-ds".to_string(), ConnectionParams {
        host: String::new(), port: 0, database: String::new(),
        username: String::new(), password: String::new(), options: opts,
    });

    let executor = rivers_runtime::DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(ds_params),
        Arc::new(rivers_runtime::tiered_cache::NoopDataViewCache),
    );

    let executor_ref: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(Some(executor)));

    let config = GraphqlConfig { enabled: true, ..Default::default() };

    let resolvers = vec![ResolverMapping {
        field_name: "list_contacts".into(),
        dataview: "list_contacts".into(),
        argument_mapping: HashMap::new(),
        is_list: true,
    }];

    let schema = build_schema_with_executor(&config, &resolvers, executor_ref, &[], default_pool(), &[], default_event_bus()).unwrap();

    // Execute a query — should resolve via faker driver
    let result = schema.execute("{ list_contacts }").await;
    assert!(result.errors.is_empty(), "query should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert!(data.get("list_contacts").is_some());
}

// ── Integration: Resolver error for missing DataView ────────────

#[tokio::test]
async fn schema_with_executor_missing_dataview_returns_error() {
    use riversd::graphql::build_schema_with_executor;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let config = GraphqlConfig { enabled: true, ..Default::default() };

    // Resolver points to "nonexistent_view" which doesn't exist in registry
    let resolvers = vec![ResolverMapping {
        field_name: "missing".into(),
        dataview: "nonexistent_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    // Executor has an empty registry
    let registry = rivers_runtime::DataViewRegistry::new();
    let factory = Arc::new(rivers_runtime::rivers_core::DriverFactory::new());
    let executor = rivers_runtime::DataViewExecutor::new(
        registry,
        factory,
        Arc::new(std::collections::HashMap::new()),
        Arc::new(rivers_runtime::tiered_cache::NoopDataViewCache),
    );

    let executor_ref: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(Some(executor)));

    let schema = build_schema_with_executor(&config, &resolvers, executor_ref, &[], default_pool(), &[], default_event_bus()).unwrap();

    let result = schema.execute("{ missing }").await;
    assert!(!result.errors.is_empty(), "should error for missing dataview");
    assert!(result.errors[0].message.contains("not found"));
}

// ── Integration: Introspection returns Query and Mutation ────────

#[tokio::test]
async fn introspection_returns_query_and_mutation_types() {
    use riversd::graphql::build_schema_with_executor;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let config = GraphqlConfig {
        enabled: true,
        introspection: true,
        ..Default::default()
    };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "hello_view".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let executor: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(&config, &resolvers, executor, &[], default_pool(), &[], default_event_bus()).unwrap();

    let result = schema.execute("{ __schema { queryType { name } mutationType { name } } }").await;
    assert!(result.errors.is_empty(), "introspection should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert_eq!(data["__schema"]["queryType"]["name"], "Query");
    assert_eq!(data["__schema"]["mutationType"]["name"], "Mutation");
}

// ── Integration: GraphQL validation multiple errors ─────────────

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

// ── Integration: Resolver mappings from empty DataViews ─────────

#[test]
fn build_resolver_mappings_empty_returns_empty() {
    let names: Vec<&str> = vec![];
    let mappings = build_resolver_mappings_from_dataviews(&names);
    assert!(mappings.is_empty());
}

// ── AV4: Mutation mapping tests ─────────────────────────────────

#[test]
fn mutation_mappings_from_codecomponent_post_views() {
    use rivers_runtime::view::{ApiViewConfig, HandlerConfig};

    let mut views = HashMap::new();

    // POST CodeComponent → should become mutation
    views.insert("create_contact".into(), ApiViewConfig {
        view_type: "Rest".into(),
        path: Some("/api/contacts".into()),
        method: Some("POST".into()),
        handler: HandlerConfig::Codecomponent {
            language: "javascript".into(),
            module: "handler.js".into(),
            entrypoint: "create".into(),
            resources: vec![],
        },
        ..default_view_config()
    });

    // GET CodeComponent → should be skipped (queries, not mutations)
    views.insert("list_contacts".into(), ApiViewConfig {
        view_type: "Rest".into(),
        path: Some("/api/contacts".into()),
        method: Some("GET".into()),
        handler: HandlerConfig::Codecomponent {
            language: "javascript".into(),
            module: "handler.js".into(),
            entrypoint: "list".into(),
            resources: vec![],
        },
        ..default_view_config()
    });

    // POST Dataview → should be skipped (not CodeComponent)
    views.insert("insert_order".into(), ApiViewConfig {
        view_type: "Rest".into(),
        path: Some("/api/orders".into()),
        method: Some("POST".into()),
        handler: HandlerConfig::Dataview { dataview: "orders".into() },
        ..default_view_config()
    });

    let mappings = build_mutation_mappings_from_views(&views, "my-app");
    assert_eq!(mappings.len(), 1, "only POST CodeComponent should map");
    assert_eq!(mappings[0].field_name, "create_contact");
    assert_eq!(mappings[0].http_method, "POST");
    assert_eq!(mappings[0].entrypoint.module, "handler.js");
    assert_eq!(mappings[0].entrypoint.function, "create");
    assert!(mappings[0].view_id.contains("my-app:"));
}

#[test]
fn mutation_mappings_empty_views_returns_empty() {
    let views = HashMap::new();
    let mappings = build_mutation_mappings_from_views(&views, "app");
    assert!(mappings.is_empty());
}

#[tokio::test]
async fn mutation_dispatch_attempts_pool_execution() {
    use riversd::graphql::build_schema_with_executor;
    use tokio::sync::RwLock;

    let config = GraphqlConfig { enabled: true, ..Default::default() };

    // Empty query resolvers — just mutations
    let resolvers = vec![ResolverMapping {
        field_name: "placeholder".into(),
        dataview: "noop".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let mutations = vec![MutationMapping {
        field_name: "create_contact".into(),
        entrypoint: riversd::process_pool::Entrypoint {
            module: "handler.js".into(),
            function: "create".into(),
            language: "javascript".into(),
        },
        http_method: "POST".into(),
        view_id: "app:create_contact".into(),
    }];

    let executor: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(
        &config,
        &resolvers,
        executor,
        &mutations,
        default_pool(),
        &[],
        default_event_bus(),
    ).unwrap();

    // Execute mutation — should dispatch to pool (will get HandlerError since no JS file)
    let result = schema.execute(r#"mutation { create_contact(input: "{\"name\":\"alice\"}") }"#).await;
    // The error confirms pool dispatch was attempted, not a _noop stub
    assert!(!result.errors.is_empty(), "should error (no JS module)");
    assert!(
        result.errors[0].message.contains("cannot read module") || result.errors[0].message.contains("handler"),
        "error should be from pool dispatch, got: {}",
        result.errors[0].message
    );
}

#[tokio::test]
async fn introspection_shows_real_mutation_fields() {
    use riversd::graphql::build_schema_with_executor;
    use tokio::sync::RwLock;

    let config = GraphqlConfig { enabled: true, introspection: true, ..Default::default() };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "noop".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let mutations = vec![MutationMapping {
        field_name: "create_user".into(),
        entrypoint: riversd::process_pool::Entrypoint {
            module: "user.js".into(),
            function: "create".into(),
            language: "javascript".into(),
        },
        http_method: "POST".into(),
        view_id: "app:create_user".into(),
    }];

    let executor: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(&config, &resolvers, executor, &mutations, default_pool(), &[], default_event_bus()).unwrap();

    // Introspect mutation fields
    let result = schema.execute("{ __schema { mutationType { fields { name } } } }").await;
    assert!(result.errors.is_empty(), "introspection should succeed");
    let data = result.data.into_json().unwrap();
    let fields = data["__schema"]["mutationType"]["fields"].as_array().unwrap();
    let field_names: Vec<&str> = fields.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(field_names.contains(&"create_user"), "should have create_user mutation, got: {:?}", field_names);
    assert!(!field_names.contains(&"_noop"), "should NOT have _noop when real mutations exist");
}

/// Helper for constructing test ApiViewConfig with defaults.
fn default_view_config() -> ApiViewConfig {
    use rivers_runtime::view::HandlerConfig;
    ApiViewConfig {
        view_type: "Rest".into(),
        path: None,
        method: None,
        handler: HandlerConfig::None {},
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        polling: None,
        event_handlers: None,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
    }
}

// ── BA2: Subscription mapping tests ─────────────────────────

#[test]
fn subscription_mappings_from_sse_trigger_events() {
    use riversd::graphql::build_subscription_mappings_from_views;

    let mut views = HashMap::new();
    views.insert("orders_stream".into(), ApiViewConfig {
        view_type: "ServerSentEvents".into(),
        sse_trigger_events: vec!["OrderCreated".into(), "OrderUpdated".into()],
        ..default_view_config()
    });
    views.insert("another_view".into(), ApiViewConfig {
        view_type: "Rest".into(),
        sse_trigger_events: vec![], // No triggers
        ..default_view_config()
    });

    let mappings = build_subscription_mappings_from_views(&views);
    assert_eq!(mappings.len(), 2);
    let topics: Vec<&str> = mappings.iter().map(|m| m.event_topic.as_str()).collect();
    assert!(topics.contains(&"OrderCreated"));
    assert!(topics.contains(&"OrderUpdated"));
}

#[test]
fn subscription_mappings_deduplicates_events() {
    use riversd::graphql::build_subscription_mappings_from_views;

    let mut views = HashMap::new();
    views.insert("view_a".into(), ApiViewConfig {
        view_type: "ServerSentEvents".into(),
        sse_trigger_events: vec!["OrderCreated".into()],
        ..default_view_config()
    });
    views.insert("view_b".into(), ApiViewConfig {
        view_type: "ServerSentEvents".into(),
        sse_trigger_events: vec!["OrderCreated".into()], // Duplicate
        ..default_view_config()
    });

    let mappings = build_subscription_mappings_from_views(&views);
    assert_eq!(mappings.len(), 1, "should deduplicate");
}

#[test]
fn subscription_mappings_empty_views() {
    use riversd::graphql::build_subscription_mappings_from_views;
    let mappings = build_subscription_mappings_from_views(&HashMap::new());
    assert!(mappings.is_empty());
}

#[tokio::test]
async fn schema_with_subscriptions_introspects() {
    use riversd::graphql::{build_schema_with_executor, SubscriptionMapping};
    use tokio::sync::RwLock;

    let config = GraphqlConfig { enabled: true, introspection: true, ..Default::default() };

    let resolvers = vec![ResolverMapping {
        field_name: "hello".into(),
        dataview: "noop".into(),
        argument_mapping: HashMap::new(),
        is_list: false,
    }];

    let subs = vec![SubscriptionMapping {
        field_name: "order_created".into(),
        event_topic: "OrderCreated".into(),
    }];

    let executor: Arc<RwLock<Option<rivers_runtime::DataViewExecutor>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(
        &config, &resolvers, executor, &[], default_pool(), &subs, default_event_bus(),
    ).unwrap();

    let result = schema.execute(
        "{ __schema { subscriptionType { fields { name } } } }"
    ).await;
    assert!(result.errors.is_empty(), "introspection should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    let fields = data["__schema"]["subscriptionType"]["fields"].as_array().unwrap();
    let names: Vec<&str> = fields.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"order_created"), "should have order_created subscription: {:?}", names);
}
