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

/// Hash-stable key for a connection tuple. Includes a SHA-256 fingerprint
/// of the password (first 8 bytes hex, ~32 bits) so two datasources sharing
/// host/port/db/user but with different passwords get isolated pools — the
/// rotation / multi-tenant case. The earlier rationale "auth will simply
/// reject and we'll re-create next time" was wrong: `get_or_create_pool`
/// returns the cached pool unconditionally with no eviction, so the first
/// password to land would win permanently.
///
/// Raw password bytes are never stored in the key — only the hash digest.
/// 32 bits of fingerprint is far more than enough to distinguish a handful
/// of credentials in a single-process cache; collision risk for distinct
/// passwords is ~1 in 4 billion.
fn pool_key(params: &ConnectionParams) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(params.password.as_bytes());
    let pwd_fingerprint = hex::encode(&hasher.finalize()[..8]);
    format!(
        "{}:{}/{}?u={}#{}",
        params.host, params.port, params.database, params.username, pwd_fingerprint,
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

/// Evict a pool entry by params. Used on auth failure so the next `connect`
/// rebuilds against fresh credentials (e.g., after rotation while the cache
/// still holds the old pool).
fn evict_pool(params: &ConnectionParams) -> Result<(), DriverError> {
    let key = pool_key(params);
    let mut cache = pool_cache()
        .lock()
        .map_err(|e| DriverError::Connection(format!("mysql pool cache poisoned: {e}")))?;
    cache.remove(&key);
    Ok(())
}

/// Detect MySQL auth-failure server errors:
///   1045 ER_ACCESS_DENIED_ERROR    — access denied for user
///   1044 ER_DBACCESS_DENIED_ERROR  — access denied for user to database
///
/// These are the codes that indicate stale/wrong credentials and warrant a
/// pool eviction + retry. Other server errors (table not found, etc.) leave
/// the pool intact.
fn is_auth_error(err: &mysql_async::Error) -> bool {
    matches!(
        err,
        mysql_async::Error::Server(srv) if srv.code == 1045 || srv.code == 1044
    )
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
        //
        // H4: on first-checkout auth failure (stale pool from rotated
        // credentials, or an OnceLock entry that was created with a wrong
        // password), evict the cache entry and rebuild once. The pool key
        // already separates by password fingerprint so this only fires when
        // the live server password no longer matches what was used to seed
        // the pool — typically rotation.
        let pool = get_or_create_pool(params)?;
        match pool.get_conn().await {
            Ok(conn) => Ok(Box::new(MysqlConnection { conn })),
            Err(e) if is_auth_error(&e) => {
                evict_pool(params)?;
                let fresh = get_or_create_pool(params)?;
                let conn = fresh.get_conn().await.map_err(|e| {
                    DriverError::Connection(format!("mysql checkout (after auth retry): {e}"))
                })?;
                Ok(Box::new(MysqlConnection { conn }))
            }
            Err(e) => Err(DriverError::Connection(format!("mysql checkout: {e}"))),
        }
    }

    fn needs_isolated_runtime(&self) -> bool {
        // Built-in drivers can run on the host's tokio runtime — no need
        // for the per-call isolated runtime that plugin cdylibs require.
        false
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
                            QueryValue::UInt(u) => u.to_string(),
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
        // BIGINT UNSIGNED round-trip: lossless via mysql_async's native UInt.
        QueryValue::UInt(u) => mysql_async::Value::UInt(*u),
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
        let value = mysql_value_to_query_value(row, i, col);
        map.insert(name, value);
    }
    map
}

