//! CouchDB plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using `reqwest` HTTP client.
//! CouchDB is a REST API — all operations are HTTP requests.
//!
//! Operations dispatch based on `query.operation`:
//! - find/select/query → POST /{db}/_find (Mango query)
//! - get → GET /{db}/{doc_id}
//! - insert/create → POST /{db}
//! - update → PUT /{db}/{doc_id} (fetches _rev first)
//! - delete/remove → DELETE /{db}/{doc_id}?rev={rev}
//! - view → GET /{db}/_design/{ddoc}/_view/{view}
//! - ping → GET /

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use tracing::debug;

use rivers_driver_sdk::{
    ABI_VERSION, Connection, ConnectionParams, DatabaseDriver, DriverError, DriverRegistrar,
    Query, QueryResult, QueryValue,
};

// ── Driver ─────────────────────────────────────────────────────────────

pub struct CouchDBDriver;

#[async_trait]
impl DatabaseDriver for CouchDBDriver {
    fn name(&self) -> &str {
        "couchdb"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let port = if params.port == 0 { 5984 } else { params.port };
        let base_url = format!("http://{}:{}", params.host, port);

        let (username, password) = if params.username.is_empty() {
            (None, None)
        } else {
            (
                Some(params.username.clone()),
                Some(params.password.clone()),
            )
        };

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| DriverError::Connection(format!("couchdb client build: {e}")))?;

        // Verify connectivity
        let mut req = client.get(&base_url);
        if let (Some(u), Some(p)) = (&username, &password) {
            req = req.basic_auth(u, Some(p));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DriverError::Connection(format!("couchdb connect: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Connection(format!(
                "couchdb connect: HTTP {}",
                resp.status()
            )));
        }

        debug!(
            host = %params.host,
            port = %port,
            database = %params.database,
            "couchdb: connected"
        );

        Ok(Box::new(CouchDBConnection {
            client,
            base_url,
            database: params.database.clone(),
            username,
            password,
        }))
    }
}

// ── Connection ─────────────────────────────────────────────────────────

pub struct CouchDBConnection {
    client: Client,
    base_url: String,
    database: String,
    username: Option<String>,
    password: Option<String>,
}

impl CouchDBConnection {
    fn db_url(&self) -> String {
        format!("{}/{}", self.base_url, self.database)
    }

    /// Apply basic auth to a request builder if credentials are present.
    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match (&self.username, &self.password) {
            (Some(u), Some(p)) => req.basic_auth(u, Some(p)),
            (Some(u), None) => req.basic_auth(u, None::<&str>),
            _ => req,
        }
    }
}

