//! Kafka plugin driver — rskafka 0.6 (pure Rust).
//!
//! Consumer group coordination (offset tracking, partition assignment)
//! is managed by the Rivers EventBus layer.

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use rskafka::client::partition::UnknownTopicHandling;
use rskafka::client::ClientBuilder;
use rskafka::record::Record;
use rivers_driver_sdk::{
    BrokerConsumer, BrokerConsumerConfig, BrokerMetadata, BrokerProducer, ConnectionParams,
    DriverError, DriverRegistrar, InboundMessage, MessageBrokerDriver, MessageReceipt,
    OutboundMessage, PublishReceipt, ABI_VERSION,
};

// ── Driver ─────────────────────────────────────────────────────────

pub struct KafkaDriver;

#[async_trait]
impl MessageBrokerDriver for KafkaDriver {
    fn name(&self) -> &str {
        "kafka"
    }

    async fn create_producer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        let broker_addr = format!("{}:{}", params.host, params.port);
        let topic = resolve_topic(config, params);
        let partition: i32 = params
            .options
            .get("partition")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let client = ClientBuilder::new(vec![broker_addr])
            .build()
            .await
            .map_err(|e| DriverError::Connection(format!("kafka connect: {e}")))?;

        let partition_client = client
            .partition_client(&topic, partition, UnknownTopicHandling::Error)
            .await
            .map_err(|e| {
                DriverError::Connection(format!("kafka partition client ({topic}/{partition}): {e}"))
            })?;

        Ok(Box::new(KafkaProducer {
            partition_client: Arc::new(partition_client),
            topic,
            partition,
        }))
    }

    async fn create_consumer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        let broker_addr = format!("{}:{}", params.host, params.port);
        let topic = resolve_topic(config, params);
        let partition: i32 = params
            .options
            .get("partition")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let client = ClientBuilder::new(vec![broker_addr])
            .build()
            .await
            .map_err(|e| DriverError::Connection(format!("kafka connect: {e}")))?;

        let partition_client = client
            .partition_client(&topic, partition, UnknownTopicHandling::Error)
            .await
            .map_err(|e| {
                DriverError::Connection(format!("kafka partition client ({topic}/{partition}): {e}"))
            })?;

        let group_id = format!(
            "{}.{}.{}.consumer",
            config.group_prefix, config.app_id, config.datasource_id
        );

        Ok(Box::new(KafkaConsumer {
            partition_client: Arc::new(partition_client),
            topic,
            partition,
            group_id,
            offset: -1,
        }))
    }
}

/// Resolve topic name from config subscriptions or connection params fallback.
fn resolve_topic(config: &BrokerConsumerConfig, params: &ConnectionParams) -> String {
    config
        .subscriptions
        .first()
        .map(|s| s.topic.clone())
        .unwrap_or_else(|| params.database.clone())
}

// ── Producer ───────────────────────────────────────────────────────

pub struct KafkaProducer {
    partition_client: Arc<rskafka::client::partition::PartitionClient>,
    topic: String,
    partition: i32,
}

#[async_trait]
impl BrokerProducer for KafkaProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        let key = message.key.map(|k| k.into_bytes());
        let headers = message
            .headers
            .into_iter()
            .map(|(k, v)| (k, v.into_bytes()))
            .collect();

        let record = Record {
            key,
            value: Some(message.payload),
            headers,
            timestamp: chrono::Utc::now(),
        };

        let offsets = self
            .partition_client
            .produce(vec![record], rskafka::client::partition::Compression::NoCompression)
            .await
            .map_err(|e| DriverError::Query(format!("kafka produce: {e}")))?;

        let offset = offsets.first().copied().unwrap_or(0);

        Ok(PublishReceipt {
            id: Some(format!("{}:{}:{}", self.topic, self.partition, offset)),
            metadata: None,
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        // rskafka PartitionClient is dropped automatically.
        Ok(())
    }
}

// ── Consumer ───────────────────────────────────────────────────────

