use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::DriverError;
use crate::types::{Query, QueryResult};

// ---------------------------------------------------------------------------
// DriverType enum (technology-path-spec §8)
// ---------------------------------------------------------------------------

/// Driver category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverType {
    Database,
    MessageBroker,
    Http,
}

// ---------------------------------------------------------------------------
// HttpMethod & ValidationDirection enums (driver-schema-validation-spec §3)
// ---------------------------------------------------------------------------

/// HTTP method associated with a schema.
///
/// Per driver-schema-validation-spec §3.2: SchemaSyntaxChecker receives the
/// method to enforce method-specific rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    DELETE,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::GET => "GET",
            HttpMethod::POST => "POST",
            HttpMethod::PUT => "PUT",
            HttpMethod::DELETE => "DELETE",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "GET" => Some(HttpMethod::GET),
            "POST" => Some(HttpMethod::POST),
            "PUT" => Some(HttpMethod::PUT),
            "DELETE" => Some(HttpMethod::DELETE),
            _ => None,
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Direction of validation — input (from client) or output (from driver).
///
/// Per driver-schema-validation-spec §3.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationDirection {
    /// POST/PUT body → validate before execution
    Input,
    /// Query results → validate before response
    Output,
}

impl std::fmt::Display for ValidationDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationDirection::Input => f.write_str("Input"),
            ValidationDirection::Output => f.write_str("Output"),
        }
    }
}

// ---------------------------------------------------------------------------
// Schema-related error types (technology-path-spec §8, spec §18.1)
// ---------------------------------------------------------------------------

/// Error from schema syntax validation (build/deploy time).
#[derive(Debug, thiserror::Error)]
pub enum SchemaSyntaxError {
    #[error("schema driver mismatch: expected '{expected}', got '{actual}' in {schema_file}")]
    DriverMismatch {
        expected: String,
        actual: String,
        schema_file: String,
    },

    #[error("missing required field '{field}' for driver '{driver}' in {schema_file}")]
    MissingRequiredField {
        field: String,
        driver: String,
        schema_file: String,
    },

    #[error("unsupported attribute '{attribute}' on field '{field}' for driver '{driver}' in {schema_file}. Supported: {supported:?}")]
    UnsupportedAttribute {
        attribute: String,
        field: String,
        driver: String,
        supported: Vec<String>,
        schema_file: String,
    },

    #[error("unsupported schema type '{schema_type}' for driver '{driver}' in {schema_file}. Supported: {supported:?}")]
    UnsupportedType {
        schema_type: String,
        driver: String,
        supported: Vec<String>,
        schema_file: String,
    },

    #[error("method {method} not supported by driver '{driver}' in {schema_file}")]
    UnsupportedMethod {
        method: String,
        driver: String,
        schema_file: String,
    },

    #[error("invalid field type '{field_type}' on field '{field}' in {schema_file}")]
    InvalidFieldType {
        field: String,
        field_type: String,
        schema_file: String,
    },

    #[error("query variable '${variable}' has no matching parameter in {schema_file}")]
    OrphanVariable {
        variable: String,
        query: String,
        schema_file: String,
    },

    #[error("parameter '{parameter}' has no matching $variable in query in {schema_file}")]
    OrphanParameter {
        parameter: String,
        query: String,
        schema_file: String,
    },

    #[error("{message} (driver: {driver}, schema: {schema_file})")]
    StructuralError {
        message: String,
        driver: String,
        schema_file: String,
    },

    // Keep the old variants for backward compatibility during migration
    #[error("invalid schema structure for driver '{driver}': {reason}")]
    InvalidStructure { driver: String, reason: String },

    #[error("unknown field type '{field_type}' for driver '{driver}'")]
    UnknownFieldType { driver: String, field_type: String },

    #[error("missing required schema field '{field}' for driver '{driver}'")]
    MissingField { driver: String, field: String },
}

/// Error from data validation (request time).
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("required field '{field}' is missing (direction: {direction})")]
    MissingRequired {
        field: String,
        direction: ValidationDirection,
    },

    #[error("type mismatch on field '{field}': expected {expected}, got {actual} (direction: {direction})")]
    TypeMismatch {
        field: String,
        expected: String,
        actual: String,
        direction: ValidationDirection,
    },

    #[error("constraint violation on field '{field}': {constraint} — value {value}, limit {limit} (direction: {direction})")]
    ConstraintViolation {
        field: String,
        constraint: String,
        value: String,
        limit: String,
        direction: ValidationDirection,
    },

    #[error("coercion failed on field '{field}': cannot coerce {from_type} to {to_type} (direction: {direction})")]
    CoercionFailed {
        field: String,
        from_type: String,
        to_type: String,
        direction: ValidationDirection,
    },

    #[error("driver '{driver}' validation not implemented")]
    DriverNotImplemented { driver: String },

    // Keep old variants for backward compatibility during migration
    #[error("value out of range on field '{field}': {reason}")]
    OutOfRange { field: String, reason: String },

    #[error("pattern mismatch on field '{field}': value does not match '{pattern}'")]
    PatternMismatch { field: String, pattern: String },

    #[error("column count mismatch: schema defines {expected} fields, result has {actual}")]
    ColumnCountMismatch { expected: usize, actual: usize },
}

