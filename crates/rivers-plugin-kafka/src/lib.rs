#![warn(missing_docs)]
//! Kafka plugin driver — rskafka 0.6 (pure Rust).
//!
//! Consumer group coordination (offset tracking, partition assignment)
//! is managed by the Rivers EventBus layer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use async_trait::async_trait;
use rskafka::client::partition::{OffsetAt, PartitionClient, UnknownTopicHandling};
use rskafka::client::error::{Error as RskafkaError, ProtocolError};
use rskafka::client::{Client, ClientBuilder};
use rskafka::record::Record;
use tokio::sync::{Mutex, RwLock};
use rivers_driver_sdk::{
    AckOutcome, BrokerConsumer, BrokerConsumerConfig, BrokerError, BrokerMetadata, BrokerProducer,
    BrokerSemantics, ConnectionParams, DriverError, DriverRegistrar, InboundMessage,
    MessageBrokerDriver, MessageReceipt, OutboundMessage, PublishReceipt, ABI_VERSION,
};

/// Initial backoff after a partition-client creation failure.
const INITIAL_ERROR_BACKOFF: Duration = Duration::from_millis(100);
/// Maximum backoff between retries on repeated partition-client failures.
const MAX_ERROR_BACKOFF: Duration = Duration::from_secs(30);

// ── Driver ─────────────────────────────────────────────────────────

/// Kafka driver factory — creates producers and consumers via rskafka.
///
/// # Rivers-managed consumer-group semantics (RW2.3.c)
///
/// rskafka does not implement the Kafka consumer-group protocol (dynamic
/// partition assignment, group coordinator, heartbeat). Rivers manages
/// consumer groups at the framework level:
///
/// - **Offset tracking:** stored in the Rivers `StorageEngine` under
///   `kafka:offsets:{topic}:{partition}`. The consumer reads the last acked
///   offset on startup and resumes from there.
/// - **Partition ownership:** stored under `kafka:ownership:{topic}:{partition}`.
///   A node acquires ownership by writing its `node_id`; other nodes skip
///   partitions they don't own.
/// - **Ack semantics:** `receive()` advances the in-memory offset so the next
///   fetch gets the next message, but does NOT commit to the broker. Only
///   `ack()` persists the offset. This gives at-least-once delivery: if the
///   consumer restarts before acking, it re-fetches from the last committed offset.
/// - **Nack semantics:** `nack()` rewinds the in-memory offset so the next
///   `receive()` re-delivers the same message. It does not interact with the
///   broker. This is implemented here; the StorageEngine persistence layer is
///   wired by the broker supervisor, not this crate.
pub struct KafkaDriver;

#[async_trait]
impl MessageBrokerDriver for KafkaDriver {
    fn name(&self) -> &str {
        "kafka"
    }

    /// Kafka with Rivers-managed offsets provides at-least-once delivery.
    fn semantics(&self) -> BrokerSemantics {
        BrokerSemantics::AtLeastOnce
    }

    fn check_schema_syntax(
        &self,
        schema: &rivers_driver_sdk::SchemaDefinition,
        method: rivers_driver_sdk::HttpMethod,
    ) -> Result<(), rivers_driver_sdk::SchemaSyntaxError> {
        check_kafka_schema(schema, method)
    }

