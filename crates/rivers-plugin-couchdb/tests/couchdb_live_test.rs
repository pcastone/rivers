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
const COUCH_USER: &str = "admin";
const TIMEOUT: Duration = Duration::from_secs(10);

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
    let password = lockbox_resolve("couchdb/test", "admin");
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
    let password = lockbox_resolve("couchdb/test", "admin");
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
