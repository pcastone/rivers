//! Argument template interpolation for `args` and `both` input modes.
//!
//! Per spec section 8.2, each element in an argument template is either a literal
//! string (passed through verbatim) or a placeholder `{key}` that is resolved
//! against the query parameters. Each placeholder produces exactly one argument —
//! no shell splitting occurs.

use rivers_driver_sdk::DriverError;

/// Interpolate argument template placeholders with parameter values.
///
/// Per spec section 8.2:
/// - `{key}` replaced with string value of corresponding param
/// - Missing key produces `DriverError::Query`
/// - Array/object values produce `DriverError::Query`
/// - Numbers render as decimal strings, booleans as `"true"`/`"false"`
/// - Each placeholder produces exactly one argument (no splitting)
/// - Extra keys in params are silently ignored
/// - Literal strings (no placeholder) pass through verbatim
pub fn interpolate(
    template: &[String],
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<String>, DriverError> {
    template.iter().map(|element| resolve(element, params)).collect()
}

/// Resolve a single template element against the parameter map.
fn resolve(
    element: &str,
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, DriverError> {
    // A placeholder is detected by checking if the string starts with '{' and ends with '}'.
    if element.starts_with('{') && element.ends_with('}') && element.len() > 2 {
        let key = &element[1..element.len() - 1];

        let value = params.get(key).ok_or_else(|| {
            DriverError::Query(format!("missing required parameter: '{key}'"))
        })?;

        match value {
            serde_json::Value::String(s) => Ok(s.clone()),
            serde_json::Value::Number(n) => Ok(n.to_string()),
            serde_json::Value::Bool(b) => Ok(b.to_string()),
            serde_json::Value::Null => Ok("null".to_string()),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                Err(DriverError::Query(format!(
                    "parameter '{key}' must be a scalar value for args template"
                )))
            }
        }
    } else {
        // Literal — pass through unchanged.
        Ok(element.to_string())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: build a `serde_json::Map` from a `serde_json::Value::Object`.
    fn map(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match value {
            serde_json::Value::Object(m) => m,
            _ => panic!("expected JSON object"),
        }
    }

    #[test]
    fn basic_interpolation() {
        let template: Vec<String> = vec![
            "{domain}".into(),
            "--type".into(),
            "{record_type}".into(),
        ];
        let params = map(json!({"domain": "example.com", "record_type": "A"}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["example.com", "--type", "A"]);
    }

    #[test]
    fn missing_key_error() {
        let template: Vec<String> = vec!["{missing}".into()];
        let params = map(json!({}));
        let err = interpolate(&template, &params).unwrap_err();
        assert!(err.to_string().contains("missing required parameter: 'missing'"));
    }

    #[test]
    fn number_value() {
        let template: Vec<String> = vec!["{timeout}".into()];
        let params = map(json!({"timeout": 30}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["30"]);
    }

    #[test]
    fn boolean_value() {
        let template: Vec<String> = vec!["{verbose}".into()];
        let params = map(json!({"verbose": true}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["true"]);
    }

    #[test]
    fn boolean_false_value() {
        let template: Vec<String> = vec!["{verbose}".into()];
        let params = map(json!({"verbose": false}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["false"]);
    }

    #[test]
    fn null_value() {
        let template: Vec<String> = vec!["{value}".into()];
        let params = map(json!({"value": null}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["null"]);
    }

    #[test]
    fn array_value_error() {
        let template: Vec<String> = vec!["{items}".into()];
        let params = map(json!({"items": [1, 2]}));
        let err = interpolate(&template, &params).unwrap_err();
        assert!(err.to_string().contains("parameter 'items' must be a scalar value"));
    }

    #[test]
    fn object_value_error() {
        let template: Vec<String> = vec!["{nested}".into()];
        let params = map(json!({"nested": {"a": 1}}));
        let err = interpolate(&template, &params).unwrap_err();
        assert!(err.to_string().contains("parameter 'nested' must be a scalar value"));
    }

    #[test]
    fn extra_keys_ignored() {
        let template: Vec<String> = vec!["{used}".into()];
        let params = map(json!({"used": "yes", "extra1": "ignored", "extra2": 42}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["yes"]);
    }

    #[test]
    fn special_characters_pass_through() {
        let template: Vec<String> = vec!["{host}".into()];
        let params = map(json!({"host": "foo;rm -rf /"}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["foo;rm -rf /"]);
    }

    #[test]
    fn empty_template() {
        let template: Vec<String> = vec![];
        let params = map(json!({"anything": "here"}));
        let result = interpolate(&template, &params).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn literal_only_template() {
        let template: Vec<String> = vec!["--help".into()];
        let params = map(json!({}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["--help"]);
    }

    #[test]
    fn mixed_literals_and_placeholders() {
        let template: Vec<String> = vec![
            "--flag".into(),
            "{value}".into(),
            "--verbose".into(),
        ];
        let params = map(json!({"value": "test"}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["--flag", "test", "--verbose"]);
    }

    #[test]
    fn float_number_value() {
        let template: Vec<String> = vec!["{rate}".into()];
        let params = map(json!({"rate": 3.14}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["3.14"]);
    }

    #[test]
    fn bare_braces_not_placeholder() {
        // "{}" is length 2, which means len() > 2 is false — treated as literal.
        let template: Vec<String> = vec!["{}".into()];
        let params = map(json!({}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["{}"]);
    }

    #[test]
    fn partial_brace_not_placeholder() {
        // Strings that have a brace only on one side are literals.
        let template: Vec<String> = vec!["{start".into(), "end}".into()];
        let params = map(json!({}));
        let result = interpolate(&template, &params).unwrap();
        assert_eq!(result, vec!["{start", "end}"]);
    }
}
