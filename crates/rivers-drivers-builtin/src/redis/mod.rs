//! RedisDriver -- full async Redis database driver implementation.
//!
//! Per `rivers-driver-spec.md` S4:
//! Redis is a first-class built-in driver with 18+ operations
//! (get, set, del, hget, hgetall, lpush, rpush, etc.).
//!
//! Uses `redis::aio::MultiplexedConnection` for non-blocking I/O.

mod cluster;
mod driver;
mod params;
mod single;
mod validation;

pub use self::driver::RedisDriver;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use rivers_driver_sdk::{Driver, HttpMethod, SchemaDefinition, SchemaFieldDef, ValidationDirection, SchemaSyntaxError, ValidationError};

    fn make_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        let mut extra = HashMap::new();
        extra.insert("key_pattern".to_string(), serde_json::json!("user:${id}"));
        SchemaDefinition {
            driver: "redis".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra,
        }
    }

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
            driver: "redis".into(),
            schema_type: schema_type.into(),
            description: String::new(),
            fields,
            extra,
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
    fn schema_syntax_hash_valid() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![
                make_field("name", "string", true),
                make_field("score", "integer", false),
            ],
        );
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_string_valid() {
        let driver = RedisDriver::new();
        let mut schema = make_schema("string", vec![]);
        schema.extra.insert("value_type".to_string(), serde_json::json!("string"));
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_string_rejects_fields() {
        let driver = RedisDriver::new();
        let mut schema = make_schema(
            "string",
            vec![make_field("a", "string", false)],
        );
        schema.extra.insert("value_type".to_string(), serde_json::json!("string"));
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_string_requires_value_type() {
        let driver = RedisDriver::new();
        let schema = make_schema("string", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "value_type"));
    }

    #[test]
    fn schema_syntax_list_requires_element_type() {
        let driver = RedisDriver::new();
        let schema = make_schema("list", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "element_type"));
    }

    #[test]
    fn schema_syntax_list_valid_with_element_type() {
        let driver = RedisDriver::new();
        let mut schema = make_schema("list", vec![]);
        schema.extra.insert("element_type".to_string(), serde_json::json!("string"));
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_sorted_set_requires_member_and_score() {
        let driver = RedisDriver::new();
        let schema = make_schema("sorted_set", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { .. }));
    }

    #[test]
    fn schema_syntax_sorted_set_valid() {
        let driver = RedisDriver::new();
        let mut schema = make_schema("sorted_set", vec![]);
        schema.extra.insert("member_type".to_string(), serde_json::json!("string"));
        schema.extra.insert("score_type".to_string(), serde_json::json!("float"));
        assert!(driver.check_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn schema_syntax_rejects_object_type() {
        let driver = RedisDriver::new();
        let schema = make_schema("object", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedType { .. }));
    }

    #[test]
    fn schema_syntax_missing_key_pattern() {
        let driver = RedisDriver::new();
        let schema = make_schema_with_extra("hash", vec![make_field("name", "string", true)], vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::MissingRequiredField { ref field, .. } if field == "key_pattern"));
    }

    #[test]
    fn schema_syntax_hash_requires_fields_for_get() {
        let driver = RedisDriver::new();
        let schema = make_schema("hash", vec![]);
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::StructuralError { .. }));
    }

    #[test]
    fn schema_syntax_rejects_faker_attribute() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field_with("name", "string", true, vec![("faker", serde_json::json!("name"))])],
        );
        let err = driver.check_schema_syntax(&schema, HttpMethod::GET).unwrap_err();
        assert!(matches!(err, SchemaSyntaxError::UnsupportedAttribute { .. }));
    }

    #[test]
    fn validate_hash_accepts_valid_object() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"name": "Alice"});
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_hash_rejects_missing_required() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field("name", "string", true)],
        );
        let data = serde_json::json!({"score": 42});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { ref field, .. } if field == "name"));
    }

    #[test]
    fn validate_string_accepts_scalar() {
        let driver = RedisDriver::new();
        let schema = make_schema("string", vec![]);
        let data = serde_json::json!("hello");
        assert!(driver.validate(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_string_rejects_object() {
        let driver = RedisDriver::new();
        let schema = make_schema("string", vec![]);
        let data = serde_json::json!({"key": "value"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }

    #[test]
    fn validate_hash_type_mismatch_detected() {
        let driver = RedisDriver::new();
        let schema = make_schema(
            "hash",
            vec![make_field("count", "integer", true)],
        );
        let data = serde_json::json!({"count": "not_a_number"});
        let err = driver.validate(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::TypeMismatch { .. }));
    }
}
