//! Live integration tests for the CouchDB driver.
//!
//! Requires a running CouchDB instance at 192.168.2.221:5984.
//! Credentials are resolved from a LockBox keystore.
//! If the service is unreachable, tests print SKIP and pass.

use std::collections::HashMap;
use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_couchdb::CouchDBDriver;

const COUCH_HOST: &str = "192.168.2.221";
const COUCH_PORT: u16 = 5984;
const COUCH_DB: &str = "test_rivers";
const COUCH_USER: &str = "rivers";
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
    let password = lockbox_resolve("couchdb/test");
    ConnectionParams {
        host: COUCH_HOST.into(),
        port: COUCH_PORT,
        database: COUCH_DB.into(),
        username: COUCH_USER.into(),
        password,
        options: HashMap::new(),
    }
}

/// Ensure the test database exists before tests run.
/// CouchDB requires the database to be explicitly created.
async fn ensure_db_exists() -> bool {
    let password = lockbox_resolve("couchdb/test");
    let url = format!(
        "http://{COUCH_USER}:{password}@{COUCH_HOST}:{COUCH_PORT}/{COUCH_DB}"
    );
    let client = reqwest::Client::new();

    // Try to create the database (ignore 412 = already exists).
    match tokio::time::timeout(TIMEOUT, client.put(&url).send()).await {
        Ok(Ok(resp)) => {
            let status = resp.status().as_u16();
            status == 201 || status == 412
        }
        _ => false,
    }
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = CouchDBDriver;
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: CouchDB unreachable at {COUCH_HOST}:{COUCH_PORT} — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: CouchDB connection timed out at {COUCH_HOST}:{COUCH_PORT}");
            None
        }
    }
}

#[tokio::test]
async fn couchdb_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };

    let result = tokio::time::timeout(TIMEOUT, conn.ping()).await;
    match result {
        Ok(Ok(())) => {} // success
        Ok(Err(e)) => panic!("ping failed: {e:?}"),
        Err(_) => panic!("ping timed out"),
    }
}

#[tokio::test]
async fn couchdb_insert_find_delete_roundtrip() {
    // Ensure database exists first.
    if !ensure_db_exists().await {
        eprintln!("SKIP: Could not create/verify CouchDB database {COUCH_DB}");
        return;
    }

    let Some(mut conn) = try_connect().await else { return };

    let unique_tag = format!(
        "rivers_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // INSERT a document.
    let insert_query = Query::with_operation("insert", "", "")
        .param("name", QueryValue::String("Alice".into()))
        .param("age", QueryValue::Integer(30))
        .param("tag", QueryValue::String(unique_tag.clone()));

    let insert_result = tokio::time::timeout(TIMEOUT, conn.execute(&insert_query))
        .await
        .expect("timed out")
        .expect("insert failed");

    assert_eq!(insert_result.affected_rows, 1);
    let doc_id = insert_result
        .last_insert_id
        .clone()
        .expect("expected an inserted document id");

    // FIND the document using Mango selector.
    let find_stmt = serde_json::json!({
        "selector": { "tag": unique_tag }
    })
    .to_string();
    let find_query = Query::with_operation("find", "", &find_stmt);

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
    let delete_query = Query::with_operation("delete", "", "")
        .param("id", QueryValue::String(doc_id.clone()));

    let delete_result = tokio::time::timeout(TIMEOUT, conn.execute(&delete_query))
        .await
        .expect("timed out")
        .expect("delete failed");

    assert_eq!(delete_result.affected_rows, 1);

    // Verify deletion — find should return 0 matching docs.
    let verify_query = Query::with_operation("find", "", &find_stmt);
    let verify_result = tokio::time::timeout(TIMEOUT, conn.execute(&verify_query))
        .await
        .expect("timed out")
        .expect("verify find failed");

    assert!(
        verify_result.rows.is_empty(),
        "expected no documents after delete"
    );
}
