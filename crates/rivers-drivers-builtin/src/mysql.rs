//! MysqlDriver — async MySQL via mysql_async.
//!
//! Per `rivers-driver-spec.md` §3.3:
//! - Positional `?` parameter binding
//! - `supports_transactions() → true`
//! - `last_insert_id` from `last_insert_id()` on result
//!
//! The driver executes raw statements via `query_iter` (reads) and
//! `query_drop` (writes). Parameter substitution is handled by the
//! DataView engine before reaching the driver, so the driver passes
//! the statement string as-is.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use mysql_async::prelude::*;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Process-global pool cache keyed by resolved `ConnectionParams` identity
/// (host + port + database + username). One `mysql_async::Pool` per distinct
/// datasource, shared across every `MysqlDriver::connect` call.
///
/// Pool reintroduction: CG4 (`docs/canary_codereivew.md` §3 + `tasks.md`).
/// The earlier per-call `Conn::new` was a workaround for the
/// host_callbacks per-call `Runtime::new()` bug, which was fixed separately.
/// The runtime fix removed the teardown pressure that was killing pool
/// background tasks; pooled connections are now safe again and we're paying
/// the full MySQL handshake on every dataview call until this lands.
fn pool_cache() -> &'static Mutex<HashMap<String, mysql_async::Pool>> {
    static CACHE: OnceLock<Mutex<HashMap<String, mysql_async::Pool>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Hash-stable key for a connection tuple. Password is intentionally excluded
/// — two datasources with the same host/port/db/user but different passwords
/// should not share a pool, but that's an edge case; if it happens the pool
/// auth will simply reject on first checkout and we'll re-create next time.
/// Including the password would leak secret bytes into map keys.
fn pool_key(params: &ConnectionParams) -> String {
    format!(
        "{}:{}/{}?u={}",
        params.host, params.port, params.database, params.username
    )
}

fn get_or_create_pool(params: &ConnectionParams) -> Result<mysql_async::Pool, DriverError> {
    let key = pool_key(params);
    let mut cache = pool_cache()
        .lock()
        .map_err(|e| DriverError::Connection(format!("mysql pool cache poisoned: {e}")))?;
    if let Some(pool) = cache.get(&key) {
        return Ok(pool.clone());
    }
    let opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(&params.host)
        .tcp_port(params.port)
        .user(Some(&params.username))
        .pass(Some(&params.password))
        .db_name(Some(&params.database));
    let pool = mysql_async::Pool::new(opts);
    cache.insert(key, pool.clone());
    Ok(pool)
}

/// Supported field types for the MySQL driver.
const MYSQL_TYPES: &[&str] = &[
    "uuid", "string", "text", "varchar", "char", "integer", "tinyint",
    "smallint", "mediumint", "int", "bigint", "float", "double", "decimal",
    "boolean", "datetime", "timestamp", "date", "time", "year",
    "json", "bytes", "blob", "enum", "set", "email", "phone", "url",
];

/// MySQL database driver backed by `mysql_async`.
///
/// Stateless factory — each call to `connect()` creates a new
/// [`MysqlConnection`] with its own connection from the pool.
///
/// See `rivers-driver-spec.md` §3.3.
pub struct MysqlDriver;

#[async_trait]
impl DatabaseDriver for MysqlDriver {
    fn name(&self) -> &str {
        "mysql"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // CG4: pool restored. Previous version used direct `Conn::new` per
        // call as a workaround for the host_callbacks `Runtime::new()`
        // teardown bug that killed mysql_async background tasks. That fix
        // landed separately; pooled connections are safe again and avoid
        // the full handshake per dataview call.
        let pool = get_or_create_pool(params)?;
        let conn = pool
            .get_conn()
            .await
            .map_err(|e| DriverError::Connection(format!("mysql checkout: {e}")))?;
        Ok(Box::new(MysqlConnection { conn }))
    }

    fn supports_transactions(&self) -> bool {
        true
    }

