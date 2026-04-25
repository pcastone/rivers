//! MessageConsumer view layer.
//!
//! Per `rivers-view-layer-spec.md` §8.
//!
//! MessageConsumer views have no HTTP route. They are event-driven,
//! subscribing to EventBus topics populated by BrokerConsumerBridge.
//! Direct HTTP access returns 400.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use rivers_runtime::rivers_core::event::Event;
use rivers_runtime::rivers_core::eventbus::{EventBus, EventHandler, HandlerPriority};
use rivers_runtime::view::ApiViewConfig;
use tokio::sync::broadcast;

use crate::process_pool::{
    Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError,
};

// ── MessageConsumer Config ──────────────────────────────────

/// Parsed MessageConsumer view configuration.
#[derive(Debug, Clone)]
pub struct MessageConsumerConfig {
    /// View identifier.
    pub view_id: String,
    /// EventBus topic to subscribe to.
    pub topic: String,
    /// Handler module and entrypoint.
    pub handler: String,
    /// Handler mode (Stream, Normal, Auto).
    pub handler_mode: Option<String>,
    /// Auth mode — "none" by default (auto-exempt), but can opt-in to "session".
    pub auth: Option<String>,
    /// App ID this consumer belongs to — required so dispatched events land in
    /// the right per-app store namespace and inherit the right capabilities.
    /// Stamped at registry build time from the bundle's app entry_point.
    pub app_id: String,
}

impl MessageConsumerConfig {
    /// Extract MessageConsumer config from an ApiViewConfig.
    ///
    /// Returns None if view_type != "MessageConsumer" or on_event is missing.
    pub fn from_view(view_id: &str, config: &ApiViewConfig, app_id: &str) -> Option<Self> {
        if config.view_type != "MessageConsumer" {
            return None;
        }

        let on_event = config.on_event.as_ref()?;

        Some(Self {
            view_id: view_id.to_string(),
            topic: on_event.topic.clone(),
            handler: on_event.handler.clone(),
            handler_mode: on_event.handler_mode.clone(),
            auth: config.auth.clone(),
            app_id: app_id.to_string(),
        })
    }
}

// ── Message Event Payload ───────────────────────────────────

/// Payload shape for a message event delivered to a MessageConsumer handler.
///
/// Per spec §8: event payload becomes the request body for the CodeComponent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEventPayload {
    /// The raw event data.
    pub data: serde_json::Value,
    /// Topic the message was received on.
    pub topic: String,
    /// Partition or queue info (if applicable).
    pub partition: Option<String>,
    /// Message offset/sequence (if applicable).
    pub offset: Option<String>,
    /// Trace ID for distributed tracing.
    pub trace_id: Option<String>,
    /// Timestamp of the original message.
    pub timestamp: Option<String>,
}

// ── MessageConsumer Registry ────────────────────────────────

/// Registry of all MessageConsumer views in the application.
///
/// Used during startup to set up EventBus subscriptions.
pub struct MessageConsumerRegistry {
    consumers: HashMap<String, MessageConsumerConfig>,
}

impl MessageConsumerRegistry {
    /// Build registry from all view configs for a given app.
    pub fn from_views(views: &HashMap<String, ApiViewConfig>, app_id: &str) -> Self {
        let mut consumers = HashMap::new();

        for (id, config) in views {
            if let Some(mc_config) = MessageConsumerConfig::from_view(id, config, app_id) {
                consumers.insert(id.clone(), mc_config);
            }
        }

        Self { consumers }
    }

    /// Get all registered consumers.
    pub fn consumers(&self) -> &HashMap<String, MessageConsumerConfig> {
        &self.consumers
    }

    /// Get a specific consumer by view ID.
    pub fn get(&self, view_id: &str) -> Option<&MessageConsumerConfig> {
        self.consumers.get(view_id)
    }

    /// Number of registered consumers.
    pub fn len(&self) -> usize {
        self.consumers.len()
    }

    /// Check if registry is empty.
    pub fn is_empty(&self) -> bool {
        self.consumers.is_empty()
    }

    /// Get all topics that need EventBus subscriptions.
    pub fn topics(&self) -> Vec<String> {
        self.consumers
            .values()
            .map(|c| c.topic.clone())
            .collect()
    }
}

// ── Validation ──────────────────────────────────────────────

