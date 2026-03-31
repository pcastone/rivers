//! Schema system — file-referenced JSON schemas with driver-aware attribute validation.
//!
//! Per `rivers-schema-spec-v2.md`.
//!
//! Schema files (`.schema.json`) define the shape of data flowing through
//! DataViews. Each field declares a Rivers primitive type and optional
//! driver-specific attributes (e.g. `faker` for synthetic data generation).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use rivers_driver_sdk::types::QueryValue;

// ── Rivers Primitive Types ────────────────────────────────────────

/// Rivers primitive types.
///
/// Per spec §2 — 11 types covering common data shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiversType {
    /// Universally unique identifier (8-4-4-4-12 hex).
    Uuid,
    /// UTF-8 text.
    String,
    /// Signed 64-bit integer.
    Integer,
    /// 64-bit floating point.
    Float,
    /// Boolean true/false.
    Boolean,
    /// Email address (local@domain).
    Email,
    /// Phone number (7+ digits).
    Phone,
    /// ISO 8601 date-time string.
    Datetime,
    /// ISO 8601 date (YYYY-MM-DD).
    Date,
    /// HTTP/HTTPS URL.
    Url,
    /// Arbitrary JSON value.
    Json,
}

// ── Schema File ───────────────────────────────────────────────────

/// A parsed schema file.
///
/// Per spec §2 — JSON files in `schemas/` with a `fields` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaFile {
    /// Always "object".
    #[serde(rename = "type")]
    pub schema_type: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: String,

    /// Field definitions.
    pub fields: Vec<SchemaField>,
}

/// A single field in a schema file.
///
/// Base attributes: name, type, required.
/// Driver-specific attributes are stored in `attributes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaField {
    /// Field name (must match result row keys).
    pub name: String,

    /// Rivers primitive type.
    #[serde(rename = "type")]
    pub field_type: RiversType,

    /// Whether this field must be present in every result row.
    #[serde(default)]
    pub required: bool,

    /// Driver-specific attributes (e.g. "faker", "min", "max", "pattern").
    /// Collected from all extra keys in the JSON beyond name/type/required.
    #[serde(flatten)]
    pub attributes: HashMap<String, serde_json::Value>,
}

// ── Schema Loader ─────────────────────────────────────────────────

/// Errors from schema operations.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    /// Schema file does not exist at the configured path.
    #[error("schema file not found: {path}")]
    FileNotFound {
        /// Filesystem path.
        path: String,
    },

    /// Schema file could not be parsed as JSON.
    #[error("schema file parse error in '{path}': {reason}")]
    ParseError {
        /// Filesystem path.
        path: String,
        /// Parse error details.
        reason: String,
    },

    /// Schema attribute not supported by the target driver.
    #[error("schema attribute '{attribute}' is not supported by driver '{driver}'. Supported attributes: {supported:?}")]
    UnsupportedAttribute {
        /// Attribute name (e.g. "faker").
        attribute: String,
        /// Driver name (e.g. "postgresql").
        driver: String,
        /// List of supported attributes for this driver.
        supported: Vec<String>,
    },

    /// Faker method string not recognized.
    #[error("unknown faker method '{method}' on field '{field}'")]
    UnknownFakerMethod {
        /// Faker method string (e.g. "name.invalid").
        method: String,
        /// Field name in the schema.
        field: String,
    },

    /// Result row value does not match the declared field type.
    #[error("type validation failed for field '{field}': expected {expected}, got {actual}")]
    TypeValidation {
        /// Field path (e.g. `row[0].email`).
        field: String,
        /// Expected type description.
        expected: String,
        /// Actual value description.
        actual: String,
    },
}

/// Parse a schema file from a JSON string.
pub fn parse_schema(content: &str, path: &str) -> Result<SchemaFile, SchemaError> {
    serde_json::from_str(content).map_err(|e| SchemaError::ParseError {
        path: path.to_string(),
        reason: e.to_string(),
    })
}

/// Parse a schema file from a JSON value.
pub fn parse_schema_value(value: &serde_json::Value, path: &str) -> Result<SchemaFile, SchemaError> {
    serde_json::from_value(value.clone()).map_err(|e| SchemaError::ParseError {
        path: path.to_string(),
        reason: e.to_string(),
    })
}

