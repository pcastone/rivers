#![warn(missing_docs)]
//! Elasticsearch plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using `reqwest` for direct REST API calls.
//! Elasticsearch is HTTP-native, so no specialized client crate is needed.
//!
//! Operations dispatch based on `query.operation`:
//! - search/find/query/select -> POST /{target}/_search
//! - insert/index/create -> POST /{target}/_doc
//! - update -> POST /{target}/_update/{id}
//! - delete/remove -> DELETE /{target}/_doc/{id}
//! - ping -> GET /

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use rivers_driver_sdk::{
    read_connect_timeout, read_max_rows, read_request_timeout, url_encode_path_segment,
    Connection, ConnectionParams, DatabaseDriver, DriverError, Query, QueryResult,
    QueryValue,
};

#[cfg(feature = "plugin-exports")]
use rivers_driver_sdk::{DriverRegistrar, ABI_VERSION};
#[cfg(feature = "plugin-exports")]
use std::sync::Arc;

// ── Driver ─────────────────────────────────────────────────────────────

/// Elasticsearch driver factory — creates connections via REST API.
pub struct ElasticsearchDriver;

#[async_trait]
impl DatabaseDriver for ElasticsearchDriver {
    fn name(&self) -> &str {
        "elasticsearch"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let scheme = params
            .options
            .get("scheme")
            .map(|s| s.as_str())
            .unwrap_or("http");
        let base_url = format!("{}://{}:{}", scheme, params.host, params.port);

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(read_connect_timeout(params)))
            .timeout(Duration::from_secs(read_request_timeout(params)))
            .build()
            .map_err(|e| DriverError::Connection(format!("elasticsearch client build failed: {e}")))?;

        // Verify connectivity with GET / using credentials if provided.
        let mut ping = client.get(&base_url);
        if !params.username.is_empty() {
            ping = ping.basic_auth(&params.username, Some(&params.password));
        }
        let resp = ping
            .send()
            .await
            .map_err(|e| DriverError::Connection(format!("elasticsearch ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Connection(format!(
                "elasticsearch returned status {}",
                resp.status()
            )));
        }

        debug!(
            base_url = %base_url,
            "elasticsearch: connected"
        );

        let default_index = params.options.get("default_index").cloned();
        let max_rows = read_max_rows(params);

        Ok(Box::new(ElasticConnection {
            client,
            base_url,
            username: params.username.clone(),
            password: params.password.clone(),
            default_index,
            max_rows,
        }))
    }

    /// G_R7.2: cdylib plugins ship their own statically-linked tokio
    /// runtime — connect() runs inside an isolated host runtime.
    fn needs_isolated_runtime(&self) -> bool { true }
}

// ── Connection ─────────────────────────────────────────────────────────

/// Active Elasticsearch connection for executing REST API operations.
pub struct ElasticConnection {
    client: Client,
    base_url: String,
    username: String,
    password: String,
    /// Default index name from datasource options (`default_index`).
    /// Used by `resolve_index` as a fallback when the query target is empty.
    default_index: Option<String>,
    /// Maximum number of rows to return from a search. Truncates with a WARN if exceeded.
    max_rows: usize,
}

impl ElasticConnection {
    /// Construct a connection for testing (uses same timeout policy as production).
    #[cfg(test)]
    fn test_instance() -> Self {
        use rivers_driver_sdk::{DEFAULT_CONNECT_TIMEOUT_SECS, DEFAULT_REQUEST_TIMEOUT_SECS};
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS))
            .build()
            .expect("test reqwest client");
        Self {
            client,
            base_url: "http://127.0.0.1:1".into(),
            username: "".into(),
            password: "".into(),
            default_index: None,
            max_rows: rivers_driver_sdk::DEFAULT_MAX_ROWS,
        }
    }

    /// Build a request builder with optional basic auth.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut builder = self.client.request(method, &url);
        if !self.username.is_empty() {
            builder = builder.basic_auth(&self.username, Some(&self.password));
        }
        builder
    }
}

