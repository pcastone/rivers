//! HTTP driver schema validation.
//!
//! Per driver-schema-validation-spec §12.

use crate::traits::{HttpMethod, SchemaDefinition, SchemaSyntaxError, ValidationDirection, ValidationError};

/// Validate an HTTP driver schema.
pub fn check_http_schema_syntax(
    schema: &SchemaDefinition,
    method: HttpMethod,
) -> Result<(), SchemaSyntaxError> {
    // type must be "object" or "stream_chunk"
    match schema.schema_type.as_str() {
        "object" | "stream_chunk" => {}
        other => {
            return Err(SchemaSyntaxError::UnsupportedType {
                schema_type: other.into(),
                driver: "http".into(),
                supported: vec!["object".into(), "stream_chunk".into()],
                schema_file: String::new(),
            });
        }
    }

    // content_type validation if present
    if let Some(ct) = schema.extra.get("content_type").and_then(|v| v.as_str()) {
        let valid_ct = [
            "application/json",
            "application/x-ndjson",
            "text/event-stream",
            "application/xml",
            "text/plain",
        ];
        if !valid_ct.contains(&ct) {
            return Err(SchemaSyntaxError::StructuralError {
                message: format!(
                    "unknown content_type '{}' — expected application/json, application/x-ndjson, text/event-stream, application/xml, text/plain",
                    ct
                ),
                driver: "http".into(),
                schema_file: String::new(),
            });
        }
    }

    // object type requires fields for POST/PUT
    if schema.schema_type == "object"
        && (method == HttpMethod::POST || method == HttpMethod::PUT)
        && schema.fields.is_empty()
    {
        return Err(SchemaSyntaxError::StructuralError {
            message: "HTTP object schemas require fields for POST/PUT".into(),
            driver: "http".into(),
            schema_file: String::new(),
        });
    }

    // Validate field attributes
    for field in &schema.fields {
        crate::validation::check_supported_attributes(
            field,
            "http",
            crate::validation::RELATIONAL_ATTRIBUTES,
            "",
        )?;
    }

    Ok(())
}

/// Validate data against an HTTP schema.
pub fn validate_http_data(
    data: &serde_json::Value,
    schema: &SchemaDefinition,
    direction: ValidationDirection,
) -> Result<(), ValidationError> {
    crate::validation::validate_fields(data, schema, direction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::SchemaFieldDef;
    use std::collections::HashMap;

    fn make_http_schema(schema_type: &str, fields: Vec<SchemaFieldDef>) -> SchemaDefinition {
        SchemaDefinition {
            driver: "http".into(),
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

    #[test]
    fn http_object_type_valid() {
        let schema = make_http_schema("object", vec![]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn http_stream_chunk_type_valid() {
        let schema = make_http_schema("stream_chunk", vec![]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn http_invalid_type_rejected() {
        let schema = make_http_schema("hash", vec![]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::GET).is_err());
    }

    #[test]
    fn http_post_requires_fields() {
        let schema = make_http_schema("object", vec![]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::POST).is_err());
    }

    #[test]
    fn http_put_requires_fields() {
        let schema = make_http_schema("object", vec![]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::PUT).is_err());
    }

    #[test]
    fn http_post_with_fields_valid() {
        let schema = make_http_schema("object", vec![make_field("name", "string", true)]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::POST).is_ok());
    }

    #[test]
    fn http_get_no_fields_valid() {
        let schema = make_http_schema("object", vec![]);
        assert!(check_http_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn http_invalid_content_type_rejected() {
        let mut schema = make_http_schema("object", vec![]);
        schema
            .extra
            .insert("content_type".into(), serde_json::json!("text/csv"));
        assert!(check_http_schema_syntax(&schema, HttpMethod::GET).is_err());
    }

    #[test]
    fn http_valid_content_type_accepted() {
        let mut schema = make_http_schema("object", vec![]);
        schema
            .extra
            .insert("content_type".into(), serde_json::json!("application/json"));
        assert!(check_http_schema_syntax(&schema, HttpMethod::GET).is_ok());
    }

    #[test]
    fn http_validate_data_passes_valid() {
        let schema = make_http_schema("object", vec![make_field("name", "string", true)]);
        let data = serde_json::json!({"name": "alice"});
        assert!(validate_http_data(&data, &schema, ValidationDirection::Input).is_ok());
    }

    #[test]
    fn http_validate_data_rejects_missing_required() {
        let schema = make_http_schema("object", vec![make_field("name", "string", true)]);
        let data = serde_json::json!({});
        assert!(validate_http_data(&data, &schema, ValidationDirection::Input).is_err());
    }
}
