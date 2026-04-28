#![warn(missing_docs)]
//! NATS plugin driver — async-nats 0.38 (pure Rust).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rivers_driver_sdk::{
    AckOutcome, BrokerConsumer, BrokerConsumerConfig, BrokerError, BrokerMetadata, BrokerProducer,
    BrokerSemantics, ConnectionParams, DriverError, DriverRegistrar, InboundMessage,
    MessageBrokerDriver, MessageReceipt, OutboundMessage, PublishReceipt, ABI_VERSION,
};

// ── Driver ─────────────────────────────────────────────────────────

/// NATS driver factory — creates producers and consumers via core NATS pub/sub.
///
/// Core NATS semantics are `AtMostOnce` — the server delivers each message
/// to all current subscribers once; there is no persistent queue or redelivery
/// on consumer restart. JetStream (not implemented here) would upgrade this to
/// `AtLeastOnce`.
pub struct NatsDriver;

#[async_trait]
impl MessageBrokerDriver for NatsDriver {
    fn name(&self) -> &str {
        "nats"
    }

    /// Core NATS is fire-and-forget pub/sub — no redelivery on nack.
    fn semantics(&self) -> BrokerSemantics {
        BrokerSemantics::AtMostOnce
    }

    async fn create_producer(
        &self,
        params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        let client = nats_connect(params).await?;
        Ok(Box::new(NatsProducer { client }))
    }

    async fn create_consumer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        let client = nats_connect(params).await?;

        // Derive the queue group name for load-balanced delivery across instances.
        // All instances sharing the same group_prefix.app_id.datasource_id will
        // form a queue group — only one member receives each message.
        let queue_group = format!(
            "{}.{}.{}",
            config.group_prefix, config.app_id, config.datasource_id
        );

        let consumer_name = format!("{queue_group}.consumer");

        // RW2.2.c: subscribe to ALL configured subjects, not just the first.
        // Each subject gets its own queue_subscribe call so the consumer can
        // receive from multiple topics in a single receive() loop.
        let subjects: Vec<String> = if config.subscriptions.is_empty() {
            // Fallback to the database field when no subscriptions are configured.
            vec![params.database.clone()]
        } else {
            config.subscriptions.iter().map(|s| s.topic.clone()).collect()
        };

        // One mpsc channel aggregates messages from all subscribers so receive()
        // can await a single channel.recv() instead of polling subscribers in
        // sequence. Each subscriber runs in its own task, so all subjects are
        // polled concurrently — no subject starves another.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<async_nats::Message>();
        let mut tasks = Vec::with_capacity(subjects.len());

        for subject in &subjects {
            // RW2.2.a: use queue_subscribe for consumer-group semantics — only
            // one consumer in the queue group receives each message.
            let sub = client
                .queue_subscribe(subject.clone(), queue_group.clone())
                .await
                .map_err(|e| {
                    DriverError::Connection(format!(
                        "nats queue_subscribe({subject}, {queue_group}): {e}"
                    ))
                })?;

            let task_tx = tx.clone();
            tasks.push(tokio::spawn(async move {
                use futures_lite::StreamExt;
                let mut sub = sub;
                while let Some(msg) = sub.next().await {
                    if task_tx.send(msg).is_err() {
                        break;
                    }
                }
            }));
        }
        // Drop the original sender so the channel closes when all tasks finish.
        drop(tx);

        let primary_subject = subjects.into_iter().next().unwrap_or_else(|| params.database.clone());

        Ok(Box::new(NatsConsumer {
            client,
            rx,
            _tasks: tasks,
            subject: primary_subject,
            consumer_name,
            sequence: 0,
        }))
    }
}

/// Connect to NATS server.
async fn nats_connect(params: &ConnectionParams) -> Result<async_nats::Client, DriverError> {
    let url = format!("nats://{}:{}", params.host, params.port);

    let connect_options = if !params.username.is_empty() {
        async_nats::ConnectOptions::with_user_and_password(
            params.username.clone(),
            params.password.clone(),
        )
    } else {
        async_nats::ConnectOptions::new()
    };

    connect_options
        .connect(&url)
        .await
        .map_err(|e| DriverError::Connection(format!("nats connect: {e}")))
}

/// Resolve subject from config subscriptions or connection params fallback.
///
/// Kept for backward-compat use in tests; the consumer now subscribes to
/// all subjects via `config.subscriptions` directly.
fn resolve_subject(config: &BrokerConsumerConfig, params: &ConnectionParams) -> String {
    config
        .subscriptions
        .first()
        .map(|s| s.topic.clone())
        .unwrap_or_else(|| params.database.clone())
}

// ── Producer ───────────────────────────────────────────────────────

/// NATS producer — publishes messages to subjects.
pub struct NatsProducer {
    client: async_nats::Client,
}

