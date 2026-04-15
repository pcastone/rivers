#![warn(missing_docs)]
//! Neo4j plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using `neo4rs` (Bolt protocol).
//! Neo4j natively uses `$name` parameter syntax — no translation needed.
//!
//! Operations dispatch based on `query.operation`:
//! - select/query/find/match → Cypher query (returns rows)
//! - insert/create → Cypher CREATE (returns rows if RETURN present, else empty)
//! - update/set → Cypher MATCH...SET (returns rows if RETURN present)
//! - delete/remove → Cypher MATCH...DELETE (returns rows if RETURN present)
//! - ping → RETURN 1

use std::collections::HashMap;

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

        // G2.2: Read scheme from options (bolt, bolt+s, neo4j, neo4j+s)
        let scheme = params.options.get("scheme").map(|s| s.as_str()).unwrap_or("bolt");
        let uri = format!("{scheme}://{}:{}", params.host, port);

        let mut builder = neo4rs::ConfigBuilder::new()
            .uri(&uri)
            .user(&params.username)
            .password(&params.password)
            .db(if params.database.is_empty() { "neo4j" } else { &params.database });

        // G2.2: Read optional config from ConnectionParams.options
        if let Some(max_conn) = params.options.get("max_connections").and_then(|v| v.parse::<usize>().ok()) {
            builder = builder.max_connections(max_conn);
        }
        if let Some(fetch) = params.options.get("fetch_size").and_then(|v| v.parse::<usize>().ok()) {
            builder = builder.fetch_size(fetch);
        }

        let config = builder
            .build()
            .map_err(|e| DriverError::Connection(format!("neo4j config: {e}")))?;

        let graph = neo4rs::Graph::connect(config)
            .map_err(|e| DriverError::Connection(format!("neo4j connect: {e}")))?;

        debug!(host = %params.host, port = %port, database = %params.database, scheme = %scheme, "neo4j: connected");

        // G2.1: Graph is Clone+Send+Sync — no Arc needed
        Ok(Box::new(Neo4jConnection { graph, txn: None }))
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
    // G2.1: Graph is already Clone+Send+Sync with internal connection pool
    graph: neo4rs::Graph,
    txn: Option<neo4rs::Txn>,
}

#[async_trait]
impl Connection for Neo4jConnection {
    fn admin_operations(&self) -> &[&str] {
        // G2.5: include database-level ops for Neo4j Enterprise
        &[
            "create_constraint", "drop_constraint",
            "create_index", "drop_index",
            "create_database", "drop_database",
        ]
    }

    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        let cypher = build_cypher(query)?;
        let has_return = statement_has_return(&query.statement);

