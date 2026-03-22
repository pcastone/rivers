//! Live integration test for RabbitMQ plugin against Podman infra.
//!
//! Requires: RabbitMQ broker. Set RIVERS_TEST_RABBITMQ_HOST (default: localhost), port 5672.
//! Credentials are resolved from a LockBox keystore.
//! Run: cargo test -p rivers-plugin-rabbitmq --test rabbitmq_live_test -- --nocapture

use std::collections::HashMap;
use rivers_driver_sdk::broker::{
    BrokerConsumerConfig, BrokerSubscription, MessageBrokerDriver, OutboundMessage,
};
use rivers_driver_sdk::ConnectionParams;
use rivers_plugin_rabbitmq::RabbitMqDriver;

fn rabbitmq_host() -> String {
    std::env::var("RIVERS_TEST_RABBITMQ_HOST").unwrap_or_else(|_| "localhost".to_string())
}

/// Resolve a single credential from a temporary LockBox keystore.
fn lockbox_resolve(name: &str, value: &str) -> String {
    use age::secrecy::ExposeSecret;
    use rivers_core::lockbox::{
        encrypt_keystore, fetch_secret_value, Keystore, KeystoreEntry, LockBoxResolver,
    };

    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    let now = chrono::Utc::now();

    let entry = KeystoreEntry {
        name: name.to_string(),
        value: value.to_string(),
        entry_type: "string".to_string(),
        aliases: vec![],
        created: now,
        updated: now,
    };
    let keystore = Keystore { version: 1, entries: vec![entry] };

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.rkeystore");
    encrypt_keystore(&path, &recipient.to_string(), &keystore).unwrap();

    let resolver = LockBoxResolver::from_entries(&keystore.entries).unwrap();
    let metadata = resolver.resolve(name).unwrap();
    let identity_str = identity.to_string();
    let resolved = fetch_secret_value(metadata, &path, identity_str.expose_secret()).unwrap();
    resolved.value
}

fn rabbitmq_params() -> ConnectionParams {
    let password = lockbox_resolve("rabbitmq/test", "guest");
    ConnectionParams {
        host: rabbitmq_host(),
        port: 5672,
        database: "/".into(), // vhost
        username: "guest".into(),
        password,
        options: HashMap::new(),
    }
}

fn broker_config(queue: &str) -> BrokerConsumerConfig {
    BrokerConsumerConfig {
        group_prefix: "rivers-test".into(),
        app_id: "test-app".into(),
        datasource_id: "rabbitmq-test".into(),
        node_id: "node-0".into(),
        reconnect_ms: 5000,
        subscriptions: vec![BrokerSubscription {
            topic: queue.into(),
            event_name: Some(queue.into()),
        }],
    }
}

#[tokio::test]
async fn rabbitmq_produce_and_consume() {
    let driver = RabbitMqDriver;
    let queue = "rivers-test-events";
    let config = broker_config(queue);

    // 1. Create a producer
    let producer_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        driver.create_producer(&rabbitmq_params(), &config),
    )
    .await;

    let mut producer = match producer_result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            println!("RabbitMQ create_producer failed (likely not running): {}", e);
            println!("RabbitMQ test SKIPPED — service not available");
            return;
        }
        Err(_) => {
            println!("RabbitMQ create_producer timed out (service not available)");
            println!("RabbitMQ test SKIPPED — service not available");
            return;
        }
    };

    // 2. Publish a message
    let payload = serde_json::json!({
        "event": "user.login",
        "user": "alice"
    });
    let message = OutboundMessage {
        destination: queue.into(),
        payload: serde_json::to_vec(&payload).unwrap(),
        headers: HashMap::new(),
        key: None,
        reply_to: None,
    };

    let receipt = producer
        .publish(message)
        .await
        .expect("publish should succeed");
    println!("Published to RabbitMQ: receipt = {:?}", receipt);

    producer.close().await.expect("close should succeed");

    // 3. Create a consumer and read back
    let mut consumer = driver
        .create_consumer(&rabbitmq_params(), &config)
        .await
        .expect("create_consumer should succeed");

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        consumer.receive(),
    )
    .await
    .expect("should receive within 10s")
    .expect("receive should succeed");

    println!("Consumed from RabbitMQ: destination={}, payload_len={}", msg.destination, msg.payload.len());

    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["event"], "user.login");
    assert_eq!(body["user"], "alice");

    consumer.ack(&msg.receipt).await.expect("ack should succeed");
    consumer.close().await.expect("close should succeed");
    println!("RabbitMQ produce→consume roundtrip PASSED");
}
