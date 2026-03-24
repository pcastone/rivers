//! RabbitMQ broker latency benchmark.
//! Run: cargo test -p rivers-plugin-rabbitmq --test driver_bench -- --nocapture
use std::collections::HashMap;
use std::time::{Duration, Instant};
use rivers_driver_sdk::broker::{BrokerConsumerConfig, BrokerSubscription, OutboundMessage};
use rivers_driver_sdk::{ConnectionParams, MessageBrokerDriver};
use rivers_plugin_rabbitmq::RabbitMqDriver;

const TIMEOUT: Duration = Duration::from_secs(10);
const ITERS: usize = 1_000;

include!("lockbox_helper.rs");

#[tokio::test]
async fn bench_rabbitmq() {
    let params = conn_params("rabbitmq/test");
    let driver = RabbitMqDriver;
    let config = BrokerConsumerConfig {
        group_prefix: "bench".into(), app_id: "bench".into(),
        datasource_id: "rmq-bench".into(), node_id: "n0".into(),
        reconnect_ms: 5000,
        subscriptions: vec![BrokerSubscription { topic: "bench-q".into(), event_name: None }],
    };

    let mut producer = match tokio::time::timeout(TIMEOUT, driver.create_producer(&params, &config)).await {
        Ok(Ok(p)) => p, _ => { println!("SKIP: RabbitMQ unreachable"); return; }
    };

    let msg = OutboundMessage {
        destination: "bench-q".into(), payload: b"bench".to_vec(),
        headers: HashMap::new(), key: None, reply_to: None,
    };

    let start = Instant::now();
    for _ in 0..ITERS { let _ = producer.publish(msg.clone()).await; }
    let total = start.elapsed();

    println!("\n  RabbitMQ: publish={:.1}μs/op  ({:.0} ops/s, {} iters)",
        total.as_micros() as f64 / ITERS as f64,
        ITERS as f64 / total.as_secs_f64(), ITERS);
}
