//! Rivers Core Config — lightweight configuration and storage types.
//!
//! This crate contains config structs, error types, event types, and the
//! StorageEngine trait + InMemoryStorageEngine. No database drivers, no
//! encryption, no network I/O beyond async primitives.
//! Used by CLI tools and storage backend crates that need these types
//! without pulling in the full rivers-core dependency tree.

#![warn(missing_docs)]

/// Server configuration types mapped from `riversd.conf`.
pub mod config;
/// Top-level error type ([`RiversError`]).
pub mod error;
/// EventBus event types and log severity levels.
pub mod event;
/// LockBox configuration (no encryption dependencies).
pub mod lockbox_config;
/// Internal KV storage trait and in-memory backend.
pub mod storage;

pub use config::ServerConfig;
pub use config::{server_config_schema, GraphqlServerConfig, TlsConfig, TlsX509Config, TlsEngineConfig, AdminTlsConfig};
pub use config::{EnginesConfig, PluginsConfig};
pub use error::RiversError;
pub use event::{Event, LogLevel};
pub use lockbox_config::LockBoxConfig;
pub use storage::{InMemoryStorageEngine, StorageEngine, StorageError};
