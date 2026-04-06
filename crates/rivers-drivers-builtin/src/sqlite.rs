//! SqliteDriver — SQLite database driver via rusqlite.
//!
//! Per `rivers-driver-spec.md` §3.4:
//! - WAL mode enabled, 5-second busy timeout
//! - Named parameters with `:name` prefix
//! - Supports `:memory:` via `database = ":memory:"`
//! - `last_insert_id` from `last_insert_rowid()`
//! - Type mapping: INTEGER -> i64, REAL -> f64, TEXT -> String, BLOB -> hex, NULL -> Null

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Supported field types for the SQLite driver.
const SQLITE_TYPES: &[&str] = &[
    "uuid", "string", "text", "integer", "float", "real", "decimal",
    "boolean", "datetime", "date", "json", "blob", "bytes",
    "email", "phone", "url",
];

/// SQLite database driver.
///
/// Creates `SqliteConnection` instances backed by rusqlite. Each connection
/// opens its own database file (or `:memory:` instance) with WAL mode and a
/// 5-second busy timeout.
///
/// See `rivers-driver-spec.md` §3.4.
pub struct SqliteDriver;

impl SqliteDriver {
    /// Create a new SQLite driver instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqliteDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseDriver for SqliteDriver {
    fn name(&self) -> &str {
        "sqlite"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // Resolve path: database field first, fall back to host (common for SQLite configs)
        let path = if !params.database.is_empty() {
            params.database.clone()
        } else if !params.host.is_empty() {
            params.host.clone()
        } else {
            return Err(DriverError::Connection(
                "sqlite: no database path — set 'database' or 'host' in datasource config".into(),
            ));
        };

        // Create parent directories if they don't exist (skip for :memory:)
        if path != ":memory:" {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
        }

        // Open connection on a blocking thread since rusqlite is synchronous.
        let conn = tokio::task::spawn_blocking(move || -> Result<rusqlite::Connection, DriverError> {
            let conn = rusqlite::Connection::open(&path)
                .map_err(|e| DriverError::Connection(format!("sqlite open '{}': {}", path, e)))?;

            // WAL mode for concurrent read performance (§3.4).
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| DriverError::Connection(format!("sqlite WAL pragma: {}", e)))?;

            // 5-second busy timeout (§3.4).
            conn.busy_timeout(Duration::from_secs(5))
                .map_err(|e| DriverError::Connection(format!("sqlite busy_timeout: {}", e)))?;

            Ok(conn)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking join: {}", e)))?
        ?;

        Ok(Box::new(SqliteConnection {
            conn: Arc::new(Mutex::new(conn)),
        }))
    }

    fn supports_transactions(&self) -> bool {
        true
    }

    fn param_style(&self) -> rivers_driver_sdk::ParamStyle {
        rivers_driver_sdk::ParamStyle::DollarNamed
    }
}

/// A live SQLite connection wrapping `rusqlite::Connection` behind `Arc<Mutex>`.
///
/// All operations are dispatched via `tokio::task::spawn_blocking` to avoid
/// blocking the async runtime. See `rivers-driver-spec.md` §3.4.
pub struct SqliteConnection {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

#[async_trait]
impl Connection for SqliteConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        let conn = Arc::clone(&self.conn);
        let statement = query.statement.clone();
        let operation = query.operation.clone();
        let parameters = query.parameters.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| DriverError::Internal(format!("sqlite mutex poisoned: {}", e)))?;

            match operation.as_str() {
                "select" | "query" | "get" | "find" => {
                    execute_query(&conn, &statement, &parameters)
                }
                "insert" | "create" => {
                    execute_insert(&conn, &statement, &parameters)
                }
                "update" => {
                    execute_write(&conn, &statement, &parameters)
                }
                "delete" | "del" | "remove" | "drop" | "truncate" => {
                    execute_write(&conn, &statement, &parameters)
                }
                "ping" => {
                    conn.execute_batch("SELECT 1")
                        .map_err(|e| DriverError::Query(format!("sqlite ping: {}", e)))?;
                    Ok(QueryResult::empty())
                }
                op => Err(DriverError::Unsupported(format!(
                    "sqlite driver does not support operation: {}",
                    op
                ))),
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking join: {}", e)))?
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| DriverError::Internal(format!("sqlite mutex poisoned: {}", e)))?;
            conn.execute_batch("SELECT 1")
                .map_err(|e| DriverError::Query(format!("sqlite ping: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DriverError::Internal(format!("spawn_blocking join: {}", e)))?
    }

    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let conn = Arc::clone(&self.conn);
        let statement = query.statement.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| DriverError::Internal(format!("sqlite lock: {e}")))?;
            conn.execute_batch(&statement)
                .map_err(|e| DriverError::Query(format!("sqlite ddl: {e}")))?;
            Ok(QueryResult::empty())
        })
        .await
        .map_err(|e| DriverError::Internal(format!("sqlite spawn: {e}")))?
    }

    fn driver_name(&self) -> &str {
        "sqlite"
    }
}

