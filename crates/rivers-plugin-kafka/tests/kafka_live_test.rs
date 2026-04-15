//! Live integration test for Kafka plugin against Podman infra.
//!
//! Credentials are resolved from a LockBox keystore at sec/lockbox/.
//! Run: cargo test -p rivers-plugin-kafka --test kafka_live_test -- --nocapture

use std::collections::HashMap;
use rivers_driver_sdk::broker::{
    BrokerConsumerConfig, BrokerSubscription, MessageBrokerDriver, OutboundMessage,
};
use rivers_driver_sdk::ConnectionParams;
use rivers_plugin_kafka::KafkaDriver;

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    let encrypted = std::fs::read(dir.join("entries/kafka/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/kafka/test.meta.json")).unwrap()
    ).unwrap();

    let hosts: Vec<String> = meta["hosts"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let (host, port) = parse_host_port(&hosts[0]);

    let mut options: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(obj) = meta["options"].as_object() {
        for (k, v) in obj { options.insert(k.clone(), v.as_str().unwrap_or("").to_string()); }
    }
    if hosts.len() > 1 {
        options.insert("hosts".into(), hosts.join(","));
        options.insert("cluster".into(), "true".into());
    }

    ConnectionParams {
        host, port,
        database: meta["database"].as_str().unwrap_or("").to_string(),
        username: meta["username"].as_str().unwrap_or("").to_string(),
        password, options,
    }
}

fn parse_host_port(s: &str) -> (String, u16) {
    match s.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(0)),
        None => (s.to_string(), 0),
    }
}

fn find_lockbox_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("RIVERS_LOCKBOX_DIR") {
        let p = std::path::PathBuf::from(&dir);
        if p.join("identity.key").exists() { return Some(p); }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..10 {
        let candidate = dir.join("sec").join("lockbox");
        if candidate.join("identity.key").exists() { return Some(candidate); }
        if !dir.pop() { break; }
    }
    None
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

/// Validates that the Kafka driver can produce a message to a topic and consume it back with correct payload.
#[tokio::test]
async fn kafka_produce_and_consume() {
    let driver = KafkaDriver;

    // Use a unique topic with timestamp to avoid collisions
    let topic = format!("rivers-test-{}", chrono::Utc::now().timestamp_millis());

    // 1. Create a producer
    let config = broker_config(&topic);
    let mut producer = driver
        .create_producer(&conn_params(), &config)
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
        .create_consumer(&conn_params(), &config)
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