#[async_trait]
impl Connection for ElasticConnection {
    fn admin_operations(&self) -> &[&str] {
        // Elasticsearch index management is not implemented in this driver.
        // DDL must be performed directly via the Elasticsearch REST API.
        &[]
    }

    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        match query.operation.as_str() {
            "search" | "find" | "query" | "select" => self.exec_search(query).await,
            "insert" | "index" | "create" => self.exec_index(query).await,
            "update" => self.exec_update(query).await,
            "delete" | "remove" => self.exec_delete(query).await,
            "ping" => self.exec_ping().await,
            other => Err(DriverError::Unsupported(format!(
                "elasticsearch: unsupported operation '{other}'"
            ))),
        }
    }

    /// Elasticsearch DDL operations (create_index, delete_index, put_mapping,
    /// update_settings) are declared in `admin_operations()` but Elasticsearch
    /// REST API index management is not implemented in this driver.
    ///
    /// All DDL/admin operations return `Unsupported` — use the Elasticsearch
    /// REST API directly or a management tool for index lifecycle operations.
    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::Unsupported(format!(
            "elasticsearch: '{}' is not implemented — Elasticsearch index management \
             requires direct REST API calls outside Rivers",
            query.operation
        )))
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        let resp = self
            .request(reqwest::Method::GET, "/")
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Query(format!(
                "elasticsearch ping returned status {}",
                resp.status()
            )));
        }
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "elasticsearch"
    }
}

// ── Response types for JSON deserialization ────────────────────────────

#[derive(Deserialize)]
struct SearchResponse {
    hits: Option<SearchHits>,
}

#[derive(Deserialize)]
struct SearchHits {
    hits: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct SearchHit {
    _id: String,
    _source: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct IndexResponse {
    _id: String,
}

// ── Operation implementations ──────────────────────────────────────────

impl ElasticConnection {
    /// Resolve the index name: try parsing "index" from the JSON statement first,
    /// then fall through to query.target, and finally to the configured `default_index`.
    fn resolve_index(&self, query: &Query) -> String {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&query.statement) {
            if let Some(idx) = json.get("index").and_then(|v| v.as_str()) {
                return idx.to_string();
            }
        }
        // Strip namespace prefix if present (e.g. "nosql:canary-es" → "canary-es").
        let target = if let Some((_ns, name)) = query.target.split_once(':') {
            name.to_string()
        } else {
            query.target.clone()
        };
        // Use default_index from datasource options when target is empty.
        if target.is_empty() {
            if let Some(ref idx) = self.default_index {
                return idx.clone();
            }
        }
        target
    }

    /// POST /{index}/_search with body from statement JSON or parameters.
    async fn exec_search(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let index = self.resolve_index(query);
        let path = format!("/{}/_search", index);

        // Try to extract search body from the JSON statement first,
        // fall back to converting parameters to JSON.
        let body = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&query.statement) {
            if let Some(body) = json.get("body") {
                body.clone()
            } else {
                // Statement is JSON but has no "body" — use parameters
                params_to_json(&query.parameters)
            }
        } else {
            // Statement is not JSON — use parameters as body
            params_to_json(&query.parameters)
        };

        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch search failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "elasticsearch search returned {status}: {text}"
            )));
        }

        let search: SearchResponse = resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch response parse failed: {e}")))?;

        let mut rows = Vec::new();
        if let Some(hits) = search.hits {
            for hit in hits.hits {
                if rows.len() >= self.max_rows {
                    tracing::warn!(
                        max_rows = self.max_rows,
                        "elasticsearch: result set truncated to max_rows limit"
                    );
                    break;
                }
                let mut row = HashMap::new();
                row.insert(
                    "_id".to_string(),
                    QueryValue::String(hit._id),
                );
                if let Some(source) = hit._source {
                    if let Some(obj) = source.as_object() {
                        for (k, v) in obj {
                            row.insert(k.clone(), json_to_query_value(v));
                        }
                    }
                }
                rows.push(row);
            }
        }

        let count = rows.len() as u64;
        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// POST /{target}/_doc with body from parameters.
    async fn exec_index(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let path = format!("/{}/_doc", self.resolve_index(query));
        let body = params_to_json(&query.parameters);

        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch index failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "elasticsearch index returned {status}: {text}"
            )));
        }

        let index_resp: IndexResponse = resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch response parse failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: Some(index_resp._id),
            column_names: None,
        })
    }

    /// POST /{target}/_update/{id} with body from parameters.
    async fn exec_update(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let id = extract_id(&query.parameters)?;
        // RW4.3.b: URL-encode the document ID so IDs containing `/`, `?`, or
        // other reserved characters don't alter the URL structure.
        let path = format!("/{}/_update/{}", self.resolve_index(query), url_encode_path_segment(&id));

        // Build the update body: fields other than "id" go into "doc".
        let fields: HashMap<String, QueryValue> = query
            .parameters
            .iter()
            .filter(|(k, _)| k.as_str() != "id")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let doc_body = params_to_json(&fields);
        let body = serde_json::json!({ "doc": doc_body });

        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch update failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "elasticsearch update returned {status}: {text}"
            )));
        }

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// DELETE /{target}/_doc/{id}
    async fn exec_delete(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let id = extract_id(&query.parameters)?;
        // RW4.3.b: URL-encode the document ID.
        let path = format!("/{}/_doc/{}", self.resolve_index(query), url_encode_path_segment(&id));

        let resp = self
            .request(reqwest::Method::DELETE, &path)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch delete failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "elasticsearch delete returned {status}: {text}"
            )));
        }

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// GET / — cluster ping.
    async fn exec_ping(&self) -> Result<QueryResult, DriverError> {
        let resp = self
            .request(reqwest::Method::GET, "/")
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("elasticsearch ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Query(format!(
                "elasticsearch ping returned status {}",
                resp.status()
            )));
        }
        Ok(QueryResult::empty())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert query parameters to a JSON object.
