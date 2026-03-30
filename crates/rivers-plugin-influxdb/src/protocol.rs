//! InfluxDB line protocol helpers and CSV response parsing.

use std::collections::HashMap;

use rivers_driver_sdk::{DriverError, Query, QueryValue};

/// Simple URL encoding for path/query segments.
pub(crate) fn urlencoded(s: &str) -> String {
    // Encode common problematic characters. For production use, a full
    // percent-encoding crate is preferred, but this covers typical org/bucket names.
    s.replace('%', "%25")
        .replace(' ', "%20")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('+', "%2B")
        .replace('#', "%23")
}

/// Parse InfluxDB annotated CSV response into rows.
///
/// InfluxDB CSV format:
/// - Lines starting with `#` are annotations (type info, group, default).
/// - Empty lines separate tables.
/// - First non-annotation line is the header row.
/// - Subsequent lines are data rows.
pub(crate) fn parse_csv_response(body: &str) -> Vec<HashMap<String, QueryValue>> {
    let mut rows = Vec::new();
    let mut headers: Vec<String> = Vec::new();

    for line in body.lines() {
        // Skip annotation lines.
        if line.starts_with('#') {
            continue;
        }

        // Empty lines separate tables — reset headers so the next
        // non-annotation line is treated as a new header row.
        if line.trim().is_empty() {
            headers.clear();
            continue;
        }

        let fields: Vec<&str> = line.split(',').collect();

        if headers.is_empty() {
            // First non-annotation line is the header.
            headers = fields.iter().map(|s| s.trim().to_string()).collect();
            continue;
        }

        // Data row.
        let mut row = HashMap::new();
        for (i, field) in fields.iter().enumerate() {
            if let Some(header) = headers.get(i) {
                if header.is_empty() {
                    continue;
                }
                let value = field.trim();
                let qv = if value.is_empty() {
                    QueryValue::Null
                } else if let Ok(i) = value.parse::<i64>() {
                    QueryValue::Integer(i)
                } else if let Ok(f) = value.parse::<f64>() {
                    QueryValue::Float(f)
                } else if value == "true" || value == "false" {
                    QueryValue::Boolean(value == "true")
                } else {
                    QueryValue::String(value.to_string())
                };
                row.insert(header.clone(), qv);
            }
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }

    rows
}

/// Build InfluxDB line protocol from query parameters.
///
/// Format: `measurement,tag1=val1,tag2=val2 field1=val1,field2=val2 [timestamp]`
pub(crate) fn build_line_protocol(query: &Query) -> Result<String, DriverError> {
    let params = &query.parameters;

    // If raw line protocol is provided, use it directly.
    if let Some(QueryValue::String(raw)) = params.get("_line_protocol") {
        return Ok(raw.clone());
    }

    // Measurement name from parameters or query target.
    let measurement = match params.get("measurement") {
        Some(QueryValue::String(m)) => m.clone(),
        _ => query.target.clone(),
    };

    // Tags (optional).
    let tag_set = match params.get("tags") {
        Some(QueryValue::Json(obj)) => {
            if let Some(map) = obj.as_object() {
                let pairs: Vec<String> = map
                    .iter()
                    .map(|(k, v)| {
                        let val = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        format!(
                            "{}={}",
                            escape_line_protocol_key(k),
                            escape_line_protocol_tag_value(&val)
                        )
                    })
                    .collect();
                if pairs.is_empty() {
                    String::new()
                } else {
                    format!(",{}", pairs.join(","))
                }
            } else {
                String::new()
            }
        }
        _ => String::new(),
    };

    // Fields (required for a valid write).
    let field_set = match params.get("fields") {
        Some(QueryValue::Json(obj)) => {
            if let Some(map) = obj.as_object() {
                let pairs: Vec<String> = map
                    .iter()
                    .map(|(k, v)| {
                        format!("{}={}", escape_line_protocol_key(k), format_field_value(v))
                    })
                    .collect();
                pairs.join(",")
            } else {
                return Err(DriverError::Query(
                    "influxdb: 'fields' parameter must be a JSON object".into(),
                ));
            }
        }
        _ => {
            // Fallback: use all non-reserved parameters as fields.
            let reserved = ["measurement", "tags", "timestamp", "_line_protocol"];
            let pairs: Vec<String> = params
                .iter()
                .filter(|(k, _)| !reserved.contains(&k.as_str()))
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        escape_line_protocol_key(k),
                        format_query_value_as_field(v)
                    )
                })
                .collect();
            if pairs.is_empty() {
                return Err(DriverError::Query(
                    "influxdb: write requires at least one field".into(),
                ));
            }
            pairs.join(",")
        }
    };

    // Timestamp (optional, nanoseconds).
    let timestamp = match params.get("timestamp") {
        Some(QueryValue::Integer(ts)) => format!(" {ts}"),
        Some(QueryValue::String(ts)) => format!(" {ts}"),
        _ => String::new(),
    };

    Ok(format!("{measurement}{tag_set} {field_set}{timestamp}"))
}

