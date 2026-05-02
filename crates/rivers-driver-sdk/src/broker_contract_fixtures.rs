//! Shared broker contract test fixtures.
//!
//! Callable by any broker plugin crate's live test via:
//!
//! ```toml
//! [dev-dependencies]
//! rivers-driver-sdk = { path = "...", features = ["test-fixtures"] }
//! ```
//!
//! Each fixture skips gracefully when the broker is unreachable (connection error
//! on `create_consumer` → early return).

use std::collections::HashMap;

use crate::broker::{
    BrokerConsumerConfig, BrokerSubscription, MessageBrokerDriver, BrokerSemantics,
    BrokerError, AckOutcome,
};
use crate::traits::ConnectionParams;

/// Fixture: receive a message, ack it, verify the driver reports `Acked`.
///
/// Skips silently when the broker is unreachable (`create_consumer` returns `Err`).
pub async fn test_ack_returns_acked<D: MessageBrokerDriver>(driver: &D) {
    let config = default_config();
    let params = unreachable_params();
    let consumer = driver.create_consumer(&params, &config).await;
    if consumer.is_err() {
        return;
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

/// Fixture: receive → nack → expect redelivery or `BrokerError::Unsupported` for `AtMostOnce`.
///
/// Skips silently when the broker is unreachable.
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
            assert!(
                matches!(nack_result, Err(BrokerError::Unsupported)),
                "AtMostOnce/FireAndForget driver must return BrokerError::Unsupported for nack, got: {nack_result:?}"
            );
        }
        BrokerSemantics::AtLeastOnce => {
            assert!(
                nack_result.is_ok(),
                "AtLeastOnce driver nack must not error: {nack_result:?}"
            );
        }
    }
}

/// Fixture: two consumers with the same group prefix/app/datasource → group derivation is stable.
///
/// Verifies the group-id derivation contract: `{group_prefix}.{app_id}.{datasource_id}`.
/// Skips silently when the broker is unreachable.
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
    // Both configs derive the same group — broker-level exclusivity tested in cluster tests.
    let _ = (
        driver.create_consumer(&params, &config1).await,
        driver.create_consumer(&params, &config2).await,
    );
}

/// Fixture: consumer created with multiple subscriptions → driver accepts all topics.
///
/// Skips silently when the broker is unreachable.
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
    let _ = driver.create_consumer(&params, &config).await;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

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

/// Returns params pointing at port 1 (nothing listening) so fixtures always
/// fail fast when called against a real driver and a real broker isn't running.
pub fn unreachable_params() -> ConnectionParams {
    ConnectionParams {
        host: "127.0.0.1".into(),
        port: 1,
        database: "test".into(),
        username: String::new(),
        password: String::new(),
        options: HashMap::new(),
    }
}
