//! Live integration tests for the SQLite driver.
//!
//! Uses `:memory:` databases — no external infrastructure needed.
//! Credentials are resolved from a LockBox keystore (see `common/mod.rs`).
//!
//! Run with: cargo test --test sqlite_live_test

mod common;

use std::collections::HashMap;
use std::time::Duration;

use rivers_core::drivers::SqliteDriver;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};

const TIMEOUT: Duration = Duration::from_secs(10);

fn memory_params() -> ConnectionParams {
    let creds = common::TestCredentials::new();
    ConnectionParams {
        host: "".into(),
        port: 0,
        database: ":memory:".into(),
        username: "".into(),
        password: creds.get("sqlite/test"),
        options: HashMap::new(),
    }
}

async fn connect_memory() -> Box<dyn rivers_driver_sdk::Connection> {
    let driver = SqliteDriver::new();
    let conn: Box<dyn rivers_driver_sdk::Connection> =
        tokio::time::timeout(TIMEOUT, DatabaseDriver::connect(&driver, &memory_params()))
            .await
            .expect("timed out")
            .expect("connect failed");
    conn
}

#[tokio::test]
async fn sqlite_connect_memory() {
    let mut conn = connect_memory().await;

    // Verify the connection works via ping
    let result = tokio::time::timeout(TIMEOUT, conn.ping())
        .await
        .expect("ping timed out");

    match result {
        Ok(()) => {} // success
        Err(e) => panic!("ping failed: {e:?}"),
    }
}

#[tokio::test]
async fn sqlite_create_insert_select_roundtrip() {
    let mut conn = connect_memory().await;

    // CREATE TABLE
    let create_query = Query::with_operation(
        "create",
        "",
        "CREATE TABLE contacts (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, age INTEGER)",
    );
    tokio::time::timeout(TIMEOUT, conn.ddl_execute(&create_query))
        .await
        .expect("timed out")
        .expect("CREATE TABLE failed");

    // INSERT
    let insert_query = Query::with_operation(
        "insert",
        "",
        "INSERT INTO contacts (name, age) VALUES ($name, $age)",
    )
    .param("name", QueryValue::String("Alice".into()))
    .param("age", QueryValue::Integer(30));

    let insert_result = tokio::time::timeout(TIMEOUT, conn.execute(&insert_query))
        .await
        .expect("timed out")
        .expect("INSERT failed");

    assert_eq!(insert_result.affected_rows, 1, "INSERT should affect 1 row");
    assert!(
        insert_result.last_insert_id.is_some(),
        "INSERT should return last_insert_id"
    );
    assert_eq!(
        insert_result.last_insert_id.as_deref(),
        Some("1"),
        "first INSERT should return rowid 1"
    );

    // INSERT another row
    let insert2_query = Query::with_operation(
        "insert",
        "",
        "INSERT INTO contacts (name, age) VALUES ($name, $age)",
    )
    .param("name", QueryValue::String("Bob".into()))
    .param("age", QueryValue::Integer(25));

    tokio::time::timeout(TIMEOUT, conn.execute(&insert2_query))
        .await
        .expect("timed out")
        .expect("second INSERT failed");

    // SELECT all
    let select_query = Query::with_operation(
        "select",
        "",
        "SELECT name, age FROM contacts ORDER BY name",
    );

    let select_result = tokio::time::timeout(TIMEOUT, conn.execute(&select_query))
        .await
        .expect("timed out")
        .expect("SELECT failed");

    assert_eq!(select_result.rows.len(), 2, "expected 2 rows");
    assert_eq!(
        select_result.rows[0].get("name"),
        Some(&QueryValue::String("Alice".into()))
    );
    assert_eq!(
        select_result.rows[0].get("age"),
        Some(&QueryValue::Integer(30))
    );
    assert_eq!(
        select_result.rows[1].get("name"),
        Some(&QueryValue::String("Bob".into()))
    );
    assert_eq!(
        select_result.rows[1].get("age"),
        Some(&QueryValue::Integer(25))
    );
}

