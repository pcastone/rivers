//! Per-request transaction state management.

use std::collections::HashMap;
use rivers_runtime::rivers_driver_sdk::DriverError;
use rivers_runtime::rivers_driver_sdk::Connection;
use tokio::sync::Mutex;

/// Holds active transaction connections for a single request.
/// Keyed by datasource name.
pub struct TransactionMap {
    connections: Mutex<HashMap<String, Box<dyn Connection>>>,
}

impl TransactionMap {
    /// Create an empty transaction map.
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    /// Begin a transaction on a datasource. Stores the connection.
    pub async fn begin(
        &self,
        datasource: &str,
        mut conn: Box<dyn Connection>,
    ) -> Result<(), DriverError> {
        let mut map = self.connections.lock().await;
        if map.contains_key(datasource) {
            return Err(DriverError::Query(format!(
                "transaction already active on datasource '{}'",
                datasource
            )));
        }
        conn.begin_transaction().await?;
        map.insert(datasource.to_string(), conn);
        Ok(())
    }

    /// Check if a transaction is active on a datasource.
    pub async fn has_transaction(&self, datasource: &str) -> bool {
        let map = self.connections.lock().await;
        map.contains_key(datasource)
    }

    /// Take the connection out for use. Caller must return it via return_connection().
    pub async fn take_connection(&self, datasource: &str) -> Option<Box<dyn Connection>> {
        let mut map = self.connections.lock().await;
        map.remove(datasource)
    }

    /// Return a connection after use.
    pub async fn return_connection(&self, datasource: &str, conn: Box<dyn Connection>) {
        let mut map = self.connections.lock().await;
        map.insert(datasource.to_string(), conn);
    }

    /// Commit the transaction. Returns the connection for pool release.
    pub async fn commit(&self, datasource: &str) -> Result<Box<dyn Connection>, DriverError> {
        let mut map = self.connections.lock().await;
        match map.remove(datasource) {
            Some(mut conn) => {
                conn.commit_transaction().await?;
                Ok(conn)
            }
            None => Err(DriverError::Query(format!(
                "no active transaction on datasource '{}'",
                datasource
            ))),
        }
    }

    /// Rollback the transaction. Connection is dropped (not returned to pool).
    pub async fn rollback(&self, datasource: &str) -> Result<(), DriverError> {
        let mut map = self.connections.lock().await;
        match map.remove(datasource) {
            Some(mut conn) => {
                conn.rollback_transaction().await?;
                Ok(())
            }
            None => Err(DriverError::Query(format!(
                "no active transaction on datasource '{}'",
                datasource
            ))),
        }
    }

    /// Auto-rollback all remaining transactions. Called at request end.
    pub async fn auto_rollback_all(&self) {
        let mut map = self.connections.lock().await;
        for (datasource, mut conn) in map.drain() {
            tracing::warn!(
                datasource = %datasource,
                "auto-rollback — handler did not commit or rollback"
            );
            if let Err(e) = conn.rollback_transaction().await {
                tracing::error!(
                    datasource = %datasource,
                    error = %e,
                    "auto-rollback failed"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::rivers_driver_sdk::{Query, QueryResult};
    use async_trait::async_trait;

    struct MockConnection {
        name: &'static str,
    }

    impl MockConnection {
        fn new(name: &'static str) -> Self {
            Self { name }
        }
    }

    #[async_trait]
    impl Connection for MockConnection {
        async fn execute(&mut self, _query: &Query) -> Result<QueryResult, DriverError> {
            Ok(QueryResult { rows: vec![], affected_rows: 0, last_insert_id: None })
        }
        async fn ping(&mut self) -> Result<(), DriverError> { Ok(()) }
        fn driver_name(&self) -> &str { self.name }
        async fn begin_transaction(&mut self) -> Result<(), DriverError> { Ok(()) }
        async fn commit_transaction(&mut self) -> Result<(), DriverError> { Ok(()) }
        async fn rollback_transaction(&mut self) -> Result<(), DriverError> { Ok(()) }
    }

    #[tokio::test]
    async fn begin_stores_connection() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new("mock"))).await.unwrap();
        assert!(map.has_transaction("pg").await);
    }

    #[tokio::test]
    async fn double_begin_fails() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new("mock"))).await.unwrap();
        let err = map.begin("pg", Box::new(MockConnection::new("mock"))).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn commit_removes_and_returns_connection() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new("mock"))).await.unwrap();
        let conn = map.commit("pg").await.unwrap();
        assert!(!map.has_transaction("pg").await);
        assert_eq!(conn.driver_name(), "mock");
    }

    #[tokio::test]
    async fn rollback_removes_connection() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new("mock"))).await.unwrap();
        map.rollback("pg").await.unwrap();
        assert!(!map.has_transaction("pg").await);
    }

    #[tokio::test]
    async fn commit_without_begin_fails() {
        let map = TransactionMap::new();
        assert!(map.commit("pg").await.is_err());
    }

    #[tokio::test]
    async fn rollback_without_begin_fails() {
        let map = TransactionMap::new();
        assert!(map.rollback("pg").await.is_err());
    }

    #[tokio::test]
    async fn auto_rollback_clears_all() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new("pg"))).await.unwrap();
        map.begin("mysql", Box::new(MockConnection::new("mysql"))).await.unwrap();
        map.auto_rollback_all().await;
        assert!(!map.has_transaction("pg").await);
        assert!(!map.has_transaction("mysql").await);
    }

    #[tokio::test]
    async fn take_and_return_connection() {
        let map = TransactionMap::new();
        map.begin("pg", Box::new(MockConnection::new("mock"))).await.unwrap();
        let conn = map.take_connection("pg").await.unwrap();
        assert!(!map.has_transaction("pg").await);
        map.return_connection("pg", conn).await;
        assert!(map.has_transaction("pg").await);
    }
}
