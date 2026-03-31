//! Rivers Driver SDK — trait contracts for database, broker, and HTTP drivers.
//!
//! This crate defines the interfaces that all Rivers drivers must implement.
//! It contains three independent driver contracts:
//!
//! - [`DatabaseDriver`] / [`Connection`] — request/response drivers for
//!   relational databases, key-value stores, and search engines.
//! - [`MessageBrokerDriver`] / [`BrokerConsumer`] / [`BrokerProducer`] —
//!   continuous-push drivers for Kafka, RabbitMQ, NATS, and Redis Streams.
//! - [`HttpDriver`](http_driver::HttpDriver) / [`HttpConnection`](http_driver::HttpConnection) —
//!   HTTP/HTTP2/SSE/WebSocket as a first-class datasource.
//!
//! Plugin crates (cdylib) depend on this SDK to implement their driver and
//! register it via [`DriverRegistrar`] at load time.

#![warn(missing_docs)]

use std::sync::Arc;

/// Message broker driver contracts — Kafka, RabbitMQ, NATS, Redis Streams.
pub mod broker;
/// Driver error types.
pub mod error;
/// HTTP driver contracts — HTTP/HTTP2/SSE/WebSocket as a datasource.
pub mod http_driver;
/// Reqwest-based HTTP driver implementation with retry and circuit breaker.
pub mod http_executor;
/// HTTP schema syntax and data validation.
pub mod http_validation;
/// Core driver traits — [`DatabaseDriver`], [`Connection`], [`Driver`], and schema types.
pub mod traits;
/// Query model, result types, and operation classification.
pub mod types;
/// Shared schema validation engine for field types and constraints.
pub mod validation;

pub use broker::{
    BrokerConsumer, BrokerConsumerConfig, BrokerMetadata, BrokerProducer, BrokerSubscription,
    FailureMode, FailurePolicy, InboundMessage, MessageBrokerDriver, MessageReceipt,
    OutboundMessage, PublishReceipt,
};
pub use error::DriverError;
pub use traits::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverType, HttpMethod,
    SchemaDefinition, SchemaFieldDef, SchemaSyntaxError, ValidationDirection, ValidationError,
};
pub use types::{classify_operation, infer_operation, OperationCategory, Query, QueryResult, QueryValue};

/// ABI version for plugin compatibility checks.
///
/// Per spec §7.2 — plugins must export `_rivers_abi_version()` returning this value.
pub const ABI_VERSION: u32 = 1;

/// Trait for plugin registration callbacks.
///
/// Per spec §7.4. Plugins call methods on this trait to register
/// their driver implementations. `DriverFactory` in `rivers-core`
/// implements this trait.
pub trait DriverRegistrar {
    /// Register a database driver implementation.
    fn register_database_driver(&mut self, driver: Arc<dyn DatabaseDriver>);
    /// Register a message broker driver implementation.
    fn register_broker_driver(&mut self, driver: Arc<dyn MessageBrokerDriver>);
}