#[async_trait]
impl Connection for CouchDBConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        match query.operation.as_str() {
            "find" | "select" | "query" => self.exec_find(query).await,
            "get" => self.exec_get(query).await,
            "insert" | "create" => self.exec_insert(query).await,
            "update" => self.exec_update(query).await,
            "delete" | "remove" | "del" => self.exec_delete(query).await,
            "view" => self.exec_view(query).await,
            "ping" => self.exec_ping().await,
            other => Err(DriverError::Unsupported(format!(
                "couchdb: unsupported operation '{other}'"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        let resp = self
            .auth(self.client.get(&self.base_url))
            .send()
            .await
            .map_err(|e| DriverError::Connection(format!("couchdb ping: {e}")))?;
        if !resp.status().is_success() {
            return Err(DriverError::Connection(format!(
                "couchdb ping: HTTP {}",
                resp.status()
            )));
        }
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "couchdb"
    }
}

impl CouchDBConnection {
    /// POST /{db}/_find — Mango query
    async fn exec_find(&self, query: &Query) -> Result<QueryResult, DriverError> {
        // Parse selector from statement (JSON string) or build from parameters
        let selector: serde_json::Value = if !query.statement.is_empty() {
            let mut sel_str = query.statement.clone();
            // Substitute $1, $2 etc. with parameter values
            let mut keys: Vec<&String> = query.parameters.keys().collect();
            keys.sort();
            for (i, key) in keys.iter().enumerate() {
                let placeholder = format!("${}", i + 1);
                if let Some(val) = query.parameters.get(*key) {
                    let replacement = match val {
                        QueryValue::String(s) => s.clone(),
                        QueryValue::Integer(n) => n.to_string(),
                        QueryValue::Float(f) => f.to_string(),
                        QueryValue::Boolean(b) => b.to_string(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    sel_str = sel_str.replace(&placeholder, &replacement);
                }
            }
            let mut parsed: serde_json::Value = serde_json::from_str(&sel_str)
                .map_err(|e| DriverError::Query(format!("couchdb: invalid selector JSON: {e}")))?;
            // Strip "operation" key — it's for Rivers dispatch, not CouchDB
            if let Some(obj) = parsed.as_object_mut() {
                obj.remove("operation");
            }
            // If parsed has a "selector" key, use as-is; otherwise wrap it
            if parsed.get("selector").is_none() {
                // The whole object IS the selector
                serde_json::json!({ "selector": parsed })
            } else {
                parsed
            }
        } else {
            // Build selector from parameters
            let mut selector = serde_json::Map::new();
            for (k, v) in &query.parameters {
                selector.insert(k.clone(), query_value_to_json(v));
            }
            serde_json::json!({ "selector": selector })
        };

        let url = format!("{}/_find", self.db_url());
        let resp = self
            .auth(self.client.post(&url))
            .json(&selector)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb find: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!("couchdb find failed: {body}")));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb find response: {e}")))?;

        let docs = body
            .get("docs")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let rows: Vec<HashMap<String, QueryValue>> = docs
            .into_iter()
            .map(|doc| json_object_to_row(&doc))
            .collect();

        let count = rows.len() as u64;
        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
        })
    }

