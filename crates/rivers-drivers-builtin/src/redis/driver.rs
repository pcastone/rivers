//! `RedisDriver` struct and `DatabaseDriver` implementation.

use async_trait::async_trait;
use rivers_driver_sdk::{Connection, ConnectionParams, DatabaseDriver, DriverError};

use super::cluster::RedisClusterConnection;
use super::single::RedisConnection;

/// Redis database driver.
///
/// Stateless factory -- each call to `connect()` creates a new
/// `RedisConnection` backed by a `MultiplexedConnection`.
pub struct RedisDriver;

impl RedisDriver {
    /// Create a new Redis driver instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for RedisDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseDriver for RedisDriver {
    fn name(&self) -> &str {
        "redis"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let is_cluster = params.options.get("cluster").map(|v| v == "true").unwrap_or(false);

        if is_cluster {
            // Cluster mode: connect to multiple nodes
            let hosts: Vec<String> = if let Some(h) = params.options.get("hosts") {
                h.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                vec![format!("{}:{}", params.host, params.port)]
            };

            let nodes: Vec<String> = hosts.iter().map(|h| {
                if params.password.is_empty() {
                    format!("redis://{h}")
                } else {
                    format!("redis://:{}@{h}", params.password)
                }
            }).collect();

            let client = redis::cluster::ClusterClient::new(nodes)
                .map_err(|e| DriverError::Connection(format!("redis cluster client: {e}")))?;

            let conn = client
                .get_async_connection()
                .await
                .map_err(|e| DriverError::Connection(format!("redis cluster connect: {e}")))?;

            Ok(Box::new(RedisClusterConnection { conn }))
        } else {
            // Single-node mode
            let db = if params.database.is_empty() {
                "0".to_string()
            } else {
                params.database.clone()
            };

            let url = if params.password.is_empty() {
                format!("redis://{}:{}/{}", params.host, params.port, db)
            } else {
                format!(
                    "redis://:{}@{}:{}/{}",
                    params.password, params.host, params.port, db
                )
            };

            let client = redis::Client::open(url.as_str())
                .map_err(|e| DriverError::Connection(format!("redis client open: {e}")))?;

            let conn = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| DriverError::Connection(format!("redis connect: {e}")))?;

            Ok(Box::new(RedisConnection { conn }))
        }
    }

    fn supports_transactions(&self) -> bool {
        false
    }

    fn supports_prepared_statements(&self) -> bool {
        false
    }
}