#[tokio::test]
async fn sqlite_named_parameter_binding() {
    let mut conn = connect_memory().await;

    // CREATE TABLE
    let create_query = Query::with_operation(
        "create",
        "",
        "CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT, price REAL, active INTEGER)",
    );
    tokio::time::timeout(TIMEOUT, conn.ddl_execute(&create_query))
        .await
        .expect("timed out")
        .expect("CREATE TABLE failed");

    // INSERT with mixed parameter types
    let insert_query = Query::with_operation(
        "insert",
        "",
        "INSERT INTO items (id, label, price, active) VALUES ($id, $label, $price, $active)",
    )
    .param("id", QueryValue::Integer(42))
    .param("label", QueryValue::String("Widget".into()))
    .param("price", QueryValue::Float(9.99))
    .param("active", QueryValue::Boolean(true));

    tokio::time::timeout(TIMEOUT, conn.execute(&insert_query))
        .await
        .expect("timed out")
        .expect("INSERT failed");

    // SELECT with named parameter filter
    let select_query = Query::with_operation(
        "select",
        "",
        "SELECT id, label, price, active FROM items WHERE id = $id",
    )
    .param("id", QueryValue::Integer(42));

    let result = tokio::time::timeout(TIMEOUT, conn.execute(&select_query))
        .await
        .expect("timed out")
        .expect("SELECT failed");

    assert_eq!(result.rows.len(), 1, "expected exactly 1 row");
    let row = &result.rows[0];
    assert_eq!(row.get("id"), Some(&QueryValue::Integer(42)));
    assert_eq!(
        row.get("label"),
        Some(&QueryValue::String("Widget".into()))
    );
    assert_eq!(row.get("price"), Some(&QueryValue::Float(9.99)));
    // SQLite stores booleans as integers (1/0)
    assert_eq!(row.get("active"), Some(&QueryValue::Integer(1)));

    // UPDATE with named parameters
    let update_query = Query::with_operation(
        "update",
        "",
        "UPDATE items SET price = $price WHERE id = $id",
    )
    .param("price", QueryValue::Float(12.50))
    .param("id", QueryValue::Integer(42));

    let update_result = tokio::time::timeout(TIMEOUT, conn.execute(&update_query))
        .await
        .expect("timed out")
        .expect("UPDATE failed");

    assert_eq!(update_result.affected_rows, 1, "UPDATE should affect 1 row");

    // Verify update
    let verify_query = Query::with_operation(
        "select",
        "",
        "SELECT price FROM items WHERE id = $id",
    )
    .param("id", QueryValue::Integer(42));

    let verify_result = tokio::time::timeout(TIMEOUT, conn.execute(&verify_query))
        .await
        .expect("timed out")
        .expect("verify SELECT failed");

    assert_eq!(verify_result.rows.len(), 1);
    assert_eq!(
        verify_result.rows[0].get("price"),
        Some(&QueryValue::Float(12.50))
    );

    // DELETE
    let delete_query = Query::with_operation(
        "delete",
        "",
        "DELETE FROM items WHERE id = $id",
    )
    .param("id", QueryValue::Integer(42));

    let delete_result = tokio::time::timeout(TIMEOUT, conn.execute(&delete_query))
        .await
        .expect("timed out")
        .expect("DELETE failed");

    assert_eq!(delete_result.affected_rows, 1, "DELETE should affect 1 row");

    // Verify deletion
    let final_query = Query::with_operation(
        "select",
        "",
        "SELECT count(*) as cnt FROM items",
    );

    let final_result = tokio::time::timeout(TIMEOUT, conn.execute(&final_query))
        .await
        .expect("timed out")
        .expect("count SELECT failed");

    assert_eq!(final_result.rows.len(), 1);
    assert_eq!(
        final_result.rows[0].get("cnt"),
        Some(&QueryValue::Integer(0))
    );
}
