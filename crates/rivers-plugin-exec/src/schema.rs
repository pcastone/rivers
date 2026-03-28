//! JSON Schema validation for ExecDriver handler args (spec S9).
//!
//! Each command definition may reference a JSON Schema file that constrains the
//! `args` object callers supply at invocation time.  `CompiledSchema` loads the
//! schema once, compiles it into a reusable validator, and exposes a single
//! `validate` method that the 11-step pipeline calls before the integrity check.

use rivers_driver_sdk::DriverError;

/// A compiled JSON Schema for validating command args.
pub struct CompiledSchema {
    validator: jsonschema::Validator,
}

impl std::fmt::Debug for CompiledSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledSchema").finish_non_exhaustive()
    }
}

impl CompiledSchema {
    /// Load and compile a JSON Schema from a file path.
    pub fn load(path: &std::path::Path) -> Result<Self, DriverError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            DriverError::Connection(format!(
                "cannot read schema file {}: {e}",
                path.display()
            ))
        })?;
        let schema_value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            DriverError::Connection(format!(
                "invalid JSON in schema file {}: {e}",
                path.display()
            ))
        })?;
        let validator = jsonschema::validator_for(&schema_value).map_err(|e| {
            DriverError::Connection(format!(
                "invalid JSON Schema in {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self { validator })
    }

    /// Validate args against this schema.
    ///
    /// Collects all validation errors into a single `DriverError::Query` so
    /// the caller gets a complete diagnostic in one shot.
    pub fn validate(&self, args: &serde_json::Value) -> Result<(), DriverError> {
        let errors: Vec<String> = self
            .validator
            .iter_errors(args)
            .map(|e| e.to_string())
            .collect();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(DriverError::Query(format!(
                "schema validation failed: {}",
                errors.join("; ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper: write content to a temp file and return it.
    fn temp_schema_file(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("create temp file");
        f.write_all(content.as_bytes()).expect("write temp file");
        f.flush().expect("flush temp file");
        f
    }

    /// The spec S9.3 example schema.
    const SPEC_SCHEMA: &str = r#"{
        "type": "object",
        "required": ["cidr", "ports"],
        "additionalProperties": false,
        "properties": {
            "cidr": {
                "type": "string",
                "pattern": "^[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}/[0-9]{1,2}$"
            },
            "ports": {
                "type": "array",
                "items": { "type": "integer", "minimum": 1, "maximum": 65535 },
                "minItems": 1,
                "maxItems": 20
            }
        }
    }"#;

    fn load_spec_schema() -> CompiledSchema {
        let f = temp_schema_file(SPEC_SCHEMA);
        CompiledSchema::load(f.path()).expect("load spec schema")
    }

    // ── Happy path ────────────────────────────────────────────────────

    #[test]
    fn valid_args_pass() {
        let schema = load_spec_schema();
        let args = serde_json::json!({
            "cidr": "10.0.1.0/24",
            "ports": [22, 80]
        });
        assert!(schema.validate(&args).is_ok());
    }

    // ── Rejection cases ───────────────────────────────────────────────

    #[test]
    fn missing_required_field_fails() {
        let schema = load_spec_schema();
        let args = serde_json::json!({
            "cidr": "10.0.1.0/24"
        });
        let err = schema.validate(&args).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("schema validation failed"),
            "expected schema validation error, got: {msg}"
        );
    }

    #[test]
    fn invalid_cidr_pattern_fails() {
        let schema = load_spec_schema();
        let args = serde_json::json!({
            "cidr": "bad",
            "ports": [22]
        });
        let err = schema.validate(&args).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("schema validation failed"),
            "expected schema validation error, got: {msg}"
        );
    }

    #[test]
    fn port_out_of_range_fails() {
        let schema = load_spec_schema();
        let args = serde_json::json!({
            "cidr": "10.0.1.0/24",
            "ports": [99999]
        });
        let err = schema.validate(&args).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("schema validation failed"),
            "expected schema validation error, got: {msg}"
        );
    }

    #[test]
    fn extra_properties_fail() {
        let schema = load_spec_schema();
        let args = serde_json::json!({
            "cidr": "10.0.1.0/24",
            "ports": [22],
            "extra": true
        });
        let err = schema.validate(&args).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("schema validation failed"),
            "expected schema validation error, got: {msg}"
        );
    }

    // ── Load-time failures ────────────────────────────────────────────

    #[test]
    fn load_nonexistent_file_fails() {
        let result = CompiledSchema::load(std::path::Path::new("/tmp/does-not-exist-12345.json"));
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cannot read schema file"),
            "expected read error, got: {msg}"
        );
    }

    #[test]
    fn load_invalid_json_fails() {
        let f = temp_schema_file("{ not valid json }}}");
        let result = CompiledSchema::load(f.path());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid JSON in schema file"),
            "expected JSON parse error, got: {msg}"
        );
    }

    #[test]
    fn load_invalid_schema_fails() {
        // Valid JSON but not a valid JSON Schema (type is not a valid value).
        let f = temp_schema_file(r#"{"type": "not_a_real_type"}"#);
        let result = CompiledSchema::load(f.path());
        // Some validators accept any JSON as a schema and only fail at
        // validation time, so we test that either loading fails OR that
        // validating a value against this bogus schema produces an error.
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("invalid JSON Schema"),
                    "expected schema error, got: {msg}"
                );
            }
            Ok(schema) => {
                // The validator accepted it — verify it rejects a string instance
                // (since "not_a_real_type" is nonsensical, the validator should
                // either reject everything or have flagged the schema itself).
                let result = schema.validate(&serde_json::json!("hello"));
                assert!(
                    result.is_err(),
                    "expected validation to fail against bogus schema"
                );
            }
        }
    }
}
