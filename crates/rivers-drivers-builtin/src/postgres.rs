//! PostgresDriver — async PostgreSQL via tokio-postgres.
//!
//! Per `rivers-driver-spec.md` §3.2:
//! - Positional `$1`, `$2` parameter binding
//! - `supports_transactions() → true`
//! - `supports_prepared_statements() → true`
//! - `last_insert_id` via `RETURNING id`

use std::collections::HashMap;

use async_trait::async_trait;
use tokio_postgres::types::{ToSql, Type};
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Supported field types for the PostgreSQL driver.
const POSTGRES_TYPES: &[&str] = &[
    "uuid", "string", "text", "integer", "bigint", "float", "decimal",
    "boolean", "datetime", "date", "json", "jsonb", "bytea", "bytes",
    "email", "phone", "url",
];

/// PostgreSQL database driver backed by `tokio-postgres`.
///
/// Stateless factory — each call to `connect()` creates a new
/// [`PostgresConnection`] with its own TCP connection to the server.
///
/// See `rivers-driver-spec.md` §3.2.
pub struct PostgresDriver;

#[async_trait]
impl DatabaseDriver for PostgresDriver {
    fn name(&self) -> &str {
        "postgres"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // Use tokio_postgres::Config builder rather than string interpolation:
        // each setter properly escapes/encodes the value, so spaces, quotes,
        // `=`, `&`, and `'` in credentials cannot break parsing or smuggle
        // additional connection options (e.g. injection of `sslmode=disable`
        // via a crafted password).
        let pg_config = build_pg_config(params);

        let (client, connection) = pg_config
            .connect(tokio_postgres::NoTls)
            .await
            .map_err(|e| DriverError::Connection(format!("postgres connect: {e}")))?;

        // Spawn the connection task — it runs until the client is dropped.
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("postgres connection error: {e}");
            }
        });

        Ok(Box::new(PostgresConnection { client }))
    }

    fn supports_transactions(&self) -> bool {
        true
    }

    fn supports_prepared_statements(&self) -> bool {
        true
    }

    fn param_style(&self) -> rivers_driver_sdk::ParamStyle {
        rivers_driver_sdk::ParamStyle::DollarPositional
    }

    fn supports_introspection(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for PostgresDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "postgres"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // PostgreSQL schemas must be type "object"
        if schema.schema_type != "object" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "postgresql".into(),
                supported: vec!["object".into()],
                schema_file: String::new(),
            });
        }
        // GET schema must have at least one field
        if method == HttpMethod::GET && schema.fields.is_empty() {
            return Err(SchemaSyntaxError::StructuralError {
                message: "GET schema must declare at least one field".into(),
                driver: "postgresql".into(),
                schema_file: String::new(),
            });
        }
        // Validate field types and attributes
        for field in &schema.fields {
            if !POSTGRES_TYPES.contains(&field.field_type.as_str()) {
                return Err(SchemaSyntaxError::InvalidFieldType {
                    field: field.name.clone(),
                    field_type: field.field_type.clone(),
                    schema_file: String::new(),
                });
            }
            // Reject unsupported attributes (e.g., "faker", "key_pattern")
            rivers_driver_sdk::validation::check_supported_attributes(
                field, "postgresql", rivers_driver_sdk::validation::RELATIONAL_ATTRIBUTES, ""
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
            "use DatabaseDriver::connect() + Connection::execute() for PostgreSQL".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // PostgresDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// A live PostgreSQL connection wrapping `tokio_postgres::Client`.
pub struct PostgresConnection {
    client: tokio_postgres::Client,
}

#[async_trait]
impl Connection for PostgresConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        match query.operation.as_str() {
            // -----------------------------------------------------------------
            // Read operations — return rows
            // -----------------------------------------------------------------
            "select" | "query" | "get" | "find" => {
                let params = build_params(&query.parameters);
                let param_refs: Vec<&(dyn ToSql + Sync)> =
                    params.iter().map(|p| &**p as &(dyn ToSql + Sync)).collect();

                let rows = self
                    .client
                    .query(&query.statement as &str, &param_refs)
                    .await
                    .map_err(|e| DriverError::Query(format!("postgres query: {e}")))?;

                let converted = rows_to_maps(&rows);
                let count = converted.len() as u64;
                let column_names = if converted.is_empty() {
                    match self.client.prepare(&query.statement as &str).await {
                        Ok(stmt) => Some(stmt.columns().iter().map(|c| c.name().to_string()).collect()),
                        Err(_) => None,
                    }
                } else {
                    None
                };
                Ok(QueryResult {
                    rows: converted,
                    affected_rows: count,
                    last_insert_id: None,
                    column_names,
                })
            }

            // -----------------------------------------------------------------
            // Insert/Create — check for RETURNING clause
            // -----------------------------------------------------------------
            "insert" | "create" => {
                let params = build_params(&query.parameters);
                let param_refs: Vec<&(dyn ToSql + Sync)> =
                    params.iter().map(|p| &**p as &(dyn ToSql + Sync)).collect();

                let stmt_upper = query.statement.to_uppercase();
                if stmt_upper.contains("RETURNING") {
                    // Use query() to get returned rows
                    let rows = self
                        .client
                        .query(&query.statement as &str, &param_refs)
                        .await
                        .map_err(|e| DriverError::Query(format!("postgres insert+returning: {e}")))?;

                    let last_id = extract_last_insert_id(&rows);
                    let converted = rows_to_maps(&rows);
                    let count = converted.len() as u64;
                    Ok(QueryResult {
                        rows: converted,
                        affected_rows: count,
                        last_insert_id: last_id,
                        column_names: None,
                    })
                } else {
                    let affected = self
                        .client
                        .execute(&query.statement as &str, &param_refs)
                        .await
                        .map_err(|e| DriverError::Query(format!("postgres insert: {e}")))?;

                    Ok(QueryResult {
                        rows: Vec::new(),
                        affected_rows: affected,
                        last_insert_id: None,
                        column_names: None,
                    })
                }
            }

            // -----------------------------------------------------------------
            // Update
            // -----------------------------------------------------------------
            "update" => {
                let params = build_params(&query.parameters);
                let param_refs: Vec<&(dyn ToSql + Sync)> =
                    params.iter().map(|p| &**p as &(dyn ToSql + Sync)).collect();

                let affected = self
                    .client
                    .execute(&query.statement as &str, &param_refs)
                    .await
                    .map_err(|e| DriverError::Query(format!("postgres update: {e}")))?;

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
                let params = build_params(&query.parameters);
                let param_refs: Vec<&(dyn ToSql + Sync)> =
                    params.iter().map(|p| &**p as &(dyn ToSql + Sync)).collect();

                let affected = self
                    .client
                    .execute(&query.statement as &str, &param_refs)
                    .await
                    .map_err(|e| DriverError::Query(format!("postgres delete: {e}")))?;

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
                self.client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|e| DriverError::Query(format!("postgres ping: {e}")))?;
                Ok(QueryResult::empty())
            }

            op => Err(DriverError::Unsupported(format!(
                "postgres driver does not support operation: {op}"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        self.client
            .simple_query("SELECT 1")
            .await
            .map_err(|e| DriverError::Connection(format!("postgres ping: {e}")))?;
        Ok(())
    }

    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        self.client
            .batch_execute("BEGIN")
            .await
            .map_err(|e| DriverError::Query(format!("postgres BEGIN: {e}")))
    }

    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        self.client
            .batch_execute("COMMIT")
            .await
            .map_err(|e| DriverError::Query(format!("postgres COMMIT: {e}")))
    }

    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        self.client
            .batch_execute("ROLLBACK")
            .await
            .map_err(|e| DriverError::Query(format!("postgres ROLLBACK: {e}")))
    }

    async fn ddl_execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let params = build_params(&query.parameters);
        let param_refs: Vec<&(dyn ToSql + Sync)> =
            params.iter().map(|p| &**p as &(dyn ToSql + Sync)).collect();

        let affected = self
            .client
            .execute(&query.statement as &str, &param_refs)
            .await
            .map_err(|e| DriverError::Query(format!("postgres ddl: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: affected,
            last_insert_id: None,
            column_names: None,
        })
    }

    fn driver_name(&self) -> &str {
        "postgres"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `tokio_postgres::Config` from `ConnectionParams`.
///
/// Each value is set via the typed builder API so that spaces, quotes, `=`,
/// `&`, and `'` characters in credentials cannot break parsing or be used to
/// inject additional connection options.
fn build_pg_config(params: &ConnectionParams) -> tokio_postgres::Config {
    let mut config = tokio_postgres::Config::new();
    config
        .host(&params.host)
        .port(params.port)
        .user(&params.username)
        .password(&params.password)
        .dbname(&params.database);
    config
}

/// Build a positional parameter list from named parameters.
///
/// Keys are sorted alphabetically. The DataView engine uses zero-padded
/// numeric keys ("001", "002") for positional styles, so alphabetical
/// sort preserves the correct positional binding order.
fn build_params(parameters: &HashMap<String, QueryValue>) -> Vec<Box<dyn ToSql + Sync + Send>> {
    let mut keys: Vec<&String> = parameters.keys().collect();
    keys.sort();

    keys.into_iter()
        .map(|key| {
            let val = &parameters[key];
            query_value_to_sql(val)
        })
        .collect()
}

/// Convert a `QueryValue` to a boxed `ToSql` for parameter binding.
fn query_value_to_sql(val: &QueryValue) -> Box<dyn ToSql + Sync + Send> {
    match val {
        QueryValue::Null => Box::new(None::<String>),
        QueryValue::Boolean(b) => Box::new(*b),
        QueryValue::Integer(i) => {
            // Use i32 for values that fit, to match PG SERIAL/INT4 columns
            if *i >= i32::MIN as i64 && *i <= i32::MAX as i64 {
                Box::new(*i as i32)
            } else {
                Box::new(*i)
            }
        }
        QueryValue::Float(f) => Box::new(*f),
        QueryValue::String(s) => Box::new(s.clone()),
        QueryValue::Json(v) => Box::new(v.clone()),
        QueryValue::Array(arr) => {
            // Serialize arrays as JSON strings.
            Box::new(serde_json::to_string(arr).unwrap_or_default())
        }
    }
}

/// Convert postgres rows into `Vec<HashMap<String, QueryValue>>`.
fn rows_to_maps(rows: &[tokio_postgres::Row]) -> Vec<HashMap<String, QueryValue>> {
    rows.iter().map(row_to_map).collect()
}

/// Convert a single row to a `HashMap<String, QueryValue>`.
fn row_to_map(row: &tokio_postgres::Row) -> HashMap<String, QueryValue> {
    let mut map = HashMap::new();
    for (i, col) in row.columns().iter().enumerate() {
        let name = col.name().to_string();
        let value = column_to_query_value(row, i, col.type_());
        map.insert(name, value);
    }
    map
}

/// Map a single column value to `QueryValue` based on postgres type.
fn column_to_query_value(row: &tokio_postgres::Row, idx: usize, pg_type: &Type) -> QueryValue {
    // Try JSON/JSONB first since the with-serde_json-1 feature is enabled.
    if *pg_type == Type::JSON || *pg_type == Type::JSONB {
        if let Ok(v) = row.try_get::<_, serde_json::Value>(idx) {
            return QueryValue::Json(v);
        }
    }

    // Boolean
    if *pg_type == Type::BOOL {
        if let Ok(v) = row.try_get::<_, bool>(idx) {
            return QueryValue::Boolean(v);
        }
        // Could be NULL
        if let Ok(None) = row.try_get::<_, Option<bool>>(idx) {
            return QueryValue::Null;
        }
    }

    // Integer types
    if *pg_type == Type::INT2 {
        if let Ok(v) = row.try_get::<_, Option<i16>>(idx) {
            return match v {
                Some(n) => QueryValue::Integer(n as i64),
                None => QueryValue::Null,
            };
        }
    }
    if *pg_type == Type::INT4 {
        if let Ok(v) = row.try_get::<_, Option<i32>>(idx) {
            return match v {
                Some(n) => QueryValue::Integer(n as i64),
                None => QueryValue::Null,
            };
        }
    }
    if *pg_type == Type::INT8 {
        if let Ok(v) = row.try_get::<_, Option<i64>>(idx) {
            return match v {
                Some(n) => QueryValue::Integer(n),
                None => QueryValue::Null,
            };
        }
    }

    // Float types
    if *pg_type == Type::FLOAT4 {
        if let Ok(v) = row.try_get::<_, Option<f32>>(idx) {
            return match v {
                Some(n) => QueryValue::Float(n as f64),
                None => QueryValue::Null,
            };
        }
    }
    if *pg_type == Type::FLOAT8 || *pg_type == Type::NUMERIC {
        if let Ok(v) = row.try_get::<_, Option<f64>>(idx) {
            return match v {
                Some(n) => QueryValue::Float(n),
                None => QueryValue::Null,
            };
        }
    }

    // Text / VARCHAR / other string-like types — try as String (covers most types).
    if let Ok(v) = row.try_get::<_, Option<String>>(idx) {
        return match v {
            Some(s) => QueryValue::String(s),
            None => QueryValue::Null,
        };
    }

    // Fallback: null
    QueryValue::Null
}

/// Extract `last_insert_id` from the first returned row's `id` column.
fn extract_last_insert_id(rows: &[tokio_postgres::Row]) -> Option<String> {
    let row = rows.first()?;
    // Try i64 first (typical serial/bigserial), then i32, then String.
    if let Ok(v) = row.try_get::<_, i64>("id") {
        return Some(v.to_string());
    }
    if let Ok(v) = row.try_get::<_, i32>("id") {
        return Some(v.to_string());
    }
    if let Ok(v) = row.try_get::<_, String>("id") {
        return Some(v);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "postgres".into(),
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
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![
                make_field("id", "uuid", true),
                make_field("name", "text", true),
                make_field("metadata", "jsonb", false),
                make_field("balance", "decimal", false),
                make_field("count", "bigint", false),
                make_field("content", "bytea", false),
                make_field("email", "email", false),
                make_field("phone", "phone", false),
                make_field("site", "url", false),
                make_field("data", "bytes", false),
            ],
        );
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_object() {
        let driver = PostgresDriver;
        let schema = make_schema("hash", vec![make_field("id", "uuid", true)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_rejects_unknown_field_type() {
        let driver = PostgresDriver;
        let schema = make_schema("object", vec![make_field("data", "blob", false)]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::InvalidFieldType { .. }));
    }

    #[test]
    fn schema_syntax_get_requires_fields() {
        let driver = PostgresDriver;
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_post_allows_empty_fields() {
        let driver = PostgresDriver;
        let schema = make_schema("object", vec![]);
        assert!(driver.check_schema_syntax(&schema, HttpMethod::POST).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_faker_attribute() {
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![make_field_with("name", "text", true, vec![("faker", serde_json::json!("name"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_accepts_valid_data() {
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![
                make_field("name", "text", true),
                make_field("age", "integer", false),
            ],
        );
        let data = serde_json::json!({"name": "Alice", "age": 30});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![make_field("email", "text", true)],
        );
        let data = serde_json::json!({"name": "Alice"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "email"));
    }

    #[test]
    fn validate_rejects_non_object_data() {
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![make_field("name", "text", true)],
        );
        let data = serde_json::json!([1, 2, 3]);
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_type_mismatch_detected() {
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![make_field("age", "integer", true)],
        );
        let data = serde_json::json!({"age": "not_a_number"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_constraint_violation_detected() {
        let driver = PostgresDriver;
        let schema = make_schema(
            "object",
            vec![make_field_with("age", "integer", true, vec![("max", serde_json::json!(150))])],
        );
        let data = serde_json::json!({"age": 200});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::ConstraintViolation { .. }));
    }

    // ── F3: connection-config builder ────────────────────────────
    //
    // Regression: prior code interpolated host/user/password/dbname into a
    // libpq-style key=value string. Special characters in the password (e.g.
    // a space, `'`, `=`, `&`) could break parsing or smuggle additional
    // options. The builder API escapes/encodes each field, so any UTF-8
    // string is safe.

    use rivers_driver_sdk::ConnectionParams;

    fn params_with_password(password: &str) -> ConnectionParams {
        ConnectionParams {
            host: "localhost".into(),
            port: 5432,
            database: "rivers".into(),
            username: "rivers".into(),
            password: password.into(),
            options: HashMap::new(),
        }
    }

    fn params_with_database(database: &str) -> ConnectionParams {
        ConnectionParams {
            host: "192.168.2.209".into(),
            port: 5432,
            database: database.into(),
            username: "rivers".into(),
            password: "rivers_test".into(),
            options: HashMap::new(),
        }
    }

    #[test]
    fn build_pg_config_handles_password_with_spaces() {
        let p = params_with_password("a password with spaces");
        let cfg = super::build_pg_config(&p);
        assert_eq!(cfg.get_password(), Some("a password with spaces".as_bytes()));
        assert_eq!(cfg.get_user(), Some("rivers"));
        assert_eq!(cfg.get_dbname(), Some("rivers"));
    }

    #[test]
    fn build_pg_config_handles_password_with_special_chars() {
        // All of these would have broken or altered the old key=value string.
        for pw in &[
            "p'ass=word",
            "p\"a&ss=word",
            "p ass'wo=rd",
            "weird & dangerous=' value",
            "trailing-backslash\\",
            "unicode-éñ-密码",
        ] {
            let p = params_with_password(pw);
            let cfg = super::build_pg_config(&p);
            assert_eq!(
                cfg.get_password(),
                Some(pw.as_bytes()),
                "password roundtrip failed for {:?}",
                pw
            );
        }
    }

    #[test]
    fn build_pg_config_handles_database_with_special_chars() {
        for db in &["db with space", "db'name", "db=other", "weird&db"] {
            let p = params_with_database(db);
            let cfg = super::build_pg_config(&p);
            assert_eq!(cfg.get_dbname(), Some(*db), "dbname roundtrip failed for {:?}", db);
        }
    }

    #[test]
    fn build_pg_config_password_cannot_inject_options() {
        // Pre-fix, this password would expand to:
        //   host=... password=secret sslmode=disable user=... dbname=...
        // which would silently disable TLS in TLS-aware deployments.
        let p = params_with_password("secret sslmode=disable");
        let cfg = super::build_pg_config(&p);
        // The full password (including the would-be injection) is taken verbatim.
        assert_eq!(
            cfg.get_password(),
            Some("secret sslmode=disable".as_bytes())
        );
        // And we did not accidentally set ssl_mode from the password contents.
        // tokio_postgres defaults to SslMode::Prefer; assert it wasn't changed
        // to SslMode::Disable by the injection.
        assert_ne!(cfg.get_ssl_mode(), tokio_postgres::config::SslMode::Disable);
    }

    /// Live PostgreSQL connection test (gated behind PG_AVAILABLE=1).
    ///
    /// Asserts that a password containing spaces and special characters
    /// can actually authenticate against a real cluster. Requires the test
    /// cluster at 192.168.2.209 with `rivers/rivers_test` credentials.
    ///
    /// Note: this test connects with the real (non-special-char) password
    /// configured on the cluster; the builder-roundtrip tests above cover
    /// the special-char escaping. We just verify that the new connect path
    /// still produces a working connection end-to-end.
    #[tokio::test]
    async fn build_pg_config_live_connect() {
        if std::env::var("PG_AVAILABLE").ok().as_deref() != Some("1") {
            println!("skipping: PG_AVAILABLE != 1");
            return;
        }
        let driver = PostgresDriver;
        let params = ConnectionParams {
            host: "192.168.2.209".into(),
            port: 5432,
            database: "rivers".into(),
            username: "rivers".into(),
            password: "rivers_test".into(),
            options: HashMap::new(),
        };
        let mut conn = driver
            .connect(&params)
            .await
            .expect("live postgres connect");
        conn.ping().await.expect("live postgres ping");
    }
}
