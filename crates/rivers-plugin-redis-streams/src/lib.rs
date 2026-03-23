//! Redis Streams plugin driver (MessageBrokerDriver).
//!
//! Implements `MessageBrokerDriver` using Redis Streams commands:
//! - Producer: XADD
//! - Consumer: XREADGROUP + XACK (consumer groups)
//!
//! Per `rivers-driver-spec.md` section 6 and `rivers-data-layer-spec.md` section 3.
//!
//! Supports both single-node and cluster modes. Cluster mode is activated
//! via `options.cluster = "true"` with optional `options.hosts` for node list.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use redis::Value as RedisValue;
use tracing::{debug, warn};

use rivers_driver_sdk::{
    BrokerConsumer, BrokerConsumerConfig, BrokerMetadata, BrokerProducer, ConnectionParams,
    DriverError, DriverRegistrar, InboundMessage, MessageBrokerDriver, MessageReceipt,
    OutboundMessage, PublishReceipt, ABI_VERSION,
};

// ── Connection wrapper ───────────────────────────────────────────────

/// Internal connection wrapper — single-node or cluster.
///
/// Both `MultiplexedConnection` and `ClusterConnection` implement
/// `redis::aio::ConnectionLike`, so raw `redis::cmd()` calls work
/// identically on both variants.
enum RedisConn {
    Single(redis::aio::MultiplexedConnection),
    Cluster(redis::cluster_async::ClusterConnection),
}

impl RedisConn {
    /// Execute a raw Redis command, dispatching to the appropriate connection type.
    async fn query_async<T: redis::FromRedisValue>(
        &mut self,
        cmd: &mut redis::Cmd,
    ) -> Result<T, redis::RedisError> {
        match self {
            RedisConn::Single(conn) => cmd.query_async(conn).await,
            RedisConn::Cluster(conn) => cmd.query_async(conn).await,
        }
    }
}

// ── Driver ─────────────────────────────────────────────────────────────

pub struct RedisStreamsDriver;

#[async_trait]
impl MessageBrokerDriver for RedisStreamsDriver {
    fn name(&self) -> &str {
        "redis-streams"
    }

    async fn create_producer(
        &self,
        params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        let conn = connect_redis(params).await?;
        Ok(Box::new(RedisStreamProducer { conn }))
    }

    async fn create_consumer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        let mut conn = connect_redis(params).await?;
        let group_name = format!(
            "{}.{}.{}",
            config.group_prefix, config.app_id, config.datasource_id
        );
        let consumer_name = config.node_id.clone();

        // Resolve the stream name from subscriptions.
        let stream = config
            .subscriptions
            .first()
            .map(|s| s.topic.clone())
            .ok_or_else(|| {
                DriverError::Connection("no subscriptions configured for consumer".into())
            })?;

        // Determine start offset: use options.start_id if present, otherwise default
        // to "$" (only new messages after group creation — matches Kafka/RabbitMQ behavior).
        // Use "0" to replay the full backlog.
        let start_id = params
            .options
            .get("start_id")
            .cloned()
            .unwrap_or_else(|| "$".to_string());

        // Create consumer group, ignoring BUSYGROUP error (group already exists).
        let result: Result<(), redis::RedisError> = conn
            .query_async(
                redis::cmd("XGROUP")
                    .arg("CREATE")
                    .arg(&stream)
                    .arg(&group_name)
                    .arg(&start_id)
                    .arg("MKSTREAM"),
            )
            .await;

        if let Err(e) = result {
            let msg = e.to_string();
            if !msg.contains("BUSYGROUP") {
                warn!(
                    stream = %stream,
                    group = %group_name,
                    error = %e,
                    "failed to create consumer group (non-BUSYGROUP error)"
                );
                return Err(DriverError::Connection(format!(
                    "failed to create consumer group: {e}"
                )));
            }
            debug!(
                stream = %stream,
                group = %group_name,
                "consumer group already exists, continuing"
            );
        }

