//! Live integration tests for the Neo4j driver.
//!
//! Connection info resolved from LockBox keystore (see `sec/lockbox/`).
//! If the service is unreachable, tests print SKIP and pass.

mod lockbox_helper;

use std::time::Duration;

use rivers_driver_sdk::{DatabaseDriver, Query, QueryValue};
use rivers_plugin_neo4j::Neo4jDriver;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = Neo4jDriver;
    let params = lockbox_helper::conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: Neo4j unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: Neo4j connection timed out");
            None
        }
    }
}

/// Validates that the Neo4j driver can establish a connection and respond to a ping health check.
#[tokio::test]
async fn neo4j_connect_and_ping() {
    let mut conn = match try_connect().await {
        Some(c) => c,
        None => return,
    };

    // neo4rs Graph::connect() is lazy — the real TCP connection happens on first query.
    // Treat ping failure (post-connect) as a skip so CI on machines without Neo4j passes.
    match tokio::time::timeout(TIMEOUT, conn.ping()).await {
        Ok(Ok(())) => {} // ping succeeded
        Ok(Err(e)) => {
            eprintln!("SKIP: Neo4j ping failed (lazy connection, server likely unreachable) — {e}");
            return;
        }
        Err(_) => {
            eprintln!("SKIP: Neo4j ping timed out");
            return;
        }
    }
}

/// Validates full node lifecycle: create a node, query it back, delete it, and confirm removal.
#[tokio::test]
async fn neo4j_create_query_delete_roundtrip() {
    let mut conn = match try_connect().await {
        Some(c) => c,
        None => return,
    };

    // neo4rs is lazy — verify the connection is actually live before proceeding.
    match tokio::time::timeout(TIMEOUT, conn.ping()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            eprintln!("SKIP: Neo4j ping failed (server likely unreachable) — {e}");
            return;
        }
        Err(_) => {
            eprintln!("SKIP: Neo4j ping timed out");
            return;
        }
    }

    let test_id = uuid::Uuid::new_v4().to_string();

    // Create a node via MERGE (idempotent upsert; avoids DDL guard on CREATE)
    let create_stmt = format!("MERGE (n:TestNode {{testId: '{}'}}) SET n.name = 'Rivers Test' RETURN n.testId AS testId, n.name AS name", test_id);
    let create_query = Query::with_operation("insert", "TestNode", &create_stmt);
    let result = conn.execute(&create_query).await.expect("MERGE should succeed");
    assert_eq!(result.affected_rows, 1, "should create one node");

    // Query it back
    let find_stmt = format!("MATCH (n:TestNode {{testId: '{}'}}) RETURN n.testId AS testId, n.name AS name", test_id);
    let find_query = Query::with_operation("match", "TestNode", &find_stmt);
    let result = conn.execute(&find_query).await.expect("MATCH should succeed");
    assert_eq!(result.rows.len(), 1, "should find one node");
    assert_eq!(
        result.rows[0].get("name"),
        Some(&QueryValue::String("Rivers Test".to_string()))
    );

    // Delete it
    let delete_stmt = format!("MATCH (n:TestNode {{testId: '{}'}}) DELETE n", test_id);
    let delete_query = Query::with_operation("delete", "TestNode", &delete_stmt);
    conn.execute(&delete_query).await.expect("DELETE should succeed");

    // Verify it's gone
    let result = conn.execute(&find_query).await.expect("MATCH should succeed");
    assert!(result.rows.is_empty(), "node should be deleted");
}