        match query.operation.as_str() {
            "select" | "query" | "find" | "match" | "get" => {
                execute_returning(&self.graph, cypher, "query").await
            }

            "insert" | "create" => {
                // G2.3: use run() when no RETURN clause
                if has_return {
                    execute_returning(&self.graph, cypher, "create").await
                } else {
                    self.graph.run(cypher)
                        .await
                        .map_err(|e| DriverError::Query(format!("neo4j create: {e}")))?;
                    Ok(QueryResult { rows: Vec::new(), affected_rows: 1, last_insert_id: None })
                }
            }

            "update" | "set" => {
                if has_return {
                    execute_returning(&self.graph, cypher, "update").await
                } else {
                    self.graph.run(cypher)
                        .await
                        .map_err(|e| DriverError::Query(format!("neo4j update: {e}")))?;
                    Ok(QueryResult::empty())
                }
            }

            // G2.4: detect RETURN for delete count
            "delete" | "remove" | "del" => {
                if has_return {
                    execute_returning(&self.graph, cypher, "delete").await
                } else {
                    self.graph.run(cypher)
                        .await
                        .map_err(|e| DriverError::Query(format!("neo4j delete: {e}")))?;
                    Ok(QueryResult::empty())
                }
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

    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        let txn = self.graph
            .start_txn()
            .await
            .map_err(|e| DriverError::Query(format!("neo4j BEGIN: {e}")))?;
        self.txn = Some(txn);
        Ok(())
    }

    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        match self.txn.take() {
            Some(txn) => {
                txn.commit()
                    .await
                    .map_err(|e| DriverError::Query(format!("neo4j COMMIT: {e}")))?;
                Ok(())
            }
            None => Err(DriverError::Query("no active neo4j transaction".into())),
        }
    }

    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        match self.txn.take() {
            Some(txn) => {
                txn.rollback()
                    .await
                    .map_err(|e| DriverError::Query(format!("neo4j ROLLBACK: {e}")))?;
                Ok(())
            }
            None => Err(DriverError::Query("no active neo4j transaction".into())),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Execute a Cypher query that returns rows, with proper error propagation.
async fn execute_returning(
    graph: &neo4rs::Graph,
    cypher: neo4rs::Query,
    op_name: &str,
) -> Result<QueryResult, DriverError> {
    let mut result = graph.execute(cypher)
        .await
        .map_err(|e| DriverError::Query(format!("neo4j {op_name}: {e}")))?;

    let mut rows = Vec::new();
    // G1.4: propagate mid-stream errors instead of swallowing
    loop {
        match result.next().await {
            Ok(Some(row)) => rows.push(row_to_map(&row)),
            Ok(None) => break,
            Err(e) => return Err(DriverError::Query(format!("neo4j {op_name} row: {e}"))),
        }
    }

    let count = rows.len() as u64;
    Ok(QueryResult {
        rows,
        affected_rows: count,
        last_insert_id: None,
    })
}

/// Build a neo4rs Query from Rivers Query (with parameter binding).
fn build_cypher(query: &Query) -> Result<neo4rs::Query, DriverError> {
    let mut cypher = neo4rs::query(&query.statement);

    for (key, val) in &query.parameters {
        match val {
            // G1.3: bind null as empty string (neo4rs BoltNull not directly supported via param)
            QueryValue::Null => {
                cypher = cypher.param(key.as_str(), "");
            }
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

/// G1.2: Convert a neo4rs Row to HashMap using row.keys() for column discovery.
fn row_to_map(row: &neo4rs::Row) -> HashMap<String, QueryValue> {
    let mut map = HashMap::new();

    // Iterate all columns returned by the Cypher query
    for key in row.keys() {
        let key_str = key.to_string();

        // Try typed extraction in order of likelihood
        if let Ok(val) = row.get::<i64>(&key_str) {
            map.insert(key_str, QueryValue::Integer(val));
        } else if let Ok(val) = row.get::<f64>(&key_str) {
            map.insert(key_str, QueryValue::Float(val));
        } else if let Ok(val) = row.get::<bool>(&key_str) {
            map.insert(key_str, QueryValue::Boolean(val));
        } else if let Ok(val) = row.get::<String>(&key_str) {
            map.insert(key_str, QueryValue::String(val));
        } else {
            // Fallback: try to get as Node and extract properties
            if let Ok(node) = row.get::<neo4rs::Node>(&key_str) {
                let mut props = HashMap::new();
                // Extract known property types from the node
                for prop_key in node.keys() {
                    let pk = prop_key.to_string();
                    if let Ok(v) = node.get::<String>(&pk) {
                        props.insert(pk, QueryValue::String(v));
                    } else if let Ok(v) = node.get::<i64>(&pk) {
                        props.insert(pk, QueryValue::Integer(v));
                    } else if let Ok(v) = node.get::<f64>(&pk) {
                        props.insert(pk, QueryValue::Float(v));
                    } else if let Ok(v) = node.get::<bool>(&pk) {
                        props.insert(pk, QueryValue::Boolean(v));
                    }
                }
                let json = serde_json::to_value(&props
                    .into_iter()
                    .map(|(k, v)| (k, query_value_to_json(&v)))
                    .collect::<HashMap<String, serde_json::Value>>()
                ).unwrap_or(serde_json::Value::Null);
                map.insert(key_str, QueryValue::Json(json));
            }
        }
    }

    map
}

/// Convert QueryValue to serde_json::Value for Node property serialization.
fn query_value_to_json(val: &QueryValue) -> serde_json::Value {
    match val {
        QueryValue::Null => serde_json::Value::Null,
        QueryValue::Boolean(b) => serde_json::Value::Bool(*b),
        QueryValue::Integer(i) => serde_json::json!(*i),
        QueryValue::Float(f) => serde_json::json!(*f),
        QueryValue::String(s) => serde_json::Value::String(s.clone()),
        QueryValue::Array(a) => serde_json::json!(a),
        QueryValue::Json(v) => v.clone(),
    }
}

/// Check if a Cypher statement contains a RETURN clause.
fn statement_has_return(statement: &str) -> bool {
    statement.to_uppercase().contains(" RETURN ")
        || statement.to_uppercase().starts_with("RETURN ")
}

// ── Plugin exports ────────────────────────────────────────────────

// G1.1: correct export name + improper_ctypes allowance
#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_abi_version() -> u32 {
    rivers_driver_sdk::ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn rivers_driver_sdk::DriverRegistrar) {
    registrar.register_database_driver(std::sync::Arc::new(Neo4jDriver));
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_name_is_neo4j() {
        assert_eq!(Neo4jDriver.name(), "neo4j");
    }

    #[test]
    fn param_style_is_dollar_named() {
        assert_eq!(Neo4jDriver.param_style(), ParamStyle::DollarNamed);
    }

    #[test]
    fn admin_operations_includes_database_ops() {
        let conn = Neo4jConnection {
            graph: neo4rs::Graph::connect(
                neo4rs::ConfigBuilder::new()
                    .uri("bolt://localhost:7687")
                    .user("neo4j")
                    .password("test")
                    .build()
                    .unwrap()
            ).unwrap_or_else(|_| panic!("skip: neo4j not available")),
            txn: None,
        };
        let ops = conn.admin_operations();
        assert!(ops.contains(&"create_constraint"));
        assert!(ops.contains(&"drop_database"));
    }

    #[test]
    fn statement_has_return_detects_return() {
        assert!(statement_has_return("MATCH (n) RETURN n"));
        assert!(statement_has_return("RETURN 1"));
        assert!(!statement_has_return("CREATE (n:User {name: 'alice'})"));
        assert!(!statement_has_return("MATCH (n) DELETE n"));
    }

    #[test]
    fn build_cypher_binds_params() {
        let mut query = Query::new("test", "MATCH (n) WHERE n.id = $id RETURN n");
        query.parameters.insert("id".to_string(), QueryValue::Integer(42));
        let cypher = build_cypher(&query).unwrap();
        // Can't inspect neo4rs Query internals, but it shouldn't error
        let _ = cypher;
    }

    #[test]
    fn build_cypher_handles_null() {
        let mut query = Query::new("test", "MATCH (n) WHERE n.name = $name RETURN n");
        query.parameters.insert("name".to_string(), QueryValue::Null);
        // Should not panic — binds empty string for null
        let cypher = build_cypher(&query).unwrap();
        let _ = cypher;
    }

    #[tokio::test]
    async fn connect_bad_host_ping_fails() {
        let driver = Neo4jDriver;
        let params = ConnectionParams {
            host: "192.0.2.1".into(), // RFC 5737 TEST-NET — guaranteed unreachable
            port: 7687,
            database: String::new(),
            username: "neo4j".into(),
            password: "test".into(),
            options: HashMap::new(),
        };
        // neo4rs Graph::connect() is lazy — may succeed (creates pool config).
        // The real failure happens on first query/ping.
        match driver.connect(&params).await {
            Ok(mut conn) => {
                let ping = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    conn.ping(),
                ).await;
                match ping {
                    Ok(Ok(_)) => panic!("expected ping to fail on unreachable host"),
                    Ok(Err(_)) => {} // connection error on ping — correct
                    Err(_) => {}     // timeout — also acceptable
                }
            }
            Err(_) => {} // immediate connection error — also correct
        }
    }
}
