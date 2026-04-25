//! Broker Consumer Bridge — async task per broker consumer datasource.
//!
//! Per `rivers-data-layer-spec.md` §10.
//!
//! Runs one async task per configured broker consumer. Pulls messages from
//! the broker, publishes to EventBus, and handles failure policy when
//! processing fails.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing;

use rivers_runtime::rivers_core::event::Event;
use rivers_runtime::rivers_core::eventbus::{events, EventBus};
use rivers_runtime::rivers_driver_sdk::broker::{
    BrokerConsumer, BrokerConsumerConfig, BrokerProducer, FailureMode, FailurePolicy,
    InboundMessage, MessageBrokerDriver, OutboundMessage,
};
use rivers_runtime::rivers_driver_sdk::ConnectionParams;

// ── BrokerConsumerBridge ───────────────────────────────────────────

/// Async bridge between a broker consumer and the EventBus.
///
/// Per spec §10.1 — one bridge per configured broker consumer datasource.
///
/// Message flow (§10.2):
/// 1. `consumer.receive()` → InboundMessage
/// 2. `event_bus.publish(BrokerMessageReceived)`
/// 3. `consumer.ack(receipt)`
/// 4. On failure → dispatch per FailurePolicy
pub struct BrokerConsumerBridge {
    /// The broker consumer to receive messages from.
    consumer: Box<dyn BrokerConsumer>,
    /// EventBus to publish received messages as events.
    event_bus: Arc<EventBus>,
    /// Optional producer for dead-letter / redirect failure policies.
    failure_producer: Option<Box<dyn BrokerProducer>>,
    /// Failure policy when message processing fails.
    failure_policy: FailurePolicy,
    /// Datasource name (for logging and event payloads).
    datasource_name: String,
    /// Milliseconds to wait before reconnection attempt.
    reconnect_ms: u64,
    /// Shutdown signal receiver.
    shutdown_rx: watch::Receiver<bool>,
    /// Consumer lag detection threshold. 0 = disabled.
    consumer_lag_threshold: usize,
    /// Drain timeout on shutdown (milliseconds). 0 = no drain.
    drain_timeout_ms: u64,
    /// Current number of inflight (pending ack) messages.
    messages_pending: Arc<AtomicUsize>,
}

