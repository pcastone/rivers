//! Dynamic GraphQL schema building from DataView resolver mappings.
//!
//! Per `rivers-view-layer-spec.md` §9.

use std::collections::HashMap;

use async_graphql::dynamic::{self, FieldFuture, FieldValue};
use axum::Router;

use super::config::GraphqlConfig;
use super::mutations::{
    build_mutation_type_with_pool, build_subscription_type, MutationMapping, SubscriptionMapping,
};
use super::types::ResolverMapping;

/// GraphQL errors.
#[derive(Debug, thiserror::Error)]
pub enum GraphqlError {
    /// GraphQL is not enabled in config.
    #[error("GraphQL not enabled")]
    NotEnabled,

    /// Query nesting exceeds the configured maximum depth.
    #[error("query too deep: depth {0} exceeds max {1}")]
    DepthExceeded(usize, usize),

    /// Query complexity score exceeds the configured maximum.
    #[error("query too complex: complexity {0} exceeds max {1}")]
    ComplexityExceeded(usize, usize),

    /// A resolver returned an error during execution.
    #[error("resolver error: {0}")]
    ResolverError(String),

    /// Failed to build the dynamic GraphQL schema.
    #[error("schema build error: {0}")]
    SchemaError(String),

    /// Feature not yet implemented.
    #[error("async-graphql integration not yet available")]
    NotImplemented,
}

/// Convert a `serde_json::Value` into an `async_graphql::Value`.
pub(crate) fn json_to_gql_value(v: &serde_json::Value) -> async_graphql::Value {
    match v {
        serde_json::Value::Null => async_graphql::Value::Null,
        serde_json::Value::Bool(b) => async_graphql::Value::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                async_graphql::Value::from(i)
            } else if let Some(f) = n.as_f64() {
                async_graphql::Value::from(f)
            } else {
                async_graphql::Value::Null
            }
        }
        serde_json::Value::String(s) => async_graphql::Value::from(s.as_str()),
        serde_json::Value::Array(arr) => {
            async_graphql::Value::List(arr.iter().map(json_to_gql_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let map: async_graphql::indexmap::IndexMap<async_graphql::Name, async_graphql::Value> =
                obj.iter()
                    .map(|(k, v)| (async_graphql::Name::new(k), json_to_gql_value(v)))
                    .collect();
            async_graphql::Value::Object(map)
        }
    }
}

/// Build a dynamic GraphQL schema from DataView resolver mappings.
///
/// Each `ResolverMapping` becomes a field on the Query type.
/// Field resolvers return `async_graphql::Value` mapped from `serde_json::Value`.
///
/// The `resolve_fn` callback is invoked for each query, receiving the DataView name
/// and a map of arguments. It should return the JSON result from the DataView engine.
pub fn build_dynamic_schema<F>(
    config: &GraphqlConfig,
    resolvers: &[ResolverMapping],
    resolve_fn: F,
) -> Result<dynamic::Schema, GraphqlError>
where
    F: Fn(&str, HashMap<String, serde_json::Value>) -> Result<serde_json::Value, String>
        + Send
        + Sync
        + 'static,
{
    let resolve_fn = std::sync::Arc::new(resolve_fn);

    let mut query = dynamic::Object::new("Query");

    for resolver in resolvers {
        let dataview = resolver.dataview.clone();
        let arg_mapping = resolver.argument_mapping.clone();
        let is_list = resolver.is_list;
        let resolve_fn = resolve_fn.clone();

        let mut field = dynamic::Field::new(
            &resolver.field_name,
            if is_list {
                dynamic::TypeRef::List(Box::new(dynamic::TypeRef::named(
                    dynamic::TypeRef::STRING,
                )))
            } else {
                dynamic::TypeRef::named(dynamic::TypeRef::STRING)
            },
            move |ctx| {
                let dataview = dataview.clone();
                let arg_mapping = arg_mapping.clone();
                let resolve_fn = resolve_fn.clone();

                FieldFuture::new(async move {
                    // Collect arguments, mapping GraphQL arg names to DataView param names
                    let mut params = HashMap::new();
                    for (gql_arg, dv_param) in &arg_mapping {
                        if let Ok(val) = ctx.args.try_get(gql_arg.as_str()) {
                            // Extract string value from the argument
                            let json_val = if let Ok(s) = val.string() {
                                serde_json::Value::String(s.to_string())
                            } else if let Ok(i) = val.i64() {
                                serde_json::json!(i)
                            } else if let Ok(f) = val.f64() {
                                serde_json::json!(f)
                            } else if let Ok(b) = val.boolean() {
                                serde_json::Value::Bool(b)
                            } else {
                                serde_json::Value::Null
                            };
                            params.insert(dv_param.clone(), json_val);
                        }
                    }

                    let result = resolve_fn(&dataview, params)
                        .map_err(|e| async_graphql::Error::new(e))?;

                    let gql_val = json_to_gql_value(&result);
                    Ok(Some(FieldValue::value(gql_val)))
                })
            },
        );

        // Add GraphQL arguments based on the argument mapping
        for gql_arg in resolver.argument_mapping.keys() {
            field = field.argument(dynamic::InputValue::new(
                gql_arg.as_str(),
                dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            ));
        }

        query = query.field(field);
    }

    let mut schema_builder = dynamic::Schema::build("Query", None, None)
        .register(query);

    // Apply limits
    schema_builder = schema_builder
        .limit_depth(config.max_depth)
        .limit_complexity(config.max_complexity);

    // Toggle introspection
    if !config.introspection {
        schema_builder = schema_builder.disable_introspection();
    }

    schema_builder.finish().map_err(|e| {
        GraphqlError::SchemaError(format!("failed to build dynamic schema: {}", e))
    })
}

