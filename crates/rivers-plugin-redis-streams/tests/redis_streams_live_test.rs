//! Live integration tests for the Redis Streams plugin driver.
//!
//! Connection info resolved from LockBox keystore (see `sec/lockbox/`).
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

const TIMEOUT: Duration = Duration::from_secs(15);

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    let encrypted = std::fs::read(dir.join("entries/redis-streams/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/redis-streams/test.meta.json")).unwrap()
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
        host,
        port,
        database: meta["database"].as_str().unwrap_or("").to_string(),
        username: meta["username"].as_str().unwrap_or("").to_string(),
        password,
        options,
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
            eprintln!("SKIP: Redis unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: Redis connection timed out");
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
                eprintln!("SKIP: Redis requires authentication");
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
    let params = conn_params();
    let hosts_str = params.options.get("hosts").cloned().unwrap_or_default();
    let hosts: Vec<&str> = if hosts_str.is_empty() {
        vec![]
    } else {
        hosts_str.split(',').collect()
    };

    if hosts.is_empty() {
        // Single-node fallback
        let url = if params.password.is_empty() {
            format!("redis://{}:{}", params.host, params.port)
        } else {
            format!("redis://:{}@{}:{}", params.password, params.host, params.port)
        };
        if let Ok(client) = redis::Client::open(url.as_str()) {
            if let Ok(mut conn) = client.get_multiplexed_async_connection().await {
                let _: Result<(), _> = redis::cmd("DEL")
                    .arg(stream)
                    .query_async(&mut conn)
                    .await;
            }
        }
    } else {
        let nodes: Vec<String> = hosts
            .iter()
            .map(|h| {
                if params.password.is_empty() {
                    format!("redis://{h}")
                } else {
                    format!("redis://:{}@{h}", params.password)
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
}