/// Convert a single column value from a MySQL row to `QueryValue`.
///
/// We try progressively more general types: i64/u64, f64, String, bytes.
/// The column metadata is used to disambiguate `BIGINT UNSIGNED` (and other
/// unsigned integer types) which the text protocol delivers as `Bytes` —
/// without the column flags we'd silently fall back to `i64` parsing and
/// truncate values above `i64::MAX`. Per H18.2.
fn mysql_value_to_query_value(
    row: &mysql_async::Row,
    idx: usize,
    column: &mysql_async::Column,
) -> QueryValue {
    use mysql_async::consts::{ColumnFlags, ColumnType};

    // Check for NULL first.
    if let Some(mysql_async::Value::NULL) = row.as_ref(idx) {
        return QueryValue::Null;
    }

    // Get the raw value to avoid panics from type mismatch in get::<T>()
    let raw = match row.as_ref(idx) {
        Some(v) => v,
        None => return QueryValue::Null,
    };

    let is_unsigned = column.flags().contains(ColumnFlags::UNSIGNED_FLAG);
    let is_integer_col = matches!(
        column.column_type(),
        ColumnType::MYSQL_TYPE_TINY
            | ColumnType::MYSQL_TYPE_SHORT
            | ColumnType::MYSQL_TYPE_INT24
            | ColumnType::MYSQL_TYPE_LONG
            | ColumnType::MYSQL_TYPE_LONGLONG,
    );

    match raw {
        mysql_async::Value::NULL => QueryValue::Null,
        // mysql_async's binary-protocol `Value::Int` carries values from
        // `BIGINT UNSIGNED` columns whenever they fit in `i64`. Use the
        // column's UNSIGNED flag to route these to `UInt`, so callers see a
        // consistent variant for the column regardless of magnitude.
        mysql_async::Value::Int(i) => {
            if is_integer_col && is_unsigned && *i >= 0 {
                QueryValue::UInt(*i as u64)
            } else {
                QueryValue::Integer(*i)
            }
        }
        mysql_async::Value::UInt(u) => QueryValue::UInt(*u),
        mysql_async::Value::Float(f) => QueryValue::Float(*f as f64),
        mysql_async::Value::Double(d) => QueryValue::Float(*d),
        mysql_async::Value::Bytes(b) => {
            let s = String::from_utf8_lossy(b).to_string();

            // H18.2: when the column is an unsigned integer type, parse as
            // u64 so values above i64::MAX (e.g. BIGINT UNSIGNED rows near
            // u64::MAX) survive the text protocol. The column-metadata path
            // is the canonical disambiguator — without it, the Bytes branch
            // would silently fall back to i64 and lose the unsigned semantic
            // even for small values.
            if is_integer_col && is_unsigned {
                if let Ok(u) = s.parse::<u64>() {
                    return QueryValue::UInt(u);
                }
                // Fall through to the legacy paths if the bytes don't parse
                // as u64 (shouldn't happen for unsigned int columns, but we
                // never silently corrupt — degrade to String preserves the
                // raw decimal text).
            }

            // Signed-or-unknown integer columns: preserve legacy i64 path,
            // but when an unsigned column above i64::MAX overflows i64
            // parsing, retry as u64 so we still get a faithful UInt.
            if let Ok(i) = s.parse::<i64>() {
                return QueryValue::Integer(i);
            }
            if let Ok(u) = s.parse::<u64>() {
                return QueryValue::UInt(u);
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
    fn query_value_to_mysql_uint_round_trip() {
        // H18.2: UInt round-trips losslessly to mysql_async::Value::UInt,
        // including values above i64::MAX which the pre-H18 i64 cast would
        // have silently corrupted.
        for val in [0u64, 42, 9_007_199_254_740_991, 9_007_199_254_740_992, u64::MAX] {
            assert_eq!(
                query_value_to_mysql(&QueryValue::UInt(val)),
                mysql_async::Value::UInt(val),
                "UInt({val}) should map to Value::UInt({val})",
            );
        }
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

    // -- G_R7.1: per-datasource shared pool key --

    fn params_for(host: &str, port: u16, user: &str, db: &str) -> ConnectionParams {
        ConnectionParams {
            host: host.into(),
            port,
            database: db.into(),
            username: user.into(),
            password: "x".into(),
            options: HashMap::new(),
        }
    }

    #[test]
    fn pool_key_distinguishes_datasources() {
        let a = pool_key(&params_for("h1", 3306, "u", "db1"));
        let b = pool_key(&params_for("h1", 3306, "u", "db2"));
        let c = pool_key(&params_for("h2", 3306, "u", "db1"));
        let d = pool_key(&params_for("h1", 3307, "u", "db1"));
        let e = pool_key(&params_for("h1", 3306, "v", "db1"));
        assert_ne!(a, b, "different db must yield different key");
        assert_ne!(a, c, "different host must yield different key");
        assert_ne!(a, d, "different port must yield different key");
        assert_ne!(a, e, "different user must yield different key");
    }

    #[test]
    fn pool_key_stable_for_same_params() {
        let a = pool_key(&params_for("h1", 3306, "u", "db1"));
        let b = pool_key(&params_for("h1", 3306, "u", "db1"));
        assert_eq!(a, b, "same params must produce same key");
    }

    #[test]
    fn pool_key_distinguishes_passwords() {
        // H4: same host/port/db/user, different passwords → different keys.
        // Without this, password rotation or multi-tenant credentials sharing
        // a logical user would collide on the first pool ever created.
        let mut a = params_for("h1", 3306, "u", "db1");
        a.password = "secret_a".into();
        let mut b = params_for("h1", 3306, "u", "db1");
        b.password = "secret_b".into();
        assert_ne!(
            pool_key(&a),
            pool_key(&b),
            "rotating a password must yield a distinct pool key"
        );
    }

    #[test]
    fn pool_key_stable_for_same_password() {
        let mut a = params_for("h1", 3306, "u", "db1");
        a.password = "shared".into();
        let mut b = params_for("h1", 3306, "u", "db1");
        b.password = "shared".into();
        assert_eq!(
            pool_key(&a),
            pool_key(&b),
            "same params + same password must produce same key"
        );
    }

    #[test]
    fn pool_key_does_not_leak_raw_password() {
        // H4: raw password bytes must never appear in the cache key — only
        // the hex-encoded SHA-256 fingerprint.
        let mut p = params_for("h1", 3306, "u", "db1");
        p.password = "supersecret_p@ssw0rd".into();
        let key = pool_key(&p);
        assert!(
            !key.contains("supersecret_p@ssw0rd"),
            "pool key must not contain raw password bytes: {key}"
        );
    }

    #[test]
    fn is_auth_error_matches_access_denied_codes() {
        // H4: 1045 + 1044 are the MySQL access-denied codes that warrant
        // pool eviction + retry. Other server errors should not trigger it.
        let access_denied_user = mysql_async::Error::Server(mysql_async::ServerError {
            code: 1045,
            message: "Access denied for user".into(),
            state: "28000".into(),
        });
        let access_denied_db = mysql_async::Error::Server(mysql_async::ServerError {
            code: 1044,
            message: "Access denied for user to database".into(),
            state: "42000".into(),
        });
        let table_missing = mysql_async::Error::Server(mysql_async::ServerError {
            code: 1146,
            message: "Table doesn't exist".into(),
            state: "42S02".into(),
        });
        assert!(is_auth_error(&access_denied_user));
        assert!(is_auth_error(&access_denied_db));
        assert!(!is_auth_error(&table_missing));
    }

    #[test]
    fn is_auth_error_boundary_codes() {
        // H4: verify exact boundary conditions for is_auth_error().
        // 1043 (ER_HANDSHAKE_ERROR) — just below the auth range → false.
        // 1044 (ER_DBACCESS_DENIED_ERROR) — access denied to database → true.
        // 1045 (ER_ACCESS_DENIED_ERROR) — access denied for user → true.
        // 1046 (ER_NO_DB_ERROR) — no database selected, not an auth error → false.
        // These boundaries ensure the eviction path fires for exactly the two
        // access-denied codes and does NOT fire for adjacent error codes.
        let make_server_err = |code: u16| -> mysql_async::Error {
            mysql_async::Error::Server(mysql_async::ServerError {
                code,
                message: format!("synthetic server error {code}"),
                state: "HY000".into(),
            })
        };

        assert!(!is_auth_error(&make_server_err(1043)), "1043 must not trigger eviction");
        assert!(is_auth_error(&make_server_err(1044)),  "1044 must trigger eviction");
        assert!(is_auth_error(&make_server_err(1045)),  "1045 must trigger eviction");
        assert!(!is_auth_error(&make_server_err(1046)), "1046 must not trigger eviction");
    }

    #[test]
    fn driver_does_not_need_isolated_runtime() {
        // G_R7.2: built-in driver returns false → DriverFactory runs
        // connect() on the active runtime instead of spawning a fresh one.
        let driver = MysqlDriver;
        assert!(!DatabaseDriver::needs_isolated_runtime(&driver));
    }
}
