//! Live integration test for Kafka plugin against Podman infra.
//!
//! Requires: Kafka broker. Set RIVERS_TEST_KAFKA_HOST (default: localhost), port 9092.
//! Run: cargo test -p rivers-plugin-kafka --test kafka_live_test -- --nocapture

use std::collections::HashMap;
use rivers_driver_sdk::broker::{
    BrokerConsumerConfig, BrokerSubscription, MessageBrokerDriver, OutboundMessage,
};
use rivers_driver_sdk::ConnectionParams;
use rivers_plugin_kafka::KafkaDriver;

fn kafka_host() -> String {
    std::env::var("RIVERS_TEST_KAFKA_HOST").unwrap_or_else(|_| "localhost".to_string())
}

fn kafka_params() -> ConnectionParams {
    ConnectionParams {
        host: kafka_host(),
        port: 9092,
        database: "".into(),
        username: "".into(),
        password: "".into(),
        options: HashMap::new(),
    }
}

fn broker_config(topic: &str) -> BrokerConsumerConfig {
    BrokerConsumerConfig {
        group_prefix: "rivers-test".into(),
        app_id: "test-app".into(),
        datasource_id: "kafka-test".into(),
        node_id: "node-0".into(),
        reconnect_ms: 5000,
        subscriptions: vec![BrokerSubscription {
            topic: topic.into(),
            event_name: Some(topic.into()),
        }],
    }
}

#[tokio::test]
async fn kafka_produce_and_consume() {
    let driver = KafkaDriver;

    // Use a unique topic with timestamp to avoid collisions
    let topic = format!("rivers-test-{}", chrono::Utc::now().timestamp_millis());

    // 1. Create a producer
    let config = broker_config(&topic);
    let mut producer = driver
        .create_producer(&kafka_params(), &config)
        .await
        .expect("create_producer should succeed");

    // 2. Publish a message
    let payload = serde_json::json!({
        "order_id": 100,
        "customer": "test-user",
        "total": 99.99
    });
    let message = OutboundMessage {
        destination: topic.clone(),
        payload: serde_json::to_vec(&payload).unwrap(),
        headers: HashMap::new(),
        key: Some("test-key".into()),
        reply_to: None,
    };

    let receipt = producer
        .publish(message)
        .await
        .expect("publish should succeed");

    println!("Published to Kafka: receipt = {:?}", receipt);
    assert!(receipt.id.is_some(), "should have a receipt ID");

    // Verify receipt contains topic:partition:offset
    let id = receipt.id.unwrap();
    assert!(id.starts_with(&topic), "receipt should contain topic name");
    println!("Kafka produce test PASSED — receipt: {}", id);

    producer.close().await.expect("close should succeed");

    // 3. Create a consumer and read back
    let mut consumer = driver
        .create_consumer(&kafka_params(), &config)
        .await
        .expect("create_consumer should succeed");

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        consumer.receive(),
    )
    .await
    .expect("should receive within 10s")
    .expect("receive should succeed");

    println!("Consumed from Kafka: topic={}, payload_len={}", msg.destination, msg.payload.len());

    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["order_id"], 100);
    assert_eq!(body["customer"], "test-user");
    println!("Kafka consume test PASSED — payload verified");

    consumer.ack(&msg.receipt).await.expect("ack should succeed");
    consumer.close().await.expect("close should succeed");
}
