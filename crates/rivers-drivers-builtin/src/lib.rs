//! Built-in database drivers for Rivers.
//!
//! This crate bundles the core database drivers (postgres, mysql, sqlite,
//! redis, memcached, faker, eventbus, rps-client) as a cdylib plugin.
//! It can be compiled statically into riversd or loaded at runtime from
//! `lib/librivers_drivers_builtin.dylib`.

#![warn(missing_docs)]

mod eventbus;
mod faker;
pub mod filesystem;
#[cfg(unix)]
mod memcached;
mod mysql;
mod postgres;
mod redis;
mod rps_client;
mod sqlite;

pub use self::eventbus::{EventBusConnection, EventBusDriver, EventBusPublisher};
pub use self::faker::{FakerConnection, FakerDriver};
pub use self::filesystem::{FilesystemConnection, FilesystemDriver};
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
    registrar.register_database_driver(Arc::new(FilesystemDriver));
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

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod registration_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CaptureRegistrar {
        names: Arc<Mutex<Vec<String>>>,
    }

    impl DriverRegistrar for CaptureRegistrar {
        fn register_database_driver(&mut self, driver: Arc<dyn rivers_driver_sdk::DatabaseDriver>) {
            self.names.lock().unwrap().push(driver.name().to_string());
        }

        fn register_broker_driver(&mut self, _driver: Arc<dyn rivers_driver_sdk::MessageBrokerDriver>) {
            // no-op for this test
        }
    }

    #[test]
    fn filesystem_driver_is_registered() {
        let mut reg = CaptureRegistrar::default();
        register_builtin_drivers(&mut reg);
        let names = reg.names.lock().unwrap().clone();
        assert!(
            names.iter().any(|n| n == "filesystem"),
            "expected 'filesystem' in registered driver names: {names:?}"
        );
    }
}
