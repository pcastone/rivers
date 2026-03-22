//! GraphQL integration types and async-graphql runtime.
//!
//! Per `rivers-view-layer-spec.md` §9.
//!
//! Provides the bridge between DataView configs and GraphQL schema generation.
//! Uses `async_graphql::dynamic` for runtime schema building from resolver mappings.

use std::collections::HashMap;

use async_graphql::dynamic::{self, FieldFuture, FieldValue};
use axum::Router;
use serde::{Deserialize, Serialize};

// ── GraphQL Config ──────────────────────────────────────────────

/// GraphQL endpoint configuration.
///
/// Per spec §9: configurable path, introspection toggle.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphqlConfig {
    /// Whether GraphQL is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Path for the GraphQL endpoint (default: "/graphql").
    #[serde(default = "default_graphql_path")]
    pub path: String,

    /// Allow introspection queries.
    #[serde(default = "default_introspection")]
    pub introspection: bool,

    /// Max query depth (default: 10).
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    /// Max query complexity (default: 1000).
    #[serde(default = "default_max_complexity")]
    pub max_complexity: usize,
}

fn default_graphql_path() -> String {
    "/graphql".to_string()
}

fn default_introspection() -> bool {
    true
}

fn default_max_depth() -> usize {
    10
}

fn default_max_complexity() -> usize {
    1000
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_graphql_path(),
            introspection: true,
            max_depth: 10,
            max_complexity: 1000,
        }
    }
}

impl From<&rivers_runtime::rivers_core::GraphqlServerConfig> for GraphqlConfig {
    fn from(server_cfg: &rivers_runtime::rivers_core::GraphqlServerConfig) -> Self {
        Self {
            enabled: server_cfg.enabled,
            path: server_cfg.path.clone(),
            introspection: server_cfg.introspection,
            max_depth: server_cfg.max_depth,
            max_complexity: server_cfg.max_complexity,
        }
    }
}

// ── Resolver Bridge ─────────────────────────────────────────────

/// Maps a GraphQL query field to a DataView.
///
/// Per spec §9.2: GraphQL query field → DataView name, arguments → DataView parameters.
#[derive(Debug, Clone, Serialize)]
pub struct ResolverMapping {
    /// GraphQL field name.
    pub field_name: String,
    /// DataView to execute.
    pub dataview: String,
    /// Argument → DataView parameter mapping.
    pub argument_mapping: HashMap<String, String>,
    /// Whether this is a list (returns array) or scalar (returns single object).
    pub is_list: bool,
}

/// A GraphQL type generated from a DataView return schema.
#[derive(Debug, Clone, Serialize)]
pub struct GraphqlType {
    pub name: String,
    pub fields: Vec<GraphqlField>,
}

/// A field in a GraphQL type.
#[derive(Debug, Clone, Serialize)]
pub struct GraphqlField {
    pub name: String,
    pub field_type: GraphqlFieldType,
    pub nullable: bool,
}

/// GraphQL field type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum GraphqlFieldType {
    String,
    Int,
    Float,
    Boolean,
    ID,
    /// Reference to another GraphQL type.
    Object(String),
    /// List of another type.
    List(Box<GraphqlFieldType>),
}

impl GraphqlFieldType {
    /// Map a JSON schema type to a GraphQL field type.
    pub fn from_json_schema_type(type_str: &str) -> Self {
        match type_str {
            "string" => GraphqlFieldType::String,
            "integer" => GraphqlFieldType::Int,
            "number" => GraphqlFieldType::Float,
            "boolean" => GraphqlFieldType::Boolean,
            _ => GraphqlFieldType::String, // fallback
        }
    }