/// Validate MessageConsumer views.
///
/// Per spec §8: no path, no HTTP route, on_event required.
pub fn validate_message_consumers(views: &HashMap<String, ApiViewConfig>) -> Vec<String> {
    let mut errors = Vec::new();

    for (id, config) in views {
        if config.view_type != "MessageConsumer" {
            continue;
        }

        // Must not have a path
        if config.path.is_some() {
            errors.push(format!(
                "MessageConsumer '{}': must not declare a path",
                id
            ));
        }

        // Must have on_event
        if config.on_event.is_none() {
            errors.push(format!(
                "MessageConsumer '{}': on_event configuration is required",
                id
            ));
        }

        // on_stream is not valid for MessageConsumer
        if config.on_stream.is_some() {
            errors.push(format!(
                "MessageConsumer '{}': on_stream is not valid for MessageConsumer views",
                id
            ));
        }
    }

    errors
}

// ── EventBus Subscription (D10) ─────────────────────────────

/// Subscribe a MessageConsumer to its EventBus topic.
///
/// Per spec §8: MessageConsumer views subscribe to EventBus topics
/// populated by BrokerConsumerBridge. Returns a broadcast receiver
/// for the topic.
pub async fn subscribe_consumer(
    config: &MessageConsumerConfig,
    event_bus: &EventBus,
) -> broadcast::Receiver<Event> {
    let (tx, rx) = broadcast::channel::<Event>(256);

    let handler = Arc::new(MessageConsumerForwarder {
        view_id: config.view_id.clone(),
        sender: tx,
    });

    event_bus
        .subscribe(
            config.topic.clone(),
            handler,
            HandlerPriority::Handle,
        )
        .await;

    rx
}

/// Internal EventHandler that forwards events to a broadcast channel.
struct MessageConsumerForwarder {
    view_id: String,
    sender: broadcast::Sender<Event>,
}

#[async_trait::async_trait]
impl EventHandler for MessageConsumerForwarder {
    async fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.sender
            .send(event.clone())
            .map_err(|_| format!("MessageConsumer '{}': no receivers", self.view_id))?;
        Ok(())
    }

    fn name(&self) -> &str {
        &self.view_id
    }
}

// ── Message Handler Dispatch (D11) ──────────────────────────

/// Dispatch a message event to the MessageConsumer's CodeComponent handler.
///
/// Per spec §8: the handler receives the message payload and topic info.
/// Returns the handler's JSON result.
pub async fn dispatch_message_event(
    pool: &ProcessPoolManager,
    config: &MessageConsumerConfig,
    payload: MessageEventPayload,
    trace_id: &str,
) -> Result<serde_json::Value, TaskError> {
    let entrypoint = Entrypoint {
        module: config.handler.clone(),
        function: "handle".to_string(),
        language: "javascript".to_string(),
    };

    let args = serde_json::json!({
        "data": payload.data,
        "topic": payload.topic,
        "partition": payload.partition,
        "offset": payload.offset,
        "timestamp": payload.timestamp,
    });

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(trace_id.to_string());
    let builder = crate::task_enrichment::enrich(
        builder,
        &config.app_id,
        rivers_runtime::process_pool::TaskKind::MessageConsumer,
    );
    let ctx = builder.build()?;

    let result = pool.dispatch("default", ctx).await?;
    Ok(result.value)
}

// ── Bulk EventBus Subscription (N4.1–N4.2) ──────────────────

/// Subscribe all MessageConsumer views to their EventBus topics.
///
/// Per technology-path-spec §12.7: the developer never sees the EventBus.
/// This wiring happens automatically at startup.
pub async fn subscribe_message_consumers(
    registry: &MessageConsumerRegistry,
    event_bus: &EventBus,
    pool: Arc<ProcessPoolManager>,
) {
    for (_, consumer) in registry.consumers() {
        let handler = MessageConsumerHandler {
            config: consumer.clone(),
            pool: pool.clone(),
        };
        event_bus
            .subscribe(
                consumer.topic.clone(),
                Arc::new(handler),
                HandlerPriority::Handle,
            )
            .await;
    }
}

/// EventHandler that dispatches incoming events to CodeComponent handlers.
struct MessageConsumerHandler {
    config: MessageConsumerConfig,
    pool: Arc<ProcessPoolManager>,
}

