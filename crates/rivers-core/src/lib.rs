//! Rivers Core — driver registry, EventBus, storage factory, logging, and TLS.
//!
//! This crate wires together the lightweight types from `rivers-core-config`
//! with runtime infrastructure: plugin loading, event dispatch, structured
//! logging, sentinel key management, and certificate generation. Everything
//! that `riversd` and `riversctl` share but that doesn't belong in the thin
//! config crate lives here.

#![warn(missing_docs)]

// Re-export everything from rivers-core-config for backward compat
pub use rivers_core_config::config;
pub use rivers_core_config::error;
pub use rivers_core_config::event;
pub use rivers_core_config::lockbox_config;

/// Per-application log file routing.
pub mod app_log_router;
/// Driver registry and plugin loading.
pub mod driver_factory;
/// Built-in driver re-exports (feature-gated).
#[cfg(feature = "drivers")]
pub mod drivers;
/// In-process pub/sub EventBus with priority-tiered dispatch.
pub mod eventbus;
/// LockBox re-exports (feature-gated).
#[cfg(feature = "lockbox")]
pub mod lockbox;
/// Structured logging via EventBus.
pub mod logging;
/// Storage factory, sentinel keys, and sweep tasks.
pub mod storage;
/// TLS certificate generation and inspection.
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
