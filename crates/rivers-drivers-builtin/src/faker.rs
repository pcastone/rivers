//! FakerDriver — configurable mock results for testing.
//!
//! Per `rivers-driver-spec.md` §3.1:
//! "Internal driver for unit tests. Returns configurable mock results."
//!
//! The FakerDriver returns rows based on query parameters:
//! - `rows` parameter (Integer) — number of rows to generate
//! - Each row contains: `id` (Integer, 1-based), `name` (String, "faker_N")
//! - `ping` always succeeds
//! - Unknown operations return `DriverError::Unsupported`

use std::collections::HashMap;

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, Driver, DriverError, DriverType, HttpMethod,
    Query, QueryResult, QueryValue, SchemaDefinition, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};

/// Mock database driver for testing.
///
/// Generates synthetic rows based on query parameters. Each row has
/// `id` (Integer) and `name` (String) fields. The number of rows
/// defaults to 1 unless a `rows` parameter is provided.
pub struct FakerDriver {
    /// Default number of rows to return when not specified in query.
    default_rows: usize,
}

impl FakerDriver {
    /// Create a new FakerDriver with default settings.
    pub fn new() -> Self {
        Self { default_rows: 1 }
    }

    /// Create a FakerDriver with a custom default row count.
    pub fn with_default_rows(default_rows: usize) -> Self {
        Self { default_rows }
    }
}

impl Default for FakerDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseDriver for FakerDriver {
    fn name(&self) -> &str {
        "faker"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Ok(Box::new(FakerConnection {
            default_rows: self.default_rows,
        }))
    }
}

// ---------------------------------------------------------------------------
// Unified Driver trait implementation (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

/// Supported faker constraint attributes.
const FAKER_SUPPORTED_ATTRIBUTES: &[&str] = &["faker", "required", "unique", "domain"];

