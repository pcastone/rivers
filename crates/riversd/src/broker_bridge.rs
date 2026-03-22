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
    BrokerConsumer, BrokerProducer, FailureMode, FailurePolicy, InboundMessage, OutboundMessage,
};

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

        // Publish to EventBus
        let event = Event::new(
            events::BROKER_MESSAGE_RECEIVED,
            serde_json::json!({
                "datasource": self.datasource_name,
                "message_id": msg.id,
                "destination": msg.destination,
                "payload_size": msg.payload.len(),
            }),
        );
        let errors = self.event_bus.publish(&event).await;

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
