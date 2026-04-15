use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::DriverError;
use crate::types::{Query, QueryResult};

// ---------------------------------------------------------------------------
// DriverType enum (technology-path-spec §8)
// ---------------------------------------------------------------------------

/// How a driver binds parameters in query text.
///
/// The DataView engine rewrites `$name` placeholders from TOML config
/// into the driver's native format before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamStyle {
    /// No placeholders in query text (Redis, MongoDB, Faker, etc.).
    None,
    /// Positional `$1`, `$2`, `$3` — parameters ordered by appearance in query (PostgreSQL).
    DollarPositional,
    /// Positional `?`, `?`, `?` — parameters ordered by appearance in query (MySQL).
    QuestionPositional,
    /// Named `$name` — pass-through, already matches spec convention (SQLite default).
    DollarNamed,
    /// Named `:name` — dollar prefix rewritten to colon (Cassandra CQL).
    ColonNamed,
}

/// Driver category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverType {
    /// Request/response drivers (PostgreSQL, MySQL, SQLite, Redis, Faker, etc.).
    Database,
    /// Continuous-push drivers (Kafka, RabbitMQ, NATS, Redis Streams).
    MessageBroker,
    /// HTTP/HTTP2/SSE/WebSocket as a datasource.
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
    /// HTTP GET.
    GET,
    /// HTTP POST.
    POST,
    /// HTTP PUT.
    PUT,
    /// HTTP DELETE.
    DELETE,
}