        Ok(Box::new(RedisStreamConsumer {
            conn,
            stream,
            group_name,
            consumer_name,
        }))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Build a Redis connection from ConnectionParams.
///
/// Uses cluster mode when `options.cluster = "true"`. In cluster mode,
/// `options.hosts` can list comma-separated host:port pairs; otherwise
/// the single `host:port` from params is used.
async fn connect_redis(params: &ConnectionParams) -> Result<RedisConn, DriverError> {
    let is_cluster = params
        .options
        .get("cluster")
        .map(|v| v == "true")
        .unwrap_or(false);

    if is_cluster {
        // Cluster mode: connect to multiple nodes
        let hosts: Vec<String> = if let Some(h) = params.options.get("hosts") {
            h.split(',').map(|s| s.trim().to_string()).collect()
        } else {
            vec![format!("{}:{}", params.host, params.port)]
        };

        let nodes: Vec<String> = hosts
            .iter()
            .map(|h| {
                if params.password.is_empty() {
                    format!("redis://{h}")
                } else {
                    format!("redis://:{}@{h}", params.password)
                }
            })
            .collect();

        let client = redis::cluster::ClusterClient::new(nodes)
            .map_err(|e| DriverError::Connection(format!("redis cluster client: {e}")))?;

        let conn = client
            .get_async_connection()
            .await
            .map_err(|e| DriverError::Connection(format!("redis cluster connect: {e}")))?;

        debug!(
            "redis-streams: connected to cluster ({} nodes)",
            hosts.len()
        );
        Ok(RedisConn::Cluster(conn))
    } else {
        // Single-node mode
        let url = if params.password.is_empty() {
            format!(
                "redis://{}:{}/{}",
                params.host, params.port, params.database
            )
        } else {
            format!(
                "redis://:{}@{}:{}/{}",
                params.password, params.host, params.port, params.database
            )
        };

        let client = redis::Client::open(url.as_str())
            .map_err(|e| DriverError::Connection(format!("redis client open failed: {e}")))?;

        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| DriverError::Connection(format!("redis connection failed: {e}")))?;

        debug!(
            "redis-streams: connected to {}:{}",
            params.host, params.port
        );
        Ok(RedisConn::Single(conn))
    }
}

/// Parse a Redis stream entry into an InboundMessage.
///
/// Stream entries from XREADGROUP come back as nested bulk arrays.
/// Structure: `[ [stream_name, [ [entry_id, [field, value, ...]], ... ]] ]`
fn parse_stream_entry(
    value: &RedisValue,
    stream: &str,
    group: &str,
    consumer: &str,
) -> Option<InboundMessage> {
    // Outer: array of streams
    let streams = match value {
        RedisValue::Array(s) => s,
        _ => return None,
    };

    // First stream result
    let stream_result = streams.first()?;
    let stream_data = match stream_result {
        RedisValue::Array(s) => s,
        _ => return None,
    };

    // stream_data[0] = stream name, stream_data[1] = entries array
    let entries = match stream_data.get(1)? {
        RedisValue::Array(e) => e,
        _ => return None,
    };

    // First entry
    let entry = match entries.first()? {
        RedisValue::Array(e) => e,
        _ => return None,
    };

    // entry[0] = ID, entry[1] = field-value pairs
    let entry_id = match entry.first()? {
        RedisValue::BulkString(b) => String::from_utf8_lossy(b).to_string(),
        RedisValue::SimpleString(s) => s.clone(),
        _ => return None,
    };

    let fields = match entry.get(1)? {
        RedisValue::Array(f) => f,
        _ => return None,
    };

    // Parse field-value pairs to find "payload"
    let mut payload_data: Vec<u8> = Vec::new();
    let mut headers = HashMap::new();
    let mut i = 0;
    while i + 1 < fields.len() {
        let key = redis_value_to_string(&fields[i]).unwrap_or_default();
        let val = redis_value_to_string(&fields[i + 1]).unwrap_or_default();

        if key == "payload" {
            // Decode base64 payload
            payload_data = BASE64.decode(&val).unwrap_or_else(|_| val.into_bytes());
        } else {
            headers.insert(key, val);
        }
        i += 2;
    }

    Some(InboundMessage {
        id: entry_id.clone(),
        destination: stream.to_string(),
        payload: payload_data,
        headers,
        timestamp: Utc::now(),
        receipt: MessageReceipt {
            handle: entry_id.clone(),
        },
        metadata: BrokerMetadata::Redis {
            stream_id: entry_id,
            group: group.to_string(),
            consumer: consumer.to_string(),
        },
    })
}