    fn param_style(&self) -> rivers_driver_sdk::ParamStyle {
        rivers_driver_sdk::ParamStyle::QuestionPositional
    }

    fn supports_introspection(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for MysqlDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "mysql"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // MySQL schemas must be type "object"
        if schema.schema_type != "object" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "mysql".into(),
                supported: vec!["object".into()],
                schema_file: String::new(),
            });
        }
        // GET schema must have at least one field
        if method == HttpMethod::GET && schema.fields.is_empty() {
            return Err(SchemaSyntaxError::StructuralError {
                message: "GET schema must declare at least one field".into(),
                driver: "mysql".into(),
                schema_file: String::new(),
            });
        }
        // Validate field types and attributes
        for field in &schema.fields {
            if !MYSQL_TYPES.contains(&field.field_type.as_str()) {
                return Err(SchemaSyntaxError::InvalidFieldType {
                    field: field.name.clone(),
                    field_type: field.field_type.clone(),
                    schema_file: String::new(),
                });
            }
            // Reject unsupported attributes (e.g., "faker", "key_pattern")
            rivers_driver_sdk::validation::check_supported_attributes(
                field, "mysql", rivers_driver_sdk::validation::RELATIONAL_ATTRIBUTES, ""
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
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for MySQL".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // MysqlDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// A live MySQL connection wrapping `mysql_async::Conn`.
///
/// Also holds a reference to the pool so it is not dropped prematurely.
pub struct MysqlConnection {
    conn: mysql_async::Conn,
}

#[async_trait]
impl Connection for MysqlConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        let params = build_mysql_params(&query.parameters);

        match query.operation.as_str() {
            // -----------------------------------------------------------------
            // Read operations — return rows
            // -----------------------------------------------------------------
            "select" | "query" | "get" | "find" => {
                let mut result = self
                    .conn
                    .exec_iter(&query.statement, params)
                    .await
                    .map_err(|e| DriverError::Query(format!("mysql query: {e}")))?;

                let mut rows = Vec::new();
                let columns: Vec<mysql_async::Column> = result
                    .columns_ref()
                    .iter()
                    .map(|c| c.clone())
                    .collect();

                result
                    .for_each(|row| {
                        let map = mysql_row_to_map(&row, &columns);
                        rows.push(map);
                    })
                    .await
                    .map_err(|e| DriverError::Query(format!("mysql fetch rows: {e}")))?;

                let count = rows.len() as u64;
                let column_names = if rows.is_empty() {
                    Some(columns.iter().map(|c| c.name_str().to_string()).collect())
                } else {
                    None
                };
                Ok(QueryResult {
                    rows,
                    affected_rows: count,
                    last_insert_id: None,
                    column_names,
                })
            }

            // -----------------------------------------------------------------
            // Insert/Create
            // -----------------------------------------------------------------
            "insert" | "create" => {
                let stmt_upper = query.statement.to_uppercase();
                if stmt_upper.contains("RETURNING") {
                    // MySQL 8.0 doesn't support RETURNING natively, but
                    // MariaDB does. Handle like a read to capture returned rows.
                    let mut result = self
                        .conn
                        .exec_iter(&query.statement, params)
                        .await
                        .map_err(|e| DriverError::Query(format!("mysql insert+returning: {e}")))?;

                    let columns: Vec<mysql_async::Column> = result
                        .columns_ref()
                        .iter()
                        .map(|c| c.clone())
                        .collect();

                    let mut rows = Vec::new();
                    result
                        .for_each(|row| {
                            rows.push(mysql_row_to_map(&row, &columns));
                        })
                        .await
                        .map_err(|e| DriverError::Query(format!("mysql insert+returning fetch: {e}")))?;

                    let count = rows.len() as u64;
                    // Try to extract id from first returned row
                    let last_id = rows
                        .first()
                        .and_then(|r| r.get("id"))
                        .map(|v| match v {
                            QueryValue::Integer(n) => n.to_string(),
                            QueryValue::String(s) => s.clone(),
                            other => format!("{:?}", other),
                        });
                    Ok(QueryResult {
                        rows,
                        affected_rows: count,
                        last_insert_id: last_id,
                        column_names: None,
                    })
                } else {
                    let result = self
                        .conn
                        .exec_iter(&query.statement, params)
                        .await
                        .map_err(|e| DriverError::Query(format!("mysql insert: {e}")))?;

                    let affected = result.affected_rows();
                    let last_id = result.last_insert_id();

                    // Must consume the result to release the connection.
                    drop(result);

                    Ok(QueryResult {
                        rows: Vec::new(),
                        affected_rows: affected,
                        last_insert_id: last_id.map(|id| id.to_string()),
                        column_names: None,
                    })
                }
            }

            // -----------------------------------------------------------------
            // Update
            // -----------------------------------------------------------------
            "update" => {
                let result = self
                    .conn
                    .exec_iter(&query.statement, params)
                    .await
                    .map_err(|e| DriverError::Query(format!("mysql update: {e}")))?;

                let affected = result.affected_rows();
                drop(result);

                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: affected,
                    last_insert_id: None,
                    column_names: None,
                })
            }

            // -----------------------------------------------------------------
            // Delete / DDL
            // -----------------------------------------------------------------
            "delete" | "del" | "drop" | "truncate" => {
                let result = self
                    .conn
                    .exec_iter(&query.statement, params)
                    .await
                    .map_err(|e| DriverError::Query(format!("mysql delete: {e}")))?;

                let affected = result.affected_rows();
                drop(result);

                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: affected,
                    last_insert_id: None,
                    column_names: None,
                })
            }

            // -----------------------------------------------------------------
            // Ping
            // -----------------------------------------------------------------
            "ping" => {
                self.conn
                    .ping()
                    .await
                    .map_err(|e| DriverError::Query(format!("mysql ping: {e}")))?;
                Ok(QueryResult::empty())
            }

            op => Err(DriverError::Unsupported(format!(
                "mysql driver does not support operation: {op}"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        self.conn
            .ping()
            .await
            .map_err(|e| DriverError::Connection(format!("mysql ping: {e}")))?;
        Ok(())
    }

    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        self.conn
            .query_drop("BEGIN")
            .await
            .map_err(|e| DriverError::Query(format!("mysql BEGIN: {e}")))
    }

    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        self.conn
            .query_drop("COMMIT")
            .await
            .map_err(|e| DriverError::Query(format!("mysql COMMIT: {e}")))
    }

    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        self.conn
            .query_drop("ROLLBACK")
            .await
            .map_err(|e| DriverError::Query(format!("mysql ROLLBACK: {e}")))
    }

    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        self.conn
            .query_drop(&query.statement)
            .await
            .map_err(|e| DriverError::Query(format!("mysql ddl: {e}")))?;
        Ok(QueryResult::empty())
    }

    fn driver_name(&self) -> &str {
        "mysql"
    }
}

