//! DDL pipeline integration tests.
//!
//! Exercises the full DDL execution pipeline from Rust:
//!   DataViewExecutor datasource resolution → DriverFactory connect →
//!   Connection::ddl_execute() → verify data persists.
//!
//! Uses SQLite temp files — no external infrastructure needed.
//!
//! Run with: cargo test -p riversd --test ddl_pipeline_tests

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::rivers_core::DriverFactory;
use rivers_runtime::rivers_driver_sdk::{ConnectionParams, Query, QueryValue};
use rivers_runtime::tiered_cache::NoopDataViewCache;
use rivers_runtime::{DataViewExecutor, DataViewRegistry};

/// Helper: build ConnectionParams pointing at a SQLite file.
fn sqlite_params(db_path: &str) -> ConnectionParams {
    let mut options = HashMap::new();
    options.insert("driver".to_string(), "sqlite".to_string());
    ConnectionParams {
        host: String::new(),
        port: 0,
        database: db_path.to_string(),
        username: String::new(),
        password: String::new(),
        options,
    }
}

/// Helper: build a DriverFactory with the SQLite driver registered.
fn factory_with_sqlite() -> DriverFactory {
    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(
        rivers_runtime::rivers_core::drivers::SqliteDriver::new(),
    ));
    factory
}

// ── Test 1: DDL create table via DriverFactory ──────────────────────

#[tokio::test]
async fn ddl_create_table_via_driver_factory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test1.db");
    let db_str = db_path.to_str().unwrap();

    let factory = factory_with_sqlite();
    let params = sqlite_params(db_str);

    // Connect and execute DDL
    let mut conn = factory.connect("sqlite", &params).await.unwrap();
    let ddl_query = Query::new(
        "test_ddl",
        "CREATE TABLE test_ddl (id INTEGER PRIMARY KEY, name TEXT)",
    );
    let result = conn.ddl_execute(&ddl_query).await;
    assert!(result.is_ok(), "DDL CREATE TABLE should succeed: {:?}", result.err());

    // Verify table exists: INSERT then SELECT
    let insert = Query::new(
        "test_ddl",
        "INSERT INTO test_ddl (id, name) VALUES (1, 'alice')",
    );
    conn.execute(&insert).await.unwrap();

    let select = Query::new("test_ddl", "SELECT id, name FROM test_ddl WHERE id = 1");
    let rows = conn.execute(&select).await.unwrap();
    assert_eq!(rows.rows.len(), 1, "should find 1 row");
    assert_eq!(
        rows.rows[0].get("name"),
        Some(&QueryValue::String("alice".into())),
    );
}

// ── Test 2: DDL pipeline through executor resolution ────────────────

#[tokio::test]
async fn ddl_pipeline_through_executor_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test2.db");
    let db_str = db_path.to_str().unwrap();

    // Build executor with a datasource named "test-sqlite"
    let factory = Arc::new(factory_with_sqlite());
    let registry = DataViewRegistry::new();

    let mut ds_params = HashMap::new();
    ds_params.insert("test-sqlite".to_string(), sqlite_params(db_str));
    let ds_params = Arc::new(ds_params);

    let executor = DataViewExecutor::new(
        registry,
        factory.clone(),
        ds_params,
        Arc::new(NoopDataViewCache),
    );

    // Step 1: Resolve datasource params via executor
    let resolved = executor.datasource_params_get("test-sqlite");
    assert!(resolved.is_some(), "datasource 'test-sqlite' should resolve");
    let resolved = resolved.unwrap();

    // Step 2: Extract driver name
    let driver_name = resolved.options.get("driver").map(|s| s.as_str()).unwrap();
    assert_eq!(driver_name, "sqlite");

    // Step 3: Connect via DriverFactory
    let mut conn = factory.connect(driver_name, resolved).await.unwrap();

    // Step 4: Execute DDL
    let ddl = Query::new("t", "CREATE TABLE pipeline_test (id INTEGER PRIMARY KEY, value TEXT)");
    conn.ddl_execute(&ddl).await.unwrap();

    // Step 5: Verify via INSERT + SELECT
    let insert = Query::new("t", "INSERT INTO pipeline_test (id, value) VALUES (42, 'pipeline-ok')");
    conn.execute(&insert).await.unwrap();

    let select = Query::new("t", "SELECT value FROM pipeline_test WHERE id = 42");
    let result = conn.execute(&select).await.unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("value"),
        Some(&QueryValue::String("pipeline-ok".into())),
    );
}

// ── Test 3: DDL whitelist gate rejects unpermitted ──────────────────

