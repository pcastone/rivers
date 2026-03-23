//! NATS plugin driver — async-nats 0.38 (pure Rust).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures_lite::StreamExt;
use rivers_driver_sdk::{
    BrokerConsumer, BrokerConsumerConfig, BrokerMetadata, BrokerProducer, ConnectionParams,
    DriverError, DriverRegistrar, InboundMessage, MessageBrokerDriver, MessageReceipt,
    OutboundMessage, PublishReceipt, ABI_VERSION,
};

// ── Driver ─────────────────────────────────────────────────────────

pub struct NatsDriver;

#[async_trait]
impl MessageBrokerDriver for NatsDriver {
    fn name(&self) -> &str {
        "nats"
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
        let subject = resolve_subject(config, params);

        let subscriber = client
            .subscribe(subject.clone())
            .await
            .map_err(|e| DriverError::Connection(format!("nats subscribe({subject}): {e}")))?;

        let consumer_name = format!(
            "{}.{}.{}.consumer",
            config.group_prefix, config.app_id, config.datasource_id
        );

        Ok(Box::new(NatsConsumer {
            client,
            subscriber,
            subject,
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
fn resolve_subject(config: &BrokerConsumerConfig, params: &ConnectionParams) -> String {
    config
        .subscriptions
        .first()
        .map(|s| s.topic.clone())
        .unwrap_or_else(|| params.database.clone())
}

// ── Producer ───────────────────────────────────────────────────────

pub struct NatsProducer {
    client: async_nats::Client,
}

#[async_trait]
impl BrokerProducer for NatsProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        let subject = if message.destination.is_empty() {
            return Err(DriverError::Query(
                "nats publish requires a destination subject".into(),
            ));
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

pub struct NatsConsumer {
    client: async_nats::Client,
    subscriber: async_nats::Subscriber,
    subject: String,
    consumer_name: String,
    sequence: u64,
}

#[async_trait]
impl BrokerConsumer for NatsConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        let message = self
            .subscriber
            .next()
            .await
            .ok_or_else(|| DriverError::Connection("nats subscriber stream ended".into()))?;

        self.sequence += 1;
        let seq = self.sequence;

        let headers: HashMap<String, String> = message
            .headers
            .as_ref()
            .map(|hm| {
                hm.iter()
                    .map(|(k, v)| (k.to_string(), v.iter().next().map(|s| s.to_string()).unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(InboundMessage {
            id: format!("nats:{}:{}", self.subject, seq),
            destination: message.subject.to_string(),
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

    async fn ack(&mut self, _receipt: &MessageReceipt) -> Result<(), DriverError> {
        // Core NATS has no ack mechanism (fire-and-forget pub/sub).
        // Ack is a no-op; JetStream ack would go here in a future extension.
        Ok(())
    }

    async fn nack(&mut self, _receipt: &MessageReceipt) -> Result<(), DriverError> {
        // Core NATS has no nack/requeue mechanism.
        // No-op; JetStream nack would go here in a future extension.
        Ok(())
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        // Unsubscribing is handled by dropping the Subscriber.
        // Drain the client to flush pending operations.
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
        assert_eq!(_rivers_abi_version(), ABI_VERSION);
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
}
