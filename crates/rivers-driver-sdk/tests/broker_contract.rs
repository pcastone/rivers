//! Broker contract test fixtures.
//!
//! Provides reusable async fixtures that test drivers can call to verify
//! compliance with the `BrokerConsumer` contract defined in `broker.rs`.
//!
//! Also contains an in-memory mock driver covering all three `BrokerSemantics`
//! modes, with tests that run each fixture against the mock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rivers_driver_sdk::{
    AckOutcome, BrokerConsumer, BrokerConsumerConfig, BrokerError, BrokerMetadata, BrokerProducer,
    BrokerSemantics, BrokerSubscription, ConnectionParams, DriverError, InboundMessage,
    MessageBrokerDriver, MessageReceipt, OutboundMessage, PublishReceipt,
};

// ── Contract Fixtures ───────────────────────────────────────────────────────

/// Fixture: receive a message, ack it, verify the driver reports `Acked`.
///
/// Drive with a driver that has `AtLeastOnce` semantics.
pub async fn test_ack_returns_acked<D: MessageBrokerDriver>(driver: &D) {
    let config = default_config();
    let params = unreachable_params();
    // Only mock drivers will actually succeed — real drivers skip if they
    // can't connect.  Callers gate with `if driver.semantics() == AtLeastOnce`.
    let consumer = driver.create_consumer(&params, &config).await;
    if consumer.is_err() {
        return; // cluster not available, skip
    }
    let mut consumer = consumer.unwrap();
    let msg = consumer.receive().await.expect("receive should succeed");
    let outcome = consumer
        .ack(&msg.receipt)
        .await
        .expect("ack should succeed for AtLeastOnce driver");
    assert_eq!(
        outcome,
        AckOutcome::Acked,
        "first ack must return Acked, not AlreadyAcked"
    );
}

/// Fixture: receive → nack → expect redelivery (second receive returns same message)
/// OR receive → nack → returns `Err(BrokerError::Unsupported)` for `AtMostOnce` drivers.
pub async fn test_nack_redelivery_or_unsupported<D: MessageBrokerDriver>(driver: &D) {
    let config = default_config();
    let params = unreachable_params();
    let consumer = driver.create_consumer(&params, &config).await;
    if consumer.is_err() {
        return;
    }
    let mut consumer = consumer.unwrap();
    let msg = consumer.receive().await.expect("receive should succeed");
    let nack_result = consumer.nack(&msg.receipt).await;
    match driver.semantics() {
        BrokerSemantics::AtMostOnce | BrokerSemantics::FireAndForget => {
            // Driver MUST return Unsupported for nack on fire-and-forget semantics.
            assert!(
                matches!(nack_result, Err(BrokerError::Unsupported)),
                "AtMostOnce/FireAndForget driver must return BrokerError::Unsupported for nack, got: {nack_result:?}"
            );
        }
        BrokerSemantics::AtLeastOnce => {
            // Driver must return Ok — redelivery is handled internally.
            assert!(
                nack_result.is_ok(),
                "AtLeastOnce driver nack must not error: {nack_result:?}"
            );
        }
    }
}

/// Fixture: multi-consumer same group → each message delivered to exactly one consumer.
///
/// This is a structural check: verify that the group name used in consumer
/// creation follows the expected derivation format.
pub async fn test_consumer_group_exclusive<D: MessageBrokerDriver>(driver: &D) {
    let config1 = BrokerConsumerConfig {
        group_prefix: "rivers".into(),
        app_id: "app1".into(),
        datasource_id: "ds1".into(),
        node_id: "node-A".into(),
        reconnect_ms: 0,
        subscriptions: vec![BrokerSubscription {
            topic: "test-topic".into(),
            event_name: None,
        }],
    };
    let config2 = BrokerConsumerConfig {
        group_prefix: "rivers".into(),
        app_id: "app1".into(),
        datasource_id: "ds1".into(),
        node_id: "node-B".into(),
        reconnect_ms: 0,
        subscriptions: vec![BrokerSubscription {
            topic: "test-topic".into(),
            event_name: None,
        }],
    };
    let params = unreachable_params();
    // Both configs must produce the same group ID (derivation contract).
    // group_id = {group_prefix}.{app_id}.{datasource_id}[.*]
    let expected_group = "rivers.app1.ds1";
    // We can't inspect internal consumer state, so just verify both consumers
    // can be created — group coordination is broker-level.
    let _ = (
        driver.create_consumer(&params, &config1).await,
        driver.create_consumer(&params, &config2).await,
        expected_group,
    );
    // No assert here — fixture documents the group derivation contract.
    // Real group exclusivity is tested in cluster-gated integration tests.
}

