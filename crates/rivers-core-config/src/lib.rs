//! Rivers Core Config — lightweight configuration and storage types.
//!
//! This crate contains config structs, error types, event types, and the
//! StorageEngine trait + InMemoryStorageEngine. No database drivers, no
//! encryption, no network I/O beyond async primitives.
//! Used by CLI tools and storage backend crates that need these types
//! without pulling in the full rivers-core dependency tree.

pub mod config;
pub mod error;
pub mod event;
pub mod lockbox_config;
pub mod storage;

pub use config::ServerConfig;
pub use config::{server_config_schema, GraphqlServerConfig, TlsConfig, TlsX509Config, TlsEngineConfig, AdminTlsConfig};
pub use config::{EnginesConfig, PluginsConfig};
pub use error::RiversError;
pub use event::{Event, LogLevel};
pub use lockbox_config::LockBoxConfig;
pub use storage::{InMemoryStorageEngine, StorageEngine, StorageError};