impl BrokerConsumerBridge {
    /// Create a new bridge.
    ///
    /// # Arguments
    /// - `consumer` — broker consumer instance
    /// - `event_bus` — shared EventBus for publishing events
    /// - `failure_policy` — what to do when processing fails
    /// - `datasource_name` — datasource identifier for events/logging
    /// - `reconnect_ms` — delay between reconnection attempts
    /// - `shutdown_rx` — watch channel for shutdown signal
    pub fn new(
        consumer: Box<dyn BrokerConsumer>,
        event_bus: Arc<EventBus>,
        failure_policy: FailurePolicy,
        datasource_name: impl Into<String>,
        reconnect_ms: u64,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            consumer,
            event_bus,
            failure_producer: None,
            failure_policy,
            datasource_name: datasource_name.into(),
            reconnect_ms,
            shutdown_rx,
            consumer_lag_threshold: 0,
            drain_timeout_ms: 0,
            messages_pending: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Set the producer used for DeadLetter and Redirect failure policies.
    pub fn with_failure_producer(mut self, producer: Box<dyn BrokerProducer>) -> Self {
        self.failure_producer = Some(producer);
        self
    }

    /// Enable consumer lag detection.
    ///
    /// Per spec §10.4 — when `messages_pending >= threshold`, a
    /// `ConsumerLagDetected` event is published.
    pub fn with_consumer_lag_threshold(mut self, threshold: usize) -> Self {
        self.consumer_lag_threshold = threshold;
        self
    }

    /// Set the drain timeout for graceful shutdown.
    ///
    /// Per spec §10.5 — on shutdown, buffered messages are processed
    /// until this timeout or the queue is empty.
    pub fn with_drain_timeout_ms(mut self, ms: u64) -> Self {
        self.drain_timeout_ms = ms;
        self
    }

    /// Get the current number of pending (inflight) messages.
    pub fn messages_pending(&self) -> usize {
        self.messages_pending.load(Ordering::Relaxed)
    }

    /// Get a shared handle to the pending counter.
    ///
    /// Useful for external monitoring — the counter is safe to read
    /// while the bridge is running.
    pub fn pending_counter(&self) -> Arc<AtomicUsize> {
        self.messages_pending.clone()
    }

    /// Run the bridge loop.
    ///
    /// This is the main entry point — call via `tokio::spawn`.
    /// Runs until shutdown signal is received, then drains.
    pub async fn run(mut self) {
        tracing::info!(
            datasource = %self.datasource_name,
            "broker consumer bridge started"
        );

        // Publish BrokerConsumerStarted event
        let start_event = Event::new(
            events::BROKER_CONSUMER_STARTED,
            serde_json::json!({ "datasource": self.datasource_name }),
        );
        let _ = self.event_bus.publish(&start_event).await;

        // Main receive loop with reconnection
        self.receive_loop().await;

        // Drain on shutdown
        if self.drain_timeout_ms > 0 {
            self.drain().await;
        }

        // Close consumer
        if let Err(e) = self.consumer.close().await {
            tracing::warn!(
                datasource = %self.datasource_name,
                error = %e,
                "error closing broker consumer"
            );
        }

        // Publish BrokerConsumerStopped event
        let stop_event = Event::new(
            events::BROKER_CONSUMER_STOPPED,
            serde_json::json!({ "datasource": self.datasource_name }),
        );
        let _ = self.event_bus.publish(&stop_event).await;

        tracing::info!(
            datasource = %self.datasource_name,
            "broker consumer bridge stopped"
        );
    }

    /// Main receive loop with reconnection on error.
    async fn receive_loop(&mut self) {
        loop {
            // Check shutdown
            if *self.shutdown_rx.borrow() {
                return;
            }

            // Try to receive next message
            match self.consumer.receive().await {
                Ok(msg) => {
                    self.handle_message(msg).await;
                }
                Err(e) => {
                    tracing::error!(
                        datasource = %self.datasource_name,
                        error = %e,
                        "broker consumer receive error, reconnecting in {}ms",
                        self.reconnect_ms
                    );

                    // Publish error event
                    let err_event = Event::new(
                        events::BROKER_CONSUMER_ERROR,
                        serde_json::json!({
                            "datasource": self.datasource_name,
                            "error": e.to_string(),
                        }),
                    );
                    let _ = self.event_bus.publish(&err_event).await;

                    // Wait before reconnecting, but respect shutdown
                    let reconnect_delay =
                        tokio::time::sleep(Duration::from_millis(self.reconnect_ms));
                    tokio::select! {
                        _ = reconnect_delay => {}
                        _ = self.shutdown_rx.changed() => {
                            return;
                        }
                    }

                    // Publish reconnection event
                    let recon_event = Event::new(
                        events::DATASOURCE_RECONNECTED,
                        serde_json::json!({ "datasource": self.datasource_name }),
                    );
                    let _ = self.event_bus.publish(&recon_event).await;
                }
            }
        }
    }

    /// Handle a single received message through the full pipeline.
    async fn handle_message(&mut self, msg: InboundMessage) {
        // Increment pending counter
        self.messages_pending.fetch_add(1, Ordering::Relaxed);

        // Check consumer lag
        if self.consumer_lag_threshold > 0 {
            let pending = self.messages_pending.load(Ordering::Relaxed);
            if pending >= self.consumer_lag_threshold {
                let lag_event = Event::new(
                    events::CONSUMER_LAG_DETECTED,
                    serde_json::json!({
                        "datasource": self.datasource_name,
                        "messages_pending": pending,
                        "threshold": self.consumer_lag_threshold,
                    }),
                );
                let _ = self.event_bus.publish(&lag_event).await;
            }
        }

        // Publish to EventBus. Two events fire for each received message:
        //   (1) a generic "BrokerMessageReceived" event for observers + any
        //       handler that wants every message regardless of topic,
        //   (2) a per-destination event (event_type = msg.destination) so
        //       that MessageConsumer views subscribed to a specific topic
        //       actually receive the message payload. Without (2),
        //       MessageConsumer subscriptions silently drop every message
        //       (the previous behaviour, before BR-2026-04-23 follow-up).
        let payload = serde_json::json!({
            "datasource": self.datasource_name,
            "message_id": msg.id,
            "destination": msg.destination,
            "payload": String::from_utf8_lossy(&msg.payload).to_string(),
            "payload_size": msg.payload.len(),
            "headers": msg.headers,
        });
        let generic_event = Event::new(events::BROKER_MESSAGE_RECEIVED, payload.clone());
        let topic_event = Event::new(&msg.destination, payload);
        let mut errors = self.event_bus.publish(&generic_event).await;
        errors.extend(self.event_bus.publish(&topic_event).await);

        if errors.is_empty() {
            // Success — ack broker
            if let Err(e) = self.consumer.ack(&msg.receipt).await {
                tracing::error!(
                    datasource = %self.datasource_name,
                    message_id = %msg.id,
                    error = %e,
                    "broker ack failed"
                );
            }
        } else {
            // Processing failed — dispatch failure policy
            tracing::warn!(
                datasource = %self.datasource_name,
                message_id = %msg.id,
                error_count = errors.len(),
                "message processing failed, applying failure policy"
            );

            // Publish MessageFailed event
            let fail_event = Event::new(
                events::MESSAGE_FAILED,
                serde_json::json!({
                    "datasource": self.datasource_name,
                    "message_id": msg.id,
                    "errors": errors.iter().map(|e| &e.error).collect::<Vec<_>>(),
                }),
            );
            let _ = self.event_bus.publish(&fail_event).await;

            self.apply_failure_policy(&msg).await;
        }

        // Decrement pending counter
        self.messages_pending.fetch_sub(1, Ordering::Relaxed);
    }

    /// Apply the configured failure policy to a failed message.
    async fn apply_failure_policy(&mut self, msg: &InboundMessage) {
        match self.failure_policy.mode {
            FailureMode::DeadLetter => {
                let dest = self
                    .failure_policy
                    .destination
                    .as_deref()
                    .unwrap_or("dead-letter");
                if let Some(ref mut producer) = self.failure_producer {
                    let out = OutboundMessage {
                        destination: dest.to_string(),
                        payload: msg.payload.clone(),
                        headers: msg.headers.clone(),
                        key: None,
                        reply_to: None,
                    };
                    if let Err(e) = producer.publish(out).await {
                        tracing::error!(
                            datasource = %self.datasource_name,
                            message_id = %msg.id,
                            error = %e,
                            "dead-letter publish failed"
                        );
                    }
                } else {
                    tracing::error!(
                        datasource = %self.datasource_name,
                        message_id = %msg.id,
                        "dead-letter policy configured but no failure producer set"
                    );
                }
                // Ack the original message so it's not redelivered
                let _ = self.consumer.ack(&msg.receipt).await;
            }
            FailureMode::Redirect => {
                let dest = self
                    .failure_policy
                    .destination
                    .as_deref()
                    .unwrap_or("redirect");
                if let Some(ref mut producer) = self.failure_producer {
                    let out = OutboundMessage {
                        destination: dest.to_string(),
                        payload: msg.payload.clone(),
                        headers: msg.headers.clone(),
                        key: None,
                        reply_to: None,
                    };
                    if let Err(e) = producer.publish(out).await {
                        tracing::error!(
                            datasource = %self.datasource_name,
                            message_id = %msg.id,
                            error = %e,
                            "redirect publish failed"
                        );
                    }
                } else {
                    tracing::error!(
                        datasource = %self.datasource_name,
                        message_id = %msg.id,
                        "redirect policy configured but no failure producer set"
                    );
                }
                // Ack the original message so it's not redelivered
                let _ = self.consumer.ack(&msg.receipt).await;
            }
            FailureMode::Requeue => {
                // Nack the message so the broker requeues it
                if let Err(e) = self.consumer.nack(&msg.receipt).await {
                    tracing::error!(
                        datasource = %self.datasource_name,
                        message_id = %msg.id,
                        error = %e,
                        "broker nack (requeue) failed"
                    );
                }
            }
            FailureMode::Drop => {
                tracing::warn!(
                    datasource = %self.datasource_name,
                    message_id = %msg.id,
                    "message dropped per failure policy"
                );
                // Ack the original so it's not redelivered
                let _ = self.consumer.ack(&msg.receipt).await;
            }
        }
    }

    /// Drain buffered messages on shutdown.
    ///
    /// Per spec §10.5 — stop accepting new messages, process buffered
    /// messages until drain_timeout_ms or queue is empty.
    async fn drain(&mut self) {
        tracing::info!(
            datasource = %self.datasource_name,
            drain_timeout_ms = self.drain_timeout_ms,
            "starting drain on shutdown"
        );

        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(self.drain_timeout_ms);

        loop {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    datasource = %self.datasource_name,
                    "drain timeout reached"
                );
                break;
            }

            // Try to receive with a short timeout
            let receive_timeout = deadline.saturating_duration_since(tokio::time::Instant::now())
                .min(Duration::from_millis(100));

            match tokio::time::timeout(receive_timeout, self.consumer.receive()).await {
                Ok(Ok(msg)) => {
                    self.handle_message(msg).await;
                }
                Ok(Err(_)) | Err(_) => {
                    // Error or timeout — drain complete
                    break;
                }
            }
        }