// ---------------------------------------------------------------------------
// Parameter binding helpers (Priority 3)
// ---------------------------------------------------------------------------

/// Build positional MySQL parameters from named parameters.
///
/// Parameters are sorted by key name alphabetically so the caller can use
/// positional `?` placeholders in a predictable order.
fn build_mysql_params(parameters: &HashMap<String, QueryValue>) -> mysql_async::Params {
    let mut keys: Vec<&String> = parameters.keys().collect();
    keys.sort();
    let values: Vec<mysql_async::Value> = keys
        .iter()
        .map(|k| query_value_to_mysql(&parameters[*k]))
        .collect();
    if values.is_empty() {
        mysql_async::Params::Empty
    } else {
        mysql_async::Params::Positional(values)
    }
}

/// Convert a `QueryValue` to a `mysql_async::Value` for parameter binding.
fn query_value_to_mysql(val: &QueryValue) -> mysql_async::Value {
    match val {
        QueryValue::Null => mysql_async::Value::NULL,
        QueryValue::Boolean(b) => mysql_async::Value::from(*b),
        QueryValue::Integer(i) => mysql_async::Value::from(*i),
        QueryValue::Float(f) => mysql_async::Value::from(*f),
        QueryValue::String(s) => mysql_async::Value::from(s.clone()),
        QueryValue::Array(arr) => {
            mysql_async::Value::from(serde_json::to_string(arr).unwrap_or_default())
        }
        QueryValue::Json(v) => {
            mysql_async::Value::from(serde_json::to_string(v).unwrap_or_default())
        }
    }
}

