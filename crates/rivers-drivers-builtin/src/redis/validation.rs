//! Unified `Driver` trait implementation for Redis (schema validation).

use std::collections::HashMap;

use async_trait::async_trait;
use rivers_driver_sdk::{
    ConnectionParams, Driver, DriverError, DriverType, HttpMethod, Query, QueryResult, QueryValue,
    SchemaDefinition, SchemaSyntaxError, ValidationDirection, ValidationError,
};

use super::driver::RedisDriver;

#[async_trait]
impl Driver for RedisDriver {
    fn driver_type(&self) -> DriverType {
        DriverType::Database
    }

    fn name(&self) -> &str {
        "redis"
    }

    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError> {
        // Type must be one of: hash, string, list, set, sorted_set
        let valid_types = ["hash", "string", "list", "set", "sorted_set"];
        if !valid_types.contains(&schema.schema_type.as_str()) {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: schema.schema_type.clone(),
                driver: "redis".into(),
                supported: valid_types.iter().map(|s| s.to_string()).collect(),
                schema_file: String::new(),
            });
        }

        // key_pattern required (check in extra)
        if !schema.extra.contains_key("key_pattern") {
            return Err(SchemaSyntaxError::MissingRequiredField {
                field: "key_pattern".into(),
                driver: "redis".into(),
                schema_file: String::new(),
            });
        }

        // Type-specific validation
        match schema.schema_type.as_str() {
            "hash" => {
                if (method == HttpMethod::POST || method == HttpMethod::GET || method == HttpMethod::PUT)
                    && schema.fields.is_empty()
                {
                    return Err(SchemaSyntaxError::StructuralError {
                        message: "Redis hash schemas require fields".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            "string" => {
                if !schema.fields.is_empty() {
                    return Err(SchemaSyntaxError::StructuralError {
                        message: "Redis string schemas must not declare fields".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
                if !schema.extra.contains_key("value_type") {
                    return Err(SchemaSyntaxError::MissingRequiredField {
                        field: "value_type".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            "list" | "set" => {
                if !schema.extra.contains_key("element_type") {
                    return Err(SchemaSyntaxError::MissingRequiredField {
                        field: "element_type".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            "sorted_set" => {
                if !schema.extra.contains_key("member_type") || !schema.extra.contains_key("score_type") {
                    return Err(SchemaSyntaxError::MissingRequiredField {
                        field: "member_type and score_type".into(),
                        driver: "redis".into(),
                        schema_file: String::new(),
                    });
                }
            }
            _ => {}
        }

        // Reject faker attribute on fields
        for field in &schema.fields {
            if field.constraints.contains_key("faker") {
                return Err(SchemaSyntaxError::UnsupportedAttribute {
                    attribute: "faker".into(),
                    field: field.name.clone(),
                    driver: "redis".into(),
                    supported: vec!["required".into(), "default".into(), "min".into(), "max".into(), "min_length".into(), "max_length".into(), "pattern".into(), "enum".into()],
                    schema_file: String::new(),
                });
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
        match schema.schema_type.as_str() {
            "hash" => {
                // Validate hash fields like relational data
                rivers_driver_sdk::validation::validate_fields(data, schema, direction)
            }
            "string" => {
                // For string type, data should be a scalar, not an object
                if data.is_object() || data.is_array() {
                    return Err(ValidationError::TypeMismatch {
                        field: "(root)".into(),
                        expected: "scalar".into(),
                        actual: rivers_driver_sdk::validation::json_type_name(data).into(),
                        direction,
                    });
                }
                Ok(())
            }
            _ => Ok(()), // list, set, sorted_set -- basic validation
        }
    }

    async fn execute(
        &self,
        _query: &Query,
        _params: &HashMap<String, QueryValue>,
    ) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented(
            "use DatabaseDriver::connect() + Connection::execute() for Redis".into(),
        ))
    }

    async fn connect(&mut self, _config: &ConnectionParams) -> Result<(), DriverError> {
        Ok(()) // RedisDriver is stateless; real connection happens via DatabaseDriver::connect()
    }

    async fn health_check(&self) -> Result<(), DriverError> {
        Ok(()) // Stateless factory
    }
}
