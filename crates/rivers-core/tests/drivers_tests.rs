//! Built-in driver tests — FakerDriver operations, stub driver names, registration.

use std::collections::HashMap;

use rivers_core::driver_factory::DriverFactory;
use rivers_core::drivers::{
    FakerDriver, MemcachedDriver, MysqlDriver, PostgresDriver, RedisDriver, RpsClientDriver,
    SqliteDriver,
};
use rivers_core::register_builtin_drivers;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, DriverError, Query, QueryValue};

fn test_params() -> ConnectionParams {
    ConnectionParams {
        host: "localhost".into(),
        port: 5432,
        database: "test".into(),
        username: "admin".into(),
        password: "secret".into(),
        options: HashMap::new(),
    }
}

// ── FakerDriver ────────────────────────────────────────────────────

#[test]
fn faker_driver_name() {
    let driver = FakerDriver::new();
    assert_eq!(driver.name(), "faker");
}

#[tokio::test]
async fn faker_connect_succeeds() {
    let driver = FakerDriver::new();
    let conn = driver.connect(&test_params()).await;
    assert!(conn.is_ok());
    assert_eq!(conn.unwrap().driver_name(), "faker");
}

#[tokio::test]
async fn faker_ping() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    assert!(conn.ping().await.is_ok());
}

#[tokio::test]
async fn faker_select_default_rows() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::new("contacts", "SELECT * FROM contacts");
    let result = conn.execute(&query).await.unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.affected_rows, 1);
    // Check row content
    let row = &result.rows[0];
    assert_eq!(row.get("id").unwrap(), &QueryValue::Integer(1));
    assert_eq!(
        row.get("name").unwrap(),
        &QueryValue::String("faker_1".into())
    );
}

#[tokio::test]
async fn faker_select_multiple_rows() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::new("contacts", "SELECT * FROM contacts")
        .param("rows", QueryValue::Integer(5));
    let result = conn.execute(&query).await.unwrap();
    assert_eq!(result.rows.len(), 5);
    assert_eq!(result.affected_rows, 5);
    // Third row should have id=3
    assert_eq!(
        result.rows[2].get("id").unwrap(),
        &QueryValue::Integer(3)
    );
}

#[tokio::test]
async fn faker_select_with_custom_default() {
    let driver = FakerDriver::with_default_rows(3);
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::new("items", "SELECT * FROM items");
    let result = conn.execute(&query).await.unwrap();
    assert_eq!(result.rows.len(), 3);
}

#[tokio::test]
async fn faker_insert() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::new("contacts", "INSERT INTO contacts (name) VALUES ('test')");
    let result = conn.execute(&query).await.unwrap();
    assert!(result.rows.is_empty());
    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.last_insert_id, Some("1".to_string()));
}

#[tokio::test]
async fn faker_update() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::new("contacts", "UPDATE contacts SET name = 'x'")
        .param("rows", QueryValue::Integer(3));
    let result = conn.execute(&query).await.unwrap();
    assert!(result.rows.is_empty());
    assert_eq!(result.affected_rows, 3);
    assert!(result.last_insert_id.is_none());
}

#[tokio::test]
async fn faker_delete() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::new("contacts", "DELETE FROM contacts WHERE id = 1");
    let result = conn.execute(&query).await.unwrap();
    assert!(result.rows.is_empty());
    assert_eq!(result.affected_rows, 1);
}

#[tokio::test]
async fn faker_ping_via_execute() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::with_operation("ping", "", "");
    let result = conn.execute(&query).await.unwrap();
    assert!(result.rows.is_empty());
    assert_eq!(result.affected_rows, 0);
}

#[tokio::test]
async fn faker_unsupported_operation() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::with_operation("stream", "topic", "SUBSCRIBE topic");
    let result = conn.execute(&query).await;
    match result {
        Err(DriverError::Unsupported(msg)) => {
            assert!(msg.contains("stream"));
        }
        Err(e) => panic!("expected Unsupported, got: {}", e),
        Ok(_) => panic!("expected error"),
    }
}

#[tokio::test]
async fn faker_get_operation() {
    let driver = FakerDriver::new();
    let mut conn = driver.connect(&test_params()).await.unwrap();
    let query = Query::with_operation("get", "key", "GET key");
    let result = conn.execute(&query).await.unwrap();
    assert_eq!(result.rows.len(), 1);
}

// ── Stub Driver Names ──────────────────────────────────────────────

#[test]
fn postgres_driver_name() {
    assert_eq!(PostgresDriver.name(), "postgres");
}

#[test]
fn mysql_driver_name() {
    assert_eq!(MysqlDriver.name(), "mysql");
}

#[test]
fn sqlite_driver_name() {
    assert_eq!(SqliteDriver.name(), "sqlite");
}

#[test]
fn redis_driver_name() {
    assert_eq!(RedisDriver.name(), "redis");
}

#[test]
fn memcached_driver_name() {
    assert_eq!(MemcachedDriver.name(), "memcached");
}

#[test]
fn rps_client_driver_name() {
    assert_eq!(RpsClientDriver.name(), "rps-client");
}

