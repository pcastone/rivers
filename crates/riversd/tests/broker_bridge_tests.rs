//! Broker Consumer Bridge tests — message flow, failure policies, lag, drain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{watch, Mutex};

use rivers_runtime::rivers_core::event::Event;
use rivers_runtime::rivers_core::eventbus::{events, EventBus, EventHandler, HandlerPriority};
use rivers_runtime::rivers_driver_sdk::broker::{
    BrokerConsumer, BrokerMetadata, BrokerProducer, FailureMode, FailurePolicy,
    InboundMessage, MessageReceipt, OutboundMessage, PublishReceipt,
};
use rivers_runtime::rivers_driver_sdk::error::DriverError;

use riversd::broker_bridge::BrokerConsumerBridge;

// ── Mock Consumer ─────────────────────────────────────────────────

struct MockConsumer {
    messages: Mutex<Vec<InboundMessage>>,
    acked: Arc<Mutex<Vec<String>>>,
    nacked: Arc<Mutex<Vec<String>>>,
    fail_receive: Mutex<bool>,
    receive_count: AtomicUsize,
}

impl MockConsumer {
    fn new(messages: Vec<InboundMessage>) -> Self {
        Self {
            messages: Mutex::new(messages),
            acked: Arc::new(Mutex::new(Vec::new())),
            nacked: Arc::new(Mutex::new(Vec::new())),
            fail_receive: Mutex::new(false),
            receive_count: AtomicUsize::new(0),
        }
    }

}

#[async_trait]
impl BrokerConsumer for MockConsumer {
    async fn receive(&mut self) -> Result<InboundMessage, DriverError> {
        self.receive_count.fetch_add(1, Ordering::Relaxed);

        let fail = *self.fail_receive.lock().await;
        if fail {
            // After first failure, stop failing
            *self.fail_receive.lock().await = false;
            return Err(DriverError::Connection("mock connection error".into()));
        }

        let mut msgs = self.messages.lock().await;
        if let Some(msg) = msgs.pop() {
            Ok(msg)
        } else {
            // Simulate blocking — just return an error to end the loop
            Err(DriverError::Connection("no more messages".into()))
        }
    }

    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        self.acked.lock().await.push(receipt.handle.clone());
        Ok(())
    }

    async fn nack(&mut self, receipt: &MessageReceipt) -> Result<(), DriverError> {
        self.nacked.lock().await.push(receipt.handle.clone());
        Ok(())
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

// ── Mock Producer ─────────────────────────────────────────────────

struct MockProducer {
    published: Arc<Mutex<Vec<OutboundMessage>>>,
}

impl MockProducer {
    fn new() -> Self {
        Self {
            published: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl BrokerProducer for MockProducer {
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError> {
        self.published.lock().await.push(message);
        Ok(PublishReceipt {
            id: Some("pub-1".into()),
            metadata: None,
        })
    }

    async fn close(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

// ── Event Collector ───────────────────────────────────────────────

struct EventCollector {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventCollector {
    fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
            },
            events,
        )
    }
}

#[async_trait]
impl EventHandler for EventCollector {
    async fn handle(&self, event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.events.lock().await.push(event.event_type.clone());
        Ok(())
    }

    fn name(&self) -> &str {
        "EventCollector"
    }
}

// ── Failing Handler (for failure policy tests) ────────────────────

struct FailingHandler;

#[async_trait]
impl EventHandler for FailingHandler {
    async fn handle(&self, _event: &Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("processing failed".into())
    }

    fn name(&self) -> &str {
        "FailingHandler"
    }
}

// ── Helper ────────────────────────────────────────────────────────

fn make_message(id: &str, destination: &str) -> InboundMessage {
    InboundMessage {
        id: id.to_string(),
        destination: destination.to_string(),
        payload: b"test-payload".to_vec(),
        headers: HashMap::new(),
        timestamp: chrono::Utc::now(),
        receipt: MessageReceipt {
            handle: format!("receipt-{}", id),
        },
        metadata: BrokerMetadata::Kafka {
            partition: 0,
            offset: 1,
            consumer_group: "test-group".into(),
        },
    }
}

fn drop_policy() -> FailurePolicy {
    FailurePolicy {
        mode: FailureMode::Drop,
        destination: None,
        handlers: vec![],
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn bridge_processes_single_message() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());
    let (collector, collected) = EventCollector::new();
    event_bus
        .subscribe(
            events::BROKER_MESSAGE_RECEIVED,
            Arc::new(collector),
            HandlerPriority::Handle,
        )
        .await;

    let consumer = MockConsumer::new(vec![make_message("msg-1", "test-topic")]);
    let acked = consumer.acked.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "test-ds",
        100,
        shutdown_rx,
    );

    // Run bridge in background, stop after processing
    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    // Verify message was published to EventBus
    let events = collected.lock().await;
    assert!(
        events.contains(&events::BROKER_MESSAGE_RECEIVED.to_string()),
        "expected BrokerMessageReceived event"
    );

    // Verify broker ack
    let acks = acked.lock().await;
    assert!(
        acks.contains(&"receipt-msg-1".to_string()),
        "expected message to be acked"
    );
}

#[tokio::test]
async fn bridge_without_storage_processes_directly() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let consumer = MockConsumer::new(vec![make_message("msg-2", "direct-topic")]);
    let acked = consumer.acked.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "direct-ds",
        100,
        shutdown_rx,
    );

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    // Verify broker ack without any storage buffering
    let acks = acked.lock().await;
    assert!(
        acks.contains(&"receipt-msg-2".to_string()),
        "message should be acked directly without storage"
    );
}

#[tokio::test]
async fn bridge_failure_policy_drop() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    // Register a failing handler so processing fails
    event_bus
        .subscribe(
            events::BROKER_MESSAGE_RECEIVED,
            Arc::new(FailingHandler),
            HandlerPriority::Expect,
        )
        .await;

    let consumer = MockConsumer::new(vec![make_message("msg-drop", "drop-topic")]);
    let acked = consumer.acked.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        FailurePolicy {
            mode: FailureMode::Drop,
            destination: None,
            handlers: vec![],
        },
        "drop-ds",
        100,
        shutdown_rx,
    );

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    // Drop policy should still ack the message
    let acks = acked.lock().await;
    assert!(
        acks.contains(&"receipt-msg-drop".to_string()),
        "drop policy should ack the message"
    );
}

