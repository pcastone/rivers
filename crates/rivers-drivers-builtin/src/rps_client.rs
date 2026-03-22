//! RpsClientDriver — stub for Rivers Provisioning Service client.
//!
//! Per `rivers-driver-spec.md` §5:
//! - Operations: `get_secret`, `validate_token`, `health`, `ping`
//! - All requests authenticated via PASETO token
//! - Always mTLS
//!
//! This driver will be implemented when RPS ships (v2).

use async_trait::async_trait;
use rivers_driver_sdk::{Connection, ConnectionParams, DatabaseDriver, DriverError};

/// Stub RPS client driver. Returns `NotImplemented` on connect.
/// Will be implemented when the RPS service is available.
pub struct RpsClientDriver;

#[async_trait]
impl DatabaseDriver for RpsClientDriver {
    fn name(&self) -> &str {
        "rps-client"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Err(DriverError::NotImplemented(
            "rps-client driver not yet implemented (requires RPS v2)".to_string(),
        ))
    }
}