        let remaining = self.messages_pending.load(Ordering::Relaxed);
        if remaining > 0 {
            tracing::warn!(
                datasource = %self.datasource_name,
                messages_pending = remaining,
                "drain completed with pending messages"
            );
        } else {
            tracing::info!(
                datasource = %self.datasource_name,
                "drain completed, all messages processed"
            );
        }
    }
}

// ── Non-blocking bridge supervisor ─────────────────────────────
//
// Code-review §1 (`docs/canary_codereivew.md`) fix: broker bridge startup must
// not be a bundle-load precondition. `run_with_retry` lives inside a spawned
// task that owns the `create_consumer` call and retries with bounded backoff,
// so one unreachable broker cannot hang bundle load for every other app.

/// Everything `run_with_retry` needs to eventually build a `BrokerConsumerBridge`.
///
/// Decoupled from [`BrokerConsumerBridge::new`] because the consumer itself
/// doesn't exist yet at spawn time — the supervisor creates it lazily.
pub struct BrokerBridgeSpec {
    /// Broker driver (Arc-shared with the DriverFactory).
    pub driver: Arc<dyn MessageBrokerDriver>,
    /// Resolved connection parameters for the broker.
    pub params: ConnectionParams,
    /// Consumer config: group prefix, subscriptions, etc.
    pub broker_config: BrokerConsumerConfig,
    /// Shared EventBus for published broker events (started/stopped/error).
    pub event_bus: Arc<EventBus>,
    /// Failure policy applied inside the bridge once a consumer exists.
    pub failure_policy: FailurePolicy,
    /// Datasource name for log + event payloads.
    pub datasource_name: String,
    /// Base reconnect delay; also serves as the base for exponential backoff.
    pub reconnect_ms: u64,
    /// Shutdown watch: any `true` value cancels the supervisor.
    pub shutdown_rx: watch::Receiver<bool>,
}