#[async_trait]
impl BrokerProducer for NatsProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        if message.destination.is_empty() {
            return Err(DriverError::Query(
                "nats publish requires a destination subject".into(),
            ));
        }

        // RW2.2.d: if a key is set, append it as a subject suffix (<base>/<key>).
        // This follows NATS subject hierarchy conventions where '/' is a valid
        // separator (though '.' is more common in NATS; we use '/' per spec).
        let subject = if let Some(ref key) = message.key {
            if key.is_empty() {
                message.destination.clone()
            } else {
                format!("{}/{}", message.destination, key)
            }
        } else {
            message.destination.clone()
        };

        let payload = bytes::Bytes::from(message.payload);

        // Use publish_with_reply_and_headers if we have headers or reply_to,
        // otherwise use simple publish.
        if !message.headers.is_empty() || message.reply_to.is_some() {
            let mut header_map = async_nats::HeaderMap::new();
            for (k, v) in &message.headers {
                header_map.insert(k.as_str(), v.as_str());
            }

            if let Some(ref reply) = message.reply_to {
                self.client
                    .publish_with_reply_and_headers(
                        subject.clone(),
                        reply.clone(),
                        header_map,
                        payload,
                    )
                    .await
                    .map_err(|e| DriverError::Query(format!("nats publish: {e}")))?;
            } else {
                self.client
                    .publish_with_headers(subject.clone(), header_map, payload)
                    .await
                    .map_err(|e| DriverError::Query(format!("nats publish: {e}")))?;
            }
        } else {
            self.client
                .publish(subject.clone(), payload)
                .await
                .map_err(|e| DriverError::Query(format!("nats publish: {e}")))?;
        }

        self.client
            .flush()
            .await
            .map_err(|e| DriverError::Query(format!("nats flush: {e}")))?;

        Ok(PublishReceipt {
            id: None,
            metadata: Some(format!("subject:{subject}")),
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        self.client
            .drain()
            .await
            .map_err(|e| DriverError::Internal(format!("nats drain: {e}")))?;
        Ok(())
    }
}

// ── Consumer ───────────────────────────────────────────────────────

/// NATS consumer — subscribes to one or more subjects via queue groups and receives messages.
///
/// Each subject has a dedicated tokio task forwarding messages into a shared mpsc channel.
/// `receive()` reads from that channel so all subjects are polled concurrently and no
/// subject starves another.
pub struct NatsConsumer {
    client: async_nats::Client,
    /// Aggregated receive channel — fed by one task per subscribed subject.
    rx: tokio::sync::mpsc::UnboundedReceiver<async_nats::Message>,
    /// Background tasks forwarding from each subscriber into `rx`. Aborted on close.
    _tasks: Vec<tokio::task::JoinHandle<()>>,
    /// The primary (first) subject — used for logging and metadata.
    subject: String,
    consumer_name: String,
    sequence: u64,
}