/// Escape measurement/tag key characters per line protocol spec.
pub(crate) fn escape_line_protocol_key(s: &str) -> String {
    s.replace(',', "\\,")
        .replace('=', "\\=")
        .replace(' ', "\\ ")
}

/// Escape tag value characters per line protocol spec.
pub(crate) fn escape_line_protocol_tag_value(s: &str) -> String {
    s.replace(',', "\\,")
        .replace('=', "\\=")
        .replace(' ', "\\ ")
}

/// Format a JSON value as an InfluxDB field value.
pub(crate) fn format_field_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            if n.is_i64() {
                format!("{}i", n.as_i64().unwrap())
            } else {
                n.to_string()
            }
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        other => format!("\"{}\"", other.to_string().replace('"', "\\\"")),
    }
}

/// Format a QueryValue as an InfluxDB field value.
pub(crate) fn format_query_value_as_field(v: &QueryValue) -> String {
    match v {
        QueryValue::Null => "\"\"".to_string(),
        QueryValue::Boolean(b) => b.to_string(),
        QueryValue::Integer(i) => format!("{i}i"),
        QueryValue::Float(f) => f.to_string(),
        QueryValue::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        QueryValue::Array(_) | QueryValue::Json(_) => {
            let json = serde_json::to_string(v).unwrap_or_default();
            format!("\"{}\"", json.replace('"', "\\\""))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_line_protocol tests ─────────────────────────────────────

    #[test]
    fn build_line_protocol_raw_passthrough() {
        let query = Query::with_operation("write", "mybucket", "").param(
            "_line_protocol",
            QueryValue::String("cpu,host=serverA value=0.64".into()),
        );
        let result = build_line_protocol(&query).unwrap();
        assert_eq!(result, "cpu,host=serverA value=0.64");
    }

    #[test]
    fn build_line_protocol_from_fields() {
        let query = Query::with_operation("write", "mybucket", "")
            .param("measurement", QueryValue::String("temperature".into()))
            .param(
                "fields",
                QueryValue::Json(serde_json::json!({"value": 23.5})),
            );
        let result = build_line_protocol(&query).unwrap();
        assert!(result.starts_with("temperature "));
        assert!(result.contains("value=23.5"));
    }

    #[test]
    fn build_line_protocol_with_tags() {
        let query = Query::with_operation("write", "mybucket", "")
            .param("measurement", QueryValue::String("cpu".into()))
            .param(
                "tags",
                QueryValue::Json(serde_json::json!({"host": "serverA"})),
            )
            .param(
                "fields",
                QueryValue::Json(serde_json::json!({"usage": 0.64})),
            );
        let result = build_line_protocol(&query).unwrap();
        assert!(result.starts_with("cpu,"));
        assert!(result.contains("host=serverA"));
        assert!(result.contains("usage=0.64"));
    }

    #[test]
    fn build_line_protocol_with_timestamp() {
        let query = Query::with_operation("write", "mybucket", "")
            .param("measurement", QueryValue::String("cpu".into()))
            .param(
                "fields",
                QueryValue::Json(serde_json::json!({"value": 1})),
            )
            .param("timestamp", QueryValue::Integer(1609459200000000000));
        let result = build_line_protocol(&query).unwrap();
        assert!(result.ends_with("1609459200000000000"));
    }

    #[test]
    fn build_line_protocol_fallback_to_target() {
        // No explicit measurement param — should use query.target
        let query =
            Query::with_operation("write", "my_measurement", "").param("value", QueryValue::Float(42.0));
        let result = build_line_protocol(&query).unwrap();
        assert!(result.starts_with("my_measurement "));
    }

    #[test]
    fn build_line_protocol_no_fields_returns_error() {
        let query = Query::with_operation("write", "mybucket", "");
        let result = build_line_protocol(&query);
        assert!(result.is_err());
    }

    // ── parse_csv_response tests ──────────────────────────────────────

    #[test]
    fn parse_csv_response_empty_returns_empty() {
        let rows = parse_csv_response("");
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_csv_response_annotations_only() {
        let csv = "#group,false,false\n#datatype,string,double\n#default,,\n";
        let rows = parse_csv_response(csv);
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_csv_response_header_only_no_data() {
        let csv = ",result,table,_time,_value\n";
        let rows = parse_csv_response(csv);
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_csv_response_with_data() {
        let csv = "\
#group,false,false,true,true
#datatype,string,long,dateTime:RFC3339,double

,result,table,_time,_value
,_result,0,2021-01-01T00:00:00Z,23.5
,_result,0,2021-01-01T01:00:00Z,24.1
";
        let rows = parse_csv_response(csv);
        assert_eq!(rows.len(), 2);

        // Check first row
        let row = &rows[0];
        assert_eq!(row.get("_value"), Some(&QueryValue::Float(23.5)));
        assert_eq!(
            row.get("result"),
            Some(&QueryValue::String("_result".into()))
        );
        assert_eq!(row.get("table"), Some(&QueryValue::Integer(0)));
    }

    #[test]
    fn parse_csv_response_booleans() {
        let csv = "active\ntrue\nfalse\n";
        let rows = parse_csv_response(csv);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("active"), Some(&QueryValue::Boolean(true)));
        assert_eq!(rows[1].get("active"), Some(&QueryValue::Boolean(false)));
    }

    // ── escape helpers tests ──────────────────────────────────────────

    #[test]
    fn escape_line_protocol_key_escapes_special_chars() {
        assert_eq!(escape_line_protocol_key("a,b"), "a\\,b");
        assert_eq!(escape_line_protocol_key("a=b"), "a\\=b");
        assert_eq!(escape_line_protocol_key("a b"), "a\\ b");
        assert_eq!(escape_line_protocol_key("normal"), "normal");
    }

    #[test]
    fn escape_line_protocol_tag_value_escapes_special_chars() {
        assert_eq!(escape_line_protocol_tag_value("a,b"), "a\\,b");
        assert_eq!(escape_line_protocol_tag_value("a=b"), "a\\=b");
        assert_eq!(escape_line_protocol_tag_value("a b"), "a\\ b");
    }

    // ── format_field_value tests ──────────────────────────────────────

    #[test]
    fn format_field_value_integer() {
        let val = serde_json::json!(42);
        assert_eq!(format_field_value(&val), "42i");
    }

    #[test]
    fn format_field_value_float() {
        let val = serde_json::json!(3.14);
        assert_eq!(format_field_value(&val), "3.14");
    }

    #[test]
    fn format_field_value_bool() {
        assert_eq!(format_field_value(&serde_json::json!(true)), "true");
        assert_eq!(format_field_value(&serde_json::json!(false)), "false");
    }

    #[test]
    fn format_field_value_string() {
        let val = serde_json::json!("hello");
        assert_eq!(format_field_value(&val), "\"hello\"");
    }

    #[test]
    fn format_field_value_string_with_quotes() {
        let val = serde_json::json!("say \"hi\"");
        assert_eq!(format_field_value(&val), "\"say \\\"hi\\\"\"");
    }

    // ── format_query_value_as_field tests ─────────────────────────────

    #[test]
    fn format_query_value_as_field_null() {
        assert_eq!(format_query_value_as_field(&QueryValue::Null), "\"\"");
    }

    #[test]
    fn format_query_value_as_field_boolean() {
        assert_eq!(
            format_query_value_as_field(&QueryValue::Boolean(true)),
            "true"
        );
    }

    #[test]
    fn format_query_value_as_field_integer() {
        assert_eq!(
            format_query_value_as_field(&QueryValue::Integer(42)),
            "42i"
        );
    }

    #[test]
    fn format_query_value_as_field_float() {
        assert_eq!(
            format_query_value_as_field(&QueryValue::Float(3.14)),
            "3.14"
        );
    }

    #[test]
    fn format_query_value_as_field_string() {
        assert_eq!(
            format_query_value_as_field(&QueryValue::String("hello".into())),
            "\"hello\""
        );
    }

    // ── urlencoded tests ──────────────────────────────────────────────

    #[test]
    fn urlencoded_encodes_special_chars() {
        assert_eq!(urlencoded("hello world"), "hello%20world");
        assert_eq!(urlencoded("a&b=c"), "a%26b%3Dc");
        assert_eq!(urlencoded("a+b"), "a%2Bb");
        assert_eq!(urlencoded("foo#bar"), "foo%23bar");
    }
}