// ── Driver Attribute Registry ─────────────────────────────────────

/// Registry of driver-supported schema attributes.
///
/// Per spec §3 — each driver declares which attributes it supports.
/// Using an unsupported attribute is a validation error.
pub struct DriverAttributeRegistry {
    /// Map of driver name → set of supported attribute names.
    drivers: HashMap<String, Vec<String>>,
}

impl DriverAttributeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            drivers: HashMap::new(),
        }
    }

    /// Create a registry with the default driver entries per spec §3.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register("faker", &["faker", "unique", "domain"]);
        reg.register("postgresql", &["min", "max", "pattern", "format"]);
        reg.register("mysql", &["min", "max", "pattern", "format"]);
        reg.register("ldap", &["pattern"]);
        reg
    }

    /// Register supported attributes for a driver.
    pub fn register(&mut self, driver: &str, attributes: &[&str]) {
        self.drivers.insert(
            driver.to_string(),
            attributes.iter().map(|a| a.to_string()).collect(),
        );
    }

    /// Get supported attributes for a driver.
    pub fn supported_attributes(&self, driver: &str) -> Option<&[String]> {
        self.drivers.get(driver).map(|v| v.as_slice())
    }

    /// Check if a driver is registered.
    pub fn has_driver(&self, driver: &str) -> bool {
        self.drivers.contains_key(driver)
    }
}

impl Default for DriverAttributeRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// ── Schema Attribute Validation ───────────────────────────────────

/// Validate that all attributes in a schema are supported by the target driver.
///
/// Per spec §7 — validation chain stage 1 and 2.
/// Returns a list of errors (empty = valid).
pub fn validate_schema_attributes(
    schema: &SchemaFile,
    driver: &str,
    registry: &DriverAttributeRegistry,
) -> Vec<SchemaError> {
    let mut errors = Vec::new();

    let supported = match registry.supported_attributes(driver) {
        Some(attrs) => attrs,
        None => {
            // Driver not in registry — no driver-specific attributes allowed
            // but we still check if any attributes are present
            &[]
        }
    };

    let supported_set: Vec<&str> = supported.iter().map(|s| s.as_str()).collect();

    for field in &schema.fields {
        for attr_name in field.attributes.keys() {
            if !supported_set.contains(&attr_name.as_str()) {
                errors.push(SchemaError::UnsupportedAttribute {
                    attribute: attr_name.clone(),
                    driver: driver.to_string(),
                    supported: supported.to_vec(),
                });
            }
        }
    }

    errors
}

// ── Faker Method Validation ───────────────────────────────────────

/// Known faker categories and their methods.
///
/// Per spec §3.
const KNOWN_FAKER_CATEGORIES: &[(&str, &[&str])] = &[
    ("name", &["firstName", "lastName", "fullName", "prefix", "suffix"]),
    ("internet", &["email", "url", "username", "ipv4", "domainName"]),
    ("phone", &["number"]),
    ("location", &["streetAddress", "city", "state", "zipCode", "country", "latitude", "longitude"]),
    ("company", &["name", "catchPhrase", "bs"]),
    ("datatype", &["uuid", "number", "float", "boolean"]),
    ("date", &["past", "future", "recent", "between"]),
    ("image", &["avatar", "url"]),
    ("lorem", &["word", "words", "sentence", "sentences", "paragraph"]),
];

/// Validate that all faker attributes in a schema use known methods.
pub fn validate_faker_methods(schema: &SchemaFile) -> Vec<SchemaError> {
    let mut errors = Vec::new();

    for field in &schema.fields {
        if let Some(faker_val) = field.attributes.get("faker") {
            if let Some(method_str) = faker_val.as_str() {
                if !is_known_faker_method(method_str) {
                    errors.push(SchemaError::UnknownFakerMethod {
                        method: method_str.to_string(),
                        field: field.name.clone(),
                    });
                }
            }
        }
    }

    errors
}

/// Check if a faker method string is known (e.g. "name.firstName").
pub fn is_known_faker_method(method: &str) -> bool {
    let parts: Vec<&str> = method.splitn(2, '.').collect();
    if parts.len() != 2 {
        return false;
    }
    let (category, method_name) = (parts[0], parts[1]);

    for (cat, methods) in KNOWN_FAKER_CATEGORIES {
        if *cat == category {
            return methods.contains(&method_name);
        }
    }
    false
}