    /// GET /{db}/{doc_id}
    async fn exec_get(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let doc_id = get_param_str(&query.parameters, "id")
            .or_else(|_| get_param_str(&query.parameters, "doc_id"))
            .or_else(|_| get_param_str(&query.parameters, "_id"))?;

        let url = format!("{}/{}", self.db_url(), doc_id);
        let resp = self
            .auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb get: {e}")))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(QueryResult {
                rows: Vec::new(),
                affected_rows: 0,
                last_insert_id: None,
            });
        }

        let doc: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb get response: {e}")))?;

        let row = json_object_to_row(&doc);
        Ok(QueryResult {
            rows: vec![row],
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    /// POST /{db} — create document
    async fn exec_insert(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let doc: serde_json::Value = if !query.statement.is_empty() {
            serde_json::from_str(&query.statement)
                .map_err(|e| DriverError::Query(format!("couchdb insert: invalid JSON: {e}")))?
        } else {
            let mut obj = serde_json::Map::new();
            for (k, v) in &query.parameters {
                if k != "_rev" {
                    obj.insert(k.clone(), query_value_to_json(v));
                }
            }
            serde_json::Value::Object(obj)
        };

        let resp = self
            .auth(self.client.post(&self.db_url()))
            .json(&doc)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb insert: {e}")))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb insert response: {e}")))?;

        let id = body.get("id").and_then(|v| v.as_str()).map(String::from);

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: id,
        })
    }

    /// PUT /{db}/{doc_id} — update document (fetches _rev first)
    async fn exec_update(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let doc_id = get_param_str(&query.parameters, "id")
            .or_else(|_| get_param_str(&query.parameters, "_id"))?;

        // Fetch current _rev
        let get_url = format!("{}/{}", self.db_url(), doc_id);
        let get_resp = self
            .auth(self.client.get(&get_url))
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb update (fetch rev): {e}")))?;

        if get_resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(DriverError::Query(format!(
                "couchdb update: document '{doc_id}' not found"
            )));
        }

        let current: serde_json::Value = get_resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb update (parse rev): {e}")))?;

        let rev = current
            .get("_rev")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DriverError::Query("couchdb update: missing _rev".into()))?;

        // Build updated document
        let mut doc = if !query.statement.is_empty() {
            serde_json::from_str::<serde_json::Value>(&query.statement)
                .map_err(|e| DriverError::Query(format!("couchdb update: invalid JSON: {e}")))?
        } else {
            let mut obj = serde_json::Map::new();
            for (k, v) in &query.parameters {
                if k != "id" && k != "_id" {
                    obj.insert(k.clone(), query_value_to_json(v));
                }
            }
            serde_json::Value::Object(obj)
        };

        // Inject _rev
        if let Some(obj) = doc.as_object_mut() {
            obj.insert("_rev".into(), serde_json::Value::String(rev.to_string()));
        }

        let put_url = format!("{}/{}", self.db_url(), doc_id);
        let resp = self
            .auth(self.client.put(&put_url))
            .json(&doc)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb update: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!("couchdb update failed: {body}")));
        }

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    /// DELETE /{db}/{doc_id}?rev={rev}
    async fn exec_delete(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let doc_id = get_param_str(&query.parameters, "id")
            .or_else(|_| get_param_str(&query.parameters, "_id"))?;

        // Fetch current _rev
        let get_url = format!("{}/{}", self.db_url(), doc_id);
        let get_resp = self
            .auth(self.client.get(&get_url))
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb delete (fetch rev): {e}")))?;

        if get_resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(QueryResult {
                rows: Vec::new(),
                affected_rows: 0,
                last_insert_id: None,
            });
        }

        let current: serde_json::Value = get_resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb delete (parse rev): {e}")))?;

        let rev = current
            .get("_rev")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DriverError::Query("couchdb delete: missing _rev".into()))?;

        let del_url = format!("{}/{}?rev={}", self.db_url(), doc_id, rev);
        let resp = self
            .auth(self.client.delete(&del_url))
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb delete: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!("couchdb delete failed: {body}")));
        }

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    /// GET /{db}/_design/{ddoc}/_view/{view}
    async fn exec_view(&self, query: &Query) -> Result<QueryResult, DriverError> {
        // statement format: "design_doc/view_name"
        let parts: Vec<&str> = query.statement.splitn(2, '/').collect();
        if parts.len() != 2 {
            return Err(DriverError::Query(
                "couchdb view: statement must be 'design_doc/view_name'".into(),
            ));
        }

        let url = format!(
            "{}/_design/{}/_view/{}",
            self.db_url(),
            parts[0],
            parts[1]
        );

        // Build query parameters with proper URL encoding via reqwest
        let query_params: Vec<(String, String)> = query
            .parameters
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    QueryValue::String(s) => format!("\"{}\"", s),
                    QueryValue::Integer(n) => n.to_string(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                (k.clone(), val)
            })
            .collect();

        let resp = self
            .auth(self.client.get(&url))
            .query(&query_params)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb view: {e}")))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("couchdb view response: {e}")))?;

        let view_rows = body
            .get("rows")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        let rows: Vec<HashMap<String, QueryValue>> = view_rows
            .into_iter()
            .map(|row| {
                let mut map = HashMap::new();
                if let Some(id) = row.get("id") {
                    map.insert("id".into(), json_to_query_value(id));
                }
                if let Some(key) = row.get("key") {
                    map.insert("key".into(), json_to_query_value(key));
                }
                if let Some(value) = row.get("value") {
                    map.insert("value".into(), json_to_query_value(value));
                }
                map
            })
            .collect();

        let count = rows.len() as u64;
        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
        })
    }

    async fn exec_ping(&mut self) -> Result<QueryResult, DriverError> {
        self.ping().await?;
        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 0,
            last_insert_id: None,
        })
    }
}

// ── Type Conversion ──────────────────────────────────────────────────

