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
        let conn_string = format!(
            "host={} port={} user={} password={} dbname={}",
            params.host, params.port, params.username, params.password, params.database
        );

        let (client, connection) = tokio_postgres::connect(&conn_string, tokio_postgres::NoTls)
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
                Ok(QueryResult {
                    rows: converted,
                    affected_rows: count,
                    last_insert_id: None,
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

    fn driver_name(&self) -> &str {
        "postgres"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a positional parameter list from named parameters.
///
/// Parameters are sorted by name alphabetically so the caller can use
/// `$1`, `$2`, ... in the statement in a predictable order.
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
}
