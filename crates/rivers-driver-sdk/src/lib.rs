use std::sync::Arc;

pub mod broker;
pub mod error;
pub mod http_driver;
pub mod http_executor;
pub mod http_validation;
pub mod traits;
pub mod types;
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