// ---------------------------------------------------------------------------
// Schema definition types (technology-path-spec §7.1)
// ---------------------------------------------------------------------------

/// A schema definition with driver type routing.
///
/// Per technology-path-spec §7.1: every schema includes a `driver` field.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchemaDefinition {
    /// Driver name that this schema targets (e.g., "postgresql", "redis", "kafka").
    pub driver: String,

    /// Schema type (e.g., "object", "hash", "message").
    #[serde(rename = "type")]
    pub schema_type: String,

    /// Human description.
    #[serde(default)]
    pub description: String,

    /// Fields for structured schemas.
    #[serde(default)]
    pub fields: Vec<SchemaFieldDef>,

    /// Additional driver-specific properties.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// A field definition within a schema.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchemaFieldDef {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(flatten)]
    pub constraints: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Connection parameters
// ---------------------------------------------------------------------------

/// Connection parameters resolved from datasource config.
///
/// `password` is always the resolved secret value — LockBox resolution
/// happens before `connect()` is called. Drivers never interact with LockBox.
#[derive(Debug, Clone)]
pub struct ConnectionParams {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub options: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Unified Driver trait (technology-path-spec §8.3)
// ---------------------------------------------------------------------------

/// Unified driver contract per technology-path-spec §8.3.
///
/// Every driver ships three capabilities:
/// - SchemaSyntaxChecker: "Is this schema well-formed for my driver?" (build/deploy time)
/// - Validator: "Does this data match this schema?" (request time)
/// - Executor: "Run this operation." (request time)
#[async_trait]
pub trait Driver: Send + Sync {
    /// The type category of this driver.
    fn driver_type(&self) -> DriverType;

    /// Unique name (e.g., "postgresql", "redis", "kafka").
    fn name(&self) -> &str;

    /// Build/deploy time — is this schema structurally valid for this driver?
    ///
    /// Per driver-schema-validation-spec §3.2: receives the HTTP method to
    /// enforce method-specific rules (e.g., GET schemas must not require a body).
    fn check_schema_syntax(
        &self,
        schema: &SchemaDefinition,
        method: HttpMethod,
    ) -> Result<(), SchemaSyntaxError>;

    /// Request time — does this data conform to this schema?
    ///
    /// Per driver-schema-validation-spec §3.3: receives the validation direction
    /// (Input for request bodies, Output for query results).
    fn validate(
        &self,
        data: &serde_json::Value,
        schema: &SchemaDefinition,
        direction: ValidationDirection,
    ) -> Result<(), ValidationError>;

    /// Request time — execute the operation.
    async fn execute(
        &self,
        query: &Query,
        params: &HashMap<String, crate::types::QueryValue>,
    ) -> Result<QueryResult, DriverError>;

    /// Connect to the underlying datasource.
    async fn connect(&mut self, config: &ConnectionParams) -> Result<(), DriverError>;

    /// Health check.
    async fn health_check(&self) -> Result<(), DriverError>;
}

// ---------------------------------------------------------------------------
// Existing operational traits (still valid, used by pool manager & drivers)
// ---------------------------------------------------------------------------
// Note: DatabaseDriver and Connection remain the operational traits.
// The unified Driver trait adds schema checking and validation capabilities.

/// A pool-owned connection to a datasource.
///
/// The `execute` method handles all operations — reads, writes, DDL.
/// The driver dispatches to the correct native call based on `query.operation`.
/// Connections are owned by the pool; when done, the pool reclaims them.
#[async_trait]
pub trait Connection: Send + Sync {
    /// Execute a query (read, write, DDL — all go through here).
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError>;

    /// Health check — returns Ok(()) or connection-level error.
    async fn ping(&mut self) -> Result<(), DriverError>;

    /// The name of the driver that created this connection.
    fn driver_name(&self) -> &str;
}

/// A named, stateless factory that creates `Connection` instances.
///
/// Drivers are registered at startup into `DriverFactory` by name.
/// Each datasource config references a driver by that name.
/// Drivers never manage connection lifecycle — they only construct
/// connections on demand.
#[async_trait]
pub trait DatabaseDriver: Send + Sync {
    /// Unique name for this driver (e.g. "postgres", "mysql", "faker").
    fn name(&self) -> &str;

    /// Create a new connection using the given parameters.
    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError>;

    /// Whether this driver supports BEGIN/COMMIT/ROLLBACK transactions.
    fn supports_transactions(&self) -> bool {
        false
    }

    /// Whether this driver supports prepared statements.
    fn supports_prepared_statements(&self) -> bool {
        false
    }
}
