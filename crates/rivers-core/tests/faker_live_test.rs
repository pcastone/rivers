//! Live integration tests for the Faker driver.
//!
//! No external infrastructure needed — the Faker driver generates
//! synthetic data in-memory.
//! Credentials are resolved from a LockBox keystore (see `common/mod.rs`).
//!
//! Run with: cargo test --test faker_live_test

mod common;

use std::collections::HashMap;
use std::time::Duration;

use rivers_core::drivers::FakerDriver;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};

const TIMEOUT: Duration = Duration::from_secs(5);

fn conn_params() -> ConnectionParams {
    let creds = common::TestCredentials::new();
    ConnectionParams {
        host: "".into(),
        port: 0,
        database: "".into(),
        username: "".into(),
        password: creds.get("faker/test"),
        options: HashMap::new(),
    }
}

async fn connect() -> Box<dyn rivers_driver_sdk::Connection> {
    let driver = FakerDriver::new();
    let conn: Box<dyn rivers_driver_sdk::Connection> =
        tokio::time::timeout(TIMEOUT, DatabaseDriver::connect(&driver, &conn_params()))
            .await
            .expect("timed out")
            .expect("connect failed");
    conn
}

#[tokio::test]
async fn faker_connect_and_ping() {
    let mut conn = connect().await;

    let ping_query = Query::with_operation("ping", "", "");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&ping_query))
        .await
        .expect("timed out")
        .expect("ping failed");

    assert_eq!(result.affected_rows, 0, "ping should return empty result");
    assert!(result.rows.is_empty(), "ping should return no rows");
}

#[tokio::test]
async fn faker_generate_data() {
    let mut conn = connect().await;

    // Request 5 rows of synthetic data
    let query = Query::with_operation("select", "contacts", "")
        .param("rows", QueryValue::Integer(5));

    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("select failed");

    assert_eq!(result.rows.len(), 5, "expected 5 rows");
    assert_eq!(result.affected_rows, 5, "affected_rows should be 5");

    // Verify each row has the expected fields
    for (i, row) in result.rows.iter().enumerate() {
        let expected_id = (i + 1) as i64;
        assert_eq!(
            row.get("id"),
            Some(&QueryValue::Integer(expected_id)),
            "row {} should have id={expected_id}",
            i
        );

        let expected_name = format!("faker_{}", i + 1);
        assert_eq!(
            row.get("name"),
            Some(&QueryValue::String(expected_name.clone())),
            "row {} should have name='{expected_name}'",
            i
        );
    }
}

#[tokio::test]
async fn faker_default_row_count() {
    let mut conn = connect().await;

    // No rows parameter — should default to 1
    let query = Query::with_operation("select", "anything", "");

    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("select failed");

    assert_eq!(result.rows.len(), 1, "default should be 1 row");
    assert_eq!(result.rows[0].get("id"), Some(&QueryValue::Integer(1)));
}

#[tokio::test]
async fn faker_insert_returns_affected_rows() {
    let mut conn = connect().await;

    let query = Query::with_operation("insert", "contacts", "")
        .param("rows", QueryValue::Integer(3));

    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("insert failed");

    assert!(result.rows.is_empty(), "insert should return no rows");
    assert_eq!(result.affected_rows, 3, "affected_rows should be 3");
    assert_eq!(
        result.last_insert_id,
        Some("1".to_string()),
        "insert should return last_insert_id"
    );
}

#[tokio::test]
async fn faker_unsupported_operation_returns_error() {
    let mut conn = connect().await;

    let query = Query::with_operation("xyzzy", "contacts", "");

    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out");

    assert!(result.is_err(), "unsupported operation should return an error");
}

#[tokio::test]
async fn faker_custom_default_rows() {
    // Test FakerDriver::with_default_rows
    let driver = FakerDriver::with_default_rows(10);
    let mut conn: Box<dyn rivers_driver_sdk::Connection> =
        tokio::time::timeout(TIMEOUT, DatabaseDriver::connect(&driver, &conn_params()))
            .await
            .expect("timed out")
            .expect("connect failed");

    // No explicit rows param
    let query = Query::with_operation("select", "data", "");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("select failed");

    assert_eq!(result.rows.len(), 10, "should use custom default of 10 rows");
}
