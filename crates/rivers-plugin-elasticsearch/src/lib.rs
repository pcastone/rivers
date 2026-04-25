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
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, DriverRegistrar, Query, QueryResult,
    QueryValue, ABI_VERSION,
};

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

        let client = Client::new();

        // Verify connectivity with GET /
        let resp = client
            .get(&base_url)
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

        Ok(Box::new(ElasticConnection {
            client,
            base_url,
            username: params.username.clone(),
            password: params.password.clone(),
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
}

impl ElasticConnection {
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
        &["create_index", "delete_index", "put_mapping", "update_settings"]
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
    /// fall back to query.target. This handles namespaced targets like "nosql:canary-es".
    fn resolve_index(&self, query: &Query) -> String {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&query.statement) {
            if let Some(idx) = json.get("index").and_then(|v| v.as_str()) {
                return idx.to_string();
            }
        }
        // Fall back to target, stripping namespace prefix if present
        if let Some((_ns, name)) = query.target.split_once(':') {
            name.to_string()
        } else {
            query.target.clone()
        }
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
        let path = format!("/{}/_update/{}", self.resolve_index(query), id);

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
        let path = format!("/{}/_doc/{}", self.resolve_index(query), id);

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
fn query_value_to_json(value: &QueryValue) -> serde_json::Value {
    match value {
        QueryValue::Null => serde_json::Value::Null,
        QueryValue::Boolean(b) => serde_json::Value::Bool(*b),
        QueryValue::Integer(i) => serde_json::json!(i),
        QueryValue::Float(f) => serde_json::json!(f),
        QueryValue::String(s) => serde_json::Value::String(s.clone()),
        QueryValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(query_value_to_json).collect())
        }
        QueryValue::Json(v) => v.clone(),
    }
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
        assert_eq!(ABI_VERSION, 1);
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
}