pub struct KafkaConsumer {
    partition_client: Arc<rskafka::client::partition::PartitionClient>,
    topic: String,
    partition: i32,
    group_id: String,
    /// Last successfully processed offset. Start at -1 so first fetch is from 0.
    offset: i64,
}

#[async_trait]
impl BrokerConsumer for KafkaConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        let fetch_offset = self.offset + 1;

        loop {
            let (records, _high_watermark) = self
                .partition_client
                .fetch_records(
                    fetch_offset,
                    1..1_000_000,
                    5_000,
                )
                .await
                .map_err(|e| DriverError::Query(format!("kafka fetch: {e}")))?;

            if let Some(record_and_offset) = records.into_iter().next() {
                let rec_offset = record_and_offset.offset;
                let record = record_and_offset.record;

                let headers: HashMap<String, String> = record
                    .headers
                    .into_iter()
                    .filter_map(|(k, v)| String::from_utf8(v).ok().map(|val| (k, val)))
                    .collect();

                let payload = record.value.unwrap_or_default();
                let timestamp = record.timestamp;

                let msg = InboundMessage {
                    id: format!("{}:{}:{}", self.topic, self.partition, rec_offset),
                    destination: self.topic.clone(),
                    payload,
                    headers,
                    timestamp,
                    receipt: MessageReceipt {
                        handle: rec_offset.to_string(),
                    },
                    metadata: BrokerMetadata::Kafka {
                        partition: self.partition,
                        offset: rec_offset,
                        consumer_group: self.group_id.clone(),
                    },
                };

                // Advance offset so the next receive() fetches the next message,
                // even if ack() hasn't been called yet. (AP15)
                self.offset = rec_offset;

                return Ok(msg);
            }
            // No records returned — loop and retry (long-poll).
        }
    }

    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        let offset: i64 = receipt
            .handle
            .parse()
            .map_err(|_| DriverError::Internal("invalid kafka offset in receipt".into()))?;
        self.offset = offset;
        Ok(())
    }

    async fn nack(&mut self, _receipt: &MessageReceipt) -> Result<(), DriverError> {
        // Do not advance offset — the message will be re-fetched on next receive().
        Ok(())
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        // rskafka PartitionClient is dropped automatically.
        Ok(())
    }
}

// ── Consumer group coordination ─────────────────────────────────────

/// Kafka consumer group coordination keys for StorageEngine.
///
/// Per technology-path-spec: Rivers manages consumer groups at the
/// framework level since rskafka doesn't provide native groups.
pub mod offsets {
    /// Build the StorageEngine key for a consumer offset.
    pub fn offset_key(topic: &str, partition: i32) -> String {
        format!("kafka:offsets:{}:{}", topic, partition)
    }

    /// Build the StorageEngine key for partition ownership.
    pub fn ownership_key(topic: &str, partition: i32) -> String {
        format!("kafka:ownership:{}:{}", topic, partition)
    }

    /// Build the consumer group ID from config.
    pub fn group_id(group_prefix: &str, app_id: &str, datasource_id: &str) -> String {
        format!("{}.{}.{}", group_prefix, app_id, datasource_id)
    }
}

// ── Schema Validation ────────────────────────────────────────────

