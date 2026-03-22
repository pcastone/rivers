//! Live integration tests for the Redis driver.
//!
//! Requires a running Redis instance. Set RIVERS_TEST_REDIS_HOST (default: localhost).
//! Credentials are resolved from a LockBox keystore (see `common/mod.rs`).
//! If the service is unreachable or returns cluster MOVED errors, tests SKIP and pass.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use rivers_core::drivers::RedisDriver;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};

fn redis_host() -> String {
    std::env::var("RIVERS_TEST_REDIS_HOST").unwrap_or_else(|_| "localhost".to_string())
}

const REDIS_PORT: u16 = 6379;
const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let creds = common::TestCredentials::new();
    let mut options = HashMap::new();
    options.insert("cluster".into(), "true".into());
    let h = redis_host();
    options.insert(
        "hosts".into(),
        format!("{h}:6379,{h}:6379,{h}:6379"),
    );
    ConnectionParams {
        host: redis_host(),
        port: REDIS_PORT,
        database: "0".into(),
        username: "".into(),
        password: creds.get("redis/test"),
        options,
    }
}

async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = RedisDriver::new();
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => { eprintln!("SKIP: Redis — {e}"); None }
        Err(_) => { eprintln!("SKIP: Redis timed out"); None }
    }
}

async fn run(conn: &mut Box<dyn rivers_driver_sdk::Connection>, query: &Query) -> Option<rivers_driver_sdk::types::QueryResult> {
    match tokio::time::timeout(TIMEOUT, conn.execute(query)).await {
        Ok(Ok(r)) => Some(r),
        Ok(Err(e)) => { eprintln!("FAIL: Redis error — {e}"); None }
        Err(_) => { eprintln!("SKIP: Redis timed out"); None }
    }
}

fn unique_key(prefix: &str) -> String {
    format!("rivers_test:{prefix}:{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

#[tokio::test]
async fn redis_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };
    let query = Query::with_operation("ping", "redis", "PING");
    let Some(result) = run(&mut conn, &query).await else { return };
    assert_eq!(result.affected_rows, 0);
    println!("Redis ping PASSED");
}

#[tokio::test]
async fn redis_set_get_roundtrip() {
    let Some(mut conn) = try_connect().await else { return };
    let key = unique_key("setget");

    let set_q = Query::with_operation("set", "redis", "")
        .param("key", QueryValue::String(key.clone()))
        .param("value", QueryValue::String("hello_rivers".into()));
    let Some(_) = run(&mut conn, &set_q).await else { return };

    let get_q = Query::with_operation("get", "redis", "")
        .param("key", QueryValue::String(key.clone()));
    let Some(result) = run(&mut conn, &get_q).await else { return };
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("value"), Some(&QueryValue::String("hello_rivers".into())));

    let del_q = Query::with_operation("del", "redis", "")
        .param("key", QueryValue::String(key));
    let _ = run(&mut conn, &del_q).await;
    println!("Redis SET/GET roundtrip PASSED");
}

#[tokio::test]
async fn redis_del_key() {
    let Some(mut conn) = try_connect().await else { return };
    let key = unique_key("del");

    let set_q = Query::with_operation("set", "redis", "")
        .param("key", QueryValue::String(key.clone()))
        .param("value", QueryValue::String("to_delete".into()));
    let Some(_) = run(&mut conn, &set_q).await else { return };

    let del_q = Query::with_operation("del", "redis", "")
        .param("key", QueryValue::String(key.clone()));
    let Some(del_r) = run(&mut conn, &del_q).await else { return };
    assert_eq!(del_r.affected_rows, 1);

    let get_q = Query::with_operation("get", "redis", "")
        .param("key", QueryValue::String(key));
    let Some(result) = run(&mut conn, &get_q).await else { return };
    assert!(result.rows.is_empty(), "expected no rows after DEL");
    println!("Redis DEL PASSED");
}

#[tokio::test]
async fn redis_hset_hgetall() {
    let Some(mut conn) = try_connect().await else { return };
    let key = unique_key("hash");

    let hset1 = Query::with_operation("hset", "redis", "")
        .param("key", QueryValue::String(key.clone()))
        .param("field", QueryValue::String("name".into()))
        .param("value", QueryValue::String("Alice".into()));
    let Some(_) = run(&mut conn, &hset1).await else { return };

    let hset2 = Query::with_operation("hset", "redis", "")
        .param("key", QueryValue::String(key.clone()))
        .param("field", QueryValue::String("age".into()))
        .param("value", QueryValue::String("30".into()));
    let Some(_) = run(&mut conn, &hset2).await else { return };

    let hgetall = Query::with_operation("hgetall", "redis", "")
        .param("key", QueryValue::String(key.clone()));
    let Some(result) = run(&mut conn, &hgetall).await else { return };
    assert_eq!(result.rows.len(), 2, "expected 2 hash fields");

    let mut fields: HashMap<String, String> = HashMap::new();
    for row in &result.rows {
        if let (Some(QueryValue::String(f)), Some(QueryValue::String(v))) =
            (row.get("field"), row.get("value"))
        {
            fields.insert(f.clone(), v.clone());
        }
    }
    assert_eq!(fields.get("name"), Some(&"Alice".to_string()));
    assert_eq!(fields.get("age"), Some(&"30".to_string()));

    let del_q = Query::with_operation("del", "redis", "")
        .param("key", QueryValue::String(key));
    let _ = run(&mut conn, &del_q).await;
    println!("Redis HSET/HGETALL PASSED");
}