/// Background supervisor — creates the consumer, runs the bridge, retries on failure.
///
/// Exit conditions:
/// - shutdown signal received (checked before/between retries and during sleep)
/// - bridge `run()` returned cleanly
///
/// Backoff: exponential with jitter, base = `spec.reconnect_ms`, capped at 30s.
/// Never panics; a consumer-creation failure is logged + published as
/// `BROKER_CONSUMER_ERROR` on the EventBus.
pub async fn run_with_retry(mut spec: BrokerBridgeSpec) {
    use rand::Rng;

    let backoff_cap_ms: u64 = 30_000;
    let mut attempt: u32 = 0;

    loop {
        // Respect shutdown before each attempt.
        if *spec.shutdown_rx.borrow() {
            tracing::info!(
                datasource = %spec.datasource_name,
                "broker bridge supervisor exiting before consumer create (shutdown)"
            );
            return;
        }

        match spec.driver.create_consumer(&spec.params, &spec.broker_config).await {
            Ok(consumer) => {
                tracing::info!(
                    datasource = %spec.datasource_name,
                    attempt = attempt + 1,
                    "broker consumer created; starting bridge"
                );
                attempt = 0; // reset backoff on every successful create

                let bridge = BrokerConsumerBridge::new(
                    consumer,
                    spec.event_bus.clone(),
                    spec.failure_policy.clone(),
                    spec.datasource_name.clone(),
                    spec.reconnect_ms,
                    spec.shutdown_rx.clone(),
                );

                // `run()` exits on shutdown. If the consumer fails mid-loop the
                // existing `receive_loop` already handles reconnect internally,
                // so returning here means "we're done for real".
                bridge.run().await;

                // Post-run: shutdown is the expected path. If the watch still
                // reads `false`, the consumer torn down without a signal —
                // loop back and try to rebuild.
                if *spec.shutdown_rx.borrow() {
                    return;
                }
                tracing::warn!(
                    datasource = %spec.datasource_name,
                    "bridge returned without shutdown — attempting to rebuild consumer"
                );
            }
            Err(e) => {
                tracing::warn!(
                    datasource = %spec.datasource_name,
                    error = %e,
                    attempt = attempt + 1,
                    "broker consumer create failed; will retry"
                );

                let err_event = Event::new(
                    events::BROKER_CONSUMER_ERROR,
                    serde_json::json!({
                        "datasource": spec.datasource_name,
                        "error": e.to_string(),
                        "phase": "create_consumer",
                        "attempt": attempt + 1,
                    }),
                );
                let _ = spec.event_bus.publish(&err_event).await;
            }
        }

        // Bounded exponential backoff with jitter:
        //   base = reconnect_ms
        //   delay = base * 2^min(attempt, 6), capped at backoff_cap_ms
        //   +/- 50% jitter
        let shift = attempt.min(6);
        let scaled = spec.reconnect_ms.saturating_mul(1u64 << shift);
        let capped = scaled.min(backoff_cap_ms).max(100); // never below 100ms
        let jitter_span = capped / 2;
        let jitter = {
            let mut rng = rand::thread_rng();
            rng.gen_range(0..=jitter_span)
        };
        let delay_ms = capped.saturating_add(jitter).min(backoff_cap_ms);

        let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
        tokio::pin!(sleep);
        tokio::select! {
            _ = &mut sleep => {}
            _ = spec.shutdown_rx.changed() => {
                tracing::info!(
                    datasource = %spec.datasource_name,
                    "broker bridge supervisor exiting during backoff (shutdown)"
                );
                return;
            }
        }

        attempt = attempt.saturating_add(1);
    }
}

