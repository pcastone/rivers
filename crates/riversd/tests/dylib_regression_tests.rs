//! Regression tests for known bugs in the Rivers driver pipeline.
//!
//! Each test guards against a specific past bug to prevent recurrence:
//!   - SQLite disk persistence vs :memory: fallback  (2026-04-06)
//!   - SQLite empty-path error handling               (2026-04-06)
//!   - DDL whitelist permit/reject logic
//!   - SQLite parameter binding order                 (Issue #54)
//!   - Full DDL→INSERT→SELECT→DELETE roundtrip
//!
//! Run with: cargo test -p riversd --test dylib_regression_tests

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::rivers_core::DriverFactory;
use rivers_runtime::rivers_core::drivers::SqliteDriver;
use rivers_runtime::rivers_core_config::config::security::is_ddl_permitted;
use rivers_runtime::rivers_driver_sdk::{ConnectionParams, Query, QueryValue};

// ── Helpers ─────────────────────────────────────────────────────────

/// Build ConnectionParams for a SQLite file, setting `database` to the given path.
fn sqlite_params(db_path: &str) -> ConnectionParams {
    let mut options = HashMap::new();
    options.insert("driver".to_string(), "sqlite".to_string());
    ConnectionParams {
        host: db_path.to_string(),
        port: 0,
        database: db_path.to_string(),
        username: String::new(),
        password: String::new(),
        options,
    }
}

/// Build ConnectionParams where only `host` has the path; `database` is empty.
fn sqlite_params_host_only(db_path: &str) -> ConnectionParams {
    let mut options = HashMap::new();
    options.insert("driver".to_string(), "sqlite".to_string());
    ConnectionParams {
        host: db_path.to_string(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options,
    }
}

/// Build a DriverFactory with the SQLite driver registered.
fn factory_with_sqlite() -> DriverFactory {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(SqliteDriver::new()));
    factory
}

// ── Test 1: SQLite uses disk, not :memory: ──────────────────────────
//
// Bug 2026-04-06: When `database` was empty, driver fell back to :memory:
// and data vanished between connections.

#[tokio::test]
async fn sqlite_uses_disk_not_memory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("disk_test.db");
    let db_str = db_path.to_str().unwrap();

    let factory = factory_with_sqlite();
    let params = sqlite_params_host_only(db_str);

    // Connection 1: create table + insert a row
    {
        let mut conn = factory.connect("sqlite", &params).await.unwrap();
        let ddl = Query::new("regression", "CREATE TABLE regression (id INTEGER PRIMARY KEY, val TEXT)");
        conn.ddl_execute(&ddl).await.unwrap();

        let mut insert = Query::new("regression", "INSERT INTO regression (id, val) VALUES ($id, $val)");
        insert.parameters.insert("id".to_string(), QueryValue::Integer(1));
        insert.parameters.insert("val".to_string(), QueryValue::String("persisted".to_string()));
        conn.execute(&insert).await.unwrap();
    }

    // Connection 2: select and verify the row survives
    {
        let mut conn = factory.connect("sqlite", &params).await.unwrap();
        let select = Query::new("regression", "SELECT id, val FROM regression WHERE id = $id")
            .param("id", QueryValue::Integer(1));
        let result = conn.execute(&select).await.unwrap();
        assert_eq!(result.rows.len(), 1, "Expected 1 row from disk-persisted database");
        assert_eq!(
            result.rows[0].get("val"),
            Some(&QueryValue::String("persisted".to_string())),
            "Row value should survive across connections"
        );
    }

    // The .db file must exist on disk
    assert!(db_path.exists(), "SQLite database file should exist on disk");
}

// ── Test 2: Both host and database empty → error ────────────────────
//
// Bug 2026-04-06: Empty paths silently opened :memory: instead of failing.

