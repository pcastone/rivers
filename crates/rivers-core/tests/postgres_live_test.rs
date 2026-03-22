//! Live integration tests for the PostgreSQL driver.
//!
//! Requires a running PostgreSQL instance at 192.168.2.209:5432.
//! Credentials are resolved from a LockBox keystore (see `common/mod.rs`).
//! If the service is unreachable, tests print SKIP and pass.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use rivers_core::drivers::PostgresDriver;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};

const PG_HOST: &str = "192.168.2.209";
const PG_PORT: u16 = 5432;
const PG_DB: &str = "postgres";
const PG_USER: &str = "postgres";
const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let creds = common::TestCredentials::new();
    ConnectionParams {
        host: PG_HOST.into(),
        port: PG_PORT,
        database: PG_DB.into(),
        username: PG_USER.into(),
        password: creds.get("postgres/test"),
        options: HashMap::new(),
    }
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = PostgresDriver;
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: PostgreSQL unreachable at {PG_HOST}:{PG_PORT} — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: PostgreSQL connection timed out at {PG_HOST}:{PG_PORT}");
            None
        }
    }
}

#[tokio::test]
async fn postgres_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };

    let result = tokio::time::timeout(TIMEOUT, conn.ping()).await;
    match result {
        Ok(Ok(())) => {} // success
        Ok(Err(e)) => panic!("ping failed: {e:?}"),
        Err(_) => panic!("ping timed out"),
    }
}

#[tokio::test]
async fn postgres_select_query() {
    let Some(mut conn) = try_connect().await else { return };

    let query = Query::new("", "SELECT 1 as val");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("query failed");

    assert!(!result.rows.is_empty(), "expected at least one row");
    let row = &result.rows[0];
    let val = row.get("val").expect("missing 'val' column");
    assert_eq!(*val, QueryValue::Integer(1), "expected Integer(1), got {val:?}");
}

#[tokio::test]
async fn postgres_create_insert_select_roundtrip() {
    let Some(mut conn) = try_connect().await else { return };

    let table_name = format!(
        "rivers_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    // CREATE TABLE
    let create_sql = format!(
        "CREATE TABLE {table_name} (id SERIAL PRIMARY KEY, name TEXT NOT NULL, age INT)"
    );
    let create_query = Query::with_operation("create", "", &create_sql);
    tokio::time::timeout(TIMEOUT, conn.execute(&create_query))
        .await
        .expect("timed out")
        .expect("CREATE TABLE failed");

    // INSERT
    let insert_sql = format!(
        "INSERT INTO {table_name} (name, age) VALUES ($1, $2)"
    );
    let insert_query = Query::with_operation("insert", "", &insert_sql)
        .param("a_name", QueryValue::String("Alice".into()))
        .param("b_age", QueryValue::Integer(30));
    tokio::time::timeout(TIMEOUT, conn.execute(&insert_query))
        .await
        .expect("timed out")
        .expect("INSERT failed");

    // SELECT
    let select_sql = format!("SELECT name, age FROM {table_name} WHERE name = $1");
    let select_query = Query::with_operation("select", "", &select_sql)
        .param("a_name", QueryValue::String("Alice".into()));
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&select_query))
        .await
        .expect("timed out")
        .expect("SELECT failed");

    assert_eq!(result.rows.len(), 1, "expected 1 row");
    assert_eq!(
        result.rows[0].get("name"),
        Some(&QueryValue::String("Alice".into()))
    );
    assert_eq!(
        result.rows[0].get("age"),
        Some(&QueryValue::Integer(30))
    );

    // DROP TABLE (cleanup)
    let drop_sql = format!("DROP TABLE {table_name}");
    let drop_query = Query::with_operation("drop", "", &drop_sql);
    tokio::time::timeout(TIMEOUT, conn.execute(&drop_query))
        .await
        .expect("timed out")
        .expect("DROP TABLE failed");
}

#[tokio::test]
async fn postgres_bad_sql_returns_error() {
    let Some(mut conn) = try_connect().await else { return };

    let query = Query::new("", "SELECT FROM nonexistent_xyz");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out");

    assert!(result.is_err(), "expected error for bad SQL, got: {result:?}");
}