    /// Convert to an async-graphql `TypeRef`.
    #[allow(dead_code)] // Reserved for GraphQL resolver support (spec §9.2)
    fn to_type_ref(&self, nullable: bool) -> dynamic::TypeRef {
        let inner = match self {
            GraphqlFieldType::String => dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            GraphqlFieldType::Int => dynamic::TypeRef::named(dynamic::TypeRef::INT),
            GraphqlFieldType::Float => dynamic::TypeRef::named(dynamic::TypeRef::FLOAT),
            GraphqlFieldType::Boolean => dynamic::TypeRef::named(dynamic::TypeRef::BOOLEAN),
            GraphqlFieldType::ID => dynamic::TypeRef::named(dynamic::TypeRef::ID),
            GraphqlFieldType::Object(name) => dynamic::TypeRef::named(name.as_str()),
            GraphqlFieldType::List(inner_type) => {
                // Inner list elements are non-null by default
                let element_ref = inner_type.to_type_ref(false);
                dynamic::TypeRef::List(Box::new(element_ref))
            }
        };

        if nullable {
            inner
        } else {
            dynamic::TypeRef::NonNull(Box::new(inner))
        }
    }
}

// ── Schema Generation ───────────────────────────────────────────

/// Generate GraphQL type definitions from DataView return schemas.
///
/// Per spec §9.2: return_schema → GraphQL object types.
pub fn generate_graphql_types(
    dataview_schemas: &HashMap<String, serde_json::Value>,
) -> Vec<GraphqlType> {
    let mut types = Vec::new();

    for (name, schema) in dataview_schemas {
        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            let fields: Vec<GraphqlField> = properties
                .iter()
                .map(|(field_name, field_schema)| {
                    let type_str = field_schema
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("string");

                    let field_type = if type_str == "array" {
                        let item_type = field_schema
                            .get("items")
                            .and_then(|i| i.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("string");
                        GraphqlFieldType::List(Box::new(
                            GraphqlFieldType::from_json_schema_type(item_type),
                        ))
                    } else {
                        GraphqlFieldType::from_json_schema_type(type_str)
                    };

                    let required = schema
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|arr| {
                            arr.iter()
                                .any(|v| v.as_str() == Some(field_name.as_str()))
                        })
                        .unwrap_or(false);

                    GraphqlField {
                        name: field_name.clone(),
                        field_type,
                        nullable: !required,
                    }
                })
                .collect();

            // Convert DataView name to PascalCase for GraphQL type name
            let type_name = to_pascal_case(name);

            types.push(GraphqlType {
                name: type_name,
                fields,
            });
        }
    }

    types
}

/// Convert snake_case or kebab-case to PascalCase.
fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.extend(chars);
                    s
                }
            }
        })
        .collect()
}

// ── Dynamic Schema Building ─────────────────────────────────────

