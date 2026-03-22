//! Live integration test for NATS plugin against Podman infra.
//!
//! Requires: NATS server. Set RIVERS_TEST_NATS_HOST (default: localhost), port 4222.
//! Run: cargo test -p rivers-plugin-nats --test nats_live_test -- --nocapture

use std::collections::HashMap;
use rivers_driver_sdk::broker::{
    BrokerConsumerConfig, BrokerSubscription, MessageBrokerDriver, OutboundMessage,
};
use rivers_driver_sdk::ConnectionParams;
use rivers_plugin_nats::NatsDriver;

fn nats_host() -> String {
    std::env::var("RIVERS_TEST_NATS_HOST").unwrap_or_else(|_| "localhost".to_string())
}

fn nats_params() -> ConnectionParams {
    ConnectionParams {
        host: nats_host(),
        port: 4222,
        database: "".into(),
        username: "".into(),
        password: "".into(),
        options: HashMap::new(),
    }
}

fn broker_config(subject: &str) -> BrokerConsumerConfig {
    BrokerConsumerConfig {
        group_prefix: "rivers-test".into(),
        app_id: "test-app".into(),
        datasource_id: "nats-test".into(),
        node_id: "node-0".into(),
        reconnect_ms: 5000,
        subscriptions: vec![BrokerSubscription {
            topic: subject.into(),
            event_name: Some(subject.into()),
        }],
    }
}

#[tokio::test]
async fn nats_produce_and_consume() {
    let driver = NatsDriver;
    let subject = "rivers.test.events";
    let config = broker_config(subject);

    // 1. Create a consumer FIRST (NATS is pub/sub — must subscribe before publish)
    let consumer_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        driver.create_consumer(&nats_params(), &config),
    )
    .await;

    let mut consumer = match consumer_result {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            println!("NATS create_consumer failed (likely not running): {}", e);
            println!("NATS test SKIPPED — service not available");
            return;
        }
        Err(_) => {
            println!("NATS create_consumer timed out (service not available)");
            println!("NATS test SKIPPED — service not available");
            return;
        }
    };

    // 2. Create a producer
    let mut producer = driver
        .create_producer(&nats_params(), &config)
        .await
        .expect("create_producer should succeed");

    // Small delay to ensure subscription is ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 3. Publish a message
    let payload = serde_json::json!({
        "event": "page_view",
        "path": "/home"
    });
    let message = OutboundMessage {
        destination: subject.into(),
        payload: serde_json::to_vec(&payload).unwrap(),
        headers: HashMap::new(),
        key: None,
        reply_to: None,
    };

    let receipt = producer
        .publish(message)
        .await
        .expect("publish should succeed");
    println!("Published to NATS: receipt = {:?}", receipt);

    producer.close().await.expect("close should succeed");

    // 4. Consume the message
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        consumer.receive(),
    )
    .await
    .expect("should receive within 10s")
    .expect("receive should succeed");

    println!("Consumed from NATS: destination={}, payload_len={}", msg.destination, msg.payload.len());

    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["event"], "page_view");
    assert_eq!(body["path"], "/home");

    consumer.ack(&msg.receipt).await.expect("ack should succeed");
    consumer.close().await.expect("close should succeed");
    println!("NATS produce→consume roundtrip PASSED");
}