/// Convert an `async_graphql::Value` back to `serde_json::Value`.
#[allow(dead_code)] // Reserved for GraphQL response mapping (spec §9.2)
pub(crate) fn gql_value_to_json(v: &async_graphql::Value) -> serde_json::Value {
    match v {
        async_graphql::Value::Null => serde_json::Value::Null,
        async_graphql::Value::Number(n) => {
            serde_json::Value::Number(serde_json::Number::from_f64(n.as_f64().unwrap_or(0.0)).unwrap_or(serde_json::Number::from(0)))
        }
        async_graphql::Value::String(s) => serde_json::Value::String(s.to_string()),
        async_graphql::Value::Boolean(b) => serde_json::Value::Bool(*b),
        async_graphql::Value::List(arr) => {
            serde_json::Value::Array(arr.iter().map(gql_value_to_json).collect())
        }
        async_graphql::Value::Object(obj) => {
            let map: serde_json::Map<String, serde_json::Value> = obj
                .iter()
                .map(|(k, v)| (k.to_string(), gql_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        async_graphql::Value::Enum(e) => serde_json::Value::String(e.to_string()),
        async_graphql::Value::Binary(b) => {
            serde_json::Value::String(hex::encode(b))
        }
    }
}

/// Create an Axum router for the GraphQL endpoint.
///
/// Mounts a POST handler for queries (and optionally GET for GraphQL Playground
/// when introspection is enabled).
pub fn graphql_router(schema: dynamic::Schema, config: &GraphqlConfig) -> Router {
    use axum::routing::{get, post};

    let schema_for_post = schema.clone();

    let post_handler = post(move |req: async_graphql_axum::GraphQLRequest| {
        let schema = schema_for_post.clone();
        async move {
            let resp = schema.execute(req.into_inner()).await;
            async_graphql_axum::GraphQLResponse::from(resp)
        }
    });

    let mut router = Router::new().route(&config.path, post_handler);

    // Add playground at the same path via GET if introspection is enabled
    if config.introspection {
        let playground_path = config.path.clone();
        let _ = playground_path; // path already used in route

        let playground_handler = get(|| async {
            axum::response::Html(
                async_graphql::http::playground_source(
                    async_graphql::http::GraphQLPlaygroundConfig::new("/graphql"),
                ),
            )
        });

        // Mount playground on a separate path to avoid route conflict
        let playground_path = format!("{}/playground", config.path.trim_end_matches('/'));
        router = router.route(&playground_path, playground_handler);
    }

    router
}

/// Build a dynamic GraphQL schema wired to a live DataViewExecutor.
///
/// Each resolver field executes the mapped DataView asynchronously via the shared executor.
/// The executor is behind `Arc<RwLock<Option<...>>>` to support hot reload.
pub fn build_schema_with_executor(
    config: &GraphqlConfig,
    resolvers: &[ResolverMapping],
    executor: std::sync::Arc<tokio::sync::RwLock<Option<std::sync::Arc<rivers_runtime::DataViewExecutor>>>>,
    mutations: &[MutationMapping],
    pool: std::sync::Arc<crate::process_pool::ProcessPoolManager>,
    subscriptions: &[SubscriptionMapping],
    event_bus: std::sync::Arc<rivers_runtime::rivers_core::EventBus>,
) -> Result<dynamic::Schema, GraphqlError> {
    let mut query = dynamic::Object::new("Query");

    for resolver in resolvers {
        let dataview = resolver.dataview.clone();
        let arg_mapping = resolver.argument_mapping.clone();
        let is_list = resolver.is_list;
        let executor = executor.clone();

        let mut field = dynamic::Field::new(
            &resolver.field_name,
            if is_list {
                dynamic::TypeRef::List(Box::new(dynamic::TypeRef::named(
                    dynamic::TypeRef::STRING,
                )))
            } else {
                dynamic::TypeRef::named(dynamic::TypeRef::STRING)
            },
            move |ctx| {
                let dataview = dataview.clone();
                let arg_mapping = arg_mapping.clone();
                let executor = executor.clone();

                FieldFuture::new(async move {
                    // Convert GraphQL args to DataView params
                    let mut params = std::collections::HashMap::new();
                    for (gql_arg, dv_param) in &arg_mapping {
                        if let Ok(val) = ctx.args.try_get(gql_arg.as_str()) {
                            let qv = if let Ok(s) = val.string() {
                                rivers_runtime::rivers_driver_sdk::types::QueryValue::String(s.to_string())
                            } else if let Ok(i) = val.i64() {
                                rivers_runtime::rivers_driver_sdk::types::QueryValue::Integer(i)
                            } else if let Ok(f) = val.f64() {
                                rivers_runtime::rivers_driver_sdk::types::QueryValue::Float(f)
                            } else if let Ok(b) = val.boolean() {
                                rivers_runtime::rivers_driver_sdk::types::QueryValue::Boolean(b)
                            } else {
                                rivers_runtime::rivers_driver_sdk::types::QueryValue::Null
                            };
                            params.insert(dv_param.clone(), qv);
                        }
                    }

                    let guard = executor.read().await;
                    let exec = guard.as_ref().ok_or_else(|| {
                        async_graphql::Error::new("DataView executor not available")
                    })?;

                    let trace_id = format!("gql-{}", uuid::Uuid::new_v4());
                    let response = exec
                        .execute(&dataview, params, "GET", &trace_id, None)
                        .await
                        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

                    // Convert QueryResult rows to GraphQL JSON value
                    let json_val = serde_json::to_value(&response.query_result.rows)
                        .unwrap_or(serde_json::Value::Null);
                    let gql_val = json_to_gql_value(&json_val);
                    Ok(Some(FieldValue::value(gql_val)))
                })
            },
        );

        // Add GraphQL arguments
        for gql_arg in resolver.argument_mapping.keys() {
            field = field.argument(dynamic::InputValue::new(
                gql_arg.as_str(),
                dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            ));
        }

        query = query.field(field);
    }

    // Build mutation type -- real CodeComponent dispatch if mutations available, else stub
    let mutation = build_mutation_type_with_pool(mutations, pool);

    // Build subscription type -- EventBus topic bridges
    let subscription = build_subscription_type(subscriptions, event_bus);

    let mut schema_builder = dynamic::Schema::build("Query", Some("Mutation"), Some("Subscription"))
        .register(query)
        .register(mutation)
        .register(subscription);

    schema_builder = schema_builder
        .limit_depth(config.max_depth)
        .limit_complexity(config.max_complexity);

    if !config.introspection {
        schema_builder = schema_builder.disable_introspection();
    }

    schema_builder.finish().map_err(|e| {
        GraphqlError::SchemaError(format!("failed to build dynamic schema: {}", e))
    })
}

// ── Validation ──────────────────────────────────────────────────

/// Validate GraphQL configuration.
pub fn validate_graphql_config(config: &GraphqlConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if config.enabled {
        if config.path.is_empty() {
            errors.push("graphql.path must not be empty".to_string());
        }
        if !config.path.starts_with('/') {
            errors.push("graphql.path must start with '/'".to_string());
        }
        if config.max_depth == 0 {
            errors.push("graphql.max_depth must be > 0".to_string());
        }
        if config.max_complexity == 0 {
            errors.push("graphql.max_complexity must be > 0".to_string());
        }
    }

    errors
}
