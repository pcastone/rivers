#![warn(missing_docs)]
//! Neo4j plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using `neo4rs` (Bolt protocol).
//! Neo4j natively uses `$name` parameter syntax — no translation needed.
//!
//! Operations dispatch based on `query.operation`:
//! - select/query/find/match → Cypher query (returns rows)
//! - insert/create → Cypher CREATE (returns created nodes)
//! - update/set → Cypher MATCH...SET (returns updated nodes)
//! - delete/remove → Cypher MATCH...DELETE
//! - ping → RETURN 1

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, ParamStyle,
    Query, QueryResult, QueryValue,
};

/// Neo4j graph database driver.
pub struct Neo4jDriver;

#[async_trait]
impl DatabaseDriver for Neo4jDriver {
    fn name(&self) -> &str {
        "neo4j"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let port = if params.port == 0 { 7687 } else { params.port };
        let uri = format!("bolt://{}:{}", params.host, port);

        let config = neo4rs::ConfigBuilder::new()
            .uri(&uri)
            .user(&params.username)
            .password(&params.password)
            .db(if params.database.is_empty() { "neo4j" } else { &params.database })
            .build()
            .map_err(|e| DriverError::Connection(format!("neo4j config: {e}")))?;

        let graph = neo4rs::Graph::connect(config)
            .map_err(|e| DriverError::Connection(format!("neo4j connect: {e}")))?;

        debug!(host = %params.host, port = %port, database = %params.database, "neo4j: connected");

        Ok(Box::new(Neo4jConnection { graph: Arc::new(graph) }))
    }

    fn supports_transactions(&self) -> bool {
        true
    }

    fn param_style(&self) -> ParamStyle {
        ParamStyle::DollarNamed
    }
}

/// A live Neo4j connection wrapping `neo4rs::Graph`.
pub struct Neo4jConnection {
    graph: Arc<neo4rs::Graph>,
}

#[async_trait]
impl Connection for Neo4jConnection {
    fn admin_operations(&self) -> &[&str] {
        &["create_constraint", "drop_constraint", "create_index", "drop_index"]
    }

    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        let cypher = build_cypher(query)?;

        match query.operation.as_str() {
            "select" | "query" | "find" | "match" | "get" => {
                let mut result = self.graph.execute(cypher)
                    .await
                    .map_err(|e| DriverError::Query(format!("neo4j query: {e}")))?;

                let mut rows = Vec::new();
                while let Ok(Some(row)) = result.next().await {
                    rows.push(row_to_map(&row));
                }

                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "insert" | "create" => {
                let mut result = self.graph.execute(cypher)
                    .await
                    .map_err(|e| DriverError::Query(format!("neo4j create: {e}")))?;

                let mut rows = Vec::new();
                while let Ok(Some(row)) = result.next().await {
                    rows.push(row_to_map(&row));
                }

                let count = rows.len().max(1) as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "update" | "set" => {
                let mut result = self.graph.execute(cypher)
                    .await
                    .map_err(|e| DriverError::Query(format!("neo4j update: {e}")))?;

                let mut rows = Vec::new();
                while let Ok(Some(row)) = result.next().await {
                    rows.push(row_to_map(&row));
                }

                let count = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                })
            }

            "delete" | "remove" | "del" => {
                self.graph.run(cypher)
                    .await
                    .map_err(|e| DriverError::Query(format!("neo4j delete: {e}")))?;

                Ok(QueryResult::empty())
            }

            "ping" => {
                self.ping().await?;
                Ok(QueryResult::empty())
            }

            op => Err(DriverError::Unsupported(format!(
                "neo4j driver does not support operation: {op}"
            ))),
        }
    }

    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let cypher = build_cypher(query)?;
        self.graph.run(cypher)
            .await
            .map_err(|e| DriverError::Query(format!("neo4j ddl: {e}")))?;
        Ok(QueryResult::empty())
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        let cypher = neo4rs::query("RETURN 1 AS ping");
        let mut result = self.graph.execute(cypher)
            .await
            .map_err(|e| DriverError::Connection(format!("neo4j ping: {e}")))?;
        let _ = result.next().await;
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "neo4j"
    }
}

/// Build a neo4rs Query from Rivers Query (with parameter binding).
fn build_cypher(query: &Query) -> Result<neo4rs::Query, DriverError> {
    let mut cypher = neo4rs::query(&query.statement);

    for (key, val) in &query.parameters {
        match val {
            QueryValue::Null => { /* neo4rs doesn't support null params — skip */ }
            QueryValue::Boolean(b) => { cypher = cypher.param(key.as_str(), *b); }
            QueryValue::Integer(i) => { cypher = cypher.param(key.as_str(), *i); }
            QueryValue::Float(f) => { cypher = cypher.param(key.as_str(), *f); }
            QueryValue::String(s) => { cypher = cypher.param(key.as_str(), s.clone()); }
            QueryValue::Array(_) | QueryValue::Json(_) => {
                let json_str = serde_json::to_string(val).unwrap_or_default();
                cypher = cypher.param(key.as_str(), json_str);
            }
        }
    }

    Ok(cypher)
}

/// Convert a neo4rs Row to a HashMap<String, QueryValue>.
fn row_to_map(row: &neo4rs::Row) -> HashMap<String, QueryValue> {
    let mut map = HashMap::new();

    // neo4rs Row doesn't expose column names directly in a simple iterator,
    // so we try common return patterns
    if let Ok(val) = row.get::<String>("n") {
        map.insert("n".to_string(), QueryValue::String(val));
    }
    if let Ok(val) = row.get::<i64>("id") {
        map.insert("id".to_string(), QueryValue::Integer(val));
    }

    // Fallback: try to get the row as a JSON-like structure
    // neo4rs 0.9 has limited column introspection
    map
}

// ── Plugin exports ────────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    rivers_driver_sdk::ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_register(registrar: &mut dyn rivers_driver_sdk::DriverRegistrar) {
    registrar.register_database_driver(Arc::new(Neo4jDriver));
}