#[tokio::test]
async fn bridge_failure_policy_requeue() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    event_bus
        .subscribe(
            events::BROKER_MESSAGE_RECEIVED,
            Arc::new(FailingHandler),
            HandlerPriority::Expect,
        )
        .await;

    let consumer = MockConsumer::new(vec![make_message("msg-requeue", "requeue-topic")]);
    let nacked = consumer.nacked.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        FailurePolicy {
            mode: FailureMode::Requeue,
            destination: None,
            handlers: vec![],
        },
        "requeue-ds",
        100,
        shutdown_rx,
    );

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    // Requeue should nack the message
    let nacks = nacked.lock().await;
    assert!(
        nacks.contains(&"receipt-msg-requeue".to_string()),
        "requeue policy should nack the message"
    );
}

#[tokio::test]
async fn bridge_failure_policy_dead_letter() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    event_bus
        .subscribe(
            events::BROKER_MESSAGE_RECEIVED,
            Arc::new(FailingHandler),
            HandlerPriority::Expect,
        )
        .await;

    let consumer = MockConsumer::new(vec![make_message("msg-dl", "dl-topic")]);
    let producer = MockProducer::new();
    let published = producer.published.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        FailurePolicy {
            mode: FailureMode::DeadLetter,
            destination: Some("dlq".into()),
            handlers: vec![],
        },
        "dl-ds",
        100,
        shutdown_rx,
    )
    .with_failure_producer(Box::new(producer));

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    // Dead letter should publish to the DLQ
    let pubs = published.lock().await;
    assert_eq!(pubs.len(), 1, "expected one dead-letter publish");
    assert_eq!(pubs[0].destination, "dlq");
}

#[tokio::test]
async fn bridge_failure_policy_redirect() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    event_bus
        .subscribe(
            events::BROKER_MESSAGE_RECEIVED,
            Arc::new(FailingHandler),
            HandlerPriority::Expect,
        )
        .await;

    let consumer = MockConsumer::new(vec![make_message("msg-redir", "redir-topic")]);
    let producer = MockProducer::new();
    let published = producer.published.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        FailurePolicy {
            mode: FailureMode::Redirect,
            destination: Some("alternate-topic".into()),
            handlers: vec![],
        },
        "redir-ds",
        100,
        shutdown_rx,
    )
    .with_failure_producer(Box::new(producer));

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    let pubs = published.lock().await;
    assert_eq!(pubs.len(), 1, "expected one redirect publish");
    assert_eq!(pubs[0].destination, "alternate-topic");
}

