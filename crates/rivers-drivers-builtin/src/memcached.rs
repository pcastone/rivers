//! MemcachedDriver — async Memcached via async-memcached.
//!
//! Per `rivers-driver-spec.md` §4.2:
//! Operations: `get`, `set`, `delete`, `ping`.
//!
//! Memcached is a key-value store, so operations are parameterized via
//! `query.parameters` rather than SQL statements:
//! - `key` — the cache key (required for get/set/delete)
//! - `value` — the value to store (required for set)
//! - `expiration` — TTL in seconds (optional for set, defaults to 0)

use std::collections::HashMap;

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Memcached database driver backed by `async-memcached`.
///
/// Stateless factory — each call to `connect()` creates a new
/// [`MemcachedConnection`] with its own TCP connection.
///
/// See `rivers-driver-spec.md` §4.2.
pub struct MemcachedDriver;

#[async_trait]
impl DatabaseDriver for MemcachedDriver {
    fn name(&self) -> &str {
        "memcached"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let dsn = format!("tcp://{}:{}", params.host, params.port);

        let client = async_memcached::Client::new(&dsn)
            .await
            .map_err(|e| DriverError::Connection(format!("memcached connect to {dsn}: {e}")))?;

        Ok(Box::new(MemcachedConnection { client }))
    }
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

#[async_trait]
impl Driver for MemcachedDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "memcached"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        if schema.schema_type != "string" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "memcached".into(),
                supported: vec!["string".into()],
                schema_file: String::new(),
            });
        }
        if !schema.extra.contains_key("key_pattern") {
            return Err(SchemaSyntaxError::MissingRequiredField {
                field: "key_pattern".into(),
                driver: "memcached".into(),
                schema_file: String::new(),
            });
        }
        if !schema.extra.contains_key("value_type") {
            return Err(SchemaSyntaxError::MissingRequiredField {
                field: "value_type".into(),
                driver: "memcached".into(),
                schema_file: String::new(),
            });
        }
        if !schema.fields.is_empty() {
            return Err(SchemaSyntaxError::StructuralError {
                message: "memcached does not support structured fields".into(),
                driver: "memcached".into(),
                schema_file: String::new(),
            });
        }
        if method == HttpMethod::PUT {
            return Err(SchemaSyntaxError::UnsupportedMethod {
                method: "PUT".into(),
                driver: "memcached".into(),
                schema_file: String::new(),
            });
        }
        Ok(())
    }

    fn validate(
        &self,
        _data: &serde_json::Value,
        _schema: &SchemaDefinition,
        _direction: ValidationDirection,
    ) -> Result<(), ValidationError> {
        Ok(()) // Simple KV — value_type checked at syntax level
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for Memcached".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // MemcachedDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// A live Memcached connection wrapping `async_memcached::Client`.
pub struct MemcachedConnection {
    client: async_memcached::Client,
}

#[async_trait]
impl Connection for MemcachedConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        match query.operation.as_str() {
            // -----------------------------------------------------------------
            // GET — retrieve a value by key
            // -----------------------------------------------------------------
            "get" => {
                let key = get_str_param(query, "key")?;

                let result = self
                    .client
                    .get(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("memcached GET: {e}")))?;

                match result {
                    Some(value) => {
                        let data = String::from_utf8(value.data)
                            .unwrap_or_else(|e| {
                                // Fall back to hex encoding for non-UTF8 data.
                                e.into_bytes()
                                    .iter()
                                    .map(|b| format!("{:02x}", b))
                                    .collect()
                            });
                        let mut row = HashMap::new();
                        row.insert("value".to_string(), QueryValue::String(data));
                        Ok(QueryResult {
                            rows: vec![row],
                            affected_rows: 1,
                            last_insert_id: None,
                        })
                    }
                    None => Ok(QueryResult::empty()),
                }
            }

            // -----------------------------------------------------------------
            // SET — store a value with optional TTL
            // -----------------------------------------------------------------
            "set" => {
                let key = get_str_param(query, "key")?;
                let value = get_str_param(query, "value")?;
                let expiration = get_int_param(query, "expiration").unwrap_or(0);

                self.client
                    .set(&key, value.as_bytes(), Some(expiration), None)
                    .await
                    .map_err(|e| DriverError::Query(format!("memcached SET: {e}")))?;

                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }

            // -----------------------------------------------------------------
            // DELETE — remove a key
            // -----------------------------------------------------------------
            "delete" | "del" => {
                let key = get_str_param(query, "key")?;

                self.client
                    .delete(&key)
                    .await
                    .map_err(|e| DriverError::Query(format!("memcached DELETE: {e}")))?;

                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }

            // -----------------------------------------------------------------
            // PING — health check via version command
            // -----------------------------------------------------------------
            "ping" => {
                self.client
                    .version()
                    .await
                    .map_err(|e| DriverError::Query(format!("memcached PING (version): {e}")))?;
                Ok(QueryResult::empty())
            }

            op => Err(DriverError::Unsupported(format!(
                "memcached driver does not support operation: {op}"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        self.client
            .version()
            .await
            .map_err(|e| DriverError::Connection(format!("memcached ping: {e}")))?;
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "memcached"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a string parameter from the query.
fn get_str_param(query: &Query, name: &str) -> Result<String, DriverError> {
    match query.parameters.get(name) {
        Some(QueryValue::String(s)) => Ok(s.clone()),
        Some(v) => Ok(format!("{:?}", v)),
        None => Err(DriverError::Query(format!(
            "missing required parameter: {name}"
        ))),
    }
}

/// Extract an integer parameter from the query.
fn get_int_param(query: &Query, name: &str) -> Result<i64, DriverError> {
    match query.parameters.get(name) {
        Some(QueryValue::Integer(n)) => Ok(*n),
        Some(QueryValue::String(s)) => s.parse::<i64>().map_err(|_| {
            DriverError::Query(format!(
                "parameter '{name}' is not a valid integer: {s}"
            ))
        }),
        Some(_) => Err(DriverError::Query(format!(
            "parameter '{name}' must be an integer"
        ))),
        None => Err(DriverError::Query(format!(
            "missing required parameter: {name}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

    fn make_schema_with_extra(
        schema_type: &str,
        fields: Vec<SchemaFieldDef>,
        extra_pairs: Vec<(&str, serde_json::Value)>,
    ) -> SchemaDefinition {
        let mut extra = HashMap::new();
        for (k, v) in extra_pairs {
            extra.insert(k.to_string(), v);
        }
        SchemaDefinition {
            driver: "memcached".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra,
        }
    }

    fn make_valid_schema() -> SchemaDefinition {
        make_schema_with_extra(
            "string",
            vec![],
            vec![
                ("key_pattern", serde_json::json!("session:${id}")),
                ("value_type", serde_json::json!("string")),
            ],
        )
    }

    fn make_field(name: &str, field_type: &str, required: bool) -> SchemaFieldDef {
        SchemaFieldDef {
            name: name.into(),
            field_type: field_type.into(),
            required,
            constraints: HashMap::new(),
        }
    }

    #[test]
    fn schema_syntax_valid() {
        let driver = MemcachedDriver;
        let schema = make_valid_schema();
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_string_type() {
        let driver = MemcachedDriver;
        let schema = make_schema_with_extra(
            "hash",
            vec![],
            vec![
                ("key_pattern", serde_json::json!("x:${id}")),
                ("value_type", serde_json::json!("string")),
            ],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_requires_key_pattern() {
        let driver = MemcachedDriver;
        let schema = make_schema_with_extra(
            "string",
            vec![],
            vec![("value_type", serde_json::json!("string"))],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "key_pattern"));
    }

    #[test]
    fn schema_syntax_requires_value_type() {
        let driver = MemcachedDriver;
        let schema = make_schema_with_extra(
            "string",
            vec![],
            vec![("key_pattern", serde_json::json!("x:${id}"))],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "value_type"));
    }

    #[test]
    fn schema_syntax_rejects_fields() {
        let driver = MemcachedDriver;
        let schema = make_schema_with_extra(
            "string",
            vec![make_field("name", "string", false)],
            vec![
                ("key_pattern", serde_json::json!("x:${id}")),
                ("value_type", serde_json::json!("string")),
            ],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_rejects_put_method() {
        let driver = MemcachedDriver;
        let schema = make_valid_schema();
        let err = driver.check_schema_syntax(&schema, HttpMethod::PUT).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedMethod { .. }));
    }

    #[test]
    fn validate_accepts_any_data() {
        let driver = MemcachedDriver;
        let schema = make_valid_schema();
        let data = serde_json::json!("hello");
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }
}
