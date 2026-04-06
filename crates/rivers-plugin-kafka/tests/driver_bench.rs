//! Kafka broker latency benchmark.
//! Run: cargo test -p rivers-plugin-kafka --test driver_bench -- --nocapture
use std::collections::HashMap;
use std::time::{Duration, Instant};
use rivers_driver_sdk::broker::{BrokerConsumerConfig, BrokerSubscription, OutboundMessage};
use rivers_driver_sdk::MessageBrokerDriver;
use rivers_plugin_kafka::KafkaDriver;

const TIMEOUT: Duration = Duration::from_secs(10);
const ITERS: usize = 500;

include!("lockbox_helper.rs");

#[tokio::test]
async fn bench_kafka() {
    let params = conn_params("kafka/test");
    let driver = KafkaDriver;
    let topic = format!("bench-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
    let config = BrokerConsumerConfig {
        group_prefix: "bench".into(), app_id: "bench".into(),
        datasource_id: "kafka-bench".into(), node_id: "n0".into(),
        reconnect_ms: 5000,
        subscriptions: vec![BrokerSubscription { topic: topic.clone(), event_name: None }],
    };

    let mut producer = match tokio::time::timeout(TIMEOUT, driver.create_producer(&params, &config)).await {
        Ok(Ok(p)) => p, _ => { println!("SKIP: Kafka unreachable"); return; }
    };

    let msg = OutboundMessage {
        destination: topic.clone(), payload: b"bench".to_vec(),
        headers: HashMap::new(), key: None, reply_to: None,
    };

    let start = Instant::now();
    for _ in 0..ITERS { let _ = producer.publish(msg.clone()).await; }
    let total = start.elapsed();

    println!("\n  Kafka: publish={:.1}μs/op  ({:.0} ops/s, {} iters)",
        total.as_micros() as f64 / ITERS as f64,
        ITERS as f64 / total.as_secs_f64(), ITERS);
}