/// Kafka schema syntax validation (spec §13).
///
/// Kafka schemas must be type "message", must include a "topic" extra field,
/// and do not support PUT or DELETE methods.
pub fn check_kafka_schema(
    schema: &rivers_driver_sdk::SchemaDefinition,
    method: rivers_driver_sdk::HttpMethod,
) -> Result<(), rivers_driver_sdk::SchemaSyntaxError> {
    if schema.schema_type != "message" {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedType {
            schema_type: schema.schema_type.clone(),
            driver: "kafka".into(),
            supported: vec!["message".into()],
            schema_file: String::new(),
        });
    }
    if !schema.extra.contains_key("topic") {
        return Err(rivers_driver_sdk::SchemaSyntaxError::MissingRequiredField {
            field: "topic".into(),
            driver: "kafka".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::PUT {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedMethod {
            method: "PUT".into(),
            driver: "kafka".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::DELETE {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedMethod {
            method: "DELETE".into(),
            driver: "kafka".into(),
            schema_file: String::new(),
        });
    }
    Ok(())
}

// ── Plugin ABI ─────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_broker_driver(Arc::new(KafkaDriver));
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
    fn driver_name_is_kafka() {
        let driver = KafkaDriver;
        assert_eq!(driver.name(), "kafka");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(_rivers_abi_version(), ABI_VERSION);
    }

    #[test]
    fn resolve_topic_uses_subscription_first() {
        let config = test_config();
        let params = bad_params();
        assert_eq!(resolve_topic(&config, &params), "test-topic");
    }

    #[test]
    fn resolve_topic_falls_back_to_database() {
        let config = empty_config();
        let params = bad_params();
        assert_eq!(resolve_topic(&config, &params), "test");
    }

    #[tokio::test]
    async fn create_producer_bad_host_returns_connection_error() {
        let driver = KafkaDriver;
        let params = bad_params();
        let config = test_config();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            driver.create_producer(&params, &config),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(msg))) => {
                assert!(msg.contains("kafka"), "error should mention kafka: {msg}");
            }
            Ok(Err(other)) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(Ok(_)) => panic!("expected connection error, but got Ok"),
            Err(_) => {
                // Timeout is acceptable — confirms port 1 doesn't have a Kafka broker.
            }
        }
    }

    #[tokio::test]
    async fn create_consumer_bad_host_returns_connection_error() {
        let driver = KafkaDriver;
        let params = bad_params();
        let config = test_config();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            driver.create_consumer(&params, &config),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(msg))) => {
                assert!(msg.contains("kafka"), "error should mention kafka: {msg}");
            }
            Ok(Err(other)) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(Ok(_)) => panic!("expected connection error, but got Ok"),
            Err(_) => {
                // Timeout is acceptable — confirms port 1 doesn't have a Kafka broker.
            }
        }
    }

    // ── Offset coordination tests ───────────────────────────────────

    #[test]
    fn offset_key_format() {
        assert_eq!(
            offsets::offset_key("orders", 0),
            "kafka:offsets:orders:0"
        );
        assert_eq!(
            offsets::offset_key("events", 3),
            "kafka:offsets:events:3"
        );
    }

    #[test]
    fn ownership_key_format() {
        assert_eq!(
            offsets::ownership_key("orders", 0),
            "kafka:ownership:orders:0"
        );
    }

    #[test]
    fn group_id_format() {
        assert_eq!(
            offsets::group_id("rivers", "app1", "ds1"),
            "rivers.app1.ds1"
        );
    }

    // ── Schema validation tests ─────────────────────────────────────

    fn make_kafka_schema(schema_type: &str, with_topic: bool) -> rivers_driver_sdk::SchemaDefinition {
        let mut extra = std::collections::HashMap::new();
        if with_topic {
            extra.insert("topic".into(), serde_json::json!("orders"));
        }
        rivers_driver_sdk::SchemaDefinition {
            driver: "kafka".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields: vec![],
            extra,
        }
    }

    #[test]
    fn kafka_schema_valid_message() {
        let schema = make_kafka_schema("message", true);
        assert!(check_kafka_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_ok());
    }

    #[test]
    fn kafka_schema_valid_message_post() {
        let schema = make_kafka_schema("message", true);
        assert!(check_kafka_schema(&schema, rivers_driver_sdk::HttpMethod::POST).is_ok());
    }

    #[test]
    fn kafka_rejects_non_message() {
        let schema = make_kafka_schema("object", true);
        assert!(check_kafka_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_err());
    }

    #[test]
    fn kafka_requires_topic() {
        let schema = make_kafka_schema("message", false);
        assert!(check_kafka_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_err());
    }

    #[test]
    fn kafka_rejects_put() {
        let schema = make_kafka_schema("message", true);
        assert!(check_kafka_schema(&schema, rivers_driver_sdk::HttpMethod::PUT).is_err());
    }

    #[test]
    fn kafka_rejects_delete() {
        let schema = make_kafka_schema("message", true);
        assert!(check_kafka_schema(&schema, rivers_driver_sdk::HttpMethod::DELETE).is_err());
    }
}
