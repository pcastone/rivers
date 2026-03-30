//! Mutation and subscription mapping for GraphQL schema.
//!
//! Mutation fields are derived from CodeComponent-backed views.
//! Subscription fields bridge EventBus topics to GraphQL subscriptions.

use std::collections::HashMap;

use async_graphql::dynamic::{self, FieldFuture, FieldValue};

use super::schema_builder::json_to_gql_value;
use super::types::ResolverMapping;

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
            // Strip namespace prefix (e.g. "address-book-service:list_contacts" -> "list_contacts")
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

/// Build the Mutation type from CodeComponent-backed mutation mappings.
///
/// When `mutations` is empty, falls back to a `_noop` placeholder for schema validity.
/// When populated, each mapping becomes a field that dispatches to ProcessPool.
pub(super) fn build_mutation_type_with_pool(
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

                    let builder = crate::process_pool::TaskContextBuilder::new()
                        .entrypoint(entrypoint)
                        .args(serde_json::json!({
                            "request": { "body": input_json, "method": http_method },
                            "view_id": view_id,
                        }))
                        .trace_id(trace_id);
                    let builder = crate::task_enrichment::enrich(builder, "");
                    let task_ctx = builder
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
pub(super) fn build_subscription_type(
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
