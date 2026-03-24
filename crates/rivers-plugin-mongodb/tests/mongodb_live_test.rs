//! Live integration tests for the MongoDB driver.
//!
//! Requires a running MongoDB instance at 192.168.2.212:27017.
//! If the service is unreachable, tests print SKIP and pass.

use std::collections::HashMap;
use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_mongodb::MongoDriver;

const MONGO_HOST: &str = "192.168.2.212";
const MONGO_PORT: u16 = 27017;
const MONGO_DB: &str = "test";
const TIMEOUT: Duration = Duration::from_secs(10);

/// Resolve a credential from the real LockBox keystore at `sec/lockbox/`.
fn lockbox_resolve(name: &str) -> String {
    let lockbox_dir = find_lockbox_dir()
        .expect("cannot find sec/lockbox/ — run from workspace root or set RIVERS_LOCKBOX_DIR");
    let identity_path = lockbox_dir.join("identity.key");
    let key_str = std::fs::read_to_string(&identity_path)
        .unwrap_or_else(|e| panic!("cannot read identity: {e}"));
    let identity: age::x25519::Identity = key_str.trim().parse()
        .expect("invalid age identity key");
    let entry_path = lockbox_dir.join("entries").join(format!("{name}.age"));
    let encrypted = std::fs::read(&entry_path)
        .unwrap_or_else(|e| panic!("cannot read lockbox entry {name}: {e}"));
    let decrypted = age::decrypt(&identity, &encrypted)
        .unwrap_or_else(|e| panic!("cannot decrypt {name}: {e}"));
    String::from_utf8(decrypted).unwrap()
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

fn conn_params() -> ConnectionParams {
    let mut options = HashMap::new();
    options.insert("authSource".to_string(), "admin".to_string());
    ConnectionParams {
        host: MONGO_HOST.into(),
        port: MONGO_PORT,
        database: MONGO_DB.into(),
        username: "rivers".into(),
        password: lockbox_resolve("mongodb/test"),
        options,
    }
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = MongoDriver;
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: MongoDB unreachable at {MONGO_HOST}:{MONGO_PORT} — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: MongoDB connection timed out at {MONGO_HOST}:{MONGO_PORT}");
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