#[async_trait::async_trait]
impl EventHandler for MessageConsumerHandler {
    async fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let entrypoint = Entrypoint {
            module: self.config.handler.clone(),
            function: self
                .config
                .handler_mode
                .clone()
                .unwrap_or_else(|| "onEvent".into()),
            language: "javascript".into(),
        };

        let args = serde_json::json!({
            "event": event.payload,
            "topic": event.event_type,
            "trace_id": event.trace_id,
            "timestamp": event.timestamp.to_rfc3339(),
        });

        let builder = TaskContextBuilder::new()
            .entrypoint(entrypoint)
            .args(args)
            .trace_id(event.trace_id.clone().unwrap_or_default());
        let builder = crate::task_enrichment::enrich(
            builder,
            &self.config.app_id,
            rivers_runtime::process_pool::TaskKind::MessageConsumer,
        );
        let task_ctx = builder
            .build()
            .map_err(|e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        self.pool
            .dispatch("default", task_ctx)
            .await
            .map_err(|e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?;

        Ok(())
    }

    fn name(&self) -> &str {
        &self.config.view_id
    }
}

// ── Error Types ─────────────────────────────────────────────

/// MessageConsumer errors.
#[derive(Debug, thiserror::Error)]
pub enum MessageConsumerError {
    /// Direct HTTP access is not allowed for MessageConsumer views.
    #[error("direct HTTP access not allowed for MessageConsumer view '{0}'")]
    DirectHttpAccess(String),

    /// Handler requires a CodeComponent that is not loaded.
    #[error("handler requires CodeComponent (not yet available)")]
    CodeComponentRequired,

    /// The referenced EventBus topic does not exist.
    #[error("topic not found: {0}")]
    TopicNotFound(String),

    /// The CodeComponent handler returned an error.
    #[error("handler error: {0}")]
    HandlerError(String),
}

// ── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscribe_consumer() {
        let event_bus = EventBus::new();
        let config = MessageConsumerConfig {
            view_id: "order_consumer".into(),
            topic: "orders.created".into(),
            handler: "order_handler.js".into(),
            handler_mode: None,
            auth: None,
            app_id: "test-app".into(),
        };

        let mut rx = subscribe_consumer(&config, &event_bus).await;

        // Verify subscription was registered
        let count = event_bus.subscriber_count("orders.created").await;
        assert_eq!(count, 1);

        // Publish an event and verify it's received
        let event = Event::new("orders.created", serde_json::json!({"order_id": "o-1"}));
        event_bus.publish(&event).await;

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, "orders.created");
    }

    #[tokio::test]
    async fn test_subscribe_message_consumers_bulk() {
        let event_bus = EventBus::new();
        let pool = Arc::new(ProcessPoolManager::from_config(&HashMap::new()));

        let mut views = HashMap::new();
        views.insert(
            "order_consumer".into(),
            MessageConsumerConfig {
                view_id: "order_consumer".into(),
                topic: "orders.created".into(),
                handler: "order_handler.js".into(),
                handler_mode: None,
                auth: None,
                app_id: "test-app".into(),
            },
        );
        views.insert(
            "payment_consumer".into(),
            MessageConsumerConfig {
                view_id: "payment_consumer".into(),
                topic: "payments.received".into(),
                handler: "payment_handler.js".into(),
                handler_mode: Some("onPayment".into()),
                auth: None,
                app_id: "test-app".into(),
            },
        );

        let registry = MessageConsumerRegistry { consumers: views };
        subscribe_message_consumers(&registry, &event_bus, pool).await;

        assert_eq!(event_bus.subscriber_count("orders.created").await, 1);
        assert_eq!(event_bus.subscriber_count("payments.received").await, 1);
    }

    #[tokio::test]
    async fn test_dispatch_message_event_engine_unavailable() {
        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let config = MessageConsumerConfig {
            view_id: "consumer".into(),
            topic: "test.topic".into(),
            handler: "handler.js".into(),
            handler_mode: None,
            auth: None,
            app_id: "test-app".into(),
        };
        let payload = MessageEventPayload {
            data: serde_json::json!({"key": "value"}),
            topic: "test.topic".into(),
            partition: None,
            offset: None,
            trace_id: None,
            timestamp: None,
        };

        let result = dispatch_message_event(&pool, &config, payload, "trace-1").await;
        assert!(result.is_err());
    }
}