fn json_to_query_value(val: &serde_json::Value) -> QueryValue {
    match val {
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
        serde_json::Value::Array(a) => {
            QueryValue::Array(a.iter().map(json_to_query_value).collect())
        }
        serde_json::Value::Object(_) => QueryValue::Json(val.clone()),
    }
}

fn query_value_to_json(val: &QueryValue) -> serde_json::Value {
    match val {
        QueryValue::Null => serde_json::Value::Null,
        QueryValue::Boolean(b) => serde_json::Value::Bool(*b),
        QueryValue::Integer(i) => serde_json::json!(i),
        QueryValue::Float(f) => serde_json::json!(f),
        QueryValue::String(s) => serde_json::Value::String(s.clone()),
        QueryValue::Array(a) => {
            serde_json::Value::Array(a.iter().map(query_value_to_json).collect())
        }
        QueryValue::Json(v) => v.clone(),
    }
}

fn json_object_to_row(doc: &serde_json::Value) -> HashMap<String, QueryValue> {
    let mut row = HashMap::new();
    if let Some(obj) = doc.as_object() {
        for (k, v) in obj {
            row.insert(k.clone(), json_to_query_value(v));
        }
    }
    row
}

fn get_param_str(
    params: &HashMap<String, QueryValue>,
    name: &str,
) -> Result<String, DriverError> {
    match params.get(name) {
        Some(QueryValue::String(s)) => Ok(s.clone()),
        Some(QueryValue::Integer(i)) => Ok(i.to_string()),
        Some(other) => Ok(format!("{:?}", other)),
        None => Err(DriverError::Query(format!(
            "couchdb: missing required parameter '{name}'"
        ))),
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
    registrar.register_database_driver(Arc::new(CouchDBDriver));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::DatabaseDriver;

    #[test]
    fn driver_name() {
        assert_eq!(CouchDBDriver.name(), "couchdb");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(_rivers_abi_version(), ABI_VERSION);
    }

    #[test]
    fn json_to_query_value_string() {
        let v = json_to_query_value(&serde_json::json!("hello"));
        assert_eq!(v, QueryValue::String("hello".into()));
    }

    #[test]
    fn json_to_query_value_integer() {
        let v = json_to_query_value(&serde_json::json!(42));
        assert_eq!(v, QueryValue::Integer(42));
    }

    #[test]
    fn json_to_query_value_null() {
        let v = json_to_query_value(&serde_json::Value::Null);
        assert_eq!(v, QueryValue::Null);
    }

    #[test]
    fn json_to_query_value_array() {
        let v = json_to_query_value(&serde_json::json!([1, "two"]));
        match v {
            QueryValue::Array(a) => {
                assert_eq!(a.len(), 2);
                assert_eq!(a[0], QueryValue::Integer(1));
                assert_eq!(a[1], QueryValue::String("two".into()));
            }
            other => panic!("expected Array, got {:?}", other),
        }
    }

    #[test]
    fn json_object_to_row_converts_fields() {
        let doc = serde_json::json!({"name": "Alice", "age": 30, "active": true});
        let row = json_object_to_row(&doc);
        assert_eq!(row.get("name"), Some(&QueryValue::String("Alice".into())));
        assert_eq!(row.get("age"), Some(&QueryValue::Integer(30)));
        assert_eq!(row.get("active"), Some(&QueryValue::Boolean(true)));
    }

    #[test]
    fn query_value_roundtrip() {
        let original = QueryValue::String("test".into());
        let json = query_value_to_json(&original);
        let back = json_to_query_value(&json);
        assert_eq!(original, back);
    }

    #[tokio::test]
    async fn connect_bad_host_returns_connection_error() {
        let driver = CouchDBDriver;
        let params = ConnectionParams {
            host: "127.0.0.1".into(),
            port: 1,
            database: "test".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            driver.connect(&params),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(_))) => {}
            Ok(Err(other)) => panic!("expected Connection error, got: {other:?}"),
            Ok(Ok(_)) => panic!("expected error"),
            Err(_) => {} // timeout OK
        }
    }
}
