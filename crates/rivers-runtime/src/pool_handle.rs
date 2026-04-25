//! Pool handle trait used by `DataViewExecutor` to acquire connections
//! without depending on the concrete `riversd::pool::PoolManager` type.
//!
//! Implemented by `riversd::pool::PoolManager`. See `docs/code_review.md`
//! P0-3 / P1-1 and `docs/superpowers/plans/2026-04-25-dataview-pool-integration.md`.

use async_trait::async_trait;
use std::sync::Arc;

use rivers_driver_sdk::error::DriverError;
use rivers_driver_sdk::traits::Connection;

/// Outcome of a pool acquire — the caller is responsible for releasing
/// the connection (via the implementer's `release` API) when done.
pub struct PooledConnection {
    /// The acquired database connection.
    pub conn: Box<dyn Connection>,
    /// Opaque release token — implementer-specific. The caller must pass
    /// it back to `release` to keep accounting honest.
    pub release_token: Box<dyn ReleaseToken>,
}

/// Type-erased token that releases a pooled connection on drop.
///
/// Implementations forward to `ConnectionPool::release(conn, Some(created_at))`.
pub trait ReleaseToken: Send + Sync {
    /// Release the held connection back to its pool.
    fn release(self: Box<Self>, conn: Box<dyn Connection>);
}

/// Snapshot returned by the pool manager — kept structurally identical
/// to `riversd::pool::PoolSnapshot` to avoid double bookkeeping.
#[derive(Debug, Clone)]
pub struct PoolHandleSnapshot {
    /// Identifier of the datasource this snapshot belongs to.
    pub datasource_id: String,
    /// Number of connections currently checked out.
    pub active_connections: usize,
    /// Number of connections sitting idle in the pool.
    pub idle_connections: usize,
    /// Sum of active and idle connections.
    pub total_connections: usize,
    /// Cumulative number of successful checkouts.
    pub checkout_count: u64,
    /// Average wait time per checkout in milliseconds.
    pub avg_wait_ms: u64,
    /// Configured maximum pool size.
    pub max_size: usize,
    /// Configured minimum idle connections.
    pub min_idle: usize,
}

/// Pool acquire/snapshot surface the executor depends on.
#[async_trait]
pub trait PoolManagerHandle: Send + Sync {
    /// Acquire a connection for a datasource. Returns `Ok(None)` when the
    /// datasource has no pool registered (e.g. broker datasources) — the
    /// caller should fall through to its existing non-pooled path.
    ///
    /// Returns `Err` for pool-level failures (timeout, circuit open, etc.).
    async fn acquire(
        &self,
        datasource_id: &str,
    ) -> Result<Option<PooledConnection>, DriverError>;

    /// Return snapshots for all registered pools.
    async fn snapshots(&self) -> Vec<PoolHandleSnapshot>;
}

/// Convenience alias for the shared handle.
pub type SharedPoolHandle = Arc<dyn PoolManagerHandle>;
