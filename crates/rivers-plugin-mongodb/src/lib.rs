#![warn(missing_docs)]
//! MongoDB plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using the official `mongodb` 3.x driver.
//! Operations dispatch based on `query.operation`:
//! - find/select/query -> collection.find()
//! - insert/create -> collection.insert_one()
//! - update -> collection.update_many()
//! - delete/remove -> collection.delete_many()
//! - ping -> db.run_command(ping: 1)

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use mongodb::bson::{self, doc, Bson, Document};
use mongodb::options::{ClientOptions, Credential};
use mongodb::{Client, ClientSession, Database};
use tracing::debug;

use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, DriverRegistrar, Query, QueryResult,
    QueryValue, ABI_VERSION,
};

// ── Driver ─────────────────────────────────────────────────────────────

/// MongoDB driver factory — creates connections via the official `mongodb` crate.
pub struct MongoDriver;

#[async_trait]
impl DatabaseDriver for MongoDriver {
    fn name(&self) -> &str {
        "mongodb"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let auth_source = params.options.get("auth_source").map(|s| s.as_str()).unwrap_or("admin");
        let replica_set = params.options.get("replica_set");

        // Build a base URI without credentials to avoid embedding secrets in the URI string.
        let mut base_uri = format!("mongodb://{}:{}/", params.host, params.port);
        if let Some(rs) = replica_set {
            base_uri.push_str(&format!("?replicaSet={}", rs));
        }

        let mut options = ClientOptions::parse(&base_uri)
            .await
            .map_err(|e| DriverError::Connection(format!("mongodb options parse failed: {e}")))?;

        // Set credentials via the structured Credential type when auth is provided.
        if !params.username.is_empty() {
            options.credential = Some(
                Credential::builder()
                    .username(params.username.clone())
                    .password(params.password.clone())
                    .source(Some(auth_source.to_string()))
                    .build(),
            );
        }

        let client = Client::with_options(options)
            .map_err(|e| DriverError::Connection(format!("mongodb connection failed: {e}")))?;

        let db = client.database(&params.database);

        // Verify connectivity with a ping.
        db.run_command(doc! { "ping": 1 })
            .await
            .map_err(|e| DriverError::Connection(format!("mongodb ping failed: {e}")))?;

        debug!(
            host = %params.host,
            port = %params.port,
            database = %params.database,
            "mongodb: connected"
        );

        let max_rows = params
            .options
            .get("max_rows")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_ROWS);

        Ok(Box::new(MongoConnection { db, session: None, max_rows }))
    }

    /// G_R7.2: cdylib plugin runs connect() in an isolated runtime.
    fn needs_isolated_runtime(&self) -> bool { true }
}

// ── Connection ─────────────────────────────────────────────────────────

/// Default maximum number of documents returned by a find() query (RW2.6.b).
const DEFAULT_MAX_ROWS: usize = 1_000;

/// Active MongoDB connection wrapping a database handle.
pub struct MongoConnection {
    db: Database,
    session: Option<ClientSession>,
    /// Maximum number of rows returned by find(). Configurable via connect options.
    max_rows: usize,
}