#[test]
fn ddl_whitelist_gate_rejects_unpermitted() {
    use rivers_runtime::rivers_core_config::config::security::is_ddl_permitted;

    // Mismatched entry — should be rejected
    assert!(
        !is_ddl_permitted("test-db", "app-123", &["other-db@other-app".to_string()]),
        "should reject when datasource/app not in whitelist"
    );

    // Exact match — should be permitted
    assert!(
        is_ddl_permitted("test-db", "app-123", &["test-db@app-123".to_string()]),
        "should permit when exact match is in whitelist"
    );

    // Empty whitelist — should reject (no entries means no DDL allowed)
    assert!(
        !is_ddl_permitted("test-db", "app-123", &[]),
        "empty whitelist should reject all DDL"
    );

    // Multiple entries — only the matching one should permit
    let whitelist = vec![
        "db-alpha@app-aaa".to_string(),
        "test-db@app-123".to_string(),
        "db-beta@app-bbb".to_string(),
    ];
    assert!(
        is_ddl_permitted("test-db", "app-123", &whitelist),
        "should find matching entry in multi-entry whitelist"
    );
    assert!(
        !is_ddl_permitted("test-db", "app-999", &whitelist),
        "should reject when app_id doesn't match any entry"
    );
}

// ── Test 4: DDL execute_ddl on executor with whitelist gate ─────────

#[tokio::test]
async fn ddl_execute_ddl_through_executor_with_whitelist() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test4.db");
    let db_str = db_path.to_str().unwrap();

    let factory = Arc::new(factory_with_sqlite());
    let registry = DataViewRegistry::new();

    let mut ds_params = HashMap::new();
    ds_params.insert("my-sqlite".to_string(), sqlite_params(db_str));

    let executor = DataViewExecutor::new(
        registry,
        factory.clone(),
        Arc::new(ds_params),
        Arc::new(NoopDataViewCache),
    );

    let ddl_query = Query::new("t", "CREATE TABLE gated_table (id INTEGER PRIMARY KEY, data TEXT)");

    // Attempt without whitelist entry — should fail
    let rejected = executor
        .execute_ddl("my-sqlite", &ddl_query, "app-uuid-1", &[], "trace-1")
        .await;
    assert!(
        rejected.is_err(),
        "DDL should be rejected without whitelist entry"
    );
    let err_msg = format!("{}", rejected.unwrap_err());
    assert!(
        err_msg.contains("not permitted"),
        "error should mention 'not permitted', got: {err_msg}"
    );

    // Attempt with correct whitelist entry — should succeed
    // Note: database field in params is the db_path, so whitelist key uses that
    let whitelist = vec![format!("{}@app-uuid-1", db_str)];
    let ok = executor
        .execute_ddl("my-sqlite", &ddl_query, "app-uuid-1", &whitelist, "trace-2")
        .await;
    assert!(ok.is_ok(), "DDL should succeed with whitelist entry: {:?}", ok.err());

    // Verify table was created by connecting and querying
    let params = sqlite_params(db_str);
    let mut conn = factory.connect("sqlite", &params).await.unwrap();
    let insert = Query::new("t", "INSERT INTO gated_table (id, data) VALUES (1, 'gated-ok')");
    conn.execute(&insert).await.unwrap();

    let select = Query::new("t", "SELECT data FROM gated_table WHERE id = 1");
    let result = conn.execute(&select).await.unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("data"),
        Some(&QueryValue::String("gated-ok".into())),
    );
}

// ── Test 5: Multiple DDL statements persist through pipeline ────────

#[tokio::test]
async fn ddl_multiple_statements_persist_through_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test5.db");
    let db_str = db_path.to_str().unwrap();

    let factory = factory_with_sqlite();
    let params = sqlite_params(db_str);

    let mut conn = factory.connect("sqlite", &params).await.unwrap();

    // Execute multiple DDL statements in a single batch
    let ddl = Query::new(
        "t",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);
         CREATE INDEX idx_posts_user ON posts(user_id);",
    );
    conn.ddl_execute(&ddl).await.unwrap();

    // Verify both tables exist via sqlite_master
    let tables = Query::new(
        "sqlite_master",
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
    );
    let result = conn.execute(&tables).await.unwrap();
    let table_names: Vec<&str> = result
        .rows
        .iter()
        .filter_map(|r| match r.get("name") {
            Some(QueryValue::String(s)) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert!(table_names.contains(&"users"), "users table must exist");
    assert!(table_names.contains(&"posts"), "posts table must exist");

    // Verify index exists
    let indexes = Query::new(
        "sqlite_master",
        "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_posts_user'",
    );
    let idx_result = conn.execute(&indexes).await.unwrap();
    assert_eq!(idx_result.rows.len(), 1, "index must exist");

    // Verify data can be inserted across both tables
    conn.execute(&Query::new("t", "INSERT INTO users (id, name) VALUES (1, 'alice')"))
        .await
        .unwrap();
    conn.execute(&Query::new(
        "t",
        "INSERT INTO posts (id, user_id, title) VALUES (1, 1, 'hello world')",
    ))
    .await
    .unwrap();

    let select = Query::new("t", "SELECT title FROM posts WHERE user_id = 1");
    let rows = conn.execute(&select).await.unwrap();
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(
        rows.rows[0].get("title"),
        Some(&QueryValue::String("hello world".into())),
    );
}
