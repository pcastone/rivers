//! Live integration tests for the Memcached driver.
//!
//! Credentials are resolved from a LockBox keystore (see `common/mod.rs`).
//! If the service is unreachable, tests print SKIP and pass.
//!
//! Run with: cargo test --test memcached_live_test

mod common;

use std::time::Duration;

use rivers_core::drivers::MemcachedDriver;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};

const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    common::TestCredentials::new().connection_params("memcached/test")
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = MemcachedDriver;
    let params = conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: Memcached unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: Memcached connection timed out");
            None
        }
    }
}

/// Generate a unique key to avoid collisions between test runs.
fn unique_key(prefix: &str) -> String {
    format!(
        "{}_{}_{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[tokio::test]
async fn memcached_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };

    let ping_query = Query::with_operation("ping", "", "");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&ping_query))
        .await
        .expect("timed out");

    match result {
        Ok(r) => assert_eq!(r.affected_rows, 0, "ping should return empty result"),
        Err(e) => panic!("ping failed: {e:?}"),
    }
}

#[tokio::test]
async fn memcached_set_get_roundtrip() {
    let Some(mut conn) = try_connect().await else { return };

    let key = unique_key("rivers_test_set_get");
    let value = "hello from rivers live test";

    // SET
    let set_query = Query::with_operation("set", "", "")
        .param("key", QueryValue::String(key.clone()))
        .param("value", QueryValue::String(value.into()))
        .param("expiration", QueryValue::Integer(60));

    let set_result = tokio::time::timeout(TIMEOUT, conn.execute(&set_query))
        .await
        .expect("set timed out")
        .expect("set failed");

    assert_eq!(set_result.affected_rows, 1, "set should report 1 affected row");

    // GET
    let get_query = Query::with_operation("get", "", "")
        .param("key", QueryValue::String(key.clone()));

    let get_result = tokio::time::timeout(TIMEOUT, conn.execute(&get_query))
        .await
        .expect("get timed out")
        .expect("get failed");

    assert_eq!(get_result.rows.len(), 1, "get should return 1 row");
    let row = &get_result.rows[0];
    assert_eq!(
        row.get("value"),
        Some(&QueryValue::String(value.into())),
        "value should match what was set"
    );

    // Cleanup: delete the key
    let del_query = Query::with_operation("delete", "", "")
        .param("key", QueryValue::String(key));
    tokio::time::timeout(TIMEOUT, conn.execute(&del_query))
        .await
        .expect("delete timed out")
        .expect("delete failed");
}

#[tokio::test]
async fn memcached_delete() {
    let Some(mut conn) = try_connect().await else { return };

    let key = unique_key("rivers_test_delete");

    // SET a value first
    let set_query = Query::with_operation("set", "", "")
        .param("key", QueryValue::String(key.clone()))
        .param("value", QueryValue::String("to_be_deleted".into()))
        .param("expiration", QueryValue::Integer(60));

    tokio::time::timeout(TIMEOUT, conn.execute(&set_query))
        .await
        .expect("set timed out")
        .expect("set failed");

    // DELETE
    let del_query = Query::with_operation("delete", "", "")
        .param("key", QueryValue::String(key.clone()));

    let del_result = tokio::time::timeout(TIMEOUT, conn.execute(&del_query))
        .await
        .expect("delete timed out")
        .expect("delete failed");

    assert_eq!(del_result.affected_rows, 1, "delete should report 1 affected row");

    // GET should return empty
    let get_query = Query::with_operation("get", "", "")
        .param("key", QueryValue::String(key));

    let get_result = tokio::time::timeout(TIMEOUT, conn.execute(&get_query))
        .await
        .expect("get timed out")
        .expect("get failed");

    assert!(
        get_result.rows.is_empty(),
        "get after delete should return no rows"
    );
}