#[async_trait]
impl Connection for MongoConnection {
    fn admin_operations(&self) -> &[&str] {
        &["create_collection", "drop_collection", "drop_database", "create_index", "drop_index", "rename_collection"]
    }

    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        match query.operation.as_str() {
            "find" | "select" | "query" => self.exec_find(query).await,
            "insert" | "create" => self.exec_insert(query).await,
            "update" => self.exec_update(query).await,
            "delete" | "remove" => self.exec_delete(query).await,
            "ping" => self.exec_ping().await,
            other => Err(DriverError::Unsupported(format!(
                "mongodb: unsupported operation '{other}'"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        self.db
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|e| DriverError::Query(format!("mongodb ping failed: {e}")))?;
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "mongodb"
    }

    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        let mut session = self.db
            .client()
            .start_session()
            .await
            .map_err(|e| DriverError::Query(format!("mongodb start session: {e}")))?;
        session
            .start_transaction()
            .await
            .map_err(|e| DriverError::Query(format!("mongodb BEGIN: {e}")))?;
        self.session = Some(session);
        Ok(())
    }

    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        match self.session.as_mut() {
            Some(session) => {
                session
                    .commit_transaction()
                    .await
                    .map_err(|e| DriverError::Query(format!("mongodb COMMIT: {e}")))?;
                self.session = None;
                Ok(())
            }
            None => Err(DriverError::Query("no active mongodb transaction".into())),
        }
    }

    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        match self.session.as_mut() {
            Some(session) => {
                session
                    .abort_transaction()
                    .await
                    .map_err(|e| DriverError::Query(format!("mongodb ROLLBACK: {e}")))?;
                self.session = None;
                Ok(())
            }
            None => Err(DriverError::Query("no active mongodb transaction".into())),
        }
    }
}

