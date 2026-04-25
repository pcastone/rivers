//! Bridge from `riversd::pool::PoolManager` to `rivers_runtime::PoolManagerHandle`.
//!
//! Lives in riversd because it depends on the concrete pool types; exposed
//! as a trait object to `DataViewExecutor` via `AppContext`.
//!
//! Per `docs/code_review.md` P0-3.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use rivers_runtime::pool_handle::{
    PoolHandleSnapshot, PoolManagerHandle, PooledConnection as HandleConn, ReleaseToken,
};
use rivers_runtime::rivers_driver_sdk::error::DriverError;
use rivers_runtime::rivers_driver_sdk::traits::Connection;

use crate::pool::{ConnectionPool, PoolError, PoolManager};

struct PoolReleaseToken {
    pool: Arc<ConnectionPool>,
    created_at: Instant,
}

impl ReleaseToken for PoolReleaseToken {
    fn release(self: Box<Self>, conn: Box<dyn Connection>) {
        let pool = self.pool;
        let created_at = self.created_at;
        // Fire-and-forget release — `release()` is sync by trait contract,
        // but the pool's release is async. Caller is in async context
        // (DataView execute), so the runtime is live for tokio::spawn.
        tokio::spawn(async move {
            pool.release(conn, Some(created_at)).await;
        });
    }
}

#[async_trait]
impl PoolManagerHandle for PoolManager {
    async fn acquire(
        &self,
        datasource_id: &str,
    ) -> Result<Option<HandleConn>, DriverError> {
        let Some(pool) = self.get_pool(datasource_id).await else {
            return Ok(None);
        };
        match pool.acquire_with_meta().await {
            Ok((conn, created_at)) => Ok(Some(HandleConn {
                conn,
                release_token: Box::new(PoolReleaseToken { pool, created_at }),
            })),
            Err(PoolError::Driver(e)) => Err(e),
            Err(other) => Err(map_pool_error(other)),
        }
    }

    async fn snapshots(&self) -> Vec<PoolHandleSnapshot> {
        // Fully-qualified to disambiguate from this trait method (otherwise
        // the call would recurse into itself instead of the inherent method).
        PoolManager::snapshots(self)
            .await
            .into_iter()
            .map(|s| PoolHandleSnapshot {
                datasource_id: s.datasource_id,
                active_connections: s.active_connections,
                idle_connections: s.idle_connections,
                total_connections: s.total_connections,
                checkout_count: s.checkout_count,
                avg_wait_ms: s.avg_wait_ms,
                max_size: s.max_size,
                min_idle: s.min_idle,
            })
            .collect()
    }
}

/// Map non-driver pool errors (CircuitOpen / Timeout / Draining / Config) into
/// `DriverError`. None of those have a one-to-one variant; they are all
/// connection-availability problems, so `DriverError::Connection` is the
/// closest fit and preserves the message verbatim. `DriverError::Other`
/// does not exist on this version of the SDK (verified via
/// `crates/rivers-driver-sdk/src/error.rs`).
fn map_pool_error(e: PoolError) -> DriverError {
    DriverError::Connection(e.to_string())
}