// ---------------------------------------------------------------------------
// Row conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `mysql_async::Row` to `HashMap<String, QueryValue>`.
fn mysql_row_to_map(
    row: &mysql_async::Row,
    columns: &[mysql_async::Column],
) -> HashMap<String, QueryValue> {
    let mut map = HashMap::new();
    for (i, col) in columns.iter().enumerate() {
        let name = col.name_str().to_string();
        let value = mysql_value_to_query_value(row, i);
        map.insert(name, value);
    }
    map
}

/// Convert a single column value from a MySQL row to `QueryValue`.
///
/// We try progressively more general types: i64, f64, String, bytes.
fn mysql_value_to_query_value(row: &mysql_async::Row, idx: usize) -> QueryValue {
    // Check for NULL first.
    if let Some(mysql_async::Value::NULL) = row.as_ref(idx) {
        return QueryValue::Null;
    }

    // Get the raw value to avoid panics from type mismatch in get::<T>()
    let raw = match row.as_ref(idx) {
        Some(v) => v,
        None => return QueryValue::Null,
    };

    match raw {
        mysql_async::Value::NULL => QueryValue::Null,
        mysql_async::Value::Int(i) => QueryValue::Integer(*i),
        mysql_async::Value::UInt(u) => QueryValue::Integer(*u as i64),
        mysql_async::Value::Float(f) => QueryValue::Float(*f as f64),
        mysql_async::Value::Double(d) => QueryValue::Float(*d),
        mysql_async::Value::Bytes(b) => {
            let s = String::from_utf8_lossy(b).to_string();
            // Try to parse as number if it looks numeric
            if let Ok(i) = s.parse::<i64>() {
                return QueryValue::Integer(i);
            }
            if let Ok(f) = s.parse::<f64>() {
                // Only use float if it has a decimal point (avoid "42" → 42.0)
                if s.contains('.') {
                    return QueryValue::Float(f);
                }
            }
            // Try JSON
            if (s.starts_with('{') && s.ends_with('}')) || (s.starts_with('[') && s.ends_with(']')) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&s) {
                    return QueryValue::Json(json);
                }
            }
            QueryValue::String(s)
        }
        mysql_async::Value::Date(..) | mysql_async::Value::Time(..) => {
            // Serialize date/time as string via the String conversion
            if let Ok(s) = mysql_async::from_value_opt::<String>(raw.clone()) {
                QueryValue::String(s)
            } else {
                QueryValue::String(format!("{:?}", raw))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "mysql".into(),
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
        let driver = MysqlDriver;
        let schema = make_schema(
            "object",
            vec![
                make_field("id", "bigint", true),
                make_field("name", "varchar", true),
                make_field("data", "json", false),
                make_field("status", "enum", false),
                make_field("tags", "set", false),
                make_field("created", "timestamp", false),
                make_field("age", "tinyint", false),
                make_field("count", "mediumint", false),
                make_field("price", "double", false),
                make_field("email", "email", false),
                make_field("phone", "phone", false),
                make_field("site", "url", false),
                make_field("content", "bytes", false),
            ],
        );
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_object() {
        let driver = MysqlDriver;
        let schema = make_schema("array", vec![make_field("id", "integer", true)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_rejects_unknown_field_type() {
        let driver = MysqlDriver;
        let schema = make_schema("object", vec![make_field("data", "jsonb", false)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::InvalidFieldType { .. }));
    }

    #[test]
    fn schema_syntax_get_requires_fields() {
        let driver = MysqlDriver;
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_accepts_all_mysql_types() {
        let driver = MysqlDriver;
        for t in MYSQL_TYPES {
            let schema = make_schema("object", vec![make_field("f", t, false)]);
            assert!(
                driver.check_schema_syntax(&schema, HttpMethod::POST).is_ok(),
                "type '{}' should be accepted",
                t,
            );
        }
    }

    #[test]
    fn schema_syntax_rejects_faker_attribute() {
        let driver = MysqlDriver;
        let schema = make_schema(
            "object",
            vec![make_field_with("name", "varchar", true, vec![("faker", serde_json::json!("name"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_accepts_valid_data() {
        let driver = MysqlDriver;
        let schema = make_schema(
            "object",
            vec![make_field("name", "varchar", true)],
        );
        let data = serde_json::json!({"name": "Alice"});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let driver = MysqlDriver;
        let schema = make_schema(
            "object",
            vec![make_field("name", "varchar", true)],
        );
        let data = serde_json::json!({"age": 30});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "name"));
    }

    #[test]
    fn validate_rejects_non_object_data() {
        let driver = MysqlDriver;
        let schema = make_schema(
            "object",
            vec![make_field("name", "varchar", true)],
        );
        let data = serde_json::json!(42);
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_constraint_violation_detected() {
        let driver = MysqlDriver;
        let schema = make_schema(
            "object",
            vec![make_field_with("name", "varchar", true, vec![("max_length", serde_json::json!(5))])],
        );
        let data = serde_json::json!({"name": "a very long name"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::ConstraintViolation { .. }));
    }

    // -- Parameter binding tests (Priority 3) --

    #[test]
    fn build_mysql_params_empty() {
        let params = HashMap::new();
        let result = build_mysql_params(&params);
        assert!(matches!(result, mysql_async::Params::Empty));
    }

    #[test]
    fn build_mysql_params_sorted_order() {
        let mut params = HashMap::new();
        params.insert("z_name".to_string(), QueryValue::String("Alice".into()));
        params.insert("a_id".to_string(), QueryValue::Integer(42));
        let result = build_mysql_params(&params);
        match result {
            mysql_async::Params::Positional(values) => {
                assert_eq!(values.len(), 2);
                // Sorted: a_id comes first, z_name second
                assert_eq!(values[0], mysql_async::Value::from(42i64));
                assert_eq!(values[1], mysql_async::Value::from("Alice".to_string()));
            }
            _ => panic!("expected Positional params"),
        }
    }

    #[test]
    fn query_value_to_mysql_null() {
        let result = query_value_to_mysql(&QueryValue::Null);
        assert_eq!(result, mysql_async::Value::NULL);
    }

    #[test]
    fn query_value_to_mysql_types() {
        assert_eq!(
            query_value_to_mysql(&QueryValue::Boolean(true)),
            mysql_async::Value::from(true),
        );
        assert_eq!(
            query_value_to_mysql(&QueryValue::Integer(99)),
            mysql_async::Value::from(99i64),
        );
        assert_eq!(
            query_value_to_mysql(&QueryValue::Float(3.14)),
            mysql_async::Value::from(3.14f64),
        );
        assert_eq!(
            query_value_to_mysql(&QueryValue::String("hello".into())),
            mysql_async::Value::from("hello".to_string()),
        );
    }

    #[test]
    fn query_value_to_mysql_json() {
        let json_val = serde_json::json!({"key": "value"});
        let result = query_value_to_mysql(&QueryValue::Json(json_val.clone()));
        let expected = mysql_async::Value::from(serde_json::to_string(&json_val).unwrap());
        assert_eq!(result, expected);
    }

    #[test]
    fn query_value_to_mysql_array() {
        let arr = vec![QueryValue::Integer(1), QueryValue::Integer(2)];
        let result = query_value_to_mysql(&QueryValue::Array(arr.clone()));
        let expected = mysql_async::Value::from(serde_json::to_string(&arr).unwrap());
        assert_eq!(result, expected);
    }
}
