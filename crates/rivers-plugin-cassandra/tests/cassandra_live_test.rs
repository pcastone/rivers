//! Live integration tests for the Cassandra driver.
//!
//! Credentials are resolved from a LockBox keystore at sec/lockbox/.
//! If the service is unreachable, tests print SKIP and pass.

use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_cassandra::CassandraDriver;

const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    let encrypted = std::fs::read(dir.join("entries/cassandra/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/cassandra/test.meta.json")).unwrap()
    ).unwrap();

    let hosts: Vec<String> = meta["hosts"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let (host, port) = parse_host_port(&hosts[0]);

    let mut options: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(obj) = meta["options"].as_object() {
        for (k, v) in obj { options.insert(k.clone(), v.as_str().unwrap_or("").to_string()); }
    }
    if hosts.len() > 1 {
        options.insert("hosts".into(), hosts.join(","));
        options.insert("cluster".into(), "true".into());
    }

    ConnectionParams {
        host, port,
        database: meta["database"].as_str().unwrap_or("").to_string(),
        username: meta["username"].as_str().unwrap_or("").to_string(),
        password, options,
    }
}

fn parse_host_port(s: &str) -> (String, u16) {
    match s.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(0)),
        None => (s.to_string(), 0),
    }
}

fn find_lockbox_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("RIVERS_LOCKBOX_DIR") {
        let p = std::path::PathBuf::from(&dir);
        if p.join("identity.key").exists() { return Some(p); }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..10 {
        let candidate = dir.join("sec").join("lockbox");
        if candidate.join("identity.key").exists() { return Some(candidate); }
        if !dir.pop() { break; }
    }
    None
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = CassandraDriver;
    let params = conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: Cassandra unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: Cassandra connection timed out");
            None
        }
    }
}

/// Validates that the Cassandra driver can establish a connection and respond to a ping health check.
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

/// Validates that a CQL query against system.local returns a valid result, proving query execution works.
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
