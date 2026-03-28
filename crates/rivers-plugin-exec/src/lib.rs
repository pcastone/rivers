//! ExecDriver plugin — controlled invocation of admin-declared external commands.
//!
//! Implements `DatabaseDriver` from `rivers-driver-sdk`. Handlers invoke commands
//! via the standard `Rivers.view.query("datasource", { command, args })` pattern.
//! The driver enforces an 11-step pipeline: command lookup, schema validation,
//! SHA-256 integrity check, semaphore acquisition, process spawn (privilege-dropped,
//! env-controlled), bounded I/O with timeout, JSON result parsing.

pub mod config;

use async_trait::async_trait;
use rivers_driver_sdk::{Connection, ConnectionParams, DatabaseDriver, DriverError};
#[cfg(feature = "plugin-exports")]
use rivers_driver_sdk::{ABI_VERSION, DriverRegistrar};
#[cfg(feature = "plugin-exports")]
use std::sync::Arc;

// ── Driver ─────────────────────────────────────────────────────────────

pub struct ExecDriver;

// Placeholder — full implementation in Task 7
#[async_trait]
impl DatabaseDriver for ExecDriver {
    fn name(&self) -> &str {
        "rivers-exec"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Err(DriverError::NotImplemented(
            "exec driver connect not yet implemented".into(),
        ))
    }
}

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
    registrar.register_database_driver(Arc::new(ExecDriver));
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::ABI_VERSION;

    #[test]
    fn driver_name_is_rivers_exec() {
        let driver = ExecDriver;
        assert_eq!(driver.name(), "rivers-exec");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }

    #[tokio::test]
    async fn connect_returns_not_implemented() {
        let driver = ExecDriver;
        let params = ConnectionParams {
            host: "localhost".into(),
            port: 0,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options: std::collections::HashMap::new(),
        };
        let result = driver.connect(&params).await;
        match result {
            Err(DriverError::NotImplemented(msg)) => {
                assert!(msg.contains("not yet implemented"));
            }
            Err(other) => panic!("expected NotImplemented, got error: {other}"),
            Ok(_) => panic!("expected NotImplemented error, got Ok"),
        }
    }
}