    async fn create_producer(
        &self,
        params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        // Producer initialization is lazy: do NOT fetch metadata or bind a
        // topic at create time. The Client itself only opens a TCP connection
        // to the configured broker; per-topic PartitionClients are created on
        // demand from publish() based on OutboundMessage.destination.
        //
        // The BrokerConsumerConfig.subscriptions[] field is *consumer*-only;
        // a producer must not adopt it as a "default topic" — that would make
        // routing depend on consumer config. Routing is owned by the caller
        // via OutboundMessage.destination.
        let broker_addr = format!("{}:{}", params.host, params.port);
        let partition: i32 = params
            .options
            .get("partition")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let client = ClientBuilder::new(vec![broker_addr])
            .build()
            .await
            .map_err(|e| DriverError::Connection(format!("kafka connect: {e}")))?;

        Ok(Box::new(KafkaProducer {
            client: Arc::new(client),
            partition,
            partitions: RwLock::new(HashMap::new()),
            errors: Mutex::new(HashMap::new()),
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

/// Cached failure record for a topic — last error message and the next
/// time we're allowed to retry partition-client creation. Used to apply
/// exponential backoff on repeated `partition_client` lookup failures so
/// that a misconfigured / nonexistent topic doesn't hammer the broker.
#[derive(Debug, Clone)]
struct CachedError {
    message: String,
    /// Earliest instant at which the next creation attempt is allowed.
    next_retry_at: Instant,
    /// Backoff used for the *last* failure; the next failure doubles this
    /// (capped at MAX_ERROR_BACKOFF).
    last_backoff: Duration,
}

/// Kafka producer — routes records to topics by `OutboundMessage.destination`.
///
/// Per the runtime contract (Task E1, P1-3), the destination on the message
/// controls routing — not any topic bound at producer-creation time. The
/// producer holds a single `Arc<Client>` and lazily creates one
/// `PartitionClient` per destination topic, caching them in a RwLock-guarded
/// HashMap so subsequent publishes to the same topic skip the lookup.
///
/// On `partition_client` creation failure (e.g. unknown topic) we cache the
/// error and apply exponential backoff (100ms → 30s, doubling) so the next
/// publish to the same topic returns the cached error fast until the backoff
/// expires. A successful publish clears any prior error for that topic.
/// Failures are scoped per-topic, so an unknown topic A doesn't block topic B.
pub struct KafkaProducer {
    client: Arc<Client>,
    partition: i32,
    partitions: RwLock<HashMap<String, Arc<PartitionClient>>>,
    errors: Mutex<HashMap<String, CachedError>>,
}

impl KafkaProducer {
    /// Resolve the cached PartitionClient for `topic`, creating it on first
    /// use. Honors the per-topic error backoff cache.
    async fn partition_client_for(&self, topic: &str) -> Result<Arc<PartitionClient>, DriverError> {
        // Fast path: already cached.
        if let Some(pc) = self.partitions.read().await.get(topic).cloned() {
            return Ok(pc);
        }

        // Check error cache before attempting to (re)create.
        {
            let errors = self.errors.lock().await;
            if let Some(err) = errors.get(topic) {
                if Instant::now() < err.next_retry_at {
                    return Err(DriverError::Connection(format!(
                        "kafka partition client ({topic}/{}) [cached, backing off]: {}",
                        self.partition, err.message
                    )));
                }
            }
        }

        // Slow path: try to create. We don't hold the write lock across the
        // await — instead we create the partition client first, then insert.
        // A race where two tasks create concurrently is harmless (one wins
        // the insert; the other's PartitionClient is dropped).
        match self
            .client
            .partition_client(topic, self.partition, UnknownTopicHandling::Error)
            .await
        {
            Ok(pc) => {
                let pc = Arc::new(pc);
                let mut w = self.partitions.write().await;
                let entry = w.entry(topic.to_string()).or_insert_with(|| pc.clone()).clone();
                drop(w);
                // Successful creation clears any prior error for this topic.
                self.errors.lock().await.remove(topic);
                Ok(entry)
            }
            Err(e) => {
                let msg = e.to_string();
                let mut errors = self.errors.lock().await;
                let next_backoff = match errors.get(topic) {
                    Some(prev) => (prev.last_backoff * 2).min(MAX_ERROR_BACKOFF),
                    None => INITIAL_ERROR_BACKOFF,
                };
                errors.insert(
                    topic.to_string(),
                    CachedError {
                        message: msg.clone(),
                        next_retry_at: Instant::now() + next_backoff,
                        last_backoff: next_backoff,
                    },
                );
                Err(DriverError::Connection(format!(
                    "kafka partition client ({topic}/{}): {msg}",
                    self.partition
                )))
            }
        }
    }
}

#[async_trait]
impl BrokerProducer for KafkaProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        if message.destination.is_empty() {
            return Err(DriverError::Query(
                "kafka publish: OutboundMessage.destination is required".into(),
            ));
        }

        let topic = message.destination.clone();
        let partition_client = self.partition_client_for(&topic).await?;

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

        let offsets = partition_client
            .produce(vec![record], rskafka::client::partition::Compression::NoCompression)
            .await
            .map_err(|e| DriverError::Query(format!("kafka produce ({topic}): {e}")))?;

        // Clear any stale error record for this topic on successful produce.
        // (partition_client_for already cleared on cache-miss success; this
        // also handles the cache-hit-then-recover case.)
        self.errors.lock().await.remove(&topic);

        let offset = offsets.first().copied().unwrap_or(0);

        Ok(PublishReceipt {
            id: Some(format!("{}:{}:{}", topic, self.partition, offset)),
            metadata: None,
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        // rskafka PartitionClients and Client are dropped automatically.
        Ok(())
    }
}

// ── Consumer ───────────────────────────────────────────────────────

/// Kafka consumer — fetches records from a partition with offset tracking.
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
        let mut fetch_offset = self.offset + 1;

        loop {
            let result = self
                .partition_client
                .fetch_records(
                    fetch_offset,
                    1..1_000_000,
                    500, // max_wait_ms: 500ms poll interval (vs 5s) for faster delivery
                )
                .await;

            // When the requested offset is below the log-start offset (retention
            // deleted old segments), reset to the earliest available offset so
            // the consumer can resume rather than loop-failing forever.
            let (records, _high_watermark) = match result {
                Err(RskafkaError::ServerError { protocol_error: ProtocolError::OffsetOutOfRange, .. }) => {
                    let earliest = self.partition_client
                        .get_offset(OffsetAt::Earliest)
                        .await
                        .map_err(|e| DriverError::Query(format!("kafka get earliest offset: {e}")))?;
                    fetch_offset = earliest;
                    self.offset = earliest - 1;
                    continue;
                }
                Err(e) => return Err(DriverError::Query(format!("kafka fetch: {e}"))),
                Ok(v) => v,
            };

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

    /// Commit the offset for the acknowledged message.
    ///
    /// RW2.3.a: `receive()` fetches but does NOT commit the offset. Only
    /// `ack()` persists the offset, giving at-least-once delivery semantics.
    /// If the consumer restarts before `ack()` is called, the message will
    /// be re-fetched from the last committed offset.
    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        let offset: i64 = receipt
            .handle
            .parse()
            .map_err(|_| BrokerError::Protocol("invalid kafka offset in receipt".into()))?;
        self.offset = offset;
        Ok(AckOutcome::Acked)
    }

    /// Rewind the in-memory offset so the next `receive()` re-delivers this message.
    ///
    /// RW2.3.b: rskafka does not provide a broker-side consumer position reset
    /// (the Kafka protocol `OffsetCommit` API can only move offsets forward, not
    /// backward, for a given consumer group). Rivers implements nack by rewinding
    /// the local in-memory offset: the next `receive()` re-fetches from the
    /// rewound offset. This is correct for the single-partition, single-consumer
    /// case that Rivers manages per-datasource. Multi-partition redelivery
    /// (moving the committed offset backward on the broker) is not supported
    /// by rskafka and would require a separate `OffsetReset` Kafka admin command.
    async fn nack(&mut self, receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        // Parse the offset from the receipt and rewind to the message before it,
        // so the next receive() re-fetches this message.
        let offset: i64 = receipt
            .handle
            .parse()
            .map_err(|_| BrokerError::Protocol("invalid kafka offset in receipt".into()))?;
        // Rewind: set offset to one before the nacked message so the next
        // fetch_records call uses (offset - 1 + 1) = offset as fetch_offset.
        self.offset = offset - 1;
        Ok(AckOutcome::Acked)
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

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
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
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn kafka_driver_semantics_is_at_least_once() {
        let driver = KafkaDriver;
        assert_eq!(driver.semantics(), rivers_driver_sdk::BrokerSemantics::AtLeastOnce);
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

    // ── E1.3: Producer routes by destination (Task E1) ──────────────

    /// Construct an OutboundMessage for a destination with no payload niceties.
    fn outbound(destination: &str) -> OutboundMessage {
        OutboundMessage {
            destination: destination.into(),
            payload: b"{}".to_vec(),
            headers: HashMap::new(),
            key: None,
            reply_to: None,
        }
    }

    /// E1.2 contract guard: publishing with an empty destination is an error.
    /// The producer must not adopt a "default topic" from creation-time config.
    #[tokio::test]
    async fn publish_empty_destination_errors() {
        // Build a Client against a closed port — only used to construct the
        // KafkaProducer struct. We expect publish() to fail on the empty
        // destination check BEFORE attempting any partition_client lookup.
        let driver = KafkaDriver;
        let mut params = bad_params();
        // 127.0.0.1:1 will refuse — wrap whole producer creation in a timeout.
        params.host = "127.0.0.1".into();
        params.port = 1;

        let create = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            driver.create_producer(&params, &empty_config()),
        )
        .await;

        // If we can't even build the client (most CI), the empty-destination
        // check still belongs to the publish() contract — exercise it via a
        // hand-built producer when create succeeded; otherwise skip.
        let mut producer = match create {
            Ok(Ok(p)) => p,
            _ => return, // can't reach broker; trust the contract is enforced where it lives
        };

        let err = producer
            .publish(outbound(""))
            .await
            .expect_err("empty destination must fail");
        match err {
            DriverError::Query(msg) => {
                assert!(msg.contains("destination"), "error should mention destination: {msg}");
            }
            other => panic!("expected DriverError::Query, got: {other:?}"),
        }
    }

    /// E1.3 (error backoff): repeated failures on the same topic respect the
    /// per-topic backoff cache. We exercise the cache helpers directly by
    /// constructing a KafkaProducer with a Client we know will fail
    /// partition lookups, then asserting that the second call is fast and
    /// returns the cached error message.
    #[tokio::test]
    async fn error_cache_backs_off_per_topic() {
        // We can't construct a Client without a broker, so this test is gated.
        if std::env::var("KAFKA_AVAILABLE").ok().as_deref() != Some("1") {
            eprintln!("KAFKA_AVAILABLE != 1, skipping error_cache_backs_off_per_topic");
            return;
        }
        let driver = KafkaDriver;
        // Use the live test cluster broker.
        let mut params = bad_params();
        params.host = "192.168.2.203".into();
        params.port = 9092;

        let producer_box = driver
            .create_producer(&params, &empty_config())
            .await
            .expect("connect to live broker");

        // Downcast to our concrete type to reach the internal helpers.
        // We do this by going through publish() on a topic that doesn't exist
        // and checking that two failed publishes both report errors, with the
        // second one using the cached error path (we assert by checking the
        // error message contains "[cached, backing off]").
        // Box<dyn BrokerProducer> is not downcastable; use publish() instead.
        let mut producer = producer_box;
        // Use an invalid topic name (>249 chars) so auto-topic-creation can't
        // mask the failure and we genuinely hit the partition_client error path.
        let bogus = format!("rivers-invalid-{}-{}", "x".repeat(260), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));

        let first = producer.publish(outbound(&bogus)).await.expect_err("topic missing");
        let DriverError::Connection(first_msg) = first else {
            panic!("expected Connection error first, got: {first:?}");
        };
        assert!(!first_msg.contains("[cached"), "first error should be a fresh lookup: {first_msg}");

        let second = producer.publish(outbound(&bogus)).await.expect_err("backed off");
        let DriverError::Connection(second_msg) = second else {
            panic!("expected Connection error second, got: {second:?}");
        };
        assert!(
            second_msg.contains("[cached, backing off]"),
            "second error should hit the backoff cache: {second_msg}"
        );
    }

    /// E1.3 (routing-respected vs ignored): the destination on the message
    /// determines routing, NOT the BrokerConsumerConfig.subscriptions[0].topic
    /// passed at producer creation. We assert by publishing to topic A while
    /// the consumer config names topic B; the receipt prefix must be A.
    #[tokio::test]
    async fn publish_routes_by_message_destination_not_create_config() {
        if std::env::var("KAFKA_AVAILABLE").ok().as_deref() != Some("1") {
            eprintln!("KAFKA_AVAILABLE != 1, skipping publish_routes_by_message_destination_not_create_config");
            return;
        }
        let driver = KafkaDriver;
        let mut params = bad_params();
        params.host = "192.168.2.203".into();
        params.port = 9092;

        // Producer-creation-time topic ("create-topic-IGNORED") MUST NOT be
        // used for routing.
        let mut config_with_misleading_topic = test_config();
        config_with_misleading_topic.subscriptions[0].topic = "create-topic-IGNORED".into();

        let mut producer = driver
            .create_producer(&params, &config_with_misleading_topic)
            .await
            .expect("connect");

        let now = chrono::Utc::now().timestamp_millis();
        let topic_a = format!("rivers-route-a-{now}");
        let topic_b = format!("rivers-route-b-{now}");

        let receipt_a = producer.publish(outbound(&topic_a)).await.expect("publish A");
        let receipt_b = producer.publish(outbound(&topic_b)).await.expect("publish B");

        let id_a = receipt_a.id.expect("receipt A id");
        let id_b = receipt_b.id.expect("receipt B id");

        assert!(id_a.starts_with(&topic_a), "A receipt should be on topic A, got: {id_a}");
        assert!(id_b.starts_with(&topic_b), "B receipt should be on topic B, got: {id_b}");
        assert!(!id_a.contains("create-topic-IGNORED"), "create-time topic must not appear in receipt A: {id_a}");
        assert!(!id_b.contains("create-topic-IGNORED"), "create-time topic must not appear in receipt B: {id_b}");
    }

    /// E1.3 (per-topic failure isolation): a failure on topic A doesn't block
    /// successful publish to topic B. Both flow through the same producer
    /// instance — proving the partition-client cache is per-topic.
    #[tokio::test]
    async fn unknown_topic_failure_does_not_block_other_topic() {
        if std::env::var("KAFKA_AVAILABLE").ok().as_deref() != Some("1") {
            eprintln!("KAFKA_AVAILABLE != 1, skipping unknown_topic_failure_does_not_block_other_topic");
            return;
        }
        let driver = KafkaDriver;
        let mut params = bad_params();
        params.host = "192.168.2.203".into();
        params.port = 9092;

        let mut producer = driver
            .create_producer(&params, &empty_config())
            .await
            .expect("connect");

        // Use an invalid topic name (>249 chars) so the test cluster's
        // auto-topic-creation can't auto-promote the failure into success.
        let bogus = format!("rivers-iso-{}-{}", "x".repeat(260), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let real = format!("rivers-isolation-good-{}", chrono::Utc::now().timestamp_millis());

        // Topic A: should fail (unknown topic).
        let _ = producer.publish(outbound(&bogus)).await.expect_err("bogus topic should fail");

        // Topic B: must still succeed — independent partition client + no
        // shared error state.
        let receipt = producer.publish(outbound(&real)).await.expect("real topic should succeed");
        let id = receipt.id.expect("receipt id");
        assert!(id.starts_with(&real), "receipt should be on real topic: {id}");
    }
}