fn params_to_json(params: &HashMap<String, QueryValue>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in params {
        map.insert(k.clone(), query_value_to_json(v));
    }
    serde_json::Value::Object(map)
}

/// Convert a QueryValue to a serde_json::Value.
///
/// Delegates to `QueryValue`'s threshold-aware `Serialize` impl (H18.1).
/// Large integers (`|v| > 2⁵³−1`) are emitted as JSON strings rather than
/// numbers — Elasticsearch indexes both, and JS clients consuming the
/// search response would otherwise silently round high-precision IDs.
fn query_value_to_json(value: &QueryValue) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// Convert a serde_json::Value to a QueryValue.
fn json_to_query_value(value: &serde_json::Value) -> QueryValue {
    match value {
        serde_json::Value::Null => QueryValue::Null,
        serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                QueryValue::Float(f)
            } else {
                QueryValue::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => QueryValue::String(s.clone()),
        serde_json::Value::Array(arr) => {
            QueryValue::Array(arr.iter().map(json_to_query_value).collect())
        }
        serde_json::Value::Object(_) => QueryValue::Json(value.clone()),
    }
}

/// Extract the document ID from query parameters.
fn extract_id(params: &HashMap<String, QueryValue>) -> Result<String, DriverError> {
    match params.get("id") {
        Some(QueryValue::String(s)) => Ok(s.clone()),
        Some(QueryValue::Integer(i)) => Ok(i.to_string()),
        Some(_) => Err(DriverError::Query(
            "elasticsearch: 'id' parameter must be a string or integer".into(),
        )),
        None => Err(DriverError::Query(
            "elasticsearch: 'id' parameter is required for update/delete".into(),
        )),
    }
}

