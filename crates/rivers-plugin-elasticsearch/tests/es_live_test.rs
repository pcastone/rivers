//! Live integration tests for the Elasticsearch driver.
//!
//! Credentials are resolved from a LockBox keystore at sec/lockbox/.
//! If the service is unreachable, tests print SKIP and pass.

use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_elasticsearch::ElasticsearchDriver;

const TIMEOUT: Duration = Duration::from_secs(10);

fn conn_params() -> ConnectionParams {
    let dir = find_lockbox_dir().expect("cannot find sec/lockbox/");
    let key_str = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let identity: age::x25519::Identity = key_str.trim().parse().unwrap();

    let encrypted = std::fs::read(dir.join("entries/elasticsearch/test.age")).unwrap();
    let password = String::from_utf8(age::decrypt(&identity, &encrypted).unwrap()).unwrap();

    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("entries/elasticsearch/test.meta.json")).unwrap()
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
    let driver = ElasticsearchDriver;
    let params = conn_params();
    match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            eprintln!("SKIP: Elasticsearch unreachable — {e}");
            None
        }
        Err(_) => {
            eprintln!("SKIP: Elasticsearch connection timed out");
            None
        }
    }
}

/// Validates that the Elasticsearch driver can establish a connection and respond to a ping health check.
#[tokio::test]
async fn es_connect_and_ping() {
    let Some(mut conn) = try_connect().await else { return };

    let query = Query::with_operation("ping", "", "");
    let result = tokio::time::timeout(TIMEOUT, conn.execute(&query))
        .await
        .expect("timed out")
        .expect("ping failed");

    assert_eq!(result.affected_rows, 0);
}

/// Validates full document lifecycle: index a document, search for it, delete it, and confirm removal.
#[tokio::test]
async fn es_index_search_delete_roundtrip() {
    let Some(mut conn) = try_connect().await else { return };

    let index_name = format!(
        "rivers-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let unique_tag = format!("tag_{}", uuid_v4_simple());

    // INDEX (insert) a document.
    // The ES driver uses query.target as the index name.
    let index_query = Query::with_operation("index", &index_name, "")
        .param("name", QueryValue::String("Alice".into()))
        .param("age", QueryValue::Integer(30))
        .param("tag", QueryValue::String(unique_tag.clone()));

    let index_result = tokio::time::timeout(TIMEOUT, conn.execute(&index_query))
        .await
        .expect("timed out")
        .expect("index failed");

    assert_eq!(index_result.affected_rows, 1);
    let doc_id = index_result
        .last_insert_id
        .clone()
        .expect("expected a document _id from index");

    // ES needs a refresh to make the document searchable.
    // Small delay to allow the index refresh.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // SEARCH for the document using the unique tag.
    let search_query = Query::with_operation("search", &index_name, "")
        .param(
            "query",
            QueryValue::Json(serde_json::json!({
                "match": { "tag": unique_tag }
            })),
        );

    let search_result = tokio::time::timeout(TIMEOUT, conn.execute(&search_query))
        .await
        .expect("timed out")
        .expect("search failed");

    assert!(
        !search_result.rows.is_empty(),
        "expected at least one search hit"
    );
    assert_eq!(
        search_result.rows[0].get("name"),
        Some(&QueryValue::String("Alice".into()))
    );

    // DELETE the document.
    let delete_query = Query::with_operation("delete", &index_name, "")
        .param("id", QueryValue::String(doc_id));

    let delete_result = tokio::time::timeout(TIMEOUT, conn.execute(&delete_query))
        .await
        .expect("timed out")
        .expect("delete failed");

    assert_eq!(delete_result.affected_rows, 1);

    // Cleanup: delete the test index via a raw HTTP request.
    // (The driver doesn't have a "drop index" operation, but the doc is already deleted.)
}

/// Generate a simple pseudo-unique ID (not a real UUID, but unique enough for tests).
fn uuid_v4_simple() -> String {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", nanos)
}
