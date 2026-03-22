//! Live integration tests for the Cassandra driver.
//!
//! Requires a running Cassandra instance. Set RIVERS_TEST_CASS_HOST (default: localhost).
//! If the service is unreachable, tests print SKIP and pass.

use std::collections::HashMap;
use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_cassandra::CassandraDriver;

const CASS_PORT: u16 = 9042;
const TIMEOUT: Duration = Duration::from_secs(10);

fn cass_host() -> String {
    std::env::var("RIVERS_TEST_CASS_HOST").unwrap_or_else(|_| "localhost".to_string())
}

fn conn_params() -> ConnectionParams {
    ConnectionParams {
        host: cass_host(),
        port: CASS_PORT,
        database: "system".into(),
        username: "".into(),
        password: "".into(),
        options: HashMap::new(),
    }
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = CassandraDriver;
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            let host = cass_host();
            eprintln!("SKIP: Cassandra unreachable at {host}:{CASS_PORT} — {e}");
            None
        }
        Err(_) => {
            let host = cass_host();
            eprintln!("SKIP: Cassandra connection timed out at {host}:{CASS_PORT}");
            None
        }
    }
}

#[tokio::test]
async fn cassandra_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };

    let result = tokio::time::timeout(TIMEOUT, conn.ping()).await;
    match result {
        Ok(Ok(())) => {} // success
        Ok(Err(e)) => panic!("ping failed: {e:?}"),
        Err(_) => panic!("ping timed out"),
    }
}

#[tokio::test]
async fn cassandra_select_system_local() {
    let Some(mut conn) = try_connect().await else { return };

    let query = Query::with_operation(
        "select",
        "",
        "SELECT release_version FROM system.local",
    );

    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("query failed");

    assert!(
        !result.rows.is_empty(),
        "expected at least one row from system.local"
    );

    let row = &result.rows[0];
    let version = row
        .get("release_version")
        .expect("missing 'release_version' column");

    match version {
        QueryValue::String(s) => {
            assert!(!s.is_empty(), "release_version should not be empty");
            eprintln!("Cassandra release_version: {s}");
        }
        other => panic!("expected String for release_version, got: {other:?}"),
    }
}