impl MongoConnection {
    /// Extract collection name from JSON statement or fall back to query.target.
    fn resolve_collection(&self, query: &Query) -> String {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&query.statement) {
            if let Some(col) = json.get("collection").and_then(|v| v.as_str()) {
                return col.to_string();
            }
        }
        query.target.clone()
    }

    /// Extract filter document from JSON statement, or use query.parameters.
    fn resolve_filter(&self, query: &Query) -> Document {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&query.statement) {
            if let Some(filter) = json.get("filter") {
                if let Ok(bson_doc) = bson::to_document(&filter) {
                    return bson_doc;
                }
            }
        }
        params_to_document(&query.parameters)
    }

    /// Execute a find query, returning matching documents as rows.
    ///
    /// RW2.6.a: passes session to collection.find() when a transaction is active.
    /// RW2.6.b: enforces `max_rows` cap — returns DriverError if exceeded.
    async fn exec_find(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let col_name = self.resolve_collection(query);
        let collection = self.db.collection::<Document>(&col_name);
        let filter = self.resolve_filter(query);
        let max_rows = self.max_rows;

        // RW2.6.a: session-aware find. MongoDB 3.x returns different cursor types
        // for session vs non-session finds, so we handle both branches independently.
        // SessionCursor::advance() requires the session to be passed on each call.
        let rows = if self.session.is_some() {
            // We need both cursor and session in scope simultaneously. Take a ref
            // to the db to build the cursor while the session is held by self.
            let session = self.session.as_mut().unwrap();
            let mut cursor = collection
                .find(filter)
                .session(&mut *session)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb find (txn) failed: {e}")))?;

            let mut rows: Vec<std::collections::HashMap<String, QueryValue>> = Vec::new();
            // SessionCursor::advance() requires the session on each call.
            let session = self.session.as_mut().unwrap();
            while cursor
                .advance(session)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb cursor error: {e}")))?
            {
                if rows.len() >= max_rows {
                    return Err(DriverError::Query(format!(
                        "mongodb find: result set exceeds max_rows limit ({max_rows}). \
                         Use pagination or increase max_rows in datasource options."
                    )));
                }
                let doc: Document = cursor
                    .deserialize_current()
                    .map_err(|e| DriverError::Query(format!("mongodb deserialize error: {e}")))?;
                rows.push(document_to_row(&doc));
            }
            rows
        } else {
            let mut cursor = collection
                .find(filter)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb find failed: {e}")))?;

            let mut rows: Vec<std::collections::HashMap<String, QueryValue>> = Vec::new();
            // mongodb 3.x Cursor provides advance()/deserialize_current() for iteration.
            while cursor
                .advance()
                .await
                .map_err(|e| DriverError::Query(format!("mongodb cursor error: {e}")))?
            {
                if rows.len() >= max_rows {
                    return Err(DriverError::Query(format!(
                        "mongodb find: result set exceeds max_rows limit ({max_rows}). \
                         Use pagination or increase max_rows in datasource options."
                    )));
                }
                let doc: Document = cursor
                    .deserialize_current()
                    .map_err(|e| DriverError::Query(format!("mongodb deserialize error: {e}")))?;
                rows.push(document_to_row(&doc));
            }
            rows
        };

        let count = rows.len() as u64;
        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// Execute an insert_one, returning the inserted document's _id.
    ///
    /// RW2.6.a: passes session to insert_one() when a transaction is active.
    async fn exec_insert(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let col_name = self.resolve_collection(query);
        let collection = self.db.collection::<Document>(&col_name);
        let doc = params_to_document(&query.parameters);

        let result = if let Some(ref mut session) = self.session {
            collection
                .insert_one(doc)
                .session(session)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb insert_one (txn) failed: {e}")))?
        } else {
            collection
                .insert_one(doc)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb insert_one failed: {e}")))?
        };

        let insert_id = bson_to_string(&result.inserted_id);

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: Some(insert_id),
            column_names: None,
        })
    }

    /// Execute an update_many with filter and $set from parameters.
    ///
    /// RW2.6.a: passes session to update_many() when a transaction is active.
    /// RW2.6.c: requires a non-empty filter to prevent broad updates.
    async fn exec_update(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let col_name = self.resolve_collection(query);
        let collection = self.db.collection::<Document>(&col_name);

        // Split parameters: "_filter" key holds the filter doc, rest is the update.
        let (filter, update_fields) = split_filter_and_fields(&query.parameters);

        // RW2.6.c: require a non-empty filter for update/delete to prevent
        // accidental broad modifications. An explicit `allow_full_scan: true`
        // query option (passed via parameters) bypasses this guard.
        let allow_full_scan = query.parameters
            .get("allow_full_scan")
            .and_then(|v| if let QueryValue::Boolean(b) = v { Some(*b) } else { None })
            .unwrap_or(false);

        if filter.is_empty() && !allow_full_scan {
            return Err(DriverError::Query(
                "mongodb update: empty filter would modify all documents. \
                 Add a filter via the '_filter' parameter, or set allow_full_scan=true \
                 to explicitly allow collection-wide updates.".into()
            ));
        }

        let update = doc! { "$set": params_to_document(&update_fields) };

        let result = if let Some(ref mut session) = self.session {
            collection
                .update_many(filter, update)
                .session(session)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb update_many (txn) failed: {e}")))?
        } else {
            collection
                .update_many(filter, update)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb update_many failed: {e}")))?
        };

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: result.modified_count,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// Execute a delete_many with filter from parameters.
    ///
    /// RW2.6.a: passes session to delete_many() when a transaction is active.
    /// RW2.6.c: requires a non-empty filter to prevent broad deletes.
    async fn exec_delete(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let col_name = self.resolve_collection(query);
        let collection = self.db.collection::<Document>(&col_name);
        let filter = params_to_document(&query.parameters);

        // RW2.6.c: require a non-empty filter for delete.
        let allow_full_scan = query.parameters
            .get("allow_full_scan")
            .and_then(|v| if let QueryValue::Boolean(b) = v { Some(*b) } else { None })
            .unwrap_or(false);

        if filter.is_empty() && !allow_full_scan {
            return Err(DriverError::Query(
                "mongodb delete: empty filter would delete all documents. \
                 Add a filter via query parameters, or set allow_full_scan=true \
                 to explicitly allow collection-wide deletes.".into()
            ));
        }

        let result = if let Some(ref mut session) = self.session {
            collection
                .delete_many(filter)
                .session(session)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb delete_many (txn) failed: {e}")))?
        } else {
            collection
                .delete_many(filter)
                .await
                .map_err(|e| DriverError::Query(format!("mongodb delete_many failed: {e}")))?
        };

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: result.deleted_count,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// Ping the database.
    async fn exec_ping(&self) -> Result<QueryResult, DriverError> {
        self.db
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|e| DriverError::Query(format!("mongodb ping failed: {e}")))?;
        Ok(QueryResult::empty())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert query parameters to a BSON Document.
fn params_to_document(params: &HashMap<String, QueryValue>) -> Document {
    let mut doc = Document::new();
    for (k, v) in params {
        doc.insert(k, query_value_to_bson(v));
    }
    doc
}

/// Convert a QueryValue to a BSON value.
///
/// `UInt` maps to `Bson::Int64` when the value fits in `i64`. Above
/// `i64::MAX`, BSON's `Int64` cannot represent it; we fall back to
/// `Bson::Decimal128` (parsed from the decimal text representation), and
/// if that fails for any reason, to `Bson::String` carrying the decimal
/// digits — value-preserving rather than silently truncating.
fn query_value_to_bson(value: &QueryValue) -> Bson {
    match value {
        QueryValue::Null => Bson::Null,
        QueryValue::Boolean(b) => Bson::Boolean(*b),
        QueryValue::Integer(i) => Bson::Int64(*i),
        QueryValue::UInt(u) => {
            if let Ok(i) = i64::try_from(*u) {
                Bson::Int64(i)
            } else {
                let s = u.to_string();
                match s.parse::<bson::Decimal128>() {
                    Ok(d) => Bson::Decimal128(d),
                    Err(_) => Bson::String(s),
                }
            }
        }
        QueryValue::Float(f) => Bson::Double(*f),
        QueryValue::String(s) => Bson::String(s.clone()),
        QueryValue::Array(arr) => {
            Bson::Array(arr.iter().map(query_value_to_bson).collect())
        }
        QueryValue::Json(v) => {
            bson::to_bson(v).unwrap_or(Bson::Null)
        }
    }
}

/// Convert a BSON Document to a row (HashMap<String, QueryValue>).
fn document_to_row(doc: &Document) -> HashMap<String, QueryValue> {
    let mut row = HashMap::new();
    for (k, v) in doc {
        row.insert(k.clone(), bson_to_query_value(v));
    }
    row
}

/// Convert a BSON value to a QueryValue.
fn bson_to_query_value(bson: &Bson) -> QueryValue {
    match bson {
        Bson::Null => QueryValue::Null,
        Bson::Boolean(b) => QueryValue::Boolean(*b),
        Bson::Int32(i) => QueryValue::Integer(*i as i64),
        Bson::Int64(i) => QueryValue::Integer(*i),
        Bson::Double(f) => QueryValue::Float(*f),
        Bson::String(s) => QueryValue::String(s.clone()),
        Bson::ObjectId(oid) => QueryValue::String(oid.to_hex()),
        Bson::DateTime(dt) => QueryValue::String(dt.to_string()),
        Bson::Array(arr) => {
            QueryValue::Array(arr.iter().map(bson_to_query_value).collect())
        }
        Bson::Document(doc) => {
            // Convert sub-document to JSON value.
            let json = bson::to_bson(doc)
                .ok()
                .and_then(|b| serde_json::to_value(&b).ok())
                .unwrap_or(serde_json::Value::Null);
            QueryValue::Json(json)
        }
        other => {
            // Fallback: serialize to JSON.
            let json = serde_json::to_value(other).unwrap_or(serde_json::Value::Null);
            QueryValue::Json(json)
        }
    }
}

/// Convert a BSON value to a string representation (for insert IDs).
fn bson_to_string(bson: &Bson) -> String {
    match bson {
        Bson::ObjectId(oid) => oid.to_hex(),
        Bson::String(s) => s.clone(),
        Bson::Int32(i) => i.to_string(),
        Bson::Int64(i) => i.to_string(),
        other => format!("{other}"),
    }
}

/// Split parameters into a filter document and remaining fields.
///
/// If a `_filter` key exists (as a Json object), it becomes the filter;
/// otherwise, all parameters are used as the filter for backward compat.
fn split_filter_and_fields(
    params: &HashMap<String, QueryValue>,
) -> (Document, HashMap<String, QueryValue>) {
    if let Some(QueryValue::Json(filter_json)) = params.get("_filter") {
        let filter = if let Some(obj) = filter_json.as_object() {
            let mut doc = Document::new();
            for (k, v) in obj {
                let qv = serde_json::from_value::<QueryValue>(v.clone())
                    .unwrap_or(QueryValue::Null);
                doc.insert(k, query_value_to_bson(&qv));
            }
            doc
        } else {
            Document::new()
        };

        let fields: HashMap<String, QueryValue> = params
            .iter()
            .filter(|(k, _)| k.as_str() != "_filter")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        (filter, fields)
    } else {
        // No explicit filter — use empty filter, all params are update fields.
        (Document::new(), params.clone())
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
    registrar.register_database_driver(Arc::new(MongoDriver));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{oid::ObjectId, Bson};
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
    fn driver_name_is_mongodb() {
        let driver = MongoDriver;
        assert_eq!(driver.name(), "mongodb");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }

    // ── query_value_to_bson tests ─────────────────────────────────────

    #[test]
    fn query_value_to_bson_null() {
        assert_eq!(query_value_to_bson(&QueryValue::Null), Bson::Null);
    }

    #[test]
    fn query_value_to_bson_boolean() {
        assert_eq!(
            query_value_to_bson(&QueryValue::Boolean(true)),
            Bson::Boolean(true)
        );
        assert_eq!(
            query_value_to_bson(&QueryValue::Boolean(false)),
            Bson::Boolean(false)
        );
    }

    #[test]
    fn query_value_to_bson_integer() {
        assert_eq!(
            query_value_to_bson(&QueryValue::Integer(42)),
            Bson::Int64(42)
        );
        assert_eq!(
            query_value_to_bson(&QueryValue::Integer(-1)),
            Bson::Int64(-1)
        );
    }

    #[test]
    fn query_value_to_bson_float() {
        assert_eq!(
            query_value_to_bson(&QueryValue::Float(3.14)),
            Bson::Double(3.14)
        );
    }

    #[test]
    fn query_value_to_bson_string() {
        assert_eq!(
            query_value_to_bson(&QueryValue::String("hello".into())),
            Bson::String("hello".into())
        );
    }

    #[test]
    fn query_value_to_bson_array() {
        let arr = QueryValue::Array(vec![
            QueryValue::Integer(1),
            QueryValue::String("two".into()),
        ]);
        let result = query_value_to_bson(&arr);
        match result {
            Bson::Array(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], Bson::Int64(1));
                assert_eq!(items[1], Bson::String("two".into()));
            }
            other => panic!("expected Bson::Array, got: {other:?}"),
        }
    }

    #[test]
    fn query_value_to_bson_json() {
        let json = serde_json::json!({"key": "value"});
        let result = query_value_to_bson(&QueryValue::Json(json));
        // Should produce a BSON Document
        match result {
            Bson::Document(doc) => {
                assert_eq!(doc.get_str("key").unwrap(), "value");
            }
            other => panic!("expected Bson::Document, got: {other:?}"),
        }
    }

    // ── bson_to_query_value tests ─────────────────────────────────────

    #[test]
    fn bson_to_query_value_null() {
        assert_eq!(bson_to_query_value(&Bson::Null), QueryValue::Null);
    }

    #[test]
    fn bson_to_query_value_boolean() {
        assert_eq!(
            bson_to_query_value(&Bson::Boolean(true)),
            QueryValue::Boolean(true)
        );
    }

    #[test]
    fn bson_to_query_value_int32() {
        assert_eq!(
            bson_to_query_value(&Bson::Int32(42)),
            QueryValue::Integer(42)
        );
    }

    #[test]
    fn bson_to_query_value_int64() {
        assert_eq!(
            bson_to_query_value(&Bson::Int64(100)),
            QueryValue::Integer(100)
        );
    }

    #[test]
    fn bson_to_query_value_double() {
        assert_eq!(
            bson_to_query_value(&Bson::Double(2.718)),
            QueryValue::Float(2.718)
        );
    }

    #[test]
    fn bson_to_query_value_string() {
        assert_eq!(
            bson_to_query_value(&Bson::String("hello".into())),
            QueryValue::String("hello".into())
        );
    }

    #[test]
    fn bson_to_query_value_object_id() {
        let oid = ObjectId::new();
        let result = bson_to_query_value(&Bson::ObjectId(oid));
        assert_eq!(result, QueryValue::String(oid.to_hex()));
    }

    #[test]
    fn bson_to_query_value_array() {
        let arr = Bson::Array(vec![Bson::Int32(1), Bson::String("two".into())]);
        let result = bson_to_query_value(&arr);
        match result {
            QueryValue::Array(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], QueryValue::Integer(1));
                assert_eq!(items[1], QueryValue::String("two".into()));
            }
            other => panic!("expected QueryValue::Array, got: {other:?}"),
        }
    }

    #[test]
    fn bson_to_query_value_document() {
        let doc = doc! { "nested": "value" };
        let result = bson_to_query_value(&Bson::Document(doc));
        match result {
            QueryValue::Json(v) => {
                assert!(v.is_object());
            }
            other => panic!("expected QueryValue::Json, got: {other:?}"),
        }
    }

    // ── params_to_document test ───────────────────────────────────────

    #[test]
    fn params_to_document_roundtrip() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), QueryValue::String("alice".into()));
        params.insert("age".to_string(), QueryValue::Integer(30));
        let doc = params_to_document(&params);
        assert_eq!(doc.get_str("name").unwrap(), "alice");
        assert_eq!(doc.get_i64("age").unwrap(), 30);
    }

    // ── split_filter_and_fields test ──────────────────────────────────

    #[test]
    fn split_filter_uses_filter_key() {
        let mut params = HashMap::new();
        params.insert(
            "_filter".to_string(),
            QueryValue::Json(serde_json::json!({"status": "active"})),
        );
        params.insert("name".to_string(), QueryValue::String("updated".into()));

        let (filter, fields) = split_filter_and_fields(&params);
        assert!(filter.get("status").is_some());
        assert!(fields.contains_key("name"));
        assert!(!fields.contains_key("_filter"));
    }

    #[test]
    fn split_filter_without_filter_key_returns_empty_filter() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), QueryValue::String("test".into()));

        let (filter, fields) = split_filter_and_fields(&params);
        assert!(filter.is_empty());
        assert!(fields.contains_key("name"));
    }

    // ── RW2.6 contract tests ──────────────────────────────────────────

    #[test]
    fn default_max_rows_is_one_thousand() {
        assert_eq!(DEFAULT_MAX_ROWS, 1_000);
    }

    #[test]
    fn split_filter_empty_without_filter_key() {
        // Confirms that an empty params map produces an empty filter —
        // the guard in exec_update/exec_delete will reject this.
        let params = HashMap::new();
        let (filter, _) = split_filter_and_fields(&params);
        assert!(filter.is_empty());
    }

    // ── connect with bad host ─────────────────────────────────────────

    #[tokio::test]
    async fn connect_bad_host_returns_connection_error() {
        let driver = MongoDriver;
        let params = bad_params();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            driver.connect(&params),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(msg))) => {
                assert!(
                    msg.contains("mongodb"),
                    "error should mention mongodb: {msg}"
                );
            }
            Ok(Err(other)) => panic!("expected DriverError::Connection, got: {other:?}"),
            Ok(Ok(_)) => panic!("expected connection error, but got Ok"),
            Err(_) => {
                // Timeout is acceptable — confirms port 1 doesn't have a MongoDB server.
            }
        }
    }
}