fn redis_value_to_string(v: &RedisValue) -> Option<String> {
    match v {
        RedisValue::BulkString(b) => Some(String::from_utf8_lossy(b).to_string()),
        RedisValue::SimpleString(s) => Some(s.clone()),
        RedisValue::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

// ── Producer ───────────────────────────────────────────────────────────

pub struct RedisStreamProducer {
    conn: RedisConn,
}

#[async_trait]
impl BrokerProducer for RedisStreamProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        let encoded = BASE64.encode(&message.payload);

        let entry_id: String = self
            .conn
            .query_async(
                redis::cmd("XADD")
                    .arg(&message.destination)
                    .arg("*")
                    .arg("payload")
                    .arg(&encoded),
            )
            .await
            .map_err(|e| DriverError::Query(format!("XADD failed: {e}")))?;

        debug!(
            stream = %message.destination,
            entry_id = %entry_id,
            "published message to redis stream"
        );

        Ok(PublishReceipt {
            id: Some(entry_id),
            metadata: None,
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        debug!("redis-streams producer closed");
        Ok(())
    }
}

// ── Consumer ───────────────────────────────────────────────────────────

pub struct RedisStreamConsumer {
    conn: RedisConn,
    stream: String,
    group_name: String,
    consumer_name: String,
}

#[async_trait]
impl BrokerConsumer for RedisStreamConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        loop {
            // XREADGROUP GROUP {group} {consumer} COUNT 1 BLOCK 5000 STREAMS {stream} >
            let result: RedisValue = self
                .conn
                .query_async(
                    redis::cmd("XREADGROUP")
                        .arg("GROUP")
                        .arg(&self.group_name)
                        .arg(&self.consumer_name)
                        .arg("COUNT")
                        .arg(1)
                        .arg("BLOCK")
                        .arg(5000)
                        .arg("STREAMS")
                        .arg(&self.stream)
                        .arg(">"),
                )
                .await
                .map_err(|e| DriverError::Query(format!("XREADGROUP failed: {e}")))?;

            // XREADGROUP returns Nil when the block timeout expires with no messages.
            if matches!(result, RedisValue::Nil) {
                continue;
            }

            if let Some(msg) = parse_stream_entry(
                &result,
                &self.stream,
                &self.group_name,
                &self.consumer_name,
            ) {
                return Ok(msg);
            }
            // No parseable entry, loop again.
        }
    }

    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        let _: i64 = self
            .conn
            .query_async(
                redis::cmd("XACK")
                    .arg(&self.stream)
                    .arg(&self.group_name)
                    .arg(&receipt.handle),
            )
            .await
            .map_err(|e| DriverError::Query(format!("XACK failed: {e}")))?;

        debug!(
            stream = %self.stream,
            entry_id = %receipt.handle,
            "acknowledged message"
        );
        Ok(())
    }

    async fn nack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        // For Redis Streams, nack means "don't ack" — the message stays in the
        // Pending Entries List (PEL) and will be redelivered on the next claim or
        // when the consumer restarts with ">" replaced by "0".
        debug!(
            stream = %self.stream,
            entry_id = %receipt.handle,
            "nack: message left in PEL for redelivery"
        );
        Ok(())
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        debug!(
            stream = %self.stream,
            group = %self.group_name,
            "redis-streams consumer closed"
        );
        Ok(())
    }
}