// ---------------------------------------------------------------------------
// Internal helpers — all run inside spawn_blocking (no async)
// ---------------------------------------------------------------------------

/// Build a vector of `(name, value)` pairs for rusqlite named-parameter binding.
/// Each parameter key is prefixed with `:` if not already present.
fn bind_params(parameters: &HashMap<String, QueryValue>) -> Vec<(String, Box<dyn rusqlite::types::ToSql>)> {
    parameters
        .iter()
        .map(|(key, val)| {
            let name = if key.starts_with(':') || key.starts_with('@') || key.starts_with('$') {
                key.clone()
            } else {
                format!("${}", key)
            };
            let boxed: Box<dyn rusqlite::types::ToSql> = match val {
                QueryValue::Null => Box::new(rusqlite::types::Null),
                QueryValue::Boolean(b) => Box::new(*b),
                QueryValue::Integer(i) => Box::new(*i),
                QueryValue::Float(f) => Box::new(*f),
                QueryValue::String(s) => Box::new(s.clone()),
                QueryValue::Array(arr) => {
                    Box::new(serde_json::to_string(arr).unwrap_or_default())
                }
                QueryValue::Json(v) => {
                    Box::new(serde_json::to_string(v).unwrap_or_default())
                }
            };
            (name, boxed)
        })
        .collect()
}

/// Execute a SELECT statement and return rows as `Vec<HashMap<String, QueryValue>>`.
fn execute_query(
    conn: &rusqlite::Connection,
    statement: &str,
    parameters: &HashMap<String, QueryValue>,
) -> Result<QueryResult, DriverError> {
    let mut stmt = conn
        .prepare(statement)
        .map_err(|e| DriverError::Query(format!("sqlite prepare: {}", e)))?;

    let bound = bind_params(parameters);
    let param_slice: Vec<(&str, &dyn rusqlite::types::ToSql)> = bound
        .iter()
        .map(|(name, val)| (name.as_str(), val.as_ref() as &dyn rusqlite::types::ToSql))
        .collect();

    let column_names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let rows_result = stmt
        .query_map(param_slice.as_slice(), |row| {
            let mut map = HashMap::new();
            for (i, col_name) in column_names.iter().enumerate() {
                let value = row_value_at(row, i);
                map.insert(col_name.clone(), value);
            }
            Ok(map)
        })
        .map_err(|e| DriverError::Query(format!("sqlite query: {}", e)))?;

    let mut rows = Vec::new();
    for row_result in rows_result {
        let row = row_result
            .map_err(|e| DriverError::Query(format!("sqlite row: {}", e)))?;
        rows.push(row);
    }

    let affected = rows.len() as u64;
    Ok(QueryResult {
        rows,
        affected_rows: affected,
        last_insert_id: None,
    })
}

/// Execute an INSERT/CREATE statement, returning affected_rows and last_insert_id.
fn execute_insert(
    conn: &rusqlite::Connection,
    statement: &str,
    parameters: &HashMap<String, QueryValue>,
) -> Result<QueryResult, DriverError> {
    let mut stmt = conn
        .prepare(statement)
        .map_err(|e| DriverError::Query(format!("sqlite prepare: {}", e)))?;

    let bound = bind_params(parameters);
    let param_slice: Vec<(&str, &dyn rusqlite::types::ToSql)> = bound
        .iter()
        .map(|(name, val)| (name.as_str(), val.as_ref() as &dyn rusqlite::types::ToSql))
        .collect();

    let affected = stmt
        .execute(param_slice.as_slice())
        .map_err(|e| DriverError::Query(format!("sqlite execute: {}", e)))?;

    let last_id = conn.last_insert_rowid();

    Ok(QueryResult {
        rows: Vec::new(),
        affected_rows: affected as u64,
        last_insert_id: Some(last_id.to_string()),
    })
}

