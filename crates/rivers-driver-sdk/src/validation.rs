//! Common schema validation engine.
//!
//! Per driver-schema-validation-spec §5: shared validation logic for
//! field type checking and constraint enforcement.

use crate::traits::{SchemaDefinition, SchemaFieldDef, ValidationDirection, ValidationError};

/// Validate all fields in data against the schema.
///
/// Checks required fields, type matching, and constraint enforcement.
pub fn validate_fields(
    data: &serde_json::Value,
    schema: &SchemaDefinition,
    direction: ValidationDirection,
) -> Result<(), ValidationError> {
    let obj = match data.as_object() {
        Some(obj) => obj,
        None if schema.fields.is_empty() => return Ok(()),
        None => {
            return Err(ValidationError::TypeMismatch {
                field: "(root)".into(),
                expected: "object".into(),
                actual: json_type_name(data).into(),
                direction,
            });
        }
    };

    for field in &schema.fields {
        match obj.get(&field.name) {
            Some(value) if !value.is_null() => {
                // Type check
                validate_field_type(value, &field.field_type, &field.name, direction)?;
                // Constraint checks
                validate_field_constraints(value, field, direction)?;
            }
            _ => {
                if field.required {
                    return Err(ValidationError::MissingRequired {
                        field: field.name.clone(),
                        direction,
                    });
                }
            }
        }
    }

    Ok(())
}