#[async_trait]
impl Driver for FakerDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "faker"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // type must be "object"
        if schema.schema_type != "object" {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "faker".into(),
                supported: vec!["object".into()],
                schema_file: String::new(),
            });
        }
        // only GET supported
        if method != HttpMethod::GET {
            return Err(SchemaSyntaxError::UnsupportedMethod {
                method: method.to_string(),
                driver: "faker".into(),
                schema_file: String::new(),
            });
        }
        // must have at least one field
        if schema.fields.is_empty() {
            return Err(SchemaSyntaxError::StructuralError {
                message: "faker schemas require fields".into(),
                driver: "faker".into(),
                schema_file: String::new(),
            });
        }
        // every field must have faker attribute; reject validation attributes
        for field in &schema.fields {
            if !field.constraints.contains_key("faker") {
                return Err(SchemaSyntaxError::MissingRequiredField {
                    field: format!("{}.faker", field.name),
                    driver: "faker".into(),
                    schema_file: String::new(),
                });
            }
            // Reject validation attributes
            for attr in ["min", "max", "pattern", "min_length", "max_length"] {
                if field.constraints.contains_key(attr) {
                    return Err(SchemaSyntaxError::UnsupportedAttribute {
                        attribute: attr.into(),
                        field: field.name.clone(),
                        driver: "faker".into(),
                        supported: FAKER_SUPPORTED_ATTRIBUTES.iter().map(|s| s.to_string()).collect(),
                        schema_file: String::new(),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate(
        &self,
        data: &serde_json::Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError> {
        // Output validation only — check generated values match types
        rivers_driver_sdk::validation::validate_fields(data, schema, direction)
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for Faker".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // FakerDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}

/// A mock connection that generates synthetic data.
pub struct FakerConnection {
    default_rows: usize,
}

impl FakerConnection {
    /// Generate N fake rows with `id` and `name` fields.
    fn generate_rows(&self, count: usize) -> Vec<HashMap<String, QueryValue>> {
        (1..=count)
            .map(|i| {
                let mut row = HashMap::new();
                row.insert("id".to_string(), QueryValue::Integer(i as i64));
                row.insert("name".to_string(), QueryValue::String(format!("faker_{}", i)));
                row
            })
            .collect()
    }

    /// Extract the requested row count from query parameters.
    fn row_count(&self, query: &Query) -> usize {
        match query.parameters.get("rows") {
            Some(QueryValue::Integer(n)) if *n > 0 => *n as usize,
            _ => self.default_rows,
        }
    }
}

#[async_trait]
impl Connection for FakerConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Schema file paths (e.g. "schemas/contact.schema.json") are used as
        // the DataView query for faker datasources. Normalise them to "select".
        let op = if query.operation.ends_with(".json") || query.operation.ends_with(".schema.json") {
            "select"
        } else {
            query.operation.as_str()
        };
        match op {
            "select" | "query" | "get" | "find" => {
                let count = self.row_count(query);
                let rows = self.generate_rows(count);
                let affected = rows.len() as u64;
                Ok(QueryResult {
                    rows,
                    affected_rows: affected,
                    last_insert_id: None,
                    column_names: None,
                })
            }
            "insert" | "set" | "create" => {
                let count = self.row_count(query);
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count as u64,
                    last_insert_id: Some("1".to_string()),
                    column_names: None,
                })
            }
            "update" => {
                let count = self.row_count(query);
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count as u64,
                    last_insert_id: None,
                    column_names: None,
                })
            }
            "delete" | "del" | "remove" => {
                let count = self.row_count(query);
                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: count as u64,
                    last_insert_id: None,
                    column_names: None,
                })
            }
            "ping" => Ok(QueryResult::empty()),
            _ => Err(DriverError::Unsupported(format!(
                "faker driver does not support operation: {}",
                query.operation
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "faker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection};

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

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "faker".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra: HashMap::new(),
        }
    }

    fn make_valid_schema() -> SchemaDefinition {
        make_schema(
            "object",
            vec![
                make_field_with("id", "uuid", true, vec![("faker", serde_json::json!("uuid"))]),
                make_field_with("name", "string", true, vec![("faker", serde_json::json!("name"))]),
                make_field_with("email", "email", false, vec![("faker", serde_json::json!("email"))]),
            ],
        )
    }

    #[test]
    fn schema_syntax_valid() {
        let driver = FakerDriver::new();
        let schema = make_valid_schema();
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_non_object_type() {
        let driver = FakerDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field_with("id", "string", true, vec![("faker", serde_json::json!("uuid"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_rejects_post_method() {
        let driver = FakerDriver::new();
        let schema = make_valid_schema();
        let err = driver.check_schema_syntax(&schema, HttpMethod::POST).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedMethod { .. }));
    }

    #[test]
    fn schema_syntax_requires_fields() {
        let driver = FakerDriver::new();
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_requires_faker_attribute() {
        let driver = FakerDriver::new();
        let schema = make_schema(
            "object",
            vec![SchemaFieldDef {
                name: "name".into(),
                field_type: "string".into(),
                required: true,
                constraints: HashMap::new(), // no faker attribute
            }],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { .. }));
    }

    #[test]
    fn schema_syntax_rejects_min_attribute() {
        let driver = FakerDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field_with("age", "integer", true, vec![
                ("faker", serde_json::json!("age")),
                ("min", serde_json::json!(0)),
            ])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn schema_syntax_rejects_pattern_attribute() {
        let driver = FakerDriver::new();
        let schema = make_schema(
            "object",
            vec![make_field_with("code", "string", true, vec![
                ("faker", serde_json::json!("code")),
                ("pattern", serde_json::json!("^[A-Z]+$")),
            ])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_accepts_valid_output_data() {
        let driver = FakerDriver::new();
        let schema = make_valid_schema();
        let data = serde_json::json!({
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "name": "Alice",
            "email": "alice@example.com"
        });
        assert!(driver.validate(&data, &schema, ValidationDirection::Output).is_ok());
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let driver = FakerDriver::new();
        let schema = make_valid_schema();
        let data = serde_json::json!({"email": "alice@example.com"});
        let err = driver.validate(&data, &schema, ValidationDirection::Output).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { .. }));
    }
}
