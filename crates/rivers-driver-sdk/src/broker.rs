//! Message broker driver contracts.
//!
//! Per `rivers-driver-spec.md` §6 and `rivers-data-layer-spec.md` §3.
//!
//! Broker drivers are stateless factories like database drivers, but
//! produce `BrokerProducer` and `BrokerConsumer` instances instead of
//! `Connection` instances.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::DriverError;
use crate::traits::ConnectionParams;

// ── Message Types ───────────────────────────────────────────────────

/// Opaque message ID. String-typed for cross-driver compatibility.
pub type MessageId = String;

/// Opaque receipt for ack/nack operations.
///
/// Drivers store their native receipt handle inside.
/// The bridge passes this back to `ack()` / `nack()` without inspecting it.
#[derive(Debug, Clone)]
pub struct MessageReceipt {
    /// Driver-internal receipt data (delivery tag, stream ID, etc.).
    pub handle: String,
}

/// Receipt returned after a successful publish.
#[derive(Debug, Clone)]
pub struct PublishReceipt {
    /// The message ID assigned by the broker, if available.
    pub id: Option<String>,
    /// Broker-specific confirmation data.
    pub metadata: Option<String>,
}

/// An inbound message received from a broker.
///
/// Per spec §3.1.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Unique message ID.
    pub id: MessageId,
    /// Queue/topic/subject/stream name.
    pub destination: String,
    /// Message payload bytes.
    pub payload: Vec<u8>,
    /// Message headers/properties.
    pub headers: HashMap<String, String>,
    /// Message timestamp.
    pub timestamp: DateTime<Utc>,
    /// Opaque receipt for ack/nack.
    pub receipt: MessageReceipt,
    /// Broker-specific metadata envelope.
    pub metadata: BrokerMetadata,
}

/// An outbound message to publish to a broker.
///
/// Per spec §3.3.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    /// Target queue/topic/subject/stream.
    pub destination: String,
    /// Message payload bytes.
    pub payload: Vec<u8>,
    /// Message headers/properties.
    pub headers: HashMap<String, String>,
    /// Partition key (Kafka) or subject suffix (NATS).
    pub key: Option<String>,
    /// Reply-to address (NATS request/reply).
    pub reply_to: Option<String>,
}

// ── BrokerMetadata ──────────────────────────────────────────────────

/// Broker-specific message envelope metadata.
///
/// Per spec §3.2. Variant is determined by the driver.
#[derive(Debug, Clone)]
pub enum BrokerMetadata {
    /// Kafka-specific metadata.
    Kafka {
        /// Partition the message was consumed from.
        partition: i32,
        /// Offset within the partition.
        offset: i64,
        /// Consumer group ID.
        consumer_group: String,
    },
    /// RabbitMQ-specific metadata.
    Rabbit {
        /// AMQP delivery tag for ack/nack.
        delivery_tag: u64,
        /// Exchange the message was published to.
        exchange: String,
        /// Routing key used for delivery.
        routing_key: String,
    },
    /// NATS JetStream-specific metadata.
    Nats {
        /// Stream sequence number.
        sequence: u64,
        /// JetStream stream name.
        stream: String,
        /// Consumer name.
        consumer: String,
    },
    /// Redis Streams-specific metadata.
    Redis {
        /// Stream entry ID (e.g. `"1234567890-0"`).
        stream_id: String,
        /// Consumer group name.
        group: String,
        /// Consumer name within the group.
        consumer: String,
    },
}

// ── ConsumerConfig ──────────────────────────────────────────────────

/// SDK-level consumer configuration passed to broker driver factory methods.
///
/// Per spec §3.7.
/// Consumer group ID is derived: `{group_prefix}.{app_id}.{datasource_id}.{component}`.
#[derive(Debug, Clone)]
pub struct BrokerConsumerConfig {
    /// Prefix for the derived consumer group ID.
    pub group_prefix: String,
    /// Application identifier (used in group ID derivation).
    pub app_id: String,
    /// Datasource identifier (used in group ID derivation).
    pub datasource_id: String,
    /// Node identifier for this Rivers instance.
    pub node_id: String,
    /// Delay in milliseconds before reconnecting after a disconnect.
    pub reconnect_ms: u64,
    /// Topics/queues/subjects to subscribe to.
    pub subscriptions: Vec<BrokerSubscription>,
}

/// A single broker subscription target.
#[derive(Debug, Clone)]
pub struct BrokerSubscription {
    /// Topic/queue/subject/stream name.
    pub topic: String,
    /// Event name to publish on the EventBus when a message is received.
    pub event_name: Option<String>,
}

// ── FailurePolicy ───────────────────────────────────────────────────

/// Failure disposition after all retries are exhausted.
///
/// Per spec §3.8.
#[derive(Debug, Clone)]
pub struct FailurePolicy {
    /// How to dispose of the failed message.
    pub mode: FailureMode,
    /// Dead-letter or redirect target name.
    pub destination: Option<String>,
    /// CodeComponent handlers invoked fire-and-forget before disposition.
    pub handlers: Vec<FailurePolicyHandler>,
}

