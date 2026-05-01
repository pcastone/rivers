//! Broker contract test fixtures.
//!
//! The canonical fixture functions live in `rivers_driver_sdk::broker_contract_fixtures`
//! (behind the `test-fixtures` feature). This file re-exports them and provides
//! an in-memory mock driver covering all three `BrokerSemantics` modes with
//! tests that run each fixture against the mock.

use std::collections::HashMap;

use async_trait::async_trait;
use rivers_driver_sdk::{
    AckOutcome, BrokerConsumer, BrokerConsumerConfig, BrokerError, BrokerMetadata, BrokerProducer,
    BrokerSemantics, BrokerSubscription, ConnectionParams, DriverError, InboundMessage,
    MessageBrokerDriver, MessageReceipt, OutboundMessage, PublishReceipt,
};
use rivers_driver_sdk::broker_contract_fixtures::{
    test_ack_returns_acked, test_nack_redelivery_or_unsupported,
    test_consumer_group_exclusive, test_multi_subscription, unreachable_params,
};

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
    state: std::sync::Arc<std::sync::Mutex<MockState>>,
}

impl MockBrokerDriver {
    fn new(semantics: BrokerSemantics) -> Self {
        let mut state = MockState::default();
        // Pre-populate with one test message.
        state.queue.push(make_test_message("msg-1"));
        state.queue.push(make_test_message("msg-2"));
        MockBrokerDriver {
            semantics,
            state: std::sync::Arc::new(std::sync::Mutex::new(state)),
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
    state: std::sync::Arc<std::sync::Mutex<MockState>>,
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
    state: std::sync::Arc<std::sync::Mutex<MockState>>,
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
                Err(BrokerError::Unsupported)
            }
            BrokerSemantics::AtLeastOnce => {
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

    // ── Fixture smoke tests against mock (sourced from broker_contract_fixtures) ──

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
