#![warn(missing_docs)]
//! RabbitMQ plugin driver — lapin 2.x (pure Rust AMQP 0.9.1).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures_lite::StreamExt;
use lapin::{
    options::{
        BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions,
        QueueDeclareOptions,
    },
    types::FieldTable,
    BasicProperties, Channel, Connection as AmqpConnection, ConnectionProperties, Consumer,
};
use rivers_driver_sdk::{
    BrokerConsumer, BrokerConsumerConfig, BrokerMetadata, BrokerProducer, ConnectionParams,
    DriverError, DriverRegistrar, InboundMessage, MessageBrokerDriver, MessageReceipt,
    OutboundMessage, PublishReceipt, ABI_VERSION,
};

// ── Driver ─────────────────────────────────────────────────────────

/// RabbitMQ driver factory — creates producers and consumers via AMQP 0.9.1.
pub struct RabbitMqDriver;

#[async_trait]
impl MessageBrokerDriver for RabbitMqDriver {
    fn name(&self) -> &str {
        "rabbitmq"
    }

    async fn create_producer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        let connection = amqp_connect(params).await?;
        let channel = connection
            .create_channel()
            .await
            .map_err(|e| DriverError::Connection(format!("rabbitmq channel: {e}")))?;

        let queue = resolve_queue(config, params);

        // Ensure the queue exists (idempotent declare).
        channel
            .queue_declare(
                &queue,
                QueueDeclareOptions {
                    durable: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| DriverError::Connection(format!("rabbitmq queue declare: {e}")))?;

        // Enable publisher confirms so publish().await returns ack/nack.
        channel
            .confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(|e| DriverError::Connection(format!("rabbitmq confirm_select: {e}")))?;

        Ok(Box::new(RabbitProducer { _conn: connection, channel, queue }))
    }

    async fn create_consumer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        let connection = amqp_connect(params).await?;
        let channel = connection
            .create_channel()
            .await
            .map_err(|e| DriverError::Connection(format!("rabbitmq channel: {e}")))?;

        let queue = resolve_queue(config, params);

        // Ensure the queue exists (idempotent declare).
        channel
            .queue_declare(
                &queue,
                QueueDeclareOptions {
                    durable: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| DriverError::Connection(format!("rabbitmq queue declare: {e}")))?;

        let consumer_tag = format!(
            "{}.{}.{}.consumer",
            config.group_prefix, config.app_id, config.datasource_id
        );

        let consumer = channel
            .basic_consume(
                &queue,
                &consumer_tag,
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(|e| DriverError::Connection(format!("rabbitmq basic_consume: {e}")))?;

        Ok(Box::new(RabbitConsumer {
            _conn: connection,
            channel,
            consumer,
            queue,
        }))
    }
}

/// Build AMQP URL and connect.
async fn amqp_connect(params: &ConnectionParams) -> Result<AmqpConnection, DriverError> {
    let vhost = params
        .options
        .get("vhost")
        .map(|v| urlencoding_encode(v))
        .unwrap_or_else(|| "%2f".to_string());

    let url = if params.username.is_empty() {
        format!("amqp://{}:{}/{}", params.host, params.port, vhost)
    } else {
        format!(
            "amqp://{}:{}@{}:{}/{}",
            urlencoding_encode(&params.username),
            urlencoding_encode(&params.password),
            params.host,
            params.port,
            vhost,
        )
    };

    AmqpConnection::connect(&url, ConnectionProperties::default())
        .await
        .map_err(|e| DriverError::Connection(format!("rabbitmq connect: {e}")))
}

/// Minimal percent-encoding for AMQP URL components.
fn urlencoding_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// Resolve queue name from config subscriptions or connection params fallback.
fn resolve_queue(config: &BrokerConsumerConfig, params: &ConnectionParams) -> String {
    config
        .subscriptions
        .first()
        .map(|s| s.topic.clone())
        .unwrap_or_else(|| params.database.clone())
}

// ── Producer ───────────────────────────────────────────────────────

/// RabbitMQ producer — publishes messages with publisher confirms.
pub struct RabbitProducer {
    _conn: AmqpConnection,
    channel: Channel,
    queue: String,
}

#[async_trait]
impl BrokerProducer for RabbitProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        let routing_key = if message.destination.is_empty() {
            &self.queue
        } else {
            &message.destination
        };

        let mut properties = BasicProperties::default().with_delivery_mode(2); // persistent