/// Execute an UPDATE/DELETE statement, returning affected_rows only.
fn execute_write(
    conn: &rusqlite::Connection,
    statement: &str,
    parameters: &HashMap<String, QueryValue>,
) -> Result<QueryResult, DriverError> {
    let mut stmt = conn
        .prepare(statement)
        .map_err(|e| DriverError::Query(format!("sqlite prepare: {}", e)))?;

    let bound = bind_params(parameters);
    let param_slice: Vec<(&str, &dyn rusqlite::types::ToSql)> = bound
        .iter()
        .map(|(name, val)| (name.as_str(), val.as_ref() as &dyn rusqlite::types::ToSql))
        .collect();

    let affected = stmt
        .execute(param_slice.as_slice())
        .map_err(|e| DriverError::Query(format!("sqlite execute: {}", e)))?;

    Ok(QueryResult {
        rows: Vec::new(),
        affected_rows: affected as u64,
        last_insert_id: None,
    })
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for SqliteDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "sqlite"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // SQLite schemas must be type "object"
        if schema.schema_type != "object" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "sqlite".into(),
                supported: vec!["object".into()],
                schema_file: String::new(),
            });
        }
        // GET schema must have at least one field
        if method == HttpMethod::GET && schema.fields.is_empty() {
            return Err(SchemaSyntaxError::StructuralError {
                message: "GET schema must declare at least one field".into(),
                driver: "sqlite".into(),
                schema_file: String::new(),
            });
        }
        // Validate field types and attributes
        for field in &schema.fields {
            if !SQLITE_TYPES.contains(&field.field_type.as_str()) {
                return Err(SchemaSyntaxError::InvalidFieldType {
                    field: field.name.clone(),
                    field_type: field.field_type.clone(),
                    schema_file: String::new(),
                });
            }
            // Reject unsupported attributes (e.g., "faker", "key_pattern")
            rivers_driver_sdk::validation::check_supported_attributes(
                field, "sqlite", rivers_driver_sdk::validation::RELATIONAL_ATTRIBUTES, ""
            )?;
        }
        Ok(())
    }

    fn validate(
        &self,
        data: &serde_json::Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError> {
        rivers_driver_sdk::validation::validate_fields(data, schema, direction)
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        // Delegate to DatabaseDriver::connect + Connection::execute pattern
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for SQLite".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // SQLiteDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// Extract a single column value from a rusqlite row, mapping to `QueryValue`.
///
/// Type mapping per `rivers-driver-spec.md` §3.4:
/// - INTEGER -> `QueryValue::Integer(i64)`
/// - REAL    -> `QueryValue::Float(f64)`
/// - TEXT    -> `QueryValue::String(String)`
/// - BLOB    -> `QueryValue::String(String)` (hex-encoded)
/// - NULL    -> `QueryValue::Null`
fn row_value_at(row: &rusqlite::Row<'_>, idx: usize) -> QueryValue {
    // Try types in order of specificity. rusqlite returns Err for type mismatches,
    // so we try each variant until one succeeds.
    if let Ok(v) = row.get::<_, rusqlite::types::Value>(idx) {
        match v {
            rusqlite::types::Value::Null => QueryValue::Null,
            rusqlite::types::Value::Integer(i) => QueryValue::Integer(i),
            rusqlite::types::Value::Real(f) => QueryValue::Float(f),
            rusqlite::types::Value::Text(s) => QueryValue::String(s),
            rusqlite::types::Value::Blob(b) => {
                // Hex-encode blob bytes without pulling in the `hex` crate.
                let hex_str: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                QueryValue::String(hex_str)
            }
        }
    } else {
        QueryValue::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "sqlite".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra: HashMap::new(),
        }
    }

    fn make_field(name: &str, field_type: &str, required: bool) -> SchemaFieldDef {
        SchemaFieldDef {
            name: name.into(),
            field_type: field_type.into(),
            required,
            constraints: HashMap::new(),
        }
    }

    fn make_field_with(
        name: &str,
        field_type: &str,
        required: bool,
        constraints: Vec<(&str, serde_json::Value)>,
    ) -> SchemaFieldDef {
        let mut c = HashMap::new();
        for (k, v) in constraints {
            c.insert(k.to_string(), v);
        }
        SchemaFieldDef {
            name: name.into(),
            field_type: field_type.into(),
            required,
            constraints: c,
        }
    }

    #[test]
    fn schema_syntax_valid_object() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![
                make_field("id", "uuid", true),
                make_field("name", "string", true),
                make_field("age", "integer", false),
                make_field("price", "decimal", false),
                make_field("data", "bytes", false),
            ],
        );
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_object_type() {
        let driver = SqliteDriver::new();
        let schema = make_schema("array", vec![make_field("id", "integer", true)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(
            matches!(err, SchemaSyntaxError::UnsupportedType { .. }),
            "expected UnsupportedType, got {:?}",
            err,
        );
    }

    #[test]
    fn schema_syntax_rejects_unknown_field_type() {
        let driver = SqliteDriver::new();
        let schema = make_schema("object", vec![make_field("data", "xml", false)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(
            matches!(err, SchemaSyntaxError::InvalidFieldType { .. }),
            "expected InvalidFieldType, got {:?}",
            err,
        );
    }

    #[test]
    fn schema_syntax_get_requires_fields() {
        let driver = SqliteDriver::new();
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(
            matches!(err, SchemaSyntaxError::StructuralError { .. }),
            "expected StructuralError, got {:?}",
            err,
        );
    }

    #[test]
    fn schema_syntax_post_allows_empty_fields() {
        let driver = SqliteDriver::new();
        let schema = make_schema("object", vec![]);
        assert!(driver.check_schema_syntax(&schema, HttpMethod::POST).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_faker_attribute() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field_with("name", "text", true, vec![("faker", serde_json::json!("name"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_accepts_valid_data() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"name": "Alice"});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"age": 30});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(
            matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "name"),
            "expected MissingRequired for 'name', got {:?}",
            err,
        );
    }

    #[test]
    fn validate_rejects_non_object_data_with_fields() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!("just a string");
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(
            matches!(err, ValidationError::TypeMismatch { .. }),
            "expected TypeMismatch, got {:?}",
            err,
        );
    }

    #[test]
    fn validate_type_mismatch_detected() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field("active", "boolean", true)],
        );
        let data = serde_json::json!({"active": "yes"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_constraint_violation_detected() {
        let driver = SqliteDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field_with("score", "integer", true, vec![("min", serde_json::json!(0))])],
        );
        let data = serde_json::json!({"score": -5});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::ConstraintViolation { .. }));
    }

    // ── Connection path resolution tests ────────────────────────────

    fn make_params(host: &str, database: &str) -> ConnectionParams {
        ConnectionParams {
            host: host.into(),
            port: 0,
            database: database.into(),
            username: String::new(),
            password: String::new(),
            options: HashMap::new(),
        }
    }

    fn q(statement: &str, params: Vec<(&str, QueryValue)>) -> Query {
        let mut query = Query::new("t", statement);
        query.parameters = params.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        query
    }

    #[tokio::test]
    async fn connect_uses_database_field() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();

        assert!(db_path.exists(), "SQLite file should exist on disk");
    }

    #[tokio::test]
    async fn connect_falls_back_to_host_when_database_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("fallback.db");
        let driver = SqliteDriver::new();
        let params = make_params(db_path.to_str().unwrap(), "");

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();

        assert!(db_path.exists(), "SQLite file should exist via host fallback");
    }

    #[tokio::test]
    async fn connect_errors_when_both_empty() {
        let driver = SqliteDriver::new();
        let params = make_params("", "");

        match driver.connect(&params).await {
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(msg.contains("no database path"), "should mention 'no database path', got: {msg}");
            }
            Ok(_) => panic!("should error when both host and database are empty"),
        }
    }

    #[tokio::test]
    async fn connect_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested/deep/dir/test.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        let mut conn = driver.connect(&params).await.unwrap();
        conn.ddl_execute(&q("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])).await.unwrap();

        assert!(db_path.exists(), "SQLite file should exist in nested directory");
    }

    #[tokio::test]
    async fn connect_insert_then_select_across_connections() {
        // Regression test: INSERT + SELECT on SEPARATE connections to the SAME file
        // must return data (not null). This catches the in-memory DB bug.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("persist.db");
        let driver = SqliteDriver::new();
        let params = make_params("", db_path.to_str().unwrap());

        // Connection 1: create table + insert a row
        let mut conn1 = driver.connect(&params).await.unwrap();
        conn1.ddl_execute(&q("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)", vec![])).await.unwrap();
        conn1.execute(&q("INSERT INTO items (id, name) VALUES (42, 'test-item')", vec![])).await.unwrap();

        // Connection 2: select (different connection, same file)
        let mut conn2 = driver.connect(&params).await.unwrap();
        let result = conn2.execute(&q("SELECT name FROM items WHERE id = 42", vec![])).await.unwrap();

        assert_eq!(result.rows.len(), 1, "SELECT on conn2 should see conn1's INSERT");
        assert_eq!(
            result.rows[0].get("name"),
            Some(&QueryValue::String("test-item".into())),
            "should read back the inserted value"
        );
    }
}
