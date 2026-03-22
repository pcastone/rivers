//! Schema system tests — parsing, attribute validation, type validation, driver registry.

use rivers_runtime::schema::*;
use rivers_driver_sdk::types::QueryValue;

// ── Schema Parsing ────────────────────────────────────────────────

#[test]
fn parse_valid_schema() {
    let json = r#"{
        "type": "object",
        "description": "Contact schema",
        "fields": [
            { "name": "id", "type": "uuid", "required": true },
            { "name": "email", "type": "email", "required": true },
            { "name": "phone", "type": "phone", "required": false }
        ]
    }"#;

    let schema = parse_schema(json, "contact.schema.json").unwrap();
    assert_eq!(schema.schema_type, "object");
    assert_eq!(schema.description, "Contact schema");
    assert_eq!(schema.fields.len(), 3);
    assert_eq!(schema.fields[0].name, "id");
    assert_eq!(schema.fields[0].field_type, RiversType::Uuid);
    assert!(schema.fields[0].required);
    assert!(!schema.fields[2].required);
}

#[test]
fn parse_schema_with_faker_attributes() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "first_name", "type": "string", "faker": "name.firstName" },
            { "name": "email", "type": "email", "faker": "internet.email", "domain": "example.com" }
        ]
    }"#;

    let schema = parse_schema(json, "faker.schema.json").unwrap();
    assert_eq!(schema.fields[0].attributes.get("faker").unwrap(), "name.firstName");
    assert_eq!(schema.fields[1].attributes.get("domain").unwrap(), "example.com");
}

#[test]
fn parse_schema_invalid_json() {
    let result = parse_schema("not json", "bad.schema.json");
    assert!(result.is_err());
    match result.unwrap_err() {
        SchemaError::ParseError { path, .. } => assert_eq!(path, "bad.schema.json"),
        other => panic!("expected ParseError, got: {}", other),
    }
}

#[test]
fn parse_schema_missing_fields() {
    let json = r#"{ "type": "object" }"#;
    let result = parse_schema(json, "missing.schema.json");
    assert!(result.is_err());
}

#[test]
fn parse_schema_all_types() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "a", "type": "uuid" },
            { "name": "b", "type": "string" },
            { "name": "c", "type": "integer" },
            { "name": "d", "type": "float" },
            { "name": "e", "type": "boolean" },
            { "name": "f", "type": "email" },
            { "name": "g", "type": "phone" },
            { "name": "h", "type": "datetime" },
            { "name": "i", "type": "date" },
            { "name": "j", "type": "url" },
            { "name": "k", "type": "json" }
        ]
    }"#;

    let schema = parse_schema(json, "all-types.schema.json").unwrap();
    assert_eq!(schema.fields.len(), 11);
    assert_eq!(schema.fields[0].field_type, RiversType::Uuid);
    assert_eq!(schema.fields[1].field_type, RiversType::String);
    assert_eq!(schema.fields[2].field_type, RiversType::Integer);
    assert_eq!(schema.fields[3].field_type, RiversType::Float);
    assert_eq!(schema.fields[4].field_type, RiversType::Boolean);
    assert_eq!(schema.fields[5].field_type, RiversType::Email);
    assert_eq!(schema.fields[6].field_type, RiversType::Phone);
    assert_eq!(schema.fields[7].field_type, RiversType::Datetime);
    assert_eq!(schema.fields[8].field_type, RiversType::Date);
    assert_eq!(schema.fields[9].field_type, RiversType::Url);
    assert_eq!(schema.fields[10].field_type, RiversType::Json);
}

// ── Driver Attribute Registry ─────────────────────────────────────

#[test]
fn registry_defaults() {
    let reg = DriverAttributeRegistry::with_defaults();
    assert!(reg.has_driver("faker"));
    assert!(reg.has_driver("postgresql"));
    assert!(reg.has_driver("mysql"));
    assert!(reg.has_driver("ldap"));
    assert!(!reg.has_driver("unknown"));
}

#[test]
fn registry_faker_attributes() {
    let reg = DriverAttributeRegistry::with_defaults();
    let attrs = reg.supported_attributes("faker").unwrap();
    assert!(attrs.contains(&"faker".to_string()));
    assert!(attrs.contains(&"unique".to_string()));
    assert!(attrs.contains(&"domain".to_string()));
}

#[test]
fn registry_postgresql_attributes() {
    let reg = DriverAttributeRegistry::with_defaults();
    let attrs = reg.supported_attributes("postgresql").unwrap();
    assert!(attrs.contains(&"min".to_string()));
    assert!(attrs.contains(&"max".to_string()));
    assert!(attrs.contains(&"pattern".to_string()));
    assert!(attrs.contains(&"format".to_string()));
}

#[test]
fn registry_custom_driver() {
    let mut reg = DriverAttributeRegistry::new();
    reg.register("custom", &["special", "magic"]);
    let attrs = reg.supported_attributes("custom").unwrap();
    assert_eq!(attrs.len(), 2);
}

// ── Schema Attribute Validation ───────────────────────────────────

