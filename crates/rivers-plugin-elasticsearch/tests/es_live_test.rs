//! Live integration tests for the Elasticsearch driver.
//!
//! Requires a running Elasticsearch instance. Set RIVERS_TEST_ES_HOST (default: localhost).
//! If the service is unreachable, tests print SKIP and pass.

use std::collections::HashMap;
use std::time::Duration;

use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};
use rivers_plugin_elasticsearch::ElasticsearchDriver;

const ES_PORT: u16 = 9200;
const TIMEOUT: Duration = Duration::from_secs(10);

fn es_host() -> String {
    std::env::var("RIVERS_TEST_ES_HOST").unwrap_or_else(|_| "localhost".to_string())
}

fn conn_params() -> ConnectionParams {
    ConnectionParams {
        host: es_host(),
        port: ES_PORT,
        database: "test-index".into(),
        username: "".into(),
        password: "".into(),
        options: HashMap::new(),
    }
}

/// Try to connect; returns None (with SKIP message) if unreachable.
async fn try_connect() -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    let driver = ElasticsearchDriver;
    match tokio::time::timeout(TIMEOUT, driver.connect(&conn_params())).await {
        Ok(Ok(conn)) => Some(conn),
        Ok(Err(e)) => {
            let host = es_host();
            eprintln!("SKIP: Elasticsearch unreachable at {host}:{ES_PORT} — {e}");
            None
        }
        Err(_) => {
            let host = es_host();
            eprintln!("SKIP: Elasticsearch connection timed out at {host}:{ES_PORT}");
            None
        }
    }
}

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
