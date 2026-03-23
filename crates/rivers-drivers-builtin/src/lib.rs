//! Built-in database drivers for Rivers.
//!
//! This crate bundles the core database drivers (postgres, mysql, sqlite,
//! redis, memcached, faker, eventbus, rps-client) as a cdylib plugin.
//! It can be compiled statically into riversd or loaded at runtime from
//! `lib/librivers_drivers_builtin.dylib`.

mod eventbus;
mod faker;
#[cfg(unix)]
mod memcached;
mod mysql;
mod postgres;
mod redis;
mod rps_client;
mod sqlite;

pub use self::eventbus::{EventBusConnection, EventBusDriver, EventBusPublisher};
pub use self::faker::{FakerConnection, FakerDriver};
#[cfg(unix)]
pub use self::memcached::{MemcachedConnection, MemcachedDriver};
pub use self::mysql::{MysqlConnection, MysqlDriver};
pub use self::postgres::{PostgresConnection, PostgresDriver};
pub use self::redis::RedisDriver;
pub use self::rps_client::RpsClientDriver;
pub use self::sqlite::SqliteDriver;

use std::sync::Arc;

use rivers_driver_sdk::DriverRegistrar;

/// Register all built-in drivers into the given registrar.
pub fn register_builtin_drivers(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(FakerDriver::new()));
    registrar.register_database_driver(Arc::new(PostgresDriver));
    registrar.register_database_driver(Arc::new(MysqlDriver));
    registrar.register_database_driver(Arc::new(SqliteDriver));
    registrar.register_database_driver(Arc::new(RedisDriver));
    #[cfg(unix)]
    registrar.register_database_driver(Arc::new(MemcachedDriver));
    registrar.register_database_driver(Arc::new(RpsClientDriver));
    registrar.register_database_driver(Arc::new(EventBusDriver::new()));
}

// ── cdylib ABI exports ──────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    rivers_driver_sdk::ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    register_builtin_drivers(registrar);
}