/// Check if a JSON value matches the declared Rivers primitive type.
pub fn validate_field_type(
    value: &serde_json::Value,
    expected_type: &str,
    field_name: &str,
    direction: ValidationDirection,
) -> Result<(), ValidationError> {
    let valid = match expected_type {
        "string" | "email" | "phone" | "url" | "datetime" | "date" => value.is_string(),
        "uuid" => value.is_string() && is_uuid_format(value.as_str().unwrap_or("")),
        "integer" => value.is_i64() || value.is_u64(),
        "float" | "decimal" => value.is_number(),
        "boolean" => value.is_boolean(),
        "json" => true, // any JSON value is valid
        "bytes" => value.is_string(), // base64-encoded
        _ => true, // unknown types pass through
    };

    if !valid {
        return Err(ValidationError::TypeMismatch {
            field: field_name.into(),
            expected: expected_type.into(),
            actual: json_type_name(value).into(),
            direction,
        });
    }

    // Format validation for specific types
    if let Some(s) = value.as_str() {
        match expected_type {
            "email" => {
                if !s.contains('@') || !s.contains('.') {
                    return Err(ValidationError::ConstraintViolation {
                        field: field_name.into(),
                        constraint: "email_format".into(),
                        value: s.into(),
                        limit: "must contain @ and .".into(),
                        direction,
                    });
                }
            }
            "url" => {
                if !s.starts_with("http://") && !s.starts_with("https://") {
                    return Err(ValidationError::ConstraintViolation {
                        field: field_name.into(),
                        constraint: "url_format".into(),
                        value: s.into(),
                        limit: "must start with http:// or https://".into(),
                        direction,
                    });
                }
            }
            "datetime" => {
                // Basic ISO 8601 check
                if s.len() < 10 || !s.contains('T') && !s.contains(' ') {
                    if s.len() != 10 {
                        // allow date-only for datetime
                        return Err(ValidationError::ConstraintViolation {
                            field: field_name.into(),
                            constraint: "datetime_format".into(),
                            value: s.into(),
                            limit: "ISO 8601 format expected".into(),
                            direction,
                        });
                    }
                }
            }
            "date" => {
                if s.len() != 10
                    || s.chars().nth(4) != Some('-')
                    || s.chars().nth(7) != Some('-')
                {
                    return Err(ValidationError::ConstraintViolation {
                        field: field_name.into(),
                        constraint: "date_format".into(),
                        value: s.into(),
                        limit: "YYYY-MM-DD format expected".into(),
                        direction,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Check field constraints: min, max, min_length, max_length, pattern, enum.
pub fn validate_field_constraints(
    value: &serde_json::Value,
    field: &SchemaFieldDef,
    direction: ValidationDirection,
) -> Result<(), ValidationError> {
    // min/max for numbers
    if let Some(min_val) = field.constraints.get("min").and_then(|v| v.as_f64()) {
        if let Some(num) = value.as_f64() {
            if num < min_val {
                return Err(ValidationError::ConstraintViolation {
                    field: field.name.clone(),
                    constraint: "min".into(),
                    value: num.to_string(),
                    limit: min_val.to_string(),
                    direction,
                });
            }
        }
    }

    if let Some(max_val) = field.constraints.get("max").and_then(|v| v.as_f64()) {
        if let Some(num) = value.as_f64() {
            if num > max_val {
                return Err(ValidationError::ConstraintViolation {
                    field: field.name.clone(),
                    constraint: "max".into(),
                    value: num.to_string(),
                    limit: max_val.to_string(),
                    direction,
                });
            }
        }
    }

    // min_length/max_length for strings
    if let Some(s) = value.as_str() {
        if let Some(min_len) = field.constraints.get("min_length").and_then(|v| v.as_u64()) {
            if (s.len() as u64) < min_len {
                return Err(ValidationError::ConstraintViolation {
                    field: field.name.clone(),
                    constraint: "min_length".into(),
                    value: s.len().to_string(),
                    limit: min_len.to_string(),
                    direction,
                });
            }
        }

        if let Some(max_len) = field.constraints.get("max_length").and_then(|v| v.as_u64()) {
            if (s.len() as u64) > max_len {
                return Err(ValidationError::ConstraintViolation {
                    field: field.name.clone(),
                    constraint: "max_length".into(),
                    value: s.len().to_string(),
                    limit: max_len.to_string(),
                    direction,
                });
            }
        }

        // pattern (regex)
        if let Some(pattern) = field.constraints.get("pattern").and_then(|v| v.as_str()) {
            // Simple pattern matching — full regex would need the regex crate
            // For V1: check if it starts with ^ and ends with $ (anchored), do basic matching
            if pattern.starts_with('^') && pattern.ends_with('$') {
                let inner = &pattern[1..pattern.len() - 1];
                // Very basic: check if the string matches a simple alternative pattern like "a|b|c"
                if inner.contains('|') {
                    let alternatives: Vec<&str> = inner.split('|').collect();
                    if !alternatives.contains(&s) {
                        return Err(ValidationError::ConstraintViolation {
                            field: field.name.clone(),
                            constraint: "pattern".into(),
                            value: s.into(),
                            limit: pattern.into(),
                            direction,
                        });
                    }
                }
            }
        }
    }

    // enum whitelist
    if let Some(enum_values) = field.constraints.get("enum").and_then(|v| v.as_array()) {
        let value_str = match value {
            serde_json::Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        let matches = enum_values.iter().any(|ev| match ev {
            serde_json::Value::String(s) => s == &value_str,
            other => serde_json::to_string(other)
                .ok()
                .map_or(false, |s| s == value_str),
        });
        if !matches {
            return Err(ValidationError::ConstraintViolation {
                field: field.name.clone(),
                constraint: "enum".into(),
                value: value_str,
                limit: format!("{:?}", enum_values),
                direction,
            });
        }
    }

    Ok(())
}

/// Get a human-readable type name for a JSON value.
pub fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "float"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Check if a string looks like a UUID v4.
fn is_uuid_format(s: &str) -> bool {
    s.len() == 36
        && s.chars().nth(8) == Some('-')
        && s.chars().nth(13) == Some('-')
        && s.chars().nth(18) == Some('-')
        && s.chars().nth(23) == Some('-')
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Validate supported attributes on a field for a specific driver.
///
/// Returns an error if any attribute is not in the supported list.
pub fn check_supported_attributes(
    field: &SchemaFieldDef,
    driver: &str,
    supported: &[&str],
    schema_file: &str,
) -> Result<(), crate::traits::SchemaSyntaxError> {
    for attr_name in field.constraints.keys() {
        if !supported.contains(&attr_name.as_str()) {
            return Err(crate::traits::SchemaSyntaxError::UnsupportedAttribute {
                attribute: attr_name.clone(),
                field: field.name.clone(),
                driver: driver.into(),
                supported: supported.iter().map(|s| s.to_string()).collect(),
                schema_file: schema_file.into(),
            });
        }
    }
    Ok(())
}

/// Common Rivers primitive types accepted by relational drivers.
pub const RELATIONAL_TYPES: &[&str] = &[
    "uuid", "string", "text", "integer", "bigint", "float", "decimal", "boolean", "datetime",
    "date", "json", "jsonb", "bytes", "email", "phone", "url",
];

/// Common supported field attributes for relational drivers.
pub const RELATIONAL_ATTRIBUTES: &[&str] = &[
    "required", "default", "min", "max", "min_length", "max_length", "pattern", "enum",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{SchemaDefinition, SchemaFieldDef};
    use std::collections::HashMap;

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

    fn make_schema(fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "postgresql".into(),
            schema_type: "object".into(),
            description: String::new(),
            fields,
            extra: HashMap::new(),
        }
    }

    #[test]
    fn validate_required_field_present() {
        let schema = make_schema(vec![make_field("name", "string", true)]);
        let data = serde_json::json!({"name": "alice"});
        assert!(validate_fields(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_required_field_missing() {
        let schema = make_schema(vec![make_field("name", "string", true)]);
        let data = serde_json::json!({});
        let err = validate_fields(&data, &schema, ValidationDirection::Input).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRequired { .. }));
    }

    #[test]
    fn validate_type_integer() {
        assert!(validate_field_type(
            &serde_json::json!(42),
            "integer",
            "age",
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_type(
            &serde_json::json!("42"),
            "integer",
            "age",
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_type_boolean() {
        assert!(validate_field_type(
            &serde_json::json!(true),
            "boolean",
            "active",
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_type(
            &serde_json::json!("true"),
            "boolean",
            "active",
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_type_email() {
        assert!(validate_field_type(
            &serde_json::json!("user@example.com"),
            "email",
            "email",
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_type(
            &serde_json::json!("notanemail"),
            "email",
            "email",
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_type_uuid() {
        assert!(validate_field_type(
            &serde_json::json!("550e8400-e29b-41d4-a716-446655440000"),
            "uuid",
            "id",
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_type(
            &serde_json::json!("not-a-uuid"),
            "uuid",
            "id",
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_min_constraint() {
        let field = make_field_with(
            "amount",
            "float",
            true,
            vec![("min", serde_json::json!(0.0))],
        );
        assert!(
            validate_field_constraints(&serde_json::json!(10.0), &field, ValidationDirection::Input)
                .is_ok()
        );
        assert!(
            validate_field_constraints(&serde_json::json!(-1.0), &field, ValidationDirection::Input)
                .is_err()
        );
    }

    #[test]
    fn validate_max_constraint() {
        let field = make_field_with(
            "age",
            "integer",
            true,
            vec![("max", serde_json::json!(150))],
        );
        assert!(
            validate_field_constraints(&serde_json::json!(30), &field, ValidationDirection::Input)
                .is_ok()
        );
        assert!(
            validate_field_constraints(&serde_json::json!(200), &field, ValidationDirection::Input)
                .is_err()
        );
    }

    #[test]
    fn validate_max_length_constraint() {
        let field = make_field_with(
            "name",
            "string",
            true,
            vec![("max_length", serde_json::json!(10))],
        );
        assert!(validate_field_constraints(
            &serde_json::json!("alice"),
            &field,
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_constraints(
            &serde_json::json!("a very long name that exceeds"),
            &field,
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_enum_constraint() {
        let field = make_field_with(
            "status",
            "string",
            true,
            vec![("enum", serde_json::json!(["active", "closed"]))],
        );
        assert!(validate_field_constraints(
            &serde_json::json!("active"),
            &field,
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_constraints(
            &serde_json::json!("deleted"),
            &field,
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_pattern_constraint() {
        let field = make_field_with(
            "status",
            "string",
            true,
            vec![("pattern", serde_json::json!("^active|closed$"))],
        );
        assert!(validate_field_constraints(
            &serde_json::json!("active"),
            &field,
            ValidationDirection::Input
        )
        .is_ok());
        assert!(validate_field_constraints(
            &serde_json::json!("deleted"),
            &field,
            ValidationDirection::Input
        )
        .is_err());
    }

    #[test]
    fn validate_optional_field_absent() {
        let schema = make_schema(vec![make_field("nickname", "string", false)]);
        let data = serde_json::json!({});
        assert!(validate_fields(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn validate_non_object_with_fields_fails() {
        let schema = make_schema(vec![make_field("name", "string", true)]);
        let data = serde_json::json!([1, 2, 3]);
        assert!(validate_fields(&data, &schema, ValidationDirection::Input).is_err());
    }

    #[test]
    fn json_type_names() {
        assert_eq!(json_type_name(&serde_json::json!(null)), "null");
        assert_eq!(json_type_name(&serde_json::json!(true)), "boolean");
        assert_eq!(json_type_name(&serde_json::json!(42)), "integer");
        assert_eq!(json_type_name(&serde_json::json!(3.14)), "float");
        assert_eq!(json_type_name(&serde_json::json!("hello")), "string");
        assert_eq!(json_type_name(&serde_json::json!([1])), "array");
        assert_eq!(json_type_name(&serde_json::json!({})), "object");
    }

    #[test]
    fn uuid_format_validation() {
        assert!(is_uuid_format("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_uuid_format("not-a-uuid"));
        assert!(!is_uuid_format("550e8400e29b41d4a716446655440000")); // no dashes
    }

    #[test]
    fn check_supported_attributes_accepts_valid() {
        let field = make_field_with("x", "integer", true, vec![("min", serde_json::json!(0))]);
        assert!(
            check_supported_attributes(&field, "postgresql", RELATIONAL_ATTRIBUTES, "test.json")
                .is_ok()
        );
    }

    #[test]
    fn check_supported_attributes_rejects_invalid() {
        let field = make_field_with(
            "x",
            "integer",
            true,
            vec![("faker", serde_json::json!("name"))],
        );
        let err =
            check_supported_attributes(&field, "postgresql", RELATIONAL_ATTRIBUTES, "test.json");
        assert!(err.is_err());
    }
}
