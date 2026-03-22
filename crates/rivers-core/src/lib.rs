// Re-export everything from rivers-core-config for backward compat
pub use rivers_core_config::config;
pub use rivers_core_config::error;
pub use rivers_core_config::event;
pub use rivers_core_config::lockbox_config;

pub mod driver_factory;
#[cfg(feature = "drivers")]
pub mod drivers;
pub mod eventbus;
#[cfg(feature = "lockbox")]
pub mod lockbox;
pub mod logging;
pub mod storage;
#[cfg(feature = "tls")]
pub mod tls;

// Re-exports from rivers-core-config (backward compat)
pub use rivers_core_config::{
    ServerConfig, server_config_schema, GraphqlServerConfig,
    TlsConfig, TlsX509Config, TlsEngineConfig, AdminTlsConfig,
    EnginesConfig, PluginsConfig,
    RiversError, Event, LogLevel, LockBoxConfig,
};

// Re-exports from this crate
pub use driver_factory::{DriverFactory, DriverRegistrar, EventNotifier};
#[cfg(feature = "drivers")]
pub use drivers::register_builtin_drivers;
pub use eventbus::{EventBus, EventHandler, GossipConfig, GossipMessage, HandlerPriority};
#[cfg(feature = "lockbox")]
pub use lockbox::LockBoxResolver;
pub use logging::LogHandler;
pub use storage::{create_storage_engine, InMemoryStorageEngine, StorageEngine, StorageError};
#[cfg(feature = "storage-backends")]
pub use rivers_storage_backends::RedisStorageEngine;
#[cfg(feature = "storage-backends")]
pub use rivers_storage_backends::SqliteStorageEngine;