#[test]
fn validate_faker_schema_valid() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "first_name", "type": "string", "faker": "name.firstName" },
            { "name": "email", "type": "email", "faker": "internet.email", "unique": true }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();
    let reg = DriverAttributeRegistry::with_defaults();
    let errors = validate_schema_attributes(&schema, "faker", &reg);
    assert!(errors.is_empty(), "expected no errors: {:?}", errors);
}

#[test]
fn validate_faker_on_postgresql_rejected() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "email", "type": "email", "faker": "internet.email" }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();
    let reg = DriverAttributeRegistry::with_defaults();
    let errors = validate_schema_attributes(&schema, "postgresql", &reg);
    assert_eq!(errors.len(), 1);
    match &errors[0] {
        SchemaError::UnsupportedAttribute { attribute, driver, .. } => {
            assert_eq!(attribute, "faker");
            assert_eq!(driver, "postgresql");
        }
        other => panic!("expected UnsupportedAttribute, got: {}", other),
    }
}

#[test]
fn validate_no_attributes_always_valid() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "id", "type": "integer", "required": true }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();
    let reg = DriverAttributeRegistry::with_defaults();
    let errors = validate_schema_attributes(&schema, "postgresql", &reg);
    assert!(errors.is_empty());
}

// ── Faker Method Validation ───────────────────────────────────────

#[test]
fn validate_faker_methods_valid() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "a", "type": "string", "faker": "name.firstName" },
            { "name": "b", "type": "string", "faker": "internet.email" },
            { "name": "c", "type": "string", "faker": "location.city" }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();
    let errors = validate_faker_methods(&schema);
    assert!(errors.is_empty());
}

#[test]
fn validate_faker_methods_unknown() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "email", "type": "string", "faker": "internet.emailAddress" }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();
    let errors = validate_faker_methods(&schema);
    assert_eq!(errors.len(), 1);
    match &errors[0] {
        SchemaError::UnknownFakerMethod { method, field } => {
            assert_eq!(method, "internet.emailAddress");
            assert_eq!(field, "email");
        }
        other => panic!("expected UnknownFakerMethod, got: {}", other),
    }
}

#[test]
fn is_known_faker_method_checks() {
    assert!(is_known_faker_method("name.firstName"));
    assert!(is_known_faker_method("internet.email"));
    assert!(is_known_faker_method("location.city"));
    assert!(is_known_faker_method("datatype.uuid"));
    assert!(is_known_faker_method("date.past"));
    assert!(is_known_faker_method("image.avatar"));
    assert!(is_known_faker_method("lorem.sentence"));
    assert!(!is_known_faker_method("unknown.method"));
    assert!(!is_known_faker_method("name.unknown"));
    assert!(!is_known_faker_method("noperiod"));
}

// ── Rivers Type Validation ────────────────────────────────────────

#[test]
fn validate_string_type() {
    assert!(validate_value_type(&QueryValue::String("hello".into()), RiversType::String));
    assert!(!validate_value_type(&QueryValue::Integer(42), RiversType::String));
}

#[test]
fn validate_integer_type() {
    assert!(validate_value_type(&QueryValue::Integer(42), RiversType::Integer));
    assert!(!validate_value_type(&QueryValue::String("42".into()), RiversType::Integer));
}

#[test]
fn validate_float_type() {
    assert!(validate_value_type(&QueryValue::Float(3.14), RiversType::Float));
    assert!(!validate_value_type(&QueryValue::Integer(3), RiversType::Float));
}

#[test]
fn validate_boolean_type() {
    assert!(validate_value_type(&QueryValue::Boolean(true), RiversType::Boolean));
    assert!(!validate_value_type(&QueryValue::String("true".into()), RiversType::Boolean));
}

#[test]
fn validate_uuid_type() {
    assert!(validate_value_type(
        &QueryValue::String("550e8400-e29b-41d4-a716-446655440000".into()),
        RiversType::Uuid
    ));
    assert!(!validate_value_type(
        &QueryValue::String("not-a-uuid".into()),
        RiversType::Uuid
    ));
}

#[test]
fn validate_email_type() {
    assert!(validate_value_type(
        &QueryValue::String("user@example.com".into()),
        RiversType::Email
    ));
    assert!(!validate_value_type(
        &QueryValue::String("not-an-email".into()),
        RiversType::Email
    ));
}

#[test]
fn validate_phone_type() {
    assert!(validate_value_type(
        &QueryValue::String("+1-555-123-4567".into()),
        RiversType::Phone
    ));
    assert!(!validate_value_type(
        &QueryValue::String("abc".into()),
        RiversType::Phone
    ));
}

#[test]
fn validate_datetime_type() {
    assert!(validate_value_type(
        &QueryValue::String("2024-01-15T10:30:00Z".into()),
        RiversType::Datetime
    ));
    assert!(!validate_value_type(
        &QueryValue::String("2024-01-15".into()),
        RiversType::Datetime
    ));
}

