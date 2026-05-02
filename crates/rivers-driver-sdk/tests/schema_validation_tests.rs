//! Schema validation chain tests (Feature 4.1–4.8).
//!
//! Tests the `SchemaSyntaxChecker`, `Validator`, and supporting types from
//! driver-schema-validation-spec §2–§5.

use std::collections::HashMap;
use rivers_driver_sdk::{
    HttpMethod, SchemaDefinition, SchemaFieldDef, SchemaSyntaxError, ValidationDirection,
    ValidationError,
};
use rivers_driver_sdk::validation::{
    check_supported_attributes, validate_fields, validate_field_type, RELATIONAL_ATTRIBUTES,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn field(name: &str, field_type: &str, required: bool) -> SchemaFieldDef {
    SchemaFieldDef {
        name: name.into(),
        field_type: field_type.into(),
        required,
        constraints: HashMap::new(),
    }
}

fn field_with(name: &str, field_type: &str, required: bool, constraints: HashMap<String, serde_json::Value>) -> SchemaFieldDef {
    SchemaFieldDef {
        name: name.into(),
        field_type: field_type.into(),
        required,
        constraints,
    }
}

fn schema(driver: &str, schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
    SchemaDefinition {
        driver: driver.into(),
        schema_type: schema_type.into(),
        description: String::new(),
        fields,
        extra: HashMap::new(),
    }
}

fn obj(pairs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v.clone());
    }
    serde_json::Value::Object(map)
}

// ── Feature 4.1 — Three-Stage Validation Chain types ─────────────────────────

#[test]
fn validation_direction_display_input() {
    assert_eq!(ValidationDirection::Input.to_string(), "Input");
}

#[test]
fn validation_direction_display_output() {
    assert_eq!(ValidationDirection::Output.to_string(), "Output");
}

#[test]
fn http_method_display_variants() {
    assert_eq!(HttpMethod::GET.to_string(), "GET");
    assert_eq!(HttpMethod::POST.to_string(), "POST");
    assert_eq!(HttpMethod::PUT.to_string(), "PUT");
    assert_eq!(HttpMethod::DELETE.to_string(), "DELETE");
}

#[test]
fn http_method_parse_case_insensitive() {
    assert_eq!(HttpMethod::from_str("get"), Some(HttpMethod::GET));
    assert_eq!(HttpMethod::from_str("POST"), Some(HttpMethod::POST));
    assert_eq!(HttpMethod::from_str("Put"), Some(HttpMethod::PUT));
    assert_eq!(HttpMethod::from_str("delete"), Some(HttpMethod::DELETE));
    assert_eq!(HttpMethod::from_str("PATCH"), None);
}

// ── Feature 4.2 — SchemaSyntaxError variants ────────────────────────────────