#[tokio::test]
async fn bridge_consumer_lag_detection() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let (collector, collected) = EventCollector::new();
    event_bus
        .subscribe(
            events::CONSUMER_LAG_DETECTED,
            Arc::new(collector),
            HandlerPriority::Handle,
        )
        .await;

    // Send 3 messages with lag threshold of 1
    let consumer = MockConsumer::new(vec![
        make_message("lag-3", "lag-topic"),
        make_message("lag-2", "lag-topic"),
        make_message("lag-1", "lag-topic"),
    ]);

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "lag-ds",
        100,
        shutdown_rx,
    )
    .with_consumer_lag_threshold(1);

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    let events = collected.lock().await;
    assert!(
        !events.is_empty(),
        "expected ConsumerLagDetected events"
    );
}

#[tokio::test]
async fn bridge_reconnection_on_error() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let (collector, collected) = EventCollector::new();
    event_bus
        .subscribe(
            events::BROKER_CONSUMER_ERROR,
            Arc::new(collector),
            HandlerPriority::Handle,
        )
        .await;

    // Consumer that fails on first receive, then succeeds, then runs out
    let mut consumer = MockConsumer::new(vec![make_message("after-error", "recon-topic")]);
    consumer.fail_receive = Mutex::new(true);

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "recon-ds",
        10, // very short reconnect for test
        shutdown_rx,
    );

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    let events = collected.lock().await;
    assert!(
        events.contains(&events::BROKER_CONSUMER_ERROR.to_string()),
        "expected BrokerConsumerError event after receive failure"
    );
}

#[tokio::test]
async fn bridge_publishes_start_and_stop_events() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let (start_collector, start_events) = EventCollector::new();
    let (stop_collector, stop_events) = EventCollector::new();

    event_bus
        .subscribe(
            events::BROKER_CONSUMER_STARTED,
            Arc::new(start_collector),
            HandlerPriority::Handle,
        )
        .await;
    event_bus
        .subscribe(
            events::BROKER_CONSUMER_STOPPED,
            Arc::new(stop_collector),
            HandlerPriority::Handle,
        )
        .await;

    let consumer = MockConsumer::new(vec![]);

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "lifecycle-ds",
        100,
        shutdown_rx,
    );

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    assert!(
        !start_events.lock().await.is_empty(),
        "expected BrokerConsumerStarted"
    );
    assert!(
        !stop_events.lock().await.is_empty(),
        "expected BrokerConsumerStopped"
    );
}

#[tokio::test]
async fn bridge_messages_pending_returns_to_zero() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let consumer = MockConsumer::new(vec![
        make_message("p-2", "pending-topic"),
        make_message("p-1", "pending-topic"),
    ]);

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "pending-ds",
        100,
        shutdown_rx,
    );

    let pending = bridge.pending_counter();

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    assert_eq!(
        pending.load(Ordering::Relaxed),
        0,
        "messages_pending should return to 0 after processing"
    );
}

#[tokio::test]
async fn bridge_drain_with_timeout() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let consumer = MockConsumer::new(vec![]);

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "drain-ds",
        100,
        shutdown_rx,
    )
    .with_drain_timeout_ms(50);

    // Signal shutdown immediately
    shutdown_tx.send(true).unwrap();

    let handle = tokio::spawn(bridge.run());
    // Should complete within drain timeout
    tokio::time::timeout(std::time::Duration::from_millis(500), handle)
        .await
        .expect("bridge should complete within timeout")
        .unwrap();
}

#[tokio::test]
async fn bridge_multiple_messages_all_acked() {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let event_bus = Arc::new(EventBus::new());

    let consumer = MockConsumer::new(vec![
        make_message("multi-3", "multi-topic"),
        make_message("multi-2", "multi-topic"),
        make_message("multi-1", "multi-topic"),
    ]);
    let acked = consumer.acked.clone();

    let bridge = BrokerConsumerBridge::new(
        Box::new(consumer),
        event_bus,
        drop_policy(),
        "multi-ds",
        100,
        shutdown_rx,
    );

    let handle = tokio::spawn(bridge.run());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();

    let acks = acked.lock().await;
    assert_eq!(acks.len(), 3, "all 3 messages should be acked");
}