// ── Plugin ABI ─────────────────────────────────────────────────────────

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(ElasticsearchDriver));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::DatabaseDriver;
    use std::collections::HashMap;

    fn bad_params() -> ConnectionParams {
        ConnectionParams {
            host: "127.0.0.1".into(),
            port: 1,
            database: "test".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        }
    }

    #[test]
    fn driver_name_is_elasticsearch() {
        let driver = ElasticsearchDriver;
        assert_eq!(driver.name(), "elasticsearch");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(rivers_driver_sdk::ABI_VERSION, 1);
    }

    // ── resolve_index tests ───────────────────────────────────────────

    #[test]
    fn resolve_index_uses_default_index_when_target_empty() {
        let mut conn = ElasticConnection::test_instance();
        conn.default_index = Some("my-default".into());
        let query = Query::with_operation("search", "", "");
        assert_eq!(conn.resolve_index(&query), "my-default");
    }

    #[test]
    fn resolve_index_prefers_target_over_default_index() {
        let mut conn = ElasticConnection::test_instance();
        conn.default_index = Some("my-default".into());
        let query = Query::with_operation("search", "explicit-index", "");
        assert_eq!(conn.resolve_index(&query), "explicit-index");
    }

    #[test]
    fn resolve_index_strips_namespace_prefix() {
        let conn = ElasticConnection::test_instance();
        let query = Query::with_operation("search", "nosql:canary-es", "");
        assert_eq!(conn.resolve_index(&query), "canary-es");
    }

    // ── json_to_query_value tests ─────────────────────────────────────

    #[test]
    fn json_to_query_value_null() {
        let val = serde_json::Value::Null;
        assert_eq!(json_to_query_value(&val), QueryValue::Null);
    }

    #[test]
    fn json_to_query_value_bool() {
        let val = serde_json::json!(true);
        assert_eq!(json_to_query_value(&val), QueryValue::Boolean(true));

        let val = serde_json::json!(false);
        assert_eq!(json_to_query_value(&val), QueryValue::Boolean(false));
    }

    #[test]
    fn json_to_query_value_integer() {
        let val = serde_json::json!(42);
        assert_eq!(json_to_query_value(&val), QueryValue::Integer(42));
    }

    #[test]
    fn json_to_query_value_float() {
        let val = serde_json::json!(3.14);
        assert_eq!(json_to_query_value(&val), QueryValue::Float(3.14));
    }

    #[test]
    fn json_to_query_value_string() {
        let val = serde_json::json!("hello");
        assert_eq!(
            json_to_query_value(&val),
            QueryValue::String("hello".into())
        );
    }

    #[test]
    fn json_to_query_value_array() {
        let val = serde_json::json!([1, "two", null]);
        let result = json_to_query_value(&val);
        match result {
            QueryValue::Array(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], QueryValue::Integer(1));
                assert_eq!(items[1], QueryValue::String("two".into()));
                assert_eq!(items[2], QueryValue::Null);
            }
            other => panic!("expected QueryValue::Array, got: {other:?}"),
        }
    }

    #[test]
    fn json_to_query_value_object() {
        let val = serde_json::json!({"key": "value"});
        let result = json_to_query_value(&val);
        match result {
            QueryValue::Json(v) => {
                assert!(v.is_object());
                assert_eq!(v["key"], "value");
            }
            other => panic!("expected QueryValue::Json, got: {other:?}"),
        }
    }

    // ── query_value_to_json tests ─────────────────────────────────────

    #[test]
    fn query_value_to_json_roundtrip() {
        let cases: Vec<(QueryValue, serde_json::Value)> = vec![
            (QueryValue::Null, serde_json::Value::Null),
            (QueryValue::Boolean(true), serde_json::json!(true)),
            (QueryValue::Integer(42), serde_json::json!(42)),
            (QueryValue::Float(3.14), serde_json::json!(3.14)),
            (
                QueryValue::String("hello".into()),
                serde_json::json!("hello"),
            ),
        ];
        for (qv, expected) in cases {
            assert_eq!(query_value_to_json(&qv), expected);
        }
    }

    // ── params_to_json test ───────────────────────────────────────────

    #[test]
    fn params_to_json_builds_object() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), QueryValue::String("alice".into()));
        params.insert("age".to_string(), QueryValue::Integer(30));
        let json = params_to_json(&params);
        assert!(json.is_object());
        let obj = json.as_object().unwrap();
        assert_eq!(obj["name"], "alice");
        assert_eq!(obj["age"], 30);
    }

    // ── extract_id tests ──────────────────────────────────────────────

    #[test]
    fn extract_id_string() {
        let mut params = HashMap::new();
        params.insert("id".to_string(), QueryValue::String("doc123".into()));
        assert_eq!(extract_id(&params).unwrap(), "doc123");
    }

    #[test]
    fn extract_id_integer() {
        let mut params = HashMap::new();
        params.insert("id".to_string(), QueryValue::Integer(42));
        assert_eq!(extract_id(&params).unwrap(), "42");
    }

    #[test]
    fn extract_id_missing_returns_error() {
        let params: HashMap<String, QueryValue> = HashMap::new();
        assert!(extract_id(&params).is_err());
    }

    #[test]
    fn extract_id_wrong_type_returns_error() {
        let mut params = HashMap::new();
        params.insert("id".to_string(), QueryValue::Boolean(true));
        assert!(extract_id(&params).is_err());
    }

    // ── connect with bad host ─────────────────────────────────────────

    #[tokio::test]
    async fn connect_bad_host_returns_connection_error() {
        let driver = ElasticsearchDriver;
        let params = bad_params();
        let result = driver.connect(&params).await;
        match result {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("elasticsearch"),
                    "error should mention elasticsearch: {msg}"
                );
            }
            Err(other) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(_) => panic!("expected connection error, but got Ok"),
        }
    }

    // ── admin_operations ─────────────────────────────────────────────

    #[test]
    fn admin_operations_is_empty() {
        let conn = ElasticConnection::test_instance();
        assert!(conn.admin_operations().is_empty(), "elasticsearch advertises no DDL operations");
    }

    // ── DDL guard blocks SQL DDL even with empty admin_operations ────

    #[test]
    fn ddl_drop_is_rejected() {
        let conn = ElasticConnection::test_instance();
        let query = Query::new("my_index", "DROP TABLE users");
        let result = rivers_driver_sdk::check_admin_guard(&query, conn.admin_operations());
        assert!(result.is_some(), "DROP TABLE must be rejected by admin guard even with empty admin_operations");
        let msg = result.unwrap();
        assert!(
            msg.contains("DDL") || msg.contains("rejected"),
            "error must indicate rejection: {msg}"
        );
    }

    #[test]
    fn ddl_create_is_rejected() {
        let conn = ElasticConnection::test_instance();
        let query = Query::new("my_index", "CREATE TABLE foo (id INT)");
        let result = rivers_driver_sdk::check_admin_guard(&query, conn.admin_operations());
        assert!(result.is_some(), "CREATE TABLE must be rejected by admin guard");
    }

    #[test]
    fn ddl_alter_is_rejected() {
        let conn = ElasticConnection::test_instance();
        let query = Query::new("my_index", "ALTER TABLE foo ADD COLUMN bar TEXT");
        let result = rivers_driver_sdk::check_admin_guard(&query, conn.admin_operations());
        assert!(result.is_some(), "ALTER TABLE must be rejected by admin guard");
    }

    #[test]
    fn ddl_truncate_is_rejected() {
        let conn = ElasticConnection::test_instance();
        let query = Query::new("my_index", "TRUNCATE TABLE foo");
        let result = rivers_driver_sdk::check_admin_guard(&query, conn.admin_operations());
        assert!(result.is_some(), "TRUNCATE TABLE must be rejected by admin guard");
    }

    // ── Normal operations pass the guard ────────────────────────────

    #[test]
    fn normal_search_operation_is_allowed() {
        let conn = ElasticConnection::test_instance();
        let query = Query::with_operation("search", "my_index", "");
        let result = rivers_driver_sdk::check_admin_guard(&query, conn.admin_operations());
        assert!(result.is_none(), "search must not be blocked by admin guard");
    }

    #[test]
    fn normal_index_operation_is_allowed() {
        let conn = ElasticConnection::test_instance();
        let query = Query::with_operation("index", "my_index", "");
        let result = rivers_driver_sdk::check_admin_guard(&query, conn.admin_operations());
        assert!(result.is_none(), "index must not be blocked by admin guard");
    }

    // ── ddl_execute returns Unsupported ───────────────────────────────

    #[tokio::test]
    async fn ddl_execute_create_index_is_unsupported() {
        let mut conn = ElasticConnection::test_instance();
        let query = Query::with_operation("create_index", "my_index", "");
        let result = conn.ddl_execute(&query).await;
        match result {
            Err(DriverError::Unsupported(msg)) => {
                assert!(
                    msg.contains("create_index"),
                    "error should name the operation: {msg}"
                );
                assert!(
                    msg.contains("elasticsearch"),
                    "error should mention elasticsearch: {msg}"
                );
            }
            other => panic!("expected DriverError::Unsupported, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ddl_execute_all_admin_ops_are_unsupported() {
        let mut conn = ElasticConnection::test_instance();
        for op in &["create_index", "delete_index", "put_mapping", "update_settings"] {
            let query = Query::with_operation(op, "my_index", "");
            match conn.ddl_execute(&query).await {
                Err(DriverError::Unsupported(_)) => {}
                other => panic!("op '{}' expected Unsupported, got: {other:?}", op),
            }
        }
    }
}
