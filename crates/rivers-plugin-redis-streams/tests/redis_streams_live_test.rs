//! Live integration tests for the Redis Streams plugin driver.
//!
//! Requires a running Redis server. Set RIVERS_TEST_REDIS_HOST (default: localhost).
//! Credentials are resolved from a LockBox keystore.
//! If the service is unreachable, tests print SKIP and pass.
//!
//! Run with: cargo test --test redis_streams_live_test

use std::collections::HashMap;
use std::time::Duration;

use rivers_driver_sdk::{
    BrokerConsumerConfig, BrokerSubscription, ConnectionParams, MessageBrokerDriver,
    OutboundMessage,
};
use rivers_plugin_redis_streams::RedisStreamsDriver;

const REDIS_PORT: u16 = 6379;
const TIMEOUT: Duration = Duration::from_secs(15);

fn redis_host() -> String {
    std::env::var("RIVERS_TEST_REDIS_HOST").unwrap_or_else(|_| "localhost".to_string())
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

fn conn_params() -> ConnectionParams {
    let password = lockbox_resolve("redis-streams/test", "rivers_test");
    let mut options = HashMap::new();
    options.insert("cluster".into(), "true".into());
    let host = redis_host();
    options.insert(
        "hosts".into(),
        format!("{host}:6379"),
    );
    ConnectionParams {
        host: redis_host(),
        port: REDIS_PORT,
        database: "0".into(),
        username: "".into(),
        password,
        options,
    }
}

/// Generate a unique stream name to avoid collisions between test runs.
fn unique_stream() -> String {
    format!(
        "rivers_test_stream_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn make_config(stream: &str) -> BrokerConsumerConfig {
    // Use unique app_id per run to avoid stale consumer group data from prior runs
    let unique_id = format!(
        "live_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    BrokerConsumerConfig {
        group_prefix: "test".into(),
        app_id: unique_id,
        datasource_id: "rs1".into(),
        node_id: "node1".into(),
        reconnect_ms: 1000,
        subscriptions: vec![BrokerSubscription {
            topic: stream.to_string(),
            event_name: Some("test.event".into()),
        }],
    }
}

/// Try to create a producer; returns None (with SKIP message) if unreachable.
async fn try_create_producer(
    stream: &str,
) -> Option<Box<dyn rivers_driver_sdk::BrokerProducer>> {
    let driver = RedisStreamsDriver;
    let config = make_config(stream);
    match tokio::time::timeout(TIMEOUT, driver.create_producer(&conn_params(), &config)).await {
        Ok(Ok(producer)) => Some(producer),
        Ok(Err(e)) => {
            let host = redis_host();
            eprintln!("SKIP: Redis unreachable at {host}:{REDIS_PORT} — {e}");
            None
        }
        Err(_) => {
            let host = redis_host();
            eprintln!("SKIP: Redis connection timed out at {host}:{REDIS_PORT}");
            None
        }
    }
}

#[tokio::test]
async fn redis_streams_produce_consume_roundtrip() {
    let stream = unique_stream();
    let Some(mut producer) = try_create_producer(&stream).await else {
        return;
    };

    // Publish a message
    let payload = b"hello from rivers live test".to_vec();
    let msg = OutboundMessage {
        destination: stream.clone(),
        payload: payload.clone(),
        headers: HashMap::new(),
        key: None,
        reply_to: None,
    };

    let receipt = match tokio::time::timeout(TIMEOUT, producer.publish(msg)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let msg = format!("{e}");
            if msg.contains("NOAUTH") || msg.contains("Authentication") {
                let host = redis_host();
                eprintln!("SKIP: Redis requires authentication at {host}:{REDIS_PORT}");
                return;
            }
            panic!("publish failed: {e:?}");
        }
        Err(_) => panic!("publish timed out"),
    };

    assert!(
        receipt.id.is_some(),
        "expected a stream entry ID from publish"
    );
    let entry_id = receipt.id.unwrap();
    assert!(
        entry_id.contains('-'),
        "expected entry ID in 'ts-seq' format, got: {entry_id}"
    );

    // Create a consumer and read the message back
    let driver = RedisStreamsDriver;
    // Use a fresh config that reads from the beginning of the stream
    let config = make_config(&stream);
    let mut consumer = tokio::time::timeout(
        TIMEOUT,
        driver.create_consumer(&conn_params(), &config),
    )
    .await
    .expect("consumer creation timed out")
    .expect("consumer creation failed");

    // The consumer reads with ">", which means only new messages after group creation.
    // Since we already published before creating the consumer group, we need to
    // publish another message that the consumer will see.
    let payload2 = b"second message for consumer".to_vec();
    let msg2 = OutboundMessage {
        destination: stream.clone(),
        payload: payload2.clone(),
        headers: HashMap::new(),
        key: None,
        reply_to: None,
    };
    tokio::time::timeout(TIMEOUT, producer.publish(msg2))
        .await
        .expect("second publish timed out")
        .expect("second publish failed");

    // Receive the message
    let inbound = tokio::time::timeout(TIMEOUT, consumer.receive())
        .await
        .expect("receive timed out")
        .expect("receive failed");

    assert_eq!(inbound.destination, stream, "destination should match stream name");
    assert_eq!(inbound.payload, payload2, "payload should match what was published");
    assert!(!inbound.id.is_empty(), "message ID should not be empty");

    // Ack the message
    tokio::time::timeout(TIMEOUT, consumer.ack(&inbound.receipt))
        .await
        .expect("ack timed out")
        .expect("ack failed");

    // Cleanup: close producer and consumer
    producer.close().await.ok();
    consumer.close().await.ok();

    // Cleanup: delete the stream using raw redis command
    cleanup_stream(&stream).await;
}

/// Delete a Redis stream for cleanup (uses cluster connection).
async fn cleanup_stream(stream: &str) {
    let password = lockbox_resolve("redis-streams/test", "rivers_test");
    let host = redis_host();
    let host_port = format!("{host}:{REDIS_PORT}");
    let hosts = [host_port.as_str()];
    let nodes: Vec<String> = hosts
        .iter()
        .map(|h| {
            if password.is_empty() {
                format!("redis://{h}")
            } else {
                format!("redis://:{}@{h}", password)
            }
        })
        .collect();
    if let Ok(client) = redis::cluster::ClusterClient::new(nodes) {
        if let Ok(mut conn) = client.get_async_connection().await {
            let _: Result<(), _> = redis::cmd("DEL")
                .arg(stream)
                .query_async(&mut conn)
                .await;
        }
    }
}
