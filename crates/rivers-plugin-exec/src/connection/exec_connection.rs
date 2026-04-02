//! `ExecConnection` struct and `Connection` trait impl.
//!
//! The struct holds command runtimes and the global concurrency semaphore.
//! The `execute()` method delegates to `execute_command()` (defined in
//! `pipeline.rs`) for the actual 11-step pipeline.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use rivers_driver_sdk::{Connection, DriverError, Query, QueryResult};

use crate::config::ExecConfig;

use super::driver::CommandRuntime;

// ── ExecConnection (Connection) ──────────────────────────────────────

/// A live connection to the exec driver, holding all command runtimes
/// and the global concurrency semaphore.
pub struct ExecConnection {
    pub(crate) config: ExecConfig,
    pub(crate) commands: HashMap<String, CommandRuntime>,
    pub(crate) global_semaphore: Arc<Semaphore>,
}

#[async_trait]
impl Connection for ExecConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        // Only "query" operation supported
        match query.operation.as_str() {
            "query" => self.execute_command(query).await,
            other => Err(DriverError::Unsupported(format!(
                "exec driver does not support '{other}' -- only 'query' is supported"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "rivers-exec"
    }
}
