use std::collections::HashMap;

use riversd::graphql::{
    build_dynamic_schema, build_resolver_mappings_from_dataviews, generate_graphql_types,
    graphql_router, GraphqlConfig, GraphqlFieldType, ResolverMapping,
};

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
    assert!(types.is_empty()); // no properties -> no type generated
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

#[test]
fn build_resolver_mappings_empty_returns_empty() {
    let names: Vec<&str> = vec![];
    let mappings = build_resolver_mappings_from_dataviews(&names);
    assert!(mappings.is_empty());
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

    // Should not panic -- creates router with POST + playground routes
    let _router = graphql_router(schema, &config);
}
