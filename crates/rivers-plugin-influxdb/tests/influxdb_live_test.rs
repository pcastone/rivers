//! Live integration tests for the InfluxDB v2 plugin driver.
//!
//! Connection info resolved from LockBox keystore (see `sec/lockbox/`).
//! If the service is unreachable, tests print SKIP and pass.
//!
//! Run with: cargo test --test influxdb_live_test

use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_influxdb::InfluxDriver;

const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    let encrypted = std::fs::read(dir.join("entries/influxdb/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/influxdb/test.meta.json")).unwrap()
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
        host,
        port,
        database: meta["database"].as_str().unwrap_or("").to_string(),
        username: meta["username"].as_str().unwrap_or("").to_string(),
        password,
        options,
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
    let driver = InfluxDriver;
    let params = conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: InfluxDB unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: InfluxDB connection timed out");
            None
        }
    }
}

#[tokio::test]
async fn influxdb_connect_and_ping() {
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
async fn influxdb_write_and_query() {
    let Some(mut conn) = try_connect().await else { return };

    let params = conn_params();
    let bucket = &params.database;

    // Use a unique tag to isolate this test run
    let test_id = format!(
        "test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // Write a data point using line protocol
    let write_query = Query::with_operation("write", bucket, "")
        .param(
            "_line_protocol",
            QueryValue::String(format!(
                "rivers_test,run_id={test_id} temperature=23.5,humidity=65i"
            )),
        );

    let write_result = tokio::time::timeout(TIMEOUT, conn.execute(&write_query)).await;
    let write_result = match write_result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let msg = format!("{e}");
            if msg.contains("401") || msg.contains("unauthorized") || msg.contains("Unauthorized") {
                eprintln!("SKIP: InfluxDB write requires valid API token (got 401). Set password to a valid InfluxDB v2 API token.");
                return;
            }
            panic!("write failed: {e:?}");
        }
        Err(_) => panic!("write timed out"),
    };

    assert_eq!(write_result.affected_rows, 1, "write should report 1 affected row");

    // Small delay to let InfluxDB index the point
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Query the data back using Flux
    let flux = format!(
        r#"from(bucket: "{bucket}")
  |> range(start: -1h)
  |> filter(fn: (r) => r._measurement == "rivers_test" and r.run_id == "{test_id}")
  |> filter(fn: (r) => r._field == "temperature")"#,
    );

    let read_query = Query::with_operation("query", "", &flux);
    let read_result = tokio::time::timeout(TIMEOUT, conn.execute(&read_query))
        .await
        .expect("query timed out")
        .expect("query failed");

    assert!(
        !read_result.rows.is_empty(),
        "expected at least one row from query, got 0"
    );

    // Verify the temperature value
    let row = &read_result.rows[0];
    let value = row.get("_value");
    assert!(
        value.is_some(),
        "expected '_value' column in row, got keys: {:?}",
        row.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        *value.unwrap(),
        QueryValue::Float(23.5),
        "expected temperature 23.5"
    );
}
