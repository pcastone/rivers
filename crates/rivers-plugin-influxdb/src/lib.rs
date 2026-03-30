//! InfluxDB v2 plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using `reqwest` for direct HTTP API calls.
//! InfluxDB v2 uses a REST API with Flux queries and line protocol writes.
//!
//! Operations dispatch based on `query.operation`:
//! - query/select/find -> POST /api/v2/query (Flux query from statement)
//! - write/insert -> POST /api/v2/write (line protocol from parameters)
//! - ping -> GET /ping

mod protocol;
mod connection;
mod batching;
mod driver;

pub use connection::InfluxConnection;
pub use driver::InfluxDriver;

#[cfg(feature = "plugin-exports")]
use std::sync::Arc;

#[cfg(feature = "plugin-exports")]
use rivers_driver_sdk::{DriverRegistrar, ABI_VERSION};

// ── Plugin ABI ─────────────────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(InfluxDriver));
}

#[cfg(test)]
mod tests {
    use rivers_driver_sdk::ABI_VERSION;

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }
}