/// Convert a `serde_json::Value` into an `async_graphql::Value`.
fn json_to_gql_value(v: &serde_json::Value) -> async_graphql::Value {
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
fn gql_value_to_json(v: &async_graphql::Value) -> serde_json::Value {
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

// ── Mutation Mapping ────────────────────────────────────────────

/// Maps a GraphQL mutation field to a CodeComponent entrypoint.
///
/// Derived from views with `handler = codecomponent` and `method != GET`.
#[derive(Debug, Clone)]
pub struct MutationMapping {
    /// GraphQL field name (derived from view_id).
    pub field_name: String,
    /// CodeComponent entrypoint to dispatch.
    pub entrypoint: crate::process_pool::Entrypoint,
    /// HTTP method the original view handles (POST, PUT, DELETE).
    pub http_method: String,
    /// Qualified view ID for tracing.
    pub view_id: String,
}

/// Build mutation mappings from views with CodeComponent handlers.
///
/// Filters for views where `handler = codecomponent` and `method != GET`.
/// Each qualifying view becomes a GraphQL Mutation field.
pub fn build_mutation_mappings_from_views(
    views: &HashMap<String, rivers_runtime::view::ApiViewConfig>,
    entry_point: &str,
) -> Vec<MutationMapping> {
    let mut mappings = Vec::new();

    for (view_id, view) in views {
        let method = view.method.as_deref().unwrap_or("GET").to_uppercase();
        if method == "GET" {
            continue;
        }

        if let rivers_runtime::view::HandlerConfig::Codecomponent {
            ref language,
            ref module,
            ref entrypoint,
            ..
        } = view.handler
        {
            // Convert view_id to valid GraphQL field name (replace dashes/dots with underscores)
            let field_name = view_id.replace(['-', '.'], "_");

            mappings.push(MutationMapping {
                field_name,
                entrypoint: crate::process_pool::Entrypoint {
                    module: module.clone(),
                    function: entrypoint.clone(),
                    language: language.clone(),
                },
                http_method: method,
                view_id: format!("{}:{}", entry_point, view_id),
            });
        }
    }

    mappings
}

// ── Schema building from bundle ────────────────────────────────

/// Build resolver mappings from DataView registry names.
///
/// Each DataView with a GET query becomes a Query field.
/// The field name is derived from the DataView name (last segment after ':').
pub fn build_resolver_mappings_from_dataviews(
    dataview_names: &[&str],
) -> Vec<ResolverMapping> {
    dataview_names
        .iter()
        .map(|name| {
            // Strip namespace prefix (e.g. "address-book-service:list_contacts" → "list_contacts")
            let field_name = name
                .rsplit(':')
                .next()
                .unwrap_or(name)
                .to_string();

            ResolverMapping {
                field_name,
                dataview: name.to_string(),
                argument_mapping: HashMap::new(),
                is_list: true, // Default to list for now; can refine later
            }
        })
        .collect()
}

/// Build a dynamic GraphQL schema wired to a live DataViewExecutor.
///
/// Each resolver field executes the mapped DataView asynchronously via the shared executor.
/// The executor is behind `Arc<RwLock<Option<...>>>` to support hot reload.
pub fn build_schema_with_executor(
    config: &GraphqlConfig,
    resolvers: &[ResolverMapping],
    executor: std::sync::Arc<tokio::sync::RwLock<Option<rivers_runtime::DataViewExecutor>>>,
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
                        .execute(&dataview, params, &trace_id)
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

    // Build mutation type — real CodeComponent dispatch if mutations available, else stub
    let mutation = build_mutation_type_with_pool(mutations, pool);

    // Build subscription type — EventBus topic bridges
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

/// Build the Mutation type from CodeComponent-backed mutation mappings.
///
/// When `mutations` is empty, falls back to a `_noop` placeholder for schema validity.
/// When populated, each mapping becomes a field that dispatches to ProcessPool.
fn build_mutation_type_with_pool(
    mutations: &[MutationMapping],
    pool: std::sync::Arc<crate::process_pool::ProcessPoolManager>,
) -> dynamic::Object {
    let mut mutation = dynamic::Object::new("Mutation");

    if mutations.is_empty() {
        // Fallback stub when no CodeComponent mutations are configured
        mutation = mutation.field(dynamic::Field::new(
            "_noop",
            dynamic::TypeRef::named(dynamic::TypeRef::BOOLEAN),
            |_ctx| FieldFuture::new(async { Ok(Some(FieldValue::value(async_graphql::Value::from(true)))) }),
        ).description("No CodeComponent mutations configured"));
        return mutation;
    }

    for mapping in mutations {
        let entrypoint = mapping.entrypoint.clone();
        let http_method = mapping.http_method.clone();
        let view_id = mapping.view_id.clone();
        let pool = pool.clone();

        let field = dynamic::Field::new(
            &mapping.field_name,
            dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            move |ctx| {
                let entrypoint = entrypoint.clone();
                let http_method = http_method.clone();
                let view_id = view_id.clone();
                let pool = pool.clone();

                FieldFuture::new(async move {
                    // Extract input argument (JSON string)
                    let input_str = ctx.args.try_get("input")
                        .ok()
                        .and_then(|v| v.string().ok())
                        .unwrap_or("{}");

                    let input_json: serde_json::Value = serde_json::from_str(input_str)
                        .unwrap_or(serde_json::json!({}));

                    let trace_id = format!("gql-mut-{}", uuid::Uuid::new_v4());

                    let task_ctx = crate::process_pool::TaskContextBuilder::new()
                        .entrypoint(entrypoint)
                        .args(serde_json::json!({
                            "request": { "body": input_json, "method": http_method },
                            "view_id": view_id,
                        }))
                        .trace_id(trace_id)
                        .build()
                        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

                    let result = pool.dispatch("default", task_ctx)
                        .await
                        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

                    let gql_val = json_to_gql_value(&result.value);
                    Ok(Some(FieldValue::value(gql_val)))
                })
            },
        ).argument(dynamic::InputValue::new(
            "input",
            dynamic::TypeRef::named(dynamic::TypeRef::STRING),
        )).description(format!("{} via CodeComponent", mapping.http_method));

        mutation = mutation.field(field);
    }

    mutation
}

// ── Subscription Mapping ────────────────────────────────────────

/// Maps a GraphQL subscription field to an EventBus topic.
#[derive(Debug, Clone)]
pub struct SubscriptionMapping {
    /// GraphQL field name.
    pub field_name: String,
    /// EventBus topic to subscribe to.
    pub event_topic: String,
}

/// Build subscription mappings from SSE views' trigger events.
///
/// Each unique event name in any view's `sse_trigger_events` becomes
/// a GraphQL Subscription field.
pub fn build_subscription_mappings_from_views(
    views: &HashMap<String, rivers_runtime::view::ApiViewConfig>,
) -> Vec<SubscriptionMapping> {
    let mut seen = std::collections::HashSet::new();
    let mut mappings = Vec::new();

    for view in views.values() {
        for event_name in &view.sse_trigger_events {
            if seen.insert(event_name.clone()) {
                let field_name = event_name.replace(['-', '.', ':'], "_");
                mappings.push(SubscriptionMapping {
                    field_name,
                    event_topic: event_name.clone(),
                });
            }
        }
    }

    mappings
}

/// Build the Subscription type from EventBus topic mappings.
///
/// Each mapping becomes a field that yields events from the EventBus as a stream.
/// When no subscriptions exist, returns a `_noop` placeholder for schema validity.
fn build_subscription_type(
    subscriptions: &[SubscriptionMapping],
    event_bus: std::sync::Arc<rivers_runtime::rivers_core::EventBus>,
) -> dynamic::Subscription {
    use tokio_stream::StreamExt as _;

    let mut sub = dynamic::Subscription::new("Subscription");

    if subscriptions.is_empty() {
        sub = sub.field(dynamic::SubscriptionField::new(
            "_noop",
            dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            |_ctx| {
                dynamic::SubscriptionFieldFuture::new(async {
                    let stream = tokio_stream::once(
                        Ok::<FieldValue<'_>, async_graphql::Error>(
                            FieldValue::value(async_graphql::Value::from("no subscriptions configured"))
                        )
                    );
                    Ok(stream)
                })
            },
        ).description("No EventBus subscriptions configured"));
        return sub;
    }

    for mapping in subscriptions {
        let topic = mapping.event_topic.clone();
        let event_bus = event_bus.clone();

        sub = sub.field(dynamic::SubscriptionField::new(
            &mapping.field_name,
            dynamic::TypeRef::named(dynamic::TypeRef::STRING),
            move |_ctx| {
                let topic = topic.clone();
                let event_bus = event_bus.clone();

                dynamic::SubscriptionFieldFuture::new(async move {
                    let sender = event_bus.subscribe_broadcast(&topic, 256).await;
                    let receiver = sender.subscribe();
                    let stream = tokio_stream::wrappers::BroadcastStream::new(receiver)
                        .filter_map(|result| {
                            match result {
                                Ok(event) => {
                                    let payload_str = serde_json::to_string(&event.payload)
                                        .unwrap_or_else(|_| "null".to_string());
                                    Some(Ok(FieldValue::value(
                                        async_graphql::Value::from(payload_str.as_str()),
                                    )))
                                }
                                Err(_) => None,
                            }
                        });
                    Ok(stream)
                })
            },
        ).description(format!("Events from EventBus topic '{}'", mapping.event_topic)));
    }

    sub
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

// ── Error Types ─────────────────────────────────────────────────

/// GraphQL errors.
#[derive(Debug, thiserror::Error)]
pub enum GraphqlError {
    #[error("GraphQL not enabled")]
    NotEnabled,

    #[error("query too deep: depth {0} exceeds max {1}")]
    DepthExceeded(usize, usize),

    #[error("query too complex: complexity {0} exceeds max {1}")]
    ComplexityExceeded(usize, usize),

    #[error("resolver error: {0}")]
    ResolverError(String),

    #[error("schema build error: {0}")]
    SchemaError(String),

    #[error("async-graphql integration not yet available")]
    NotImplemented,
}