// ── Plugin ABI ─────────────────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_broker_driver(Arc::new(RedisStreamsDriver));
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
            database: "0".into(),
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

    #[test]
    fn driver_name_is_redis_streams() {
        let driver = RedisStreamsDriver;
        assert_eq!(driver.name(), "redis-streams");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn consumer_group_name_derivation() {
        let config = test_config();
        let group_name = format!(
            "{}.{}.{}",
            config.group_prefix, config.app_id, config.datasource_id
        );
        assert_eq!(group_name, "test.app1.ds1");
    }

    #[test]
    fn redis_value_to_string_bulk() {
        let val = RedisValue::BulkString(b"hello".to_vec());
        assert_eq!(redis_value_to_string(&val), Some("hello".to_string()));
    }

    #[test]
    fn redis_value_to_string_simple() {
        let val = RedisValue::SimpleString("world".into());
        assert_eq!(redis_value_to_string(&val), Some("world".to_string()));
    }

    #[test]
    fn redis_value_to_string_int() {
        let val = RedisValue::Int(42);
        assert_eq!(redis_value_to_string(&val), Some("42".to_string()));
    }

    #[test]
    fn redis_value_to_string_nil_returns_none() {
        let val = RedisValue::Nil;
        assert_eq!(redis_value_to_string(&val), None);
    }

    #[test]
    fn parse_stream_entry_returns_none_for_nil() {
        let val = RedisValue::Nil;
        assert!(parse_stream_entry(&val, "stream", "group", "consumer").is_none());
    }

    #[test]
    fn parse_stream_entry_parses_valid_structure() {
        // Build a realistic Redis XREADGROUP response:
        // [ [stream_name, [ [entry_id, [field, value, ...]] ]] ]
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let entry = RedisValue::Array(vec![
            RedisValue::BulkString(b"1234-0".to_vec()),
            RedisValue::Array(vec![
                RedisValue::BulkString(b"payload".to_vec()),
                RedisValue::BulkString(payload_b64.into_bytes()),
                RedisValue::BulkString(b"header1".to_vec()),
                RedisValue::BulkString(b"value1".to_vec()),
            ]),
        ]);
        let stream_result = RedisValue::Array(vec![
            RedisValue::BulkString(b"my-stream".to_vec()),
            RedisValue::Array(vec![entry]),
        ]);
        let outer = RedisValue::Array(vec![stream_result]);

        let msg = parse_stream_entry(&outer, "my-stream", "my-group", "my-consumer");
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.id, "1234-0");
        assert_eq!(msg.destination, "my-stream");
        assert_eq!(msg.payload, b"hello");
        assert_eq!(msg.headers.get("header1").unwrap(), "value1");
        assert_eq!(msg.receipt.handle, "1234-0");
        match &msg.metadata {
            BrokerMetadata::Redis {
                stream_id,
                group,
                consumer,
            } => {
                assert_eq!(stream_id, "1234-0");
                assert_eq!(group, "my-group");
                assert_eq!(consumer, "my-consumer");
            }
            other => panic!("expected BrokerMetadata::Redis, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_producer_bad_host_returns_connection_error() {
        let driver = RedisStreamsDriver;
        let params = bad_params();
        let config = test_config();
        let result = driver.create_producer(&params, &config).await;
        match result {
            Err(DriverError::Connection(msg)) => {
                assert!(msg.contains("redis"), "error should mention redis: {msg}");
            }
            Err(other) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(_) => panic!("expected connection error, but got Ok"),
        }
    }

    #[tokio::test]
    async fn create_consumer_bad_host_returns_connection_error() {
        let driver = RedisStreamsDriver;
        let params = bad_params();
        let config = test_config();
        let result = driver.create_consumer(&params, &config).await;
        match result {
            Err(DriverError::Connection(msg)) => {
                assert!(msg.contains("redis"), "error should mention redis: {msg}");
            }
            Err(other) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(_) => panic!("expected connection error, but got Ok"),
        }
    }
}