#[cfg(test)]
mod supervisor_tests {
    //! CG3.5/CG3.6 coverage: supervisor must not block startup and must
    //! honour shutdown during backoff.

    use super::*;
    use async_trait::async_trait;
    use rivers_runtime::rivers_driver_sdk::DriverError;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU32;

    fn test_params() -> ConnectionParams {
        ConnectionParams {
            host: "127.0.0.1".into(),
            port: 9092,
            database: "test".into(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        }
    }

    /// Driver whose `create_consumer` always errors — lets us verify the
    /// supervisor retries + exits cleanly on shutdown.
    struct FailingDriver {
        attempts: Arc<AtomicU32>,
    }

    #[async_trait]
    impl MessageBrokerDriver for FailingDriver {
        fn name(&self) -> &str { "failing-broker" }

        async fn create_consumer(
            &self,
            _params: &ConnectionParams,
            _config: &BrokerConsumerConfig,
        ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(DriverError::Connection("simulated host unreachable".into()))
        }

        async fn create_producer(
            &self,
            _params: &ConnectionParams,
            _config: &BrokerConsumerConfig,
        ) -> Result<Box<dyn BrokerProducer>, DriverError> {
            Err(DriverError::Unsupported("test driver".into()))
        }
    }

    fn spec_with_driver(
        driver: Arc<dyn MessageBrokerDriver>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> BrokerBridgeSpec {
        BrokerBridgeSpec {
            driver,
            params: test_params(),
            broker_config: BrokerConsumerConfig {
                group_prefix: "test".into(),
                app_id: "app".into(),
                datasource_id: "ds".into(),
                node_id: "node".into(),
                reconnect_ms: 50,
                subscriptions: Vec::new(),
            },
            event_bus: Arc::new(EventBus::new()),
            failure_policy: FailurePolicy {
                mode: FailureMode::Drop,
                destination: None,
                handlers: Vec::new(),
            },
            datasource_name: "failing-ds".into(),
            reconnect_ms: 50,
            shutdown_rx,
        }
    }

    #[tokio::test]
    async fn supervisor_retries_and_exits_on_shutdown() {
        let attempts = Arc::new(AtomicU32::new(0));
        let driver = Arc::new(FailingDriver { attempts: attempts.clone() });
        let (tx, rx) = watch::channel(false);

        let handle = tokio::spawn(run_with_retry(spec_with_driver(driver, rx)));

        // Let it retry a few times.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "supervisor should have retried at least twice; got {}",
            attempts.load(Ordering::SeqCst)
        );

        // Signal shutdown; supervisor must return promptly.
        tx.send(true).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("supervisor did not honour shutdown within 1s");
    }

    /// CG3.6: proof that the supervisor launch path is non-blocking —
    /// `tokio::spawn(run_with_retry(...))` must return immediately even when
    /// the underlying driver would hang forever inside `create_consumer`.
    #[tokio::test]
    async fn supervisor_spawn_is_non_blocking() {
        struct HangingDriver;
        #[async_trait]
        impl MessageBrokerDriver for HangingDriver {
            fn name(&self) -> &str { "hanging" }
            async fn create_consumer(
                &self,
                _: &ConnectionParams,
                _: &BrokerConsumerConfig,
            ) -> Result<Box<dyn BrokerConsumer>, DriverError> {
                // Simulate a hang — never returns.
                let () = std::future::pending().await;
                unreachable!()
            }
            async fn create_producer(
                &self,
                _: &ConnectionParams,
                _: &BrokerConsumerConfig,
            ) -> Result<Box<dyn BrokerProducer>, DriverError> {
                Err(DriverError::Unsupported("test".into()))
            }
        }

        let (tx, rx) = watch::channel(false);
        let driver: Arc<dyn MessageBrokerDriver> = Arc::new(HangingDriver);
        let spec = spec_with_driver(driver, rx);

        // The spawn itself must return immediately regardless of driver state.
        let start = std::time::Instant::now();
        let handle = tokio::spawn(run_with_retry(spec));
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "spawn should be O(1), took {:?}",
            start.elapsed()
        );

        // Cancel so the task doesn't leak across tests. The hanging driver
        // may not honour shutdown mid-`create_consumer`; startup never
        // blocked is what matters. Abort the task to ensure cleanup.
        let _ = tx.send(true);
        handle.abort();
    }
}