// ── Rivers Type Validation ────────────────────────────────────────

/// Validate a QueryValue against a Rivers primitive type.
///
/// Per spec §2 — type checking for return_schema validation.
pub fn validate_value_type(value: &QueryValue, expected: RiversType) -> bool {
    match (expected, value) {
        (RiversType::String, QueryValue::String(_)) => true,
        (RiversType::Integer, QueryValue::Integer(_)) => true,
        (RiversType::Float, QueryValue::Float(_)) => true,
        (RiversType::Boolean, QueryValue::Boolean(_)) => true,
        (RiversType::Json, QueryValue::Json(_)) => true,

        // UUID — must be a string that looks like a UUID
        (RiversType::Uuid, QueryValue::String(s)) => is_valid_uuid(s),

        // Email — must be a string with @ and domain
        (RiversType::Email, QueryValue::String(s)) => is_valid_email(s),

        // Phone — must be a string with digits
        (RiversType::Phone, QueryValue::String(s)) => is_valid_phone(s),

        // Datetime — ISO 8601 datetime string
        (RiversType::Datetime, QueryValue::String(s)) => is_valid_datetime(s),

        // Date — ISO 8601 date string
        (RiversType::Date, QueryValue::String(s)) => is_valid_date(s),

        // URL — must be a string that looks like a URL
        (RiversType::Url, QueryValue::String(s)) => is_valid_url(s),

        _ => false,
    }
}

/// Validate uuid format: 8-4-4-4-12 hex pattern.
pub fn is_valid_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    parts
        .iter()
        .zip(expected_lens.iter())
        .all(|(part, len)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Validate email format: contains @ with non-empty local and domain parts.
pub fn is_valid_email(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(2, '@').collect();
    parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.')
}

/// Validate phone format: contains at least some digits.
pub fn is_valid_phone(s: &str) -> bool {
    let digits: usize = s.chars().filter(|c| c.is_ascii_digit()).count();
    digits >= 7
}

/// Validate ISO 8601 datetime format (basic check).
pub fn is_valid_datetime(s: &str) -> bool {
    // Accept formats like "2024-01-15T10:30:00Z" or "2024-01-15T10:30:00+00:00"
    s.len() >= 19 && s.contains('T') && {
        let date_part = &s[..10];
        date_part.len() == 10 && date_part.chars().nth(4) == Some('-') && date_part.chars().nth(7) == Some('-')
    }
}

/// Validate ISO 8601 date format (YYYY-MM-DD).
pub fn is_valid_date(s: &str) -> bool {
    s.len() == 10
        && s.chars().nth(4) == Some('-')
        && s.chars().nth(7) == Some('-')
        && s[..4].chars().all(|c| c.is_ascii_digit())
        && s[5..7].chars().all(|c| c.is_ascii_digit())
        && s[8..10].chars().all(|c| c.is_ascii_digit())
}

/// Validate URL format (basic check: scheme + authority).
pub fn is_valid_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://")) && s.len() > 8
}

// ── Return Schema Validation ──────────────────────────────────────

/// Validate a set of result rows against a return schema.
///
/// Per spec §5 — checks that required fields are present and all
/// values match their declared types.
pub fn validate_query_result(
    rows: &[HashMap<String, QueryValue>],
    schema: &SchemaFile,
) -> Vec<SchemaError> {
    let mut errors = Vec::new();

    for (row_idx, row) in rows.iter().enumerate() {
        for field in &schema.fields {
            match row.get(&field.name) {
                Some(value) => {
                    if !validate_value_type(value, field.field_type) {
                        errors.push(SchemaError::TypeValidation {
                            field: format!("row[{}].{}", row_idx, field.name),
                            expected: format!("{:?}", field.field_type),
                            actual: format!("{:?}", value),
                        });
                    }
                }
                None => {
                    if field.required {
                        errors.push(SchemaError::TypeValidation {
                            field: format!("row[{}].{}", row_idx, field.name),
                            expected: format!("{:?} (required)", field.field_type),
                            actual: "missing".to_string(),
                        });
                    }
                }
            }
        }
    }

    errors
}
