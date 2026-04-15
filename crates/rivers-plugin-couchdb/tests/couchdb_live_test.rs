//! Live integration tests for the CouchDB driver.
//!
//! Connection info resolved from LockBox keystore (see `sec/lockbox/`).
//! If the service is unreachable, tests print SKIP and pass.

use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_couchdb::CouchDBDriver;

const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    let encrypted = std::fs::read(dir.join("entries/couchdb/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/couchdb/test.meta.json")).unwrap()
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

/// Ensure the test database exists before tests run.
/// CouchDB requires the database to be explicitly created.
async fn ensure_db_exists() -> bool {
    let params = conn_params();
    let url = format!(
        "http://{}:{}@{}:{}/{}",
        params.username, params.password, params.host, params.port, params.database
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
    let params = conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: CouchDB unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: CouchDB connection timed out");
            None
        }
    }
}

/// Validates that the CouchDB driver can establish a connection and respond to a ping health check.
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

/// Validates full document lifecycle: insert a document, find it by ID, delete it, and confirm removal.
#[tokio::test]
async fn couchdb_insert_find_delete_roundtrip() {
    // Ensure database exists first.
    if !ensure_db_exists().await {
        eprintln!("SKIP: Could not create/verify CouchDB database");
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