        // Map headers into AMQP properties table.
        if !message.headers.is_empty() {
            let mut table = FieldTable::default();
            for (k, v) in &message.headers {
                table.insert(
                    k.clone().into(),
                    lapin::types::AMQPValue::LongString(v.clone().into()),
                );
            }
            properties = properties.with_headers(table);
        }

        let confirm = self
            .channel
            .basic_publish(
                "",
                routing_key,
                BasicPublishOptions::default(),
                &message.payload,
                properties,
            )
            .await
            .map_err(|e| DriverError::Query(format!("rabbitmq publish: {e}")))?
            .await
            .map_err(|e| DriverError::Query(format!("rabbitmq publish confirm: {e}")))?;

        if !confirm.is_ack() {
            return Err(DriverError::Query("rabbitmq publish nacked by broker".into()));
        }

        Ok(PublishReceipt {
            id: None,
            metadata: None,
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        self.channel
            .close(200, "normal shutdown")
            .await
            .map_err(|e| DriverError::Internal(format!("rabbitmq channel close: {e}")))?;
        Ok(())
    }
}

// ── Consumer ───────────────────────────────────────────────────────

/// RabbitMQ consumer — receives messages with manual ack/nack.
pub struct RabbitConsumer {
    _conn: AmqpConnection,
    channel: Channel,
    consumer: Consumer,
    queue: String,
}

#[async_trait]
impl BrokerConsumer for RabbitConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        let delivery = self
            .consumer
            .next()
            .await
            .ok_or_else(|| DriverError::Connection("rabbitmq consumer stream ended".into()))?
            .map_err(|e| DriverError::Query(format!("rabbitmq delivery error: {e}")))?;

        let delivery_tag = delivery.delivery_tag;
        let exchange = delivery.exchange.to_string();
        let routing_key = delivery.routing_key.to_string();