#[test]
fn schema_syntax_error_driver_mismatch_display() {
    let e = SchemaSyntaxError::DriverMismatch {
        expected: "postgresql".into(),
        actual: "redis".into(),
        schema_file: "contact.schema.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("postgresql"), "missing expected: {s}");
    assert!(s.contains("redis"), "missing actual: {s}");
}

#[test]
fn schema_syntax_error_missing_required_field_display() {
    let e = SchemaSyntaxError::MissingRequiredField {
        field: "faker".into(),
        driver: "faker".into(),
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("faker"), "{s}");
}

#[test]
fn schema_syntax_error_unsupported_attribute_display() {
    let e = SchemaSyntaxError::UnsupportedAttribute {
        attribute: "key_pattern".into(),
        field: "id".into(),
        driver: "postgresql".into(),
        supported: vec!["min".into(), "max".into()],
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("key_pattern"), "{s}");
    assert!(s.contains("id"), "{s}");
}

#[test]
fn schema_syntax_error_unsupported_type_display() {
    let e = SchemaSyntaxError::UnsupportedType {
        schema_type: "hash".into(),
        driver: "postgresql".into(),
        supported: vec!["object".into()],
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("hash"), "{s}");
    assert!(s.contains("postgresql"), "{s}");
}

#[test]
fn schema_syntax_error_unsupported_method_display() {
    let e = SchemaSyntaxError::UnsupportedMethod {
        method: "DELETE".into(),
        driver: "faker".into(),
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("DELETE"), "{s}");
    assert!(s.contains("faker"), "{s}");
}

#[test]
fn schema_syntax_error_orphan_variable_display() {
    let e = SchemaSyntaxError::OrphanVariable {
        variable: "user_id".into(),
        query: "SELECT * FROM users WHERE id = $user_id".into(),
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("user_id"), "{s}");
}

#[test]
fn schema_syntax_error_orphan_parameter_display() {
    let e = SchemaSyntaxError::OrphanParameter {
        parameter: "email".into(),
        query: "SELECT * FROM users".into(),
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("email"), "{s}");
}

#[test]
fn schema_syntax_error_structural_error_display() {
    let e = SchemaSyntaxError::StructuralError {
        message: "faker schemas require fields".into(),
        driver: "faker".into(),
        schema_file: "test.json".into(),
    };
    let s = e.to_string();
    assert!(s.contains("faker schemas require fields"), "{s}");
}

// ── Feature 4.2 — ValidationError variants ──────────────────────────────────

#[test]
fn validation_error_missing_required_display() {
    let e = ValidationError::MissingRequired {
        field: "email".into(),
        direction: ValidationDirection::Input,
    };
    let s = e.to_string();
    assert!(s.contains("email"), "{s}");
}

#[test]
fn validation_error_type_mismatch_display() {
    let e = ValidationError::TypeMismatch {
        field: "age".into(),
        expected: "integer".into(),
        actual: "string".into(),
        direction: ValidationDirection::Input,
    };
    let s = e.to_string();
    assert!(s.contains("age"), "{s}");
    assert!(s.contains("integer"), "{s}");
}

// ── Feature 4.3 — Per-method schema model: validate_fields direction ─────────

#[test]
fn validate_fields_input_direction_required_present() {
    let s = schema("postgresql", "object", vec![field("name", "string", true)]);
    let data = obj(&[("name", serde_json::json!("Alice"))]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_ok());
}

#[test]
fn validate_fields_input_direction_required_missing() {
    let s = schema("postgresql", "object", vec![field("name", "string", true)]);
    let data = obj(&[]);
    let err = validate_fields(&data, &s, ValidationDirection::Input).unwrap_err();
    match err {
        ValidationError::MissingRequired { ref field, direction } => {
            assert_eq!(field, "name");
            assert_eq!(direction, ValidationDirection::Input);
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn validate_fields_output_direction_required_missing_carries_output_direction() {
    let s = schema("postgresql", "object", vec![field("id", "uuid", true)]);
    let data = obj(&[]);
    let err = validate_fields(&data, &s, ValidationDirection::Output).unwrap_err();
    match err {
        ValidationError::MissingRequired { direction, .. } => {
            assert_eq!(direction, ValidationDirection::Output);
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn validate_fields_optional_field_absent_is_ok() {
    let s = schema("postgresql", "object", vec![field("bio", "string", false)]);
    let data = obj(&[]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_ok());
}

#[test]
fn validate_fields_empty_schema_empty_object_is_ok() {
    let s = schema("faker", "object", vec![]);
    let data = obj(&[]);
    assert!(validate_fields(&data, &s, ValidationDirection::Output).is_ok());
}

// ── Feature 4.6 — Rivers Primitive Types ─────────────────────────────────────

#[test]
fn primitive_type_string_accepts_string() {
    assert!(validate_field_type(&serde_json::json!("hello"), "string", "f", ValidationDirection::Input).is_ok());
}

#[test]
fn primitive_type_integer_accepts_integer() {
    assert!(validate_field_type(&serde_json::json!(42), "integer", "f", ValidationDirection::Input).is_ok());
}

#[test]
fn primitive_type_float_accepts_float() {
    assert!(validate_field_type(&serde_json::json!(3.14), "float", "f", ValidationDirection::Input).is_ok());
}

#[test]
fn primitive_type_boolean_accepts_bool() {
    assert!(validate_field_type(&serde_json::json!(true), "boolean", "f", ValidationDirection::Input).is_ok());
}

#[test]
fn primitive_type_uuid_accepts_valid_uuid() {
    assert!(validate_field_type(
        &serde_json::json!("550e8400-e29b-41d4-a716-446655440000"),
        "uuid", "id", ValidationDirection::Input
    ).is_ok());
}

#[test]
fn primitive_type_uuid_rejects_non_uuid_string() {
    assert!(validate_field_type(
        &serde_json::json!("not-a-uuid"),
        "uuid", "id", ValidationDirection::Input
    ).is_err());
}

#[test]
fn primitive_type_email_accepts_valid_email() {
    assert!(validate_field_type(
        &serde_json::json!("alice@example.com"),
        "email", "email", ValidationDirection::Input
    ).is_ok());
}

#[test]
fn primitive_type_email_rejects_non_email() {
    assert!(validate_field_type(
        &serde_json::json!("not-an-email"),
        "email", "email", ValidationDirection::Input
    ).is_err());
}

#[test]
fn primitive_type_json_accepts_any_value() {
    assert!(validate_field_type(&serde_json::json!({"any": "thing"}), "json", "f", ValidationDirection::Input).is_ok());
    assert!(validate_field_type(&serde_json::json!([1, 2, 3]), "json", "f", ValidationDirection::Input).is_ok());
    assert!(validate_field_type(&serde_json::json!(null), "json", "f", ValidationDirection::Input).is_ok());
}

#[test]
fn primitive_type_integer_rejects_string() {
    assert!(validate_field_type(
        &serde_json::json!("42"),
        "integer", "count", ValidationDirection::Input
    ).is_err());
}

// ── Feature 4.5 — Constraints (validate_fields integration) ──────────────────

#[test]
fn constraint_min_passes_when_value_at_min() {
    let mut c = HashMap::new();
    c.insert("min".to_string(), serde_json::json!(0));
    let s = schema("postgresql", "object", vec![field_with("count", "integer", true, c)]);
    let data = obj(&[("count", serde_json::json!(0))]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_ok());
}

#[test]
fn constraint_max_fails_when_value_exceeds_max() {
    let mut c = HashMap::new();
    c.insert("max".to_string(), serde_json::json!(100));
    let s = schema("postgresql", "object", vec![field_with("score", "integer", true, c)]);
    let data = obj(&[("score", serde_json::json!(101))]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_err());
}

#[test]
fn constraint_enum_passes_allowed_value() {
    let mut c = HashMap::new();
    c.insert("enum".to_string(), serde_json::json!(["admin", "user", "guest"]));
    let s = schema("postgresql", "object", vec![field_with("role", "string", true, c)]);
    let data = obj(&[("role", serde_json::json!("admin"))]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_ok());
}

#[test]
fn constraint_enum_rejects_unlisted_value() {
    let mut c = HashMap::new();
    c.insert("enum".to_string(), serde_json::json!(["admin", "user"]));
    let s = schema("postgresql", "object", vec![field_with("role", "string", true, c)]);
    let data = obj(&[("role", serde_json::json!("superuser"))]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_err());
}

#[test]
fn constraint_max_length_rejects_too_long_string() {
    let mut c = HashMap::new();
    c.insert("max_length".to_string(), serde_json::json!(5));
    let s = schema("postgresql", "object", vec![field_with("code", "string", true, c)]);
    let data = obj(&[("code", serde_json::json!("toolongstring"))]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_err());
}

// ── Feature 4.5 — check_supported_attributes ─────────────────────────────────

#[test]
fn check_supported_attributes_accepts_known_relational_attr() {
    let f = field_with("name", "string", false, {
        let mut c = HashMap::new();
        c.insert("min_length".to_string(), serde_json::json!(1));
        c
    });
    assert!(check_supported_attributes(&f, "postgresql", RELATIONAL_ATTRIBUTES, "test.json").is_ok());
}

#[test]
fn check_supported_attributes_rejects_unknown_attr() {
    let f = field_with("id", "string", false, {
        let mut c = HashMap::new();
        c.insert("faker".to_string(), serde_json::json!("name.firstName"));
        c
    });
    let err = check_supported_attributes(&f, "postgresql", RELATIONAL_ATTRIBUTES, "test.json");
    assert!(err.is_err(), "expected error for 'faker' attribute on postgresql field");
}

// ── Feature 4.1 — SchemaDefinition serde round-trip ──────────────────────────

#[test]
fn schema_definition_serializes_to_json() {
    let s = schema("postgresql", "object", vec![field("id", "uuid", true)]);
    let json = serde_json::to_string(&s).unwrap();
    let back: SchemaDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(back.driver, "postgresql");
    assert_eq!(back.schema_type, "object");
    assert_eq!(back.fields.len(), 1);
    assert_eq!(back.fields[0].name, "id");
    assert_eq!(back.fields[0].field_type, "uuid");
    assert!(back.fields[0].required);
}

#[test]
fn schema_definition_with_extra_fields_roundtrips() {
    let json = serde_json::json!({
        "driver": "redis",
        "type": "hash",
        "description": "user session hash",
        "fields": [],
        "key_pattern": "session:{id}"
    });
    let s: SchemaDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(s.driver, "redis");
    assert_eq!(s.schema_type, "hash");
    assert!(s.extra.contains_key("key_pattern"), "extra field should be captured");
}

// ── Feature 4.3 — non-object data rejects when fields are declared ────────────

#[test]
fn validate_fields_non_object_with_fields_fails() {
    let s = schema("postgresql", "object", vec![field("name", "string", false)]);
    let data = serde_json::json!([1, 2, 3]);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_err());
}

#[test]
fn validate_fields_non_object_with_no_fields_passes() {
    let s = schema("postgresql", "object", vec![]);
    let data = serde_json::json!(null);
    assert!(validate_fields(&data, &s, ValidationDirection::Input).is_ok());
}