// ── Real Drivers Return Connection Error (no server running) ──────

#[tokio::test]
async fn postgres_connect_fails_without_server() {
    let result = PostgresDriver.connect(&test_params()).await;
    assert!(result.is_err(), "should fail without a running postgres server");
}

#[tokio::test]
async fn mysql_connect_fails_without_server() {
    let result = MysqlDriver.connect(&test_params()).await;
    assert!(result.is_err(), "should fail without a running mysql server");
}

#[tokio::test]
async fn sqlite_connect_memory_succeeds() {
    let mut params = test_params();
    params.database = ":memory:".to_string();
    let result = SqliteDriver.connect(&params).await;
    assert!(result.is_ok(), "in-memory sqlite should succeed: {:?}", result.err());
}

#[tokio::test]
async fn redis_connect_fails_without_server() {
    let result = RedisDriver.connect(&test_params()).await;
    assert!(result.is_err(), "should fail without a running redis server");
}

#[tokio::test]
async fn memcached_connect_without_server() {
    let result = MemcachedDriver.connect(&test_params()).await;
    // async-memcached may connect lazily — either an error or a client that
    // fails on first operation is acceptable without a running server.
    if let Ok(mut conn) = result {
        // If connect succeeded, ping should fail
        assert!(conn.ping().await.is_err());
    }
}

// ── RPS Client Remains a Stub ─────────────────────────────────────

#[tokio::test]
async fn rps_client_connect_unsupported() {
    match RpsClientDriver.connect(&test_params()).await {
        Err(DriverError::NotImplemented(msg)) => assert!(msg.contains("rps-client")),
        Err(e) => panic!("expected NotImplemented, got: {}", e),
        Ok(_) => panic!("expected error"),
    }
}

// ── SQLite In-Memory Smoke Test ───────────────────────────────────

#[tokio::test]
async fn sqlite_memory_ping() {
    let mut params = test_params();
    params.database = ":memory:".to_string();
    let mut conn = SqliteDriver.connect(&params).await.unwrap();
    assert!(conn.ping().await.is_ok());
}

#[tokio::test]
async fn sqlite_memory_execute_select() {
    use rivers_driver_sdk::{Query, QueryValue};
    let mut params = test_params();
    params.database = ":memory:".to_string();
    let mut conn = SqliteDriver.connect(&params).await.unwrap();

    // Create a table via ddl_execute (DDL guard blocks CREATE in execute() since H1)
    let create = Query::with_operation("create", "test", "CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT)");
    conn.ddl_execute(&create).await.unwrap();

    let insert = Query::with_operation("insert", "test", "INSERT INTO test (id, name) VALUES ($id, $name)")
        .param("id", QueryValue::Integer(1))
        .param("name", QueryValue::String("alice".into()));
    let result = conn.execute(&insert).await.unwrap();
    assert_eq!(result.affected_rows, 1);

    // Select it back
    let select = Query::new("test", "SELECT id, name FROM test");
    let result = conn.execute(&select).await.unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("name"), Some(&QueryValue::String("alice".into())));
}

// ── Transaction/Prepared Statement Support ─────────────────────────

#[test]
fn postgres_supports_transactions() {
    assert!(PostgresDriver.supports_transactions());
    assert!(PostgresDriver.supports_prepared_statements());
}

#[test]
fn mysql_supports_transactions() {
    assert!(MysqlDriver.supports_transactions());
    assert!(!MysqlDriver.supports_prepared_statements()); // default false
}

#[test]
fn sqlite_supports_transactions() {
    assert!(SqliteDriver.supports_transactions());
}

#[test]
fn redis_no_transactions() {
    assert!(!RedisDriver.supports_transactions());
}

// ── register_builtin_drivers ───────────────────────────────────────

#[test]
fn register_builtin_drivers_populates_factory() {
    let mut factory = DriverFactory::new();
    register_builtin_drivers(&mut factory);
    assert_eq!(factory.total_count(), 9); // faker, postgres, mysql, sqlite, redis, memcached, rps-client, eventbus, filesystem

    let names = factory.driver_names();
    assert!(names.contains(&"faker"));
    assert!(names.contains(&"postgres"));
    assert!(names.contains(&"mysql"));
    assert!(names.contains(&"sqlite"));
    assert!(names.contains(&"redis"));
    assert!(names.contains(&"memcached"));
    assert!(names.contains(&"rps-client"));
    assert!(names.contains(&"eventbus"));
    assert!(names.contains(&"filesystem"));
}

#[tokio::test]
async fn register_builtin_faker_connects() {
    let mut factory = DriverFactory::new();
    register_builtin_drivers(&mut factory);
    let conn = factory.connect("faker", &test_params()).await;
    assert!(conn.is_ok());
    assert_eq!(conn.unwrap().driver_name(), "faker");
}

#[tokio::test]
async fn register_builtin_postgres_fails() {
    let mut factory = DriverFactory::new();
    register_builtin_drivers(&mut factory);
    let result = factory.connect("postgres", &test_params()).await;
    assert!(result.is_err());
}