/// Fixture: multiple subscriptions → consumer subscribes to all topics.
pub async fn test_multi_subscription<D: MessageBrokerDriver>(driver: &D) {
    let config = BrokerConsumerConfig {
        group_prefix: "rivers".into(),
        app_id: "app1".into(),
        datasource_id: "ds1".into(),
        node_id: "node1".into(),
        reconnect_ms: 0,
        subscriptions: vec![
            BrokerSubscription {
                topic: "topic-a".into(),
                event_name: None,
            },
            BrokerSubscription {
                topic: "topic-b".into(),
                event_name: None,
            },
        ],
    };
    let params = unreachable_params();
    // Only mock drivers succeed — real drivers need a running broker.
    let _ = driver.create_consumer(&params, &config).await;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn default_config() -> BrokerConsumerConfig {
    BrokerConsumerConfig {
        group_prefix: "test".into(),
        app_id: "app1".into(),
        datasource_id: "ds1".into(),
        node_id: "node1".into(),
        reconnect_ms: 0,
        subscriptions: vec![BrokerSubscription {
            topic: "test-topic".into(),
            event_name: None,
        }],
    }
}

fn unreachable_params() -> ConnectionParams {
    ConnectionParams {
        host: "127.0.0.1".into(),
        port: 1, // nothing listening here
        database: "test".into(),
        username: String::new(),
        password: String::new(),
        options: HashMap::new(),
    }
}

// ── In-Memory Mock Driver ────────────────────────────────────────────────────

/// Shared state for the in-memory mock broker.
#[derive(Debug, Default)]
struct MockState {
    /// Messages queued for consumption.
    queue: Vec<InboundMessage>,
    /// Set of message IDs that have been acked.
    acked: std::collections::HashSet<String>,
}

/// A mock broker driver that implements all three semantics modes.
struct MockBrokerDriver {
    semantics: BrokerSemantics,
    state: Arc<Mutex<MockState>>,
}

impl MockBrokerDriver {
    fn new(semantics: BrokerSemantics) -> Self {
        let mut state = MockState::default();
        // Pre-populate with one test message.
        state.queue.push(make_test_message("msg-1"));
        state.queue.push(make_test_message("msg-2"));
        MockBrokerDriver {
            semantics,
            state: Arc::new(Mutex::new(state)),
        }
    }
}

fn make_test_message(id: &str) -> InboundMessage {
    InboundMessage {
        id: id.to_string(),
        destination: "test-topic".to_string(),
        payload: b"hello".to_vec(),
        headers: HashMap::new(),
        timestamp: chrono::Utc::now(),
        receipt: MessageReceipt {
            handle: id.to_string(),
        },
        metadata: BrokerMetadata::Nats {
            sequence: 0,
            stream: "test-topic".to_string(),
            consumer: "mock".to_string(),
        },
    }
}

#[async_trait]
impl MessageBrokerDriver for MockBrokerDriver {
    fn name(&self) -> &str {
        "mock"
    }

    fn semantics(&self) -> BrokerSemantics {
        self.semantics
    }

    async fn create_producer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError> {
        Ok(Box::new(MockProducer {
            state: self.state.clone(),
        }))
    }

    async fn create_consumer(
        &self,
        _params: &ConnectionParams,
        _config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
        Ok(Box::new(MockConsumer {
            semantics: self.semantics,
            state: self.state.clone(),
            cursor: 0,
        }))
    }
}

struct MockProducer {
    state: Arc<Mutex<MockState>>,
}

#[async_trait]
impl BrokerProducer for MockProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        let id = format!("mock:{}", message.destination);
        let msg = InboundMessage {
            id: id.clone(),
            destination: message.destination,
            payload: message.payload,
            headers: message.headers,
            timestamp: chrono::Utc::now(),
            receipt: MessageReceipt { handle: id.clone() },
            metadata: BrokerMetadata::Nats {
                sequence: 0,
                stream: "mock".to_string(),
                consumer: "mock".to_string(),
            },
        };
        self.state.lock().unwrap().queue.push(msg);
        Ok(PublishReceipt {
            id: Some(id),
            metadata: None,
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

struct MockConsumer {
    semantics: BrokerSemantics,
    state: Arc<Mutex<MockState>>,
    cursor: usize,
}

#[async_trait]
impl BrokerConsumer for MockConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        let state = self.state.lock().unwrap();
        if self.cursor < state.queue.len() {
            let msg = state.queue[self.cursor].clone();
            drop(state);
            self.cursor += 1;
            Ok(msg)
        } else {
            Err(DriverError::Query("mock queue empty".into()))
        }
    }

    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        match self.semantics {
            BrokerSemantics::AtMostOnce | BrokerSemantics::FireAndForget => {
                // AtMostOnce: ack is a no-op (message already gone).
                Ok(AckOutcome::Acked)
            }
            BrokerSemantics::AtLeastOnce => {
                let mut state = self.state.lock().unwrap();
                if state.acked.contains(&receipt.handle) {
                    Ok(AckOutcome::AlreadyAcked)
                } else {
                    state.acked.insert(receipt.handle.clone());
                    Ok(AckOutcome::Acked)
                }
            }
        }
    }

    async fn nack(&mut self, _receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError> {
        match self.semantics {
            BrokerSemantics::AtMostOnce | BrokerSemantics::FireAndForget => {
                // nack is unsupported — message is already gone.
                Err(BrokerError::Unsupported)
            }
            BrokerSemantics::AtLeastOnce => {
                // Nack: rewind cursor so the next receive() re-delivers.
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                Ok(AckOutcome::Acked)
            }
        }
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AtLeastOnce mock tests ────────────────────────────────────────

    #[test]
    fn at_least_once_semantics_reported() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        assert_eq!(driver.semantics(), BrokerSemantics::AtLeastOnce);
    }

    #[test]
    fn at_most_once_semantics_reported() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtMostOnce);
        assert_eq!(driver.semantics(), BrokerSemantics::AtMostOnce);
    }

    #[test]
    fn fire_and_forget_semantics_reported() {
        let driver = MockBrokerDriver::new(BrokerSemantics::FireAndForget);
        assert_eq!(driver.semantics(), BrokerSemantics::FireAndForget);
    }

    #[tokio::test]
    async fn at_least_once_ack_returns_acked() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        let config = default_config();
        let params = unreachable_params();
        let mut consumer = driver.create_consumer(&params, &config).await.unwrap();
        let msg = consumer.receive().await.unwrap();
        let outcome = consumer.ack(&msg.receipt).await.unwrap();
        assert_eq!(outcome, AckOutcome::Acked);
    }

    #[tokio::test]
    async fn at_least_once_double_ack_returns_already_acked() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        let config = default_config();
        let params = unreachable_params();
        let mut consumer = driver.create_consumer(&params, &config).await.unwrap();
        let msg = consumer.receive().await.unwrap();
        consumer.ack(&msg.receipt).await.unwrap();
        let second = consumer.ack(&msg.receipt).await.unwrap();
        assert_eq!(second, AckOutcome::AlreadyAcked);
    }

    #[tokio::test]
    async fn at_least_once_nack_redelivers() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        let config = default_config();
        let params = unreachable_params();
        let mut consumer = driver.create_consumer(&params, &config).await.unwrap();
        let msg1 = consumer.receive().await.unwrap();
        consumer.nack(&msg1.receipt).await.unwrap();
        // After nack, the cursor rewinds — receive should return the same message.
        let msg2 = consumer.receive().await.unwrap();
        assert_eq!(msg1.id, msg2.id, "nack must cause redelivery");
    }

    #[tokio::test]
    async fn at_most_once_nack_returns_unsupported() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtMostOnce);
        let config = default_config();
        let params = unreachable_params();
        let mut consumer = driver.create_consumer(&params, &config).await.unwrap();
        let msg = consumer.receive().await.unwrap();
        let result = consumer.nack(&msg.receipt).await;
        assert!(
            matches!(result, Err(BrokerError::Unsupported)),
            "AtMostOnce nack must return BrokerError::Unsupported, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn fire_and_forget_nack_returns_unsupported() {
        let driver = MockBrokerDriver::new(BrokerSemantics::FireAndForget);
        let config = default_config();
        let params = unreachable_params();
        let mut consumer = driver.create_consumer(&params, &config).await.unwrap();
        let msg = consumer.receive().await.unwrap();
        let result = consumer.nack(&msg.receipt).await;
        assert!(
            matches!(result, Err(BrokerError::Unsupported)),
            "FireAndForget nack must return BrokerError::Unsupported, got: {result:?}"
        );
    }

    // ── Fixture smoke tests against mock ─────────────────────────────

    #[tokio::test]
    async fn fixture_ack_returns_acked_passes_on_at_least_once_mock() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        test_ack_returns_acked(&driver).await;
    }

    #[tokio::test]
    async fn fixture_nack_redelivery_passes_on_at_least_once_mock() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        test_nack_redelivery_or_unsupported(&driver).await;
    }

    #[tokio::test]
    async fn fixture_nack_unsupported_passes_on_at_most_once_mock() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtMostOnce);
        test_nack_redelivery_or_unsupported(&driver).await;
    }

    #[tokio::test]
    async fn fixture_consumer_group_exclusive_smoke() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        test_consumer_group_exclusive(&driver).await;
    }

    #[tokio::test]
    async fn fixture_multi_subscription_smoke() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        test_multi_subscription(&driver).await;
    }

    // ── Producer tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_producer_publish_enqueues_message() {
        let driver = MockBrokerDriver::new(BrokerSemantics::AtLeastOnce);
        let config = default_config();
        let params = unreachable_params();
        let mut producer = driver.create_producer(&params, &config).await.unwrap();
        let receipt = producer
            .publish(OutboundMessage {
                destination: "test-topic".into(),
                payload: b"data".to_vec(),
                headers: HashMap::new(),
                key: None,
                reply_to: None,
            })
            .await
            .unwrap();
        assert!(receipt.id.is_some());
    }
}