#[async_trait]
impl BrokerConsumer for NatsConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        let message = self
            .rx
            .recv()
            .await
            .ok_or_else(|| DriverError::Connection("nats subscriber streams ended".into()))?;

        self.sequence += 1;
        let seq = self.sequence;

        let headers: HashMap<String, String> = message
            .headers
            .as_ref()
            .map(|hm| {
                hm.iter()
                    .map(|(k, v)| {
                        (
                            k.to_string(),
                            v.iter().next().map(|s| s.to_string()).unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let destination = message.subject.to_string();
        Ok(InboundMessage {
            id: format!("nats:{}:{}", destination, seq),
            destination,
            payload: message.payload.to_vec(),
            headers,
            timestamp: Utc::now(),
            receipt: MessageReceipt {
                handle: seq.to_string(),
            },
            metadata: BrokerMetadata::Nats {
                sequence: seq,
                stream: self.subject.clone(),
                consumer: self.consumer_name.clone(),
            },
        })
    }

    /// Ack on core NATS is a best-effort no-op.
    ///
    /// Core NATS has no acknowledgement protocol — messages are delivered once
    /// and not tracked after delivery. The `AtMostOnce` semantics declared on
    /// `NatsDriver` communicate this to the BrokerConsumerBridge.
    async fn ack(&mut self, _receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        Ok(AckOutcome::Acked)
    }

    /// Nack is not supported on core NATS — returns `Err(BrokerError::Unsupported)`.
    ///
    /// Core NATS has no nack/requeue mechanism. Messages are delivered once to
    /// all queue-group members; there is no way to return a message to the broker
    /// for redelivery. JetStream provides redelivery and would change this.
    async fn nack(&mut self, _receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        Err(BrokerError::Unsupported)
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        for task in &self._tasks {
            task.abort();
        }
        self.client
            .drain()
            .await
            .map_err(|e| DriverError::Internal(format!("nats drain: {e}")))?;
        Ok(())
    }
}

// ── Schema Validation ────────────────────────────────────────────

/// NATS schema syntax validation (spec §15).
///
/// NATS schemas must be type "message", must include a "subject" extra field,
/// and do not support PUT or DELETE methods.
pub fn check_nats_schema(
    schema: &rivers_driver_sdk::SchemaDefinition,
    method: rivers_driver_sdk::HttpMethod,
) -> Result<(), rivers_driver_sdk::SchemaSyntaxError> {
    if schema.schema_type != "message" {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedType {
            schema_type: schema.schema_type.clone(),
            driver: "nats".into(),
            supported: vec!["message".into()],
            schema_file: String::new(),
        });
    }
    if !schema.extra.contains_key("subject") {
        return Err(rivers_driver_sdk::SchemaSyntaxError::MissingRequiredField {
            field: "subject".into(),
            driver: "nats".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::PUT {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedMethod {
            method: "PUT".into(),
            driver: "nats".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::DELETE {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedMethod {
            method: "DELETE".into(),
            driver: "nats".into(),
            schema_file: String::new(),
        });
    }
    Ok(())
}

// ── Plugin ABI ─────────────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_broker_driver(Arc::new(NatsDriver));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{BrokerSubscription, MessageBrokerDriver};
    use std::collections::HashMap;

    fn bad_params() -> ConnectionParams {
        ConnectionParams {
            host: "127.0.0.1".into(),
            port: 1,
            database: "test".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        }
    }

    fn test_config() -> BrokerConsumerConfig {
        BrokerConsumerConfig {
            group_prefix: "test".into(),
            app_id: "app1".into(),
            datasource_id: "ds1".into(),
            node_id: "node1".into(),
            reconnect_ms: 1000,
            subscriptions: vec![BrokerSubscription {
                topic: "test-topic".into(),
                event_name: Some("test.event".into()),
            }],
        }
    }

    fn empty_config() -> BrokerConsumerConfig {
        BrokerConsumerConfig {
            group_prefix: "test".into(),
            app_id: "app1".into(),
            datasource_id: "ds1".into(),
            node_id: "node1".into(),
            reconnect_ms: 1000,
            subscriptions: vec![],
        }
    }

    #[test]
    fn driver_name_is_nats() {
        let driver = NatsDriver;
        assert_eq!(driver.name(), "nats");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn resolve_subject_uses_subscription_first() {
        let config = test_config();
        let params = bad_params();
        assert_eq!(resolve_subject(&config, &params), "test-topic");
    }

    #[test]
    fn resolve_subject_falls_back_to_database() {
        let config = empty_config();
        let params = bad_params();
        assert_eq!(resolve_subject(&config, &params), "test");
    }

    #[tokio::test]
    async fn create_producer_bad_host_returns_connection_error() {
        let driver = NatsDriver;
        let params = bad_params();
        let config = test_config();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            driver.create_producer(&params, &config),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(msg))) => {
                assert!(msg.contains("nats"), "error should mention nats: {msg}");
            }
            Ok(Err(other)) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(Ok(_)) => panic!("expected connection error, but got Ok"),
            Err(_) => {
                // Timeout is acceptable — confirms port 1 doesn't have a NATS server.
            }
        }
    }

    #[tokio::test]
    async fn create_consumer_bad_host_returns_connection_error() {
        let driver = NatsDriver;
        let params = bad_params();
        let config = test_config();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            driver.create_consumer(&params, &config),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(msg))) => {
                assert!(msg.contains("nats"), "error should mention nats: {msg}");
            }
            Ok(Err(other)) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(Ok(_)) => panic!("expected connection error, but got Ok"),
            Err(_) => {
                // Timeout is acceptable — confirms port 1 doesn't have a NATS server.
            }
        }
    }

    // ── Schema validation tests ─────────────────────────────────────

    fn make_nats_schema(schema_type: &str, with_subject: bool) -> rivers_driver_sdk::SchemaDefinition {
        let mut extra = std::collections::HashMap::new();
        if with_subject {
            extra.insert("subject".into(), serde_json::json!("orders.>"));
        }
        rivers_driver_sdk::SchemaDefinition {
            driver: "nats".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields: vec![],
            extra,
        }
    }

    #[test]
    fn nats_schema_valid_message() {
        let schema = make_nats_schema("message", true);
        assert!(check_nats_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_ok());
    }

    #[test]
    fn nats_schema_valid_message_post() {
        let schema = make_nats_schema("message", true);
        assert!(check_nats_schema(&schema, rivers_driver_sdk::HttpMethod::POST).is_ok());
    }

    #[test]
    fn nats_rejects_non_message() {
        let schema = make_nats_schema("object", true);
        assert!(check_nats_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_err());
    }

    #[test]
    fn nats_requires_subject() {
        let schema = make_nats_schema("message", false);
        assert!(check_nats_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_err());
    }

    #[test]
    fn nats_rejects_put() {
        let schema = make_nats_schema("message", true);
        assert!(check_nats_schema(&schema, rivers_driver_sdk::HttpMethod::PUT).is_err());
    }

    #[test]
    fn nats_rejects_delete() {
        let schema = make_nats_schema("message", true);
        assert!(check_nats_schema(&schema, rivers_driver_sdk::HttpMethod::DELETE).is_err());
    }

    // ── RW2.2 contract tests ────────────────────────────────────────

    #[test]
    fn nats_driver_semantics_is_at_most_once() {
        let driver = NatsDriver;
        assert_eq!(driver.semantics(), rivers_driver_sdk::BrokerSemantics::AtMostOnce);
    }
}