        let headers: HashMap<String, String> = delivery
            .properties
            .headers()
            .as_ref()
            .map(|table| {
                table
                    .inner()
                    .iter()
                    .filter_map(|(k, v)| {
                        let val = match v {
                            lapin::types::AMQPValue::LongString(s) => {
                                Some(s.to_string())
                            }
                            lapin::types::AMQPValue::ShortString(s) => {
                                Some(s.to_string())
                            }
                            _ => None,
                        };
                        val.map(|v| (k.to_string(), v))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(InboundMessage {
            id: format!("rmq:{}", delivery_tag),
            destination: self.queue.clone(),
            payload: delivery.data,
            headers,
            timestamp: Utc::now(),
            receipt: MessageReceipt {
                handle: delivery_tag.to_string(),
            },
            metadata: BrokerMetadata::Rabbit {
                delivery_tag,
                exchange,
                routing_key,
            },
        })
    }

    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        let tag: u64 = receipt
            .handle
            .parse()
            .map_err(|_| DriverError::Internal("invalid rabbitmq delivery tag in receipt".into()))?;
        self.channel
            .basic_ack(tag, BasicAckOptions::default())
            .await
            .map_err(|e| DriverError::Query(format!("rabbitmq ack: {e}")))?;
        Ok(())
    }

    async fn nack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        let tag: u64 = receipt
            .handle
            .parse()
            .map_err(|_| DriverError::Internal("invalid rabbitmq delivery tag in receipt".into()))?;
        self.channel
            .basic_nack(
                tag,
                BasicNackOptions {
                    requeue: true,
                    ..BasicNackOptions::default()
                },
            )
            .await
            .map_err(|e| DriverError::Query(format!("rabbitmq nack: {e}")))?;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        self.channel
            .close(200, "normal shutdown")
            .await
            .map_err(|e| DriverError::Internal(format!("rabbitmq channel close: {e}")))?;
        Ok(())
    }
}

// ── Schema Validation ────────────────────────────────────────────

/// RabbitMQ schema syntax validation (spec §14).
///
/// RabbitMQ schemas must be type "message". POST requires an "exchange" extra
/// field, GET requires a "queue" extra field. PUT and DELETE are not supported.
pub fn check_rabbitmq_schema(
    schema: &rivers_driver_sdk::SchemaDefinition,
    method: rivers_driver_sdk::HttpMethod,
) -> Result<(), rivers_driver_sdk::SchemaSyntaxError> {
    if schema.schema_type != "message" {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedType {
            schema_type: schema.schema_type.clone(),
            driver: "rabbitmq".into(),
            supported: vec!["message".into()],
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::POST && !schema.extra.contains_key("exchange") {
        return Err(rivers_driver_sdk::SchemaSyntaxError::MissingRequiredField {
            field: "exchange".into(),
            driver: "rabbitmq".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::GET && !schema.extra.contains_key("queue") {
        return Err(rivers_driver_sdk::SchemaSyntaxError::MissingRequiredField {
            field: "queue".into(),
            driver: "rabbitmq".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::PUT {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedMethod {
            method: "PUT".into(),
            driver: "rabbitmq".into(),
            schema_file: String::new(),
        });
    }
    if method == rivers_driver_sdk::HttpMethod::DELETE {
        return Err(rivers_driver_sdk::SchemaSyntaxError::UnsupportedMethod {
            method: "DELETE".into(),
            driver: "rabbitmq".into(),
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
    registrar.register_broker_driver(Arc::new(RabbitMqDriver));
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
    fn driver_name_is_rabbitmq() {
        let driver = RabbitMqDriver;
        assert_eq!(driver.name(), "rabbitmq");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn resolve_queue_uses_subscription_first() {
        let config = test_config();
        let params = bad_params();
        assert_eq!(resolve_queue(&config, &params), "test-topic");
    }

    #[test]
    fn resolve_queue_falls_back_to_database() {
        let config = empty_config();
        let params = bad_params();
        assert_eq!(resolve_queue(&config, &params), "test");
    }

    #[test]
    fn urlencoding_encodes_special_chars() {
        assert_eq!(urlencoding_encode("hello world"), "hello%20world");
        assert_eq!(urlencoding_encode("user@host"), "user%40host");
        assert_eq!(urlencoding_encode("p@ss:w0rd!"), "p%40ss%3Aw0rd%21");
        assert_eq!(urlencoding_encode("simple"), "simple");
        assert_eq!(urlencoding_encode("a/b"), "a%2Fb");
    }

    #[test]
    fn urlencoding_preserves_unreserved_chars() {
        // RFC 3986 unreserved: A-Z a-z 0-9 - _ . ~
        assert_eq!(urlencoding_encode("AZaz09-_.~"), "AZaz09-_.~");
    }

    #[tokio::test]
    async fn create_producer_bad_host_returns_connection_error() {
        let driver = RabbitMqDriver;
        let params = bad_params();
        let config = test_config();
        let result = driver.create_producer(&params, &config).await;
        match result {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("rabbitmq"),
                    "error should mention rabbitmq: {msg}"
                );
            }
            Err(other) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(_) => panic!("expected connection error, but got Ok"),
        }
    }

    #[tokio::test]
    async fn create_consumer_bad_host_returns_connection_error() {
        let driver = RabbitMqDriver;
        let params = bad_params();
        let config = test_config();
        let result = driver.create_consumer(&params, &config).await;
        match result {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("rabbitmq"),
                    "error should mention rabbitmq: {msg}"
                );
            }
            Err(other) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(_) => panic!("expected connection error, but got Ok"),
        }
    }

    // ── Schema validation tests ─────────────────────────────────────

    fn make_rmq_schema(
        schema_type: &str,
        exchange: bool,
        queue: bool,
    ) -> rivers_driver_sdk::SchemaDefinition {
        let mut extra = std::collections::HashMap::new();
        if exchange {
            extra.insert("exchange".into(), serde_json::json!("orders.exchange"));
        }
        if queue {
            extra.insert("queue".into(), serde_json::json!("orders.queue"));
        }
        rivers_driver_sdk::SchemaDefinition {
            driver: "rabbitmq".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields: vec![],
            extra,
        }
    }

    #[test]
    fn rabbitmq_schema_valid_message_post() {
        let schema = make_rmq_schema("message", true, false);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::POST).is_ok());
    }

    #[test]
    fn rabbitmq_schema_valid_message_get() {
        let schema = make_rmq_schema("message", false, true);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_ok());
    }

    #[test]
    fn rabbitmq_rejects_non_message() {
        let schema = make_rmq_schema("object", true, true);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_err());
    }

    #[test]
    fn rabbitmq_post_requires_exchange() {
        let schema = make_rmq_schema("message", false, true);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::POST).is_err());
    }

    #[test]
    fn rabbitmq_get_requires_queue() {
        let schema = make_rmq_schema("message", true, false);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::GET).is_err());
    }

    #[test]
    fn rabbitmq_rejects_put() {
        let schema = make_rmq_schema("message", true, true);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::PUT).is_err());
    }

    #[test]
    fn rabbitmq_rejects_delete() {
        let schema = make_rmq_schema("message", true, true);
        assert!(check_rabbitmq_schema(&schema, rivers_driver_sdk::HttpMethod::DELETE).is_err());
    }
}