/// What happens when message processing fails.
///
/// Per spec §3.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureMode {
    /// Route to a dead-letter destination datasource.
    DeadLetter,
    /// Return to source broker (requeue).
    Requeue,
    /// Publish to a different topic/queue.
    Redirect,
    /// Discard silently.
    Drop,
}

/// A CodeComponent handler invoked on message failure.
#[derive(Debug, Clone)]
pub struct FailurePolicyHandler {
    /// CodeComponent module path.
    pub module: String,
}

// ── Broker Semantics Contract ───────────────────────────────────────

/// Delivery semantics guaranteed by a broker driver.
///
/// Per spec §3.4: each driver declares which semantics it can honor.
/// The BrokerConsumerBridge uses this to decide whether to call
/// `ack()`/`nack()` or treat them as no-ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerSemantics {
    /// Ack/nack are honored; an un-acked (nacked) message will be redelivered.
    ///
    /// Examples: Kafka (Rivers-managed offset), RabbitMQ (AMQP basic.nack + requeue),
    /// Redis Streams (PEL + XAUTOCLAIM).
    AtLeastOnce,
    /// No ack/nack tracking; the broker delivers each message at most once.
    ///
    /// Example: NATS core pub/sub (fire-and-forget to all current subscribers).
    AtMostOnce,
    /// Neither ack nor redelivery is tracked — messages are consumed and gone.
    ///
    /// Reserved for future use.
    FireAndForget,
}

/// Outcome of a successful `ack()` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOutcome {
    /// The message was acknowledged for the first time.
    Acked,
    /// The message had already been acknowledged (idempotent re-ack).
    AlreadyAcked,
}

/// Broker-level error distinct from transport-level [`DriverError`].
///
/// Returned by `ack()` and `nack()` on `BrokerConsumer`.
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    /// This driver cannot honor the requested operation (e.g., nack on NATS core).
    #[error("broker operation not supported by this driver")]
    Unsupported,
    /// A network or protocol-level transport failure.
    #[error("broker transport error: {0}")]
    Transport(String),
    /// A broker protocol-level error (unexpected response, framing, etc.).
    #[error("broker protocol error: {0}")]
    Protocol(String),
}

// ── Broker Traits ───────────────────────────────────────────────────

/// A named, stateless factory that creates broker producer and consumer instances.
///
/// Per spec §3.4.
/// Broker drivers that also implement `DatabaseDriver` register under both registries.
#[async_trait]
pub trait MessageBrokerDriver: Send + Sync {
    /// Unique name for this broker driver (e.g. "kafka", "rabbitmq", "nats").
    fn name(&self) -> &str;

    /// Delivery semantics this driver can guarantee.
    ///
    /// The BrokerConsumerBridge uses this to decide how to handle
    /// `ack()`/`nack()` results. Defaults to `AtLeastOnce` for backward
    /// compatibility; drivers that cannot honor redelivery override this.
    fn semantics(&self) -> BrokerSemantics {
        BrokerSemantics::AtLeastOnce
    }

    /// Create a new producer instance.
    async fn create_producer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerProducer>, DriverError>;

    /// Create a new consumer instance.
    async fn create_consumer(
        &self,
        params: &ConnectionParams,
        config: &BrokerConsumerConfig,
    ) -> Result<Box<dyn BrokerConsumer>, DriverError>;
}

/// A continuous consumer that receives messages from a broker.
///
/// Per spec §3.5.
/// Owned by BrokerConsumerBridge — one consumer per datasource subscription.
#[async_trait]
pub trait BrokerConsumer: Send + Sync {
    /// Receive the next message. Blocks until a message is available.
    async fn receive(&mut self) -> Result<InboundMessage, DriverError>;

    /// Acknowledge successful processing of a message.
    ///
    /// Returns `Ok(AckOutcome::Acked)` on first ack, `Ok(AckOutcome::AlreadyAcked)`
    /// if the message was already acknowledged (idempotent).
    /// Returns `Err(BrokerError::Unsupported)` if the driver's semantics do not
    /// support acknowledgement (e.g., `AtMostOnce` drivers).
    async fn ack(&mut self, receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError>;

    /// Negatively acknowledge a message (reject/requeue).
    ///
    /// Drivers that cannot honor redelivery (e.g., NATS core) MUST return
    /// `Err(BrokerError::Unsupported)` rather than `Ok(())`.
    async fn nack(&mut self, receipt: &MessageReceipt) -> Result<AckOutcome, BrokerError>;

    /// Close the consumer gracefully.
    async fn close(&mut self) -> Result<(), DriverError>;
}

/// A producer that publishes messages to a broker.
///
/// Per spec §3.6.
#[async_trait]
pub trait BrokerProducer: Send + Sync {
    /// Publish a message. Returns a receipt on success.
    async fn publish(&mut self, message: OutboundMessage) -> Result<PublishReceipt, DriverError>;

    /// Close the producer gracefully.
    async fn close(&mut self) -> Result<(), DriverError>;
}