impl HttpMethod {
    /// Return the method as a static string slice.
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::GET => "GET",
            HttpMethod::POST => "POST",
            HttpMethod::PUT => "PUT",
            HttpMethod::DELETE => "DELETE",
        }
    }

    /// Parse a string into an `HttpMethod`. Case-insensitive.
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
    /// Schema declares a different driver than the datasource provides.
    #[error("schema driver mismatch: expected '{expected}', got '{actual}' in {schema_file}")]
    DriverMismatch {
        /// Driver name the schema was written for.
        expected: String,
        /// Driver name the datasource actually uses.
        actual: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// A field required by the driver is not declared in the schema.
    #[error("missing required field '{field}' for driver '{driver}' in {schema_file}")]
    MissingRequiredField {
        /// The missing field name.
        field: String,
        /// Driver that requires the field.
        driver: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// A field attribute is not recognized by the target driver.
    #[error("unsupported attribute '{attribute}' on field '{field}' for driver '{driver}' in {schema_file}. Supported: {supported:?}")]
    UnsupportedAttribute {
        /// The unrecognized attribute name.
        attribute: String,
        /// Field the attribute was declared on.
        field: String,
        /// Driver that rejected the attribute.
        driver: String,
        /// Attributes this driver accepts.
        supported: Vec<String>,
        /// Path to the schema file.
        schema_file: String,
    },

    /// Schema type is not supported by the driver.
    #[error("unsupported schema type '{schema_type}' for driver '{driver}' in {schema_file}. Supported: {supported:?}")]
    UnsupportedType {
        /// The rejected schema type.
        schema_type: String,
        /// Driver that rejected the type.
        driver: String,
        /// Types this driver accepts.
        supported: Vec<String>,
        /// Path to the schema file.
        schema_file: String,
    },

    /// HTTP method is not supported by the driver for this schema.
    #[error("method {method} not supported by driver '{driver}' in {schema_file}")]
    UnsupportedMethod {
        /// The rejected HTTP method.
        method: String,
        /// Driver that rejected the method.
        driver: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// A field declares a type the driver does not recognize.
    #[error("invalid field type '{field_type}' on field '{field}' in {schema_file}")]
    InvalidFieldType {
        /// Field with the invalid type.
        field: String,
        /// The unrecognized type string.
        field_type: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// A `$variable` in the query has no corresponding parameter declaration.
    #[error("query variable '${variable}' has no matching parameter in {schema_file}")]
    OrphanVariable {
        /// The unmatched variable name.
        variable: String,
        /// The query containing the orphan.
        query: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// A declared parameter has no corresponding `$variable` in the query.
    #[error("parameter '{parameter}' has no matching $variable in query in {schema_file}")]
    OrphanParameter {
        /// The unmatched parameter name.
        parameter: String,
        /// The query that should reference it.
        query: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// General structural issue with the schema.
    #[error("{message} (driver: {driver}, schema: {schema_file})")]
    StructuralError {
        /// Description of the structural problem.
        message: String,
        /// Driver that detected the issue.
        driver: String,
        /// Path to the schema file.
        schema_file: String,
    },

    /// Legacy: invalid schema structure (kept for migration compatibility).
    #[error("invalid schema structure for driver '{driver}': {reason}")]
    InvalidStructure {
        /// Driver name.
        driver: String,
        /// Reason for rejection.
        reason: String,
    },

    /// Legacy: unknown field type (kept for migration compatibility).
    #[error("unknown field type '{field_type}' for driver '{driver}'")]
    UnknownFieldType {
        /// Driver name.
        driver: String,
        /// The unrecognized type.
        field_type: String,
    },

    /// Legacy: missing required field (kept for migration compatibility).
    #[error("missing required schema field '{field}' for driver '{driver}'")]
    MissingField {
        /// Driver name.
        driver: String,
        /// The missing field.
        field: String,
    },
}

/// Error from data validation (request time).
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// A required field is absent from the data.
    #[error("required field '{field}' is missing (direction: {direction})")]
    MissingRequired {
        /// Name of the missing field.
        field: String,
        /// Whether this was input or output validation.
        direction: ValidationDirection,
    },

    /// A field value does not match the declared type.
    #[error("type mismatch on field '{field}': expected {expected}, got {actual} (direction: {direction})")]
    TypeMismatch {
        /// Field with the type mismatch.
        field: String,
        /// Type declared in the schema.
        expected: String,
        /// Type of the actual value.
        actual: String,
        /// Whether this was input or output validation.
        direction: ValidationDirection,
    },

    /// A field value violates a declared constraint (min, max, length, pattern, enum).
    #[error("constraint violation on field '{field}': {constraint} — value {value}, limit {limit} (direction: {direction})")]
    ConstraintViolation {
        /// Field with the violation.
        field: String,
        /// Which constraint was violated.
        constraint: String,
        /// The actual value that failed.
        value: String,
        /// The constraint limit.
        limit: String,
        /// Whether this was input or output validation.
        direction: ValidationDirection,
    },

    /// Type coercion between compatible types failed.
    #[error("coercion failed on field '{field}': cannot coerce {from_type} to {to_type} (direction: {direction})")]
    CoercionFailed {
        /// Field where coercion was attempted.
        field: String,
        /// Source type.
        from_type: String,
        /// Target type.
        to_type: String,
        /// Whether this was input or output validation.
        direction: ValidationDirection,
    },

    /// The driver does not implement schema validation.
    #[error("driver '{driver}' validation not implemented")]
    DriverNotImplemented {
        /// Driver name.
        driver: String,
    },

    /// Legacy: value out of range (kept for migration compatibility).
    #[error("value out of range on field '{field}': {reason}")]
    OutOfRange {
        /// Field name.
        field: String,
        /// Description of the range violation.
        reason: String,
    },

    /// Legacy: pattern mismatch (kept for migration compatibility).
    #[error("pattern mismatch on field '{field}': value does not match '{pattern}'")]
    PatternMismatch {
        /// Field name.
        field: String,
        /// The expected pattern.
        pattern: String,
    },

    /// Legacy: column count mismatch (kept for migration compatibility).
    #[error("column count mismatch: schema defines {expected} fields, result has {actual}")]
    ColumnCountMismatch {
        /// Number of fields in the schema.
        expected: usize,
        /// Number of columns in the result.
        actual: usize,
    },
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
    /// Field name (matches a column or property in the data).
    pub name: String,
    /// Rivers primitive type (e.g. `"string"`, `"integer"`, `"uuid"`, `"email"`).
    #[serde(rename = "type")]
    pub field_type: String,
    /// Whether this field must be present in the data.
    #[serde(default)]
    pub required: bool,
    /// Driver-specific constraints (min, max, min_length, max_length, pattern, enum).
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
    /// Hostname or IP address.
    pub host: String,
    /// Port number.
    pub port: u16,
    /// Database, bucket, or keyspace name.
    pub database: String,
    /// Authentication username.
    pub username: String,
    /// Authentication password (resolved from LockBox before reaching the driver).
    pub password: String,
    /// Driver-specific connection options.
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
/// The `execute` method handles DML operations (SELECT, INSERT, UPDATE, DELETE).
/// It MUST reject DDL statements and admin operations with [`DriverError::Forbidden`].
///
/// The `ddl_execute` method handles DDL and admin operations. It is only callable
/// from the ApplicationInit execution context. The caller (DataViewEngine) enforces
/// whitelist checks before calling this method.
///
/// Connections are owned by the pool; when done, the pool reclaims them.
#[async_trait]
pub trait Connection: Send + Sync {
    /// Execute a DML query (SELECT, INSERT, UPDATE, DELETE, or equivalent).
    ///
    /// MUST reject DDL statements (SQL) and admin operations (non-SQL)
    /// with [`DriverError::Forbidden`]. Use [`check_admin_guard`](crate::check_admin_guard)
    /// for a combined check.
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError>;

    /// Execute a DDL statement or admin operation.
    ///
    /// Only callable from ApplicationInit context. The caller (DataViewEngine)
    /// is responsible for whitelist enforcement (Gate 3).
    ///
    /// Default implementation returns [`DriverError::Unsupported`] — drivers that
    /// support DDL/admin operations (postgres, mysql, sqlite, redis, etc.) override.
    async fn ddl_execute(&mut self, _query: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support DDL/admin operations",
            self.driver_name()
        )))
    }

    /// Returns operation tokens this driver considers admin/DDL-like.
    ///
    /// `execute()` MUST reject queries whose operation matches this list.
    /// `ddl_execute()` accepts these operations.
    ///
    /// SQL drivers return an empty slice (they use [`is_ddl_statement`](crate::is_ddl_statement)
    /// on the statement text instead). Non-SQL drivers return their admin operation tokens.
    fn admin_operations(&self) -> &[&str] {
        &[]
    }

    /// Health check — returns Ok(()) or connection-level error.
    async fn ping(&mut self) -> Result<(), DriverError>;

    /// The name of the driver that created this connection.
    fn driver_name(&self) -> &str;

    /// Begin a transaction on this connection.
    async fn begin_transaction(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support transactions",
            self.driver_name()
        )))
    }

    /// Commit the active transaction.
    async fn commit_transaction(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support transactions",
            self.driver_name()
        )))
    }

    /// Rollback the active transaction.
    async fn rollback_transaction(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support transactions",
            self.driver_name()
        )))
    }

    /// Prepare a query for repeated execution. No-op by default.
    async fn prepare(&mut self, _query: &str) -> Result<(), DriverError> {
        Ok(())
    }

    /// Execute a previously prepared query. Falls through to execute() by default.
    async fn execute_prepared(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        self.execute(query).await
    }

    /// Check if a query has been prepared on this connection.
    fn has_prepared(&self, _query: &str) -> bool {
        false
    }
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

    /// How this driver binds parameters in query text.
    ///
    /// The DataView engine uses this to rewrite `$name` placeholders
    /// from TOML config into the driver's native format before dispatch.
    fn param_style(&self) -> ParamStyle {
        ParamStyle::None
    }

    /// Whether this driver supports schema introspection at startup.
    fn supports_introspection(&self) -> bool {
        false
    }
}
