mod graphql_common;

use std::collections::HashMap;
use std::sync::Arc;

use riversd::graphql::{
    build_mutation_mappings_from_views, build_schema_with_executor, GraphqlConfig, MutationMapping,
    ResolverMapping,
};
use rivers_runtime::view::ApiViewConfig;
use tokio::sync::RwLock;

use graphql_common::{default_event_bus, default_pool, default_view_config};

// ── Schema with executor (mutation stub) ────────────────────────

#[tokio::test]
async fn schema_with_executor_includes_mutation_stub() {
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

    let executor: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(&config, &resolvers, executor, &[], default_pool(), &[], default_event_bus()).unwrap();

    // Execute the mutation stub
    let result = schema.execute("mutation { _noop }").await;
    assert!(result.errors.is_empty(), "mutation _noop should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert_eq!(data["_noop"], true);
}

// ── Integration: Schema with executor end-to-end ────────────────

#[tokio::test]
async fn schema_with_real_executor_resolves_query() {
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
        circuit_breaker_id: None,
        prepared: false,
        query_params: std::collections::HashMap::new(),
        get_query: None, post_query: None, put_query: None, delete_query: None,
        get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
        get_parameters: Vec::new(), post_parameters: Vec::new(),
        put_parameters: Vec::new(), delete_parameters: Vec::new(),
        streaming: false,
        max_rows: 1000,
        skip_introspect: false,
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

    let executor_ref: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
        Arc::new(RwLock::new(Some(Arc::new(executor))));

    let config = GraphqlConfig { enabled: true, ..Default::default() };

    let resolvers = vec![ResolverMapping {
        field_name: "list_contacts".into(),
        dataview: "list_contacts".into(),
        argument_mapping: HashMap::new(),
        is_list: true,
    }];

    let schema = build_schema_with_executor(&config, &resolvers, executor_ref, &[], default_pool(), &[], default_event_bus()).unwrap();

    // Execute a query -- should resolve via faker driver
    let result = schema.execute("{ list_contacts }").await;
    assert!(result.errors.is_empty(), "query should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert!(data.get("list_contacts").is_some());
}

// ── Integration: Resolver error for missing DataView ────────────

#[tokio::test]
async fn schema_with_executor_missing_dataview_returns_error() {
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

    let executor_ref: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
        Arc::new(RwLock::new(Some(Arc::new(executor))));

    let schema = build_schema_with_executor(&config, &resolvers, executor_ref, &[], default_pool(), &[], default_event_bus()).unwrap();

    let result = schema.execute("{ missing }").await;
    assert!(!result.errors.is_empty(), "should error for missing dataview");
    assert!(result.errors[0].message.contains("not found"));
}

// ── Integration: Introspection returns Query and Mutation ────────

#[tokio::test]
async fn introspection_returns_query_and_mutation_types() {
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

    let executor: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
        Arc::new(RwLock::new(None));

    let schema = build_schema_with_executor(&config, &resolvers, executor, &[], default_pool(), &[], default_event_bus()).unwrap();

    let result = schema.execute("{ __schema { queryType { name } mutationType { name } } }").await;
    assert!(result.errors.is_empty(), "introspection should succeed: {:?}", result.errors);

    let data = result.data.into_json().unwrap();
    assert_eq!(data["__schema"]["queryType"]["name"], "Query");
    assert_eq!(data["__schema"]["mutationType"]["name"], "Mutation");
}

// ── AV4: Mutation mapping tests ─────────────────────────────────

#[test]
fn mutation_mappings_from_codecomponent_post_views() {
    use rivers_runtime::view::HandlerConfig;

    let mut views = HashMap::new();

    // POST CodeComponent -> should become mutation
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

    // GET CodeComponent -> should be skipped (queries, not mutations)
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

    // POST Dataview -> should be skipped (not CodeComponent)
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
    let config = GraphqlConfig { enabled: true, ..Default::default() };

    // Empty query resolvers -- just mutations
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

    let executor: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
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

    // Execute mutation -- should dispatch to pool (will get HandlerError since no JS file)
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

    let executor: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
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
    use riversd::graphql::SubscriptionMapping;

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

    let executor: Arc<RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>> =
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