#[test]
fn validate_date_type() {
    assert!(validate_value_type(
        &QueryValue::String("2024-01-15".into()),
        RiversType::Date
    ));
    assert!(!validate_value_type(
        &QueryValue::String("01/15/2024".into()),
        RiversType::Date
    ));
}

#[test]
fn validate_url_type() {
    assert!(validate_value_type(
        &QueryValue::String("https://example.com".into()),
        RiversType::Url
    ));
    assert!(!validate_value_type(
        &QueryValue::String("not-a-url".into()),
        RiversType::Url
    ));
}

#[test]
fn validate_json_type() {
    assert!(validate_value_type(
        &QueryValue::Json(serde_json::json!({"key": "value"})),
        RiversType::Json
    ));
    assert!(!validate_value_type(
        &QueryValue::String("{}".into()),
        RiversType::Json
    ));
}

// ── Format Validators ─────────────────────────────────────────────

#[test]
fn uuid_validation() {
    assert!(is_valid_uuid("550e8400-e29b-41d4-a716-446655440000"));
    assert!(!is_valid_uuid("not-a-uuid"));
    assert!(!is_valid_uuid("550e8400-e29b-41d4-a716")); // too short
    assert!(!is_valid_uuid("550e8400-e29b-41d4-a716-44665544000g")); // non-hex
}

#[test]
fn email_validation() {
    assert!(is_valid_email("user@example.com"));
    assert!(is_valid_email("a@b.c"));
    assert!(!is_valid_email("no-at-sign"));
    assert!(!is_valid_email("@no-local.com"));
    assert!(!is_valid_email("no-domain@"));
}

#[test]
fn phone_validation() {
    assert!(is_valid_phone("+1-555-123-4567"));
    assert!(is_valid_phone("5551234567"));
    assert!(!is_valid_phone("123")); // too few digits
    assert!(!is_valid_phone("abcdefg"));
}

#[test]
fn datetime_validation() {
    assert!(is_valid_datetime("2024-01-15T10:30:00Z"));
    assert!(is_valid_datetime("2024-01-15T10:30:00+05:30"));
    assert!(!is_valid_datetime("2024-01-15"));
    assert!(!is_valid_datetime("not a date"));
}

#[test]
fn date_validation() {
    assert!(is_valid_date("2024-01-15"));
    assert!(!is_valid_date("01/15/2024"));
    assert!(!is_valid_date("2024-1-5"));
}

#[test]
fn url_validation() {
    assert!(is_valid_url("https://example.com"));
    assert!(is_valid_url("http://localhost:3000"));
    assert!(!is_valid_url("ftp://files.example.com"));
    assert!(!is_valid_url("not-a-url"));
}

// ── Return Schema Validation ──────────────────────────────────────

#[test]
fn validate_query_result_valid() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "id", "type": "integer", "required": true },
            { "name": "name", "type": "string", "required": true }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();

    let rows = vec![
        [
            ("id".to_string(), QueryValue::Integer(1)),
            ("name".to_string(), QueryValue::String("Alice".into())),
        ]
        .into_iter()
        .collect(),
    ];

    let errors = validate_query_result(&rows, &schema);
    assert!(errors.is_empty(), "expected no errors: {:?}", errors);
}

#[test]
fn validate_query_result_missing_required() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "id", "type": "integer", "required": true },
            { "name": "name", "type": "string", "required": true }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();

    let rows = vec![[("id".to_string(), QueryValue::Integer(1))]
        .into_iter()
        .collect()];

    let errors = validate_query_result(&rows, &schema);
    assert_eq!(errors.len(), 1);
}

#[test]
fn validate_query_result_wrong_type() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "id", "type": "integer", "required": true }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();

    let rows = vec![[("id".to_string(), QueryValue::String("not-an-int".into()))]
        .into_iter()
        .collect()];

    let errors = validate_query_result(&rows, &schema);
    assert_eq!(errors.len(), 1);
}

#[test]
fn validate_query_result_optional_missing_ok() {
    let json = r#"{
        "type": "object",
        "fields": [
            { "name": "id", "type": "integer", "required": true },
            { "name": "nickname", "type": "string", "required": false }
        ]
    }"#;
    let schema = parse_schema(json, "test.json").unwrap();

    let rows = vec![[("id".to_string(), QueryValue::Integer(1))]
        .into_iter()
        .collect()];

    let errors = validate_query_result(&rows, &schema);
    assert!(errors.is_empty(), "optional missing field should not error");
}

// ── Error Display ─────────────────────────────────────────────────

#[test]
fn schema_error_display() {
    let err = SchemaError::UnsupportedAttribute {
        attribute: "faker".into(),
        driver: "postgresql".into(),
        supported: vec!["min".into(), "max".into()],
    };
    let msg = err.to_string();
    assert!(msg.contains("faker"));
    assert!(msg.contains("postgresql"));
}

#[test]
fn schema_error_unknown_faker() {
    let err = SchemaError::UnknownFakerMethod {
        method: "internet.emailAddress".into(),
        field: "email".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("internet.emailAddress"));
    assert!(msg.contains("email"));
}
