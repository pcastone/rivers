//! InfluxDB driver — implements `DatabaseDriver` for connection creation.

use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::debug;

use rivers_driver_sdk::{read_connect_timeout, read_request_timeout, Connection, ConnectionParams, DatabaseDriver, DriverError};

use crate::batching::BatchingInfluxConnection;
use crate::connection::InfluxConnection;

// ── Driver ─────────────────────────────────────────────────────────────

/// InfluxDB v2 driver factory — creates connections via HTTP API.
pub struct InfluxDriver;

#[async_trait]
impl DatabaseDriver for InfluxDriver {
    fn name(&self) -> &str {
        "influxdb"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let scheme = params
            .options
            .get("scheme")
            .map(|s| s.as_str())
            .unwrap_or("http");
        let base_url = format!("{}://{}:{}", scheme, params.host, params.port);
        let token = params.password.clone(); // InfluxDB uses API tokens via password field.
        let org = params.options.get("org").cloned().unwrap_or_default();

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(read_connect_timeout(params)))
            .timeout(Duration::from_secs(read_request_timeout(params)))
            .build()
            .map_err(|e| DriverError::Connection(format!("influxdb client build failed: {e}")))?;

        // Verify connectivity with GET /ping
        let resp = client
            .get(format!("{}/ping", base_url))
            .send()
            .await
            .map_err(|e| DriverError::Connection(format!("influxdb ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Connection(format!(
                "influxdb ping returned status {}",
                resp.status()
            )));
        }

        // Read write_batch config from connection options
        let batch_enabled = params
            .options
            .get("write_batch_enabled")
            .map(|v| v == "true")
            .unwrap_or(false);
        let batch_max_size = params
            .options
            .get("write_batch_max_size")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1000);
        let batch_flush_ms = params
            .options
            .get("write_batch_flush_interval_ms")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1000);

        if batch_enabled {
            debug!(
                base_url = %base_url,
                org = %org,
                batch_max_size = batch_max_size,
                batch_flush_ms = batch_flush_ms,
                "influxdb: connected (write batching enabled)"
            );

            Ok(Box::new(BatchingInfluxConnection {
                inner: InfluxConnection {
                    client,
                    base_url,
                    org,
                    token,
                },
                buffer: Mutex::new(Vec::with_capacity(batch_max_size)),
                max_size: batch_max_size,
                flush_interval_ms: batch_flush_ms,
                last_flush: Mutex::new(std::time::Instant::now()),
            }))
        } else {
            debug!(
                base_url = %base_url,
                org = %org,
                "influxdb: connected"
            );

            Ok(Box::new(InfluxConnection {
                client,
                base_url,
                org,
                token,
            }))
        }
    }

    /// G_R7.2: cdylib plugin runs connect() in an isolated runtime.
    fn needs_isolated_runtime(&self) -> bool { true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn bad_params() -> ConnectionParams {
        ConnectionParams {
            host: "127.0.0.1".into(),
            port: 1,
            database: "test".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        }
    }

    #[test]
    fn driver_name_is_influxdb() {
        let driver = InfluxDriver;
        assert_eq!(driver.name(), "influxdb");
    }

    #[tokio::test]
    async fn connect_bad_host_returns_connection_error() {
        let driver = InfluxDriver;
        let params = bad_params();
        let result = driver.connect(&params).await;
        match result {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("influxdb"),
                    "error should mention influxdb: {msg}"
                );
            }
            Err(other) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(_) => panic!("expected connection error, but got Ok"),
        }
    }
}