#[tokio::test]
async fn sqlite_errors_when_both_empty() {
    let factory = factory_with_sqlite();
    let mut options = HashMap::new();
    options.insert("driver".to_string(), "sqlite".to_string());
    let params = ConnectionParams {
        host: String::new(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options,
    };

    let result = factory.connect("sqlite", &params).await;
    assert!(
        result.is_err(),
        "Connecting with empty host AND database should fail, but it succeeded"
    );
}

// ── Test 3: DDL whitelist rejects wrong app ─────────────────────────

#[test]
fn ddl_whitelist_rejects_wrong_app() {
    let whitelist = vec!["mydb@correct-app".to_string()];
    assert!(
        !is_ddl_permitted("mydb", "wrong-app", &whitelist),
        "DDL should be denied when app_id does not match whitelist entry"
    );
}

// ── Test 4: DDL whitelist permits correct app ───────────────────────

#[test]
fn ddl_whitelist_permits_correct_app() {
    let whitelist = vec!["mydb@correct-app".to_string()];
    assert!(
        is_ddl_permitted("mydb", "correct-app", &whitelist),
        "DDL should be permitted when database@appId matches whitelist"
    );
}

// ── Test 5: DDL whitelist empty → false ─────────────────────────────

#[test]
fn ddl_whitelist_empty_returns_false() {
    let whitelist: Vec<String> = vec![];
    assert!(
        !is_ddl_permitted("mydb", "app-1", &whitelist),
        "Empty DDL whitelist should deny all access"
    );
}

// ── Test 6: Parameter binding order (Issue #54) ─────────────────────
//
// Names are deliberately non-alphabetical (`zname` before `age`) to
// catch HashMap iteration-order bugs that swap bound values.

#[tokio::test]
async fn sqlite_parameter_binding_order() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("param_order.db");
    let db_str = db_path.to_str().unwrap();

    let factory = factory_with_sqlite();
    let params = sqlite_params(db_str);
    let mut conn = factory.connect("sqlite", &params).await.unwrap();

    // DDL
    let ddl = Query::new("people", "CREATE TABLE people (zname TEXT, age INTEGER)");
    conn.ddl_execute(&ddl).await.unwrap();

    // INSERT with non-alphabetical named params
    let insert = Query::new(
        "people",
        "INSERT INTO people (zname, age) VALUES ($zname, $age)",
    )
    .param("zname", QueryValue::String("Alice".to_string()))
    .param("age", QueryValue::Integer(30));
    conn.execute(&insert).await.unwrap();

    // SELECT and verify values are not swapped
    let select = Query::new("people", "SELECT zname, age FROM people");
    let result = conn.execute(&select).await.unwrap();
    assert_eq!(result.rows.len(), 1, "Should have 1 row");

    let row = &result.rows[0];
    assert_eq!(
        row.get("zname"),
        Some(&QueryValue::String("Alice".to_string())),
        "zname should be 'Alice', not the age value"
    );
    assert_eq!(
        row.get("age"),
        Some(&QueryValue::Integer(30)),
        "age should be 30, not the name value"
    );
}

// ── Test 7: Full DDL→INSERT→SELECT→DELETE roundtrip ─────────────────

#[tokio::test]
async fn sqlite_ddl_then_insert_select_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("roundtrip.db");
    let db_str = db_path.to_str().unwrap();

    let factory = factory_with_sqlite();
    let params = sqlite_params(db_str);
    let mut conn = factory.connect("sqlite", &params).await.unwrap();

    // 1. DDL — create table
    let ddl = Query::new(
        "items",
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, qty INTEGER)",
    );
    let ddl_result = conn.ddl_execute(&ddl).await;
    assert!(ddl_result.is_ok(), "CREATE TABLE failed: {:?}", ddl_result.err());

    // 2. INSERT
    let insert = Query::new(
        "items",
        "INSERT INTO items (id, name, qty) VALUES ($id, $name, $qty)",
    )
    .param("id", QueryValue::Integer(1))
    .param("name", QueryValue::String("Widget".to_string()))
    .param("qty", QueryValue::Integer(42));
    let insert_result = conn.execute(&insert).await.unwrap();
    assert_eq!(insert_result.affected_rows, 1, "INSERT should affect 1 row");

    // 3. SELECT — verify the row
    let select = Query::new("items", "SELECT id, name, qty FROM items WHERE id = $id")
        .param("id", QueryValue::Integer(1));
    let select_result = conn.execute(&select).await.unwrap();
    assert_eq!(select_result.rows.len(), 1, "SELECT should return 1 row");
    assert_eq!(
        select_result.rows[0].get("name"),
        Some(&QueryValue::String("Widget".to_string()))
    );
    assert_eq!(
        select_result.rows[0].get("qty"),
        Some(&QueryValue::Integer(42))
    );

    // 4. DELETE — remove the row
    let delete = Query::new("items", "DELETE FROM items WHERE id = $id")
        .param("id", QueryValue::Integer(1));
    let delete_result = conn.execute(&delete).await.unwrap();
    assert_eq!(delete_result.affected_rows, 1, "DELETE should affect 1 row");

    // 5. Verify empty
    let verify = Query::new("items", "SELECT id FROM items");
    let verify_result = conn.execute(&verify).await.unwrap();
    assert_eq!(verify_result.rows.len(), 0, "Table should be empty after DELETE");
}
