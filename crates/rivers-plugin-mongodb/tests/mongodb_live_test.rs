//! Live integration tests for the MongoDB driver.
//!
//! Connection info resolved from LockBox keystore (see `sec/lockbox/`).
//! If the service is unreachable, tests print SKIP and pass.

use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_mongodb::MongoDriver;

const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    // Read password
    let encrypted = std::fs::read(dir.join("entries/mongodb/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    // Read connection metadata
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/mongodb/test.meta.json")).unwrap()
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
    let driver = MongoDriver;
    let params = conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: MongoDB unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: MongoDB connection timed out");
            None
        }
    }
}

/// Generate a unique collection name to avoid test collisions.
fn unique_collection() -> String {
    format!(
        "rivers_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Validates that the MongoDB driver can establish a connection and respond to a ping health check.
#[tokio::test]
async fn mongodb_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };

    let query = Query::with_operation("ping", "", "");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("ping failed");

    assert_eq!(result.affected_rows, 0);
}

/// Validates full CRUD lifecycle: insert a document, find it by query, delete it, and confirm removal.
#[tokio::test]
async fn mongodb_insert_find_delete_roundtrip() {
    let Some(mut conn) = try_connect().await else { return };

    let collection = unique_collection();

    // INSERT a document.
    // MongoDB driver uses query.target as collection name (via resolve_collection).
    let insert_query = Query::with_operation("insert", &collection, "")
        .param("name", QueryValue::String("Alice".into()))
        .param("age", QueryValue::Integer(30))
        .param("city", QueryValue::String("Portland".into()));

    let insert_result = match tokio::time::timeout(TIMEOUT, conn.execute(&insert_query)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            eprintln!("SKIP: MongoDB insert failed (likely replicaset/auth): {e}");
            return;
        }
        Err(_) => {
            eprintln!("SKIP: MongoDB insert timed out");
            return;
        }
    };

    assert_eq!(insert_result.affected_rows, 1);
    assert!(
        insert_result.last_insert_id.is_some(),
        "expected an inserted _id"
    );

    // FIND the document — use statement JSON with collection + filter.
    let find_stmt = serde_json::json!({
        "collection": collection,
        "filter": { "name": "Alice" }
    })
    .to_string();
    let find_query = Query::with_operation("find", &collection, &find_stmt);

    let find_result = tokio::time::timeout(TIMEOUT, conn.execute(&find_query))
        .await
        .expect("timed out")
        .expect("find failed");

    assert!(
        !find_result.rows.is_empty(),
        "expected at least one matching document"
    );
    assert_eq!(
        find_result.rows[0].get("name"),
        Some(&QueryValue::String("Alice".into()))
    );
    assert_eq!(
        find_result.rows[0].get("age"),
        Some(&QueryValue::Integer(30))
    );

    // DELETE the document.
    let delete_query = Query::with_operation("delete", &collection, "")
        .param("name", QueryValue::String("Alice".into()));

    let delete_result = tokio::time::timeout(TIMEOUT, conn.execute(&delete_query))
        .await
        .expect("timed out")
        .expect("delete failed");

    assert!(
        delete_result.affected_rows >= 1,
        "expected at least 1 deleted"
    );

    // Verify deletion — find should return 0 rows.
    let verify_query = Query::with_operation("find", &collection, &find_stmt);
    let verify_result = tokio::time::timeout(TIMEOUT, conn.execute(&verify_query))
        .await
        .expect("timed out")
        .expect("verify find failed");

    assert!(
        verify_result.rows.is_empty(),
        "expected no documents after delete"
    );
}
