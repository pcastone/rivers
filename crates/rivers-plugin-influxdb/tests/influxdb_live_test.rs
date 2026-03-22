//! Live integration tests for the InfluxDB v2 plugin driver.
//!
//! Requires a running InfluxDB v2 instance. Set RIVERS_TEST_INFLUX_HOST (default: localhost).
//! Credentials are resolved from a LockBox keystore.
//! If the service is unreachable, tests print SKIP and pass.
//!
//! Run with: cargo test --test influxdb_live_test

use std::collections::HashMap;
use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_influxdb::InfluxDriver;

const INFLUX_PORT: u16 = 8086;
const INFLUX_ORG: &str = "rivers";
const INFLUX_BUCKET: &str = "test";
const INFLUX_USER: &str = "rivers";
const TIMEOUT: Duration = Duration::from_secs(10);

fn influx_host() -> String {
    std::env::var("RIVERS_TEST_INFLUX_HOST").unwrap_or_else(|_| "localhost".to_string())
}

/// Resolve a single credential from a temporary LockBox keystore.
fn lockbox_resolve(name: &str, value: &str) -> String {
    use age::secrecy::ExposeSecret;
    use rivers_core::lockbox::{
        encrypt_keystore, fetch_secret_value, Keystore, KeystoreEntry, LockBoxResolver,
    };

    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    let now = chrono::Utc::now();

    let entry = KeystoreEntry {
        name: name.to_string(),
        value: value.to_string(),
        entry_type: "string".to_string(),
        aliases: vec![],
        created: now,
        updated: now,
    };
    let keystore = Keystore { version: 1, entries: vec![entry] };

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.rkeystore");
    encrypt_keystore(&path, &recipient.to_string(), &keystore).unwrap();

    let resolver = LockBoxResolver::from_entries(&keystore.entries).unwrap();
    let metadata = resolver.resolve(name).unwrap();
    let identity_str = identity.to_string();
    let resolved = fetch_secret_value(metadata, &path, identity_str.expose_secret()).unwrap();
    resolved.value
}

fn conn_params() -> ConnectionParams {
    let password = lockbox_resolve("influxdb/test", "rivers-test");
    let mut options = HashMap::new();
    options.insert("org".to_string(), INFLUX_ORG.to_string());
    ConnectionParams {
        host: influx_host(),
        port: INFLUX_PORT,
        database: INFLUX_BUCKET.into(),
        username: INFLUX_USER.into(),
        password,
        options,
    }
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = InfluxDriver;
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            let host = influx_host();
            eprintln!("SKIP: InfluxDB unreachable at {host}:{INFLUX_PORT} — {e}");
            None
        }
        Err(_) => {
            let host = influx_host();
            eprintln!("SKIP: InfluxDB connection timed out at {host}:{INFLUX_PORT}");
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

    // Use a unique tag to isolate this test run
    let test_id = format!(
        "test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // Write a data point using line protocol
    let write_query = Query::with_operation("write", INFLUX_BUCKET, "")
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
        r#"from(bucket: "{INFLUX_BUCKET}")
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
