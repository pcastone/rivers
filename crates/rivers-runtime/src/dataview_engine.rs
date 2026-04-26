//! DataView engine — named, parameterized, schema-validated query execution facade.
//!
//! Per `rivers-data-layer-spec.md` §6.
//!
//! Execution sequence (§6.2):
//! 1. Registry lookup → 2. Parameter validation → 3. Cache check →
//! 4. Pool acquire → 5. driver.execute → 6. Release → 7. Schema validate →
//! 8. Cache populate → 9. Return

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rivers_core::DriverFactory;
use rivers_driver_sdk::broker::{BrokerConsumerConfig, OutboundMessage};
use rivers_driver_sdk::error::DriverError;
use rivers_driver_sdk::traits::Connection;
use rivers_driver_sdk::types::{Query, QueryResult, QueryValue};
use rivers_driver_sdk::ConnectionParams;

use crate::dataview::DataViewConfig;
use crate::tiered_cache::{cache_key, DataViewCache};

// ── Execution Context ─────────────────────────────────────────────

/// Execution context for datasource operations.
///
/// Determines whether DDL/admin operations are permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionContext {
    /// Normal request dispatch — DDL/admin ops blocked by Gate 1.
    ViewRequest,
    /// Application init handler — DDL/admin ops permitted if whitelisted.
    ApplicationInit,
}

// ── Connection Acquirer (D2) ──────────────────────────────────────

/// Errors returned by `ConnectionAcquirer::acquire`.
///
/// This is a slim, runtime-crate-local mirror of `riversd::pool::PoolError`
/// so `DataViewExecutor` can route through the pool without depending on
/// the binary crate (`riversd`). Error variants map 1:1 to the underlying
/// pool error categories.
#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    /// No pool registered for the requested datasource id.
    #[error("no pool registered for datasource '{0}'")]
    UnknownDatasource(String),
    /// Circuit breaker is open for the datasource.
    #[error("circuit breaker is open for datasource '{0}'")]
    CircuitOpen(String),
    /// Acquire timed out waiting for an available connection.
    #[error("connection acquire timeout for datasource '{datasource}' ({timeout_ms}ms)")]
    Timeout {
        /// Datasource the timeout occurred on.
        datasource: String,
        /// Configured timeout that elapsed.
        timeout_ms: u64,
    },
    /// Pool is draining — no new checkouts.
    #[error("pool is draining for datasource '{0}'")]
    Draining(String),
    /// Underlying driver error.
    #[error("driver error: {0}")]
    Driver(String),
    /// Other / opaque pool error (passes through human message).
    #[error("pool error: {0}")]
    Other(String),
}

/// Opaque connection guard — returned from `ConnectionAcquirer::acquire`.
///
/// Implementations own a checked-out `Box<dyn Connection>` and arrange for
/// it to be returned to the underlying pool when dropped. The executor only
/// needs `conn_mut()` for the duration of one DataView call.
pub trait PooledConnection: Send {
    /// Mutable access to the underlying connection.
    fn conn_mut(&mut self) -> &mut Box<dyn Connection>;
}

/// Acquire connections from a per-datasource pool.
///
/// Implemented by `riversd::pool::PoolManager`. Held as
/// `Arc<dyn ConnectionAcquirer>` inside `DataViewExecutor` so the runtime
/// can route through the pool without depending on the binary crate.
#[async_trait::async_trait]
pub trait ConnectionAcquirer: Send + Sync {
    /// Acquire a connection from the named datasource's pool.
    async fn acquire(&self, datasource_id: &str) -> Result<Box<dyn PooledConnection>, AcquireError>;

    /// Whether a pool is registered for the given datasource id.
    ///
    /// The executor uses this to fall back to the direct-connect path for
    /// broker datasources (which are not registered as pools).
    async fn has_pool(&self, datasource_id: &str) -> bool;
}

// ── DataView Errors ───────────────────────────────────────────────

/// Errors from DataView operations.
#[derive(Debug, thiserror::Error)]
pub enum DataViewError {
    /// Named DataView does not exist in the registry.
    #[error("dataview not found: '{name}'")]
    NotFound {
        /// DataView name that was looked up.
        name: String,
    },

    /// A required parameter was not supplied.
    #[error("missing required parameter '{name}' for dataview '{dataview}'")]
    MissingParameter {
        /// Parameter name.
        name: String,
        /// DataView name.
        dataview: String,
    },

    /// Supplied parameter value does not match the declared type.
    #[error("parameter type mismatch for '{name}': expected {expected}, got {actual}")]
    ParameterTypeMismatch {
        /// Parameter name.
        name: String,
        /// Expected type (e.g. "integer").
        expected: String,
        /// Actual type of the supplied value.
        actual: String,
    },

    /// Unknown parameter supplied when `strict_parameters` is enabled.
    #[error("unknown parameter '{name}' for dataview '{dataview}' (strict mode)")]
    UnknownParameter {
        /// Parameter name.
        name: String,
        /// DataView name.
        dataview: String,
    },

    /// Return-schema validation failed.
    #[error("schema validation failed: {reason}")]
    Schema {
        /// Human-readable validation failure.
        reason: String,
    },

    /// Connection pool or datasource error.
    #[error("pool error: {0}")]
    Pool(String),

    /// Driver-level execution error.
    #[error("driver error: {0}")]
    Driver(String),

    /// Malformed request (empty name, zero timeout, etc.).
    #[error("invalid request: {reason}")]
    InvalidRequest {
        /// What was wrong with the request.
        reason: String,
    },

    /// Schema attribute not supported by the target driver.
    #[error("unsupported schema attribute: {attribute} for driver {driver}")]
    UnsupportedSchemaAttribute {
        /// Attribute name.
        attribute: String,
        /// Driver name.
        driver: String,
    },

    /// Schema file does not exist at the configured path.
    #[error("schema file not found: {path}")]
    SchemaFileNotFound {
        /// Filesystem path.
        path: String,
    },

    /// Schema file could not be parsed as JSON.
    #[error("schema file parse error: {path}: {reason}")]
    SchemaFileParseError {
        /// Filesystem path.
        path: String,
        /// Parse error details.
        reason: String,
    },

    /// Unknown faker method referenced in schema.
    #[error("unknown faker method: {method}")]
    UnknownFakerMethod {
        /// Faker method string (e.g. "name.invalid").
        method: String,
    },

    /// Cache read/write error.
    #[error("cache error: {0}")]
    Cache(String),

    /// Request-level timeout (D3 / P1-10) — the combined acquire + execute
    /// budget elapsed before the call finished.
    ///
    /// Emitted by `DataViewExecutor::execute` when `request.timeout_ms` is
    /// positive and the acquire-and-execute future runs longer than that
    /// budget. Carries the datasource id and the configured timeout so the
    /// log line is actionable.
    #[error("dataview timeout: datasource '{datasource_id}' exceeded {timeout_ms}ms budget")]
    Timeout {
        /// Datasource id the timeout was enforced against.
        datasource_id: String,
        /// Configured per-request timeout in milliseconds that elapsed.
        timeout_ms: u64,
    },
}

// ── DataView Request / Response ───────────────────────────────────

/// A request to execute a DataView.
///
/// Per spec §6.3.
#[derive(Debug, Clone)]
pub struct DataViewRequest {
    /// DataView name (must match a registry entry).
    pub name: String,
    /// HTTP method for per-method query/parameter resolution.
    pub method: String,
    /// Resolved parameter values.
    pub parameters: HashMap<String, QueryValue>,
    /// Optional per-request timeout override.
    pub timeout_ms: Option<u64>,
    /// Distributed trace ID for observability.
    pub trace_id: String,
    /// When true, skip cache lookup and force a fresh execution.
    pub cache_bypass: bool,
}

/// Response from a DataView execution.
///
/// Per spec §6.3. query_result is Arc-wrapped to avoid deep clones on cache hits.
#[derive(Debug, Clone)]
pub struct DataViewResponse {
    /// Query result (Arc-wrapped to avoid deep clones on cache hits).
    pub query_result: Arc<QueryResult>,
    /// Wall-clock execution time in milliseconds.
    pub execution_time_ms: u64,
    /// Whether this result came from the cache.
    pub cache_hit: bool,
    /// Distributed trace ID echoed from the request.
    pub trace_id: String,
}

// ── DataView Request Builder ──────────────────────────────────────

/// Builder for constructing validated DataView requests.
///
/// Per spec §6.3 — validates name is non-empty, timeout > 0.
pub struct DataViewRequestBuilder {
    name: String,
    method: String,
    parameters: HashMap<String, QueryValue>,
    timeout_ms: Option<u64>,
    trace_id: String,
    cache_bypass: bool,
}

impl DataViewRequestBuilder {
    /// Create a new builder for the given DataView name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            method: "GET".to_string(),
            parameters: HashMap::new(),
            timeout_ms: None,
            trace_id: String::new(),
            cache_bypass: false,
        }
    }

    /// Set the HTTP method for per-method parameter/query resolution.
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// Set a parameter value.
    pub fn param(mut self, name: impl Into<String>, value: QueryValue) -> Self {
        self.parameters.insert(name.into(), value);
        self
    }

    /// Set all parameters at once.
    pub fn params(mut self, params: HashMap<String, QueryValue>) -> Self {
        self.parameters = params;
        self
    }

    /// Set the timeout in milliseconds.
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    /// Set the trace ID.
    pub fn trace_id(mut self, id: impl Into<String>) -> Self {
        self.trace_id = id.into();
        self
    }

    /// Bypass the cache for this request.
    pub fn cache_bypass(mut self, bypass: bool) -> Self {
        self.cache_bypass = bypass;
        self
    }

    /// Build a basic request (validates name and timeout only).
    pub fn build(self) -> Result<DataViewRequest, DataViewError> {
        if self.name.is_empty() {
            return Err(DataViewError::InvalidRequest {
                reason: "dataview name must not be empty".into(),
            });
        }
        if let Some(ms) = self.timeout_ms {
            if ms == 0 {
                return Err(DataViewError::InvalidRequest {
                    reason: "timeout_ms must be greater than 0".into(),
                });
            }
        }
        Ok(DataViewRequest {
            name: self.name,
            method: self.method,
            parameters: self.parameters,
            timeout_ms: self.timeout_ms,
            trace_id: self.trace_id,
            cache_bypass: self.cache_bypass,
        })
    }

    /// Build a request with parameter validation against a DataViewConfig.
    ///
    /// Per spec §6.5 — validates required params, applies zero-value defaults
    /// for optional params, and rejects unknown params in strict mode.
    pub fn build_for(mut self, config: &DataViewConfig) -> Result<DataViewRequest, DataViewError> {
        if self.name.is_empty() {
            return Err(DataViewError::InvalidRequest {
                reason: "dataview name must not be empty".into(),
            });
        }
        if let Some(ms) = self.timeout_ms {
            if ms == 0 {
                return Err(DataViewError::InvalidRequest {
                    reason: "timeout_ms must be greater than 0".into(),
                });
            }
        }

        // Resolve parameters for the request method (falls back to GET params)
        let method_params = config.parameters_for_method(&self.method);

        // Strict mode: reject unknown parameters
        if config.strict_parameters {
            let known: Vec<&str> = method_params.iter().map(|p| p.name.as_str()).collect();
            for param_name in self.parameters.keys() {
                if !known.contains(&param_name.as_str()) {
                    return Err(DataViewError::UnknownParameter {
                        name: param_name.clone(),
                        dataview: config.name.clone(),
                    });
                }
            }
        }

        // Validate required params and apply defaults for optional
        for param_def in method_params {
            if !self.parameters.contains_key(&param_def.name) {
                if param_def.required {
                    return Err(DataViewError::MissingParameter {
                        name: param_def.name.clone(),
                        dataview: config.name.clone(),
                    });
                }
                // Use configured default if available, otherwise zero-value
                let default_val = param_def.default.as_ref()
                    .and_then(|d| json_value_to_query_value(d, &param_def.param_type))
                    .unwrap_or_else(|| zero_value_for_type(&param_def.param_type));
                self.parameters.insert(param_def.name.clone(), default_val);
            } else {
                // Coerce string values to target type (path params always arrive as strings)
                let value = &self.parameters[&param_def.name];
                if !matches_param_type(value, &param_def.param_type) {
                    if let Some(coerced) = coerce_param_type(value, &param_def.param_type) {
                        self.parameters.insert(param_def.name.clone(), coerced);
                    } else {
                        return Err(DataViewError::ParameterTypeMismatch {
                            name: param_def.name.clone(),
                            expected: param_def.param_type.clone(),
                            actual: query_value_type_name(value).to_string(),
                        });
                    }
                }
            }
        }

        Ok(DataViewRequest {
            name: self.name,
            method: self.method,
            parameters: self.parameters,
            timeout_ms: self.timeout_ms,
            trace_id: self.trace_id,
            cache_bypass: self.cache_bypass,
        })
    }
}

// ── Parameter Type Helpers ────────────────────────────────────────

/// Return the zero-value default for a parameter type.
///
/// Per spec §6.5: "" for String, 0 for Integer, 0.0 for Float,
/// false for Boolean, [] for Array.
/// Convert a `serde_json::Value` default into a `QueryValue` for the given type.
///
/// String defaults are coerced to the target type so that e.g. `default = "25"`
/// on an integer parameter yields `QueryValue::Integer(25)`.
fn json_value_to_query_value(val: &serde_json::Value, target_type: &str) -> Option<QueryValue> {
    let qv = match val {
        serde_json::Value::String(s) => QueryValue::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() { QueryValue::Integer(i) }
            else if let Some(f) = n.as_f64() { QueryValue::Float(f) }
            else { return None; }
        }
        serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
        serde_json::Value::Null => QueryValue::Null,
        _ => return None,
    };
    // Coerce if type differs (e.g., default "25" for integer → Integer(25))
    if let QueryValue::String(_) = &qv {
        if target_type != "string" {
            return coerce_param_type(&qv, target_type);
        }
    }
    Some(qv)
}

/// Return the zero-value default for a parameter type.
///
/// Per spec §6.5: `""` for String, `0` for Integer, `0.0` for Float,
/// `false` for Boolean, `[]` for Array.
pub fn zero_value_for_type(param_type: &str) -> QueryValue {
    match param_type.to_lowercase().as_str() {
        "string" | "uuid" | "date" => QueryValue::String(String::new()),
        "integer" => QueryValue::Integer(0),
        "float" | "decimal" => QueryValue::Float(0.0),
        "boolean" => QueryValue::Boolean(false),
        "array" => QueryValue::Array(Vec::new()),
        _ => QueryValue::Null,
    }
}

/// Check if a QueryValue matches an expected parameter type.
pub fn matches_param_type(value: &QueryValue, param_type: &str) -> bool {
    match param_type.to_lowercase().as_str() {
        "string" | "uuid" | "date" => matches!(value, QueryValue::String(_)),
        "integer" => matches!(value, QueryValue::Integer(_)),
        "float" | "decimal" => matches!(value, QueryValue::Float(_)),
        "boolean" => matches!(value, QueryValue::Boolean(_)),
        "array" => matches!(value, QueryValue::Array(_)),
        _ => true, // unknown types pass through
    }
}

/// Attempt to coerce a QueryValue to the expected type.
///
/// Path params always arrive as strings. This coerces "11" → Integer(11), etc.
pub fn coerce_param_type(value: &QueryValue, target_type: &str) -> Option<QueryValue> {
    match (value, target_type.to_lowercase().as_str()) {
        // String → target type (path params arrive as strings)
        (QueryValue::String(s), "integer") => s.parse::<i64>().ok().map(QueryValue::Integer),
        (QueryValue::String(s), "float" | "decimal") => s.parse::<f64>().ok().map(QueryValue::Float),
        (QueryValue::String(s), "boolean") => match s.as_str() {
            "true" | "1" => Some(QueryValue::Boolean(true)),
            "false" | "0" => Some(QueryValue::Boolean(false)),
            _ => None,
        },
        // UUID validation — keep as string, validate format
        (QueryValue::String(s), "uuid") => {
            if s.len() == 36
                && s.chars().nth(8) == Some('-')
                && s.chars().nth(13) == Some('-')
                && s.chars().nth(18) == Some('-')
                && s.chars().nth(23) == Some('-')
                && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            {
                Some(QueryValue::String(s.clone()))
            } else {
                None
            }
        },
        // Date validation — YYYY-MM-DD format
        (QueryValue::String(s), "date") => {
            if s.len() == 10
                && s.chars().nth(4) == Some('-')
                && s.chars().nth(7) == Some('-')
            {
                Some(QueryValue::String(s.clone()))
            } else {
                None
            }
        },
        // Array from comma-separated string
        (QueryValue::String(s), "array") => {
            let parts: Vec<QueryValue> = s.split(',')
                .map(|v| QueryValue::String(v.trim().to_string()))
                .collect();
            Some(QueryValue::Array(parts))
        },
        // Float → Integer (truncate if lossless)
        (QueryValue::Float(f), "integer") => {
            let i = *f as i64;
            if (i as f64 - f).abs() < f64::EPSILON { Some(QueryValue::Integer(i)) } else { None }
        }
        // Integer → Float/Decimal (always lossless for reasonable values)
        (QueryValue::Integer(i), "float" | "decimal") => Some(QueryValue::Float(*i as f64)),
        _ => None,
    }
}

/// Get a human-readable type name for a QueryValue.
pub fn query_value_type_name(value: &QueryValue) -> &str {
    match value {
        QueryValue::Null => "null",
        QueryValue::Boolean(_) => "boolean",
        QueryValue::Integer(_) => "integer",
        QueryValue::Float(_) => "float",
        QueryValue::String(_) => "string",
        QueryValue::Array(_) => "array",
        QueryValue::Json(_) => "json",
    }
}

// ── DataView Registry ─────────────────────────────────────────────

/// Registry of DataView configurations.
///
/// Per spec §6.1 — name → DataViewConfig lookup.
pub struct DataViewRegistry {
    views: HashMap<String, DataViewConfig>,
}

impl DataViewRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            views: HashMap::new(),
        }
    }

    /// Register a DataView configuration.
    ///
    /// Overwrites any existing config with the same name.
    pub fn register(&mut self, config: DataViewConfig) {
        self.views.insert(config.name.clone(), config);
    }

    /// Look up a DataView by name.
    pub fn get(&self, name: &str) -> Option<&DataViewConfig> {
        self.views.get(name)
    }

    /// Find a DataView whose name ends with the given suffix.
    ///
    /// Used by host callbacks to resolve unqualified names like `"list_records"`
    /// against namespaced entries like `"handlers:list_records"`.
    pub fn find_by_suffix(&self, suffix: &str) -> Option<String> {
        self.views.keys().find(|k| k.ends_with(suffix)).cloned()
    }

    /// Return the number of registered DataViews.
    pub fn count(&self) -> usize {
        self.views.len()
    }

    /// Return all registered DataView names.
    pub fn names(&self) -> Vec<&str> {
        self.views.keys().map(|k| k.as_str()).collect()
    }
}

impl Default for DataViewRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Build Query from Config ───────────────────────────────────────

/// Build a `Query` from a DataViewConfig and request parameters.
///
/// Per spec §6.7 — parameters are passed via `Query.parameters`.
/// The query statement comes from the DataViewConfig.
pub fn build_query(config: &DataViewConfig, params: &HashMap<String, QueryValue>, method: &str) -> Query {
    // Resolve query for the request method (falls back to GET query).
    let statement = config
        .query_for_method(method)
        .unwrap_or_default();
    let mut query = Query::new(&config.datasource, statement);
    for (k, v) in params {
        query.parameters.insert(k.clone(), v.clone());
    }
    query
}

// ── Execution Timer ───────────────────────────────────────────────

/// Measure execution time and build a DataViewResponse.
pub fn build_response(
    query_result: Arc<QueryResult>,
    start: Instant,
    cache_hit: bool,
    trace_id: String,
) -> DataViewResponse {
    DataViewResponse {
        query_result,
        execution_time_ms: start.elapsed().as_millis() as u64,
        cache_hit,
        trace_id,
    }
}

// ── DataView Executor (X4) ──────────────────────────────────────────

/// Inner outcome of `connect_and_execute_or_broker`: either a normal
/// driver result, or an already-built `DataViewResponse` for the broker
/// produce path (which short-circuits cache + max_rows handling).
enum FactoryOutcome {
    Query(Result<QueryResult, DriverError>),
    BrokerResponse(DataViewResponse),
}

/// Outcome of the acquire+execute future inside `DataViewExecutor::execute`.
///
/// Mirrors `FactoryOutcome` but lifted to the outer scope so the
/// `tokio::time::timeout` wrapper can return it. Carrying the broker
/// short-circuit through the timeout boundary lets us preserve the
/// existing broker-produce flow (which builds its own `DataViewResponse`
/// and bypasses the post-execute cache / max_rows / schema pipeline).
enum InnerOutcome {
    Query(Result<QueryResult, DriverError>),
    BrokerResponse(DataViewResponse),
}


/// Execution facade for DataViews — combines registry lookup, parameter
/// validation, query building, and driver execution in one call.
///
/// Per spec §6.2: registry lookup → param validation → build query →
/// connect → execute → return result.
pub struct DataViewExecutor {
    registry: DataViewRegistry,
    factory: Arc<DriverFactory>,
    /// Datasource name → ConnectionParams mapping.
    /// Wrapped in Arc so callers can share one copy instead of cloning
    /// all connection params (which include passwords) into separate heap allocations.
    datasource_params: Arc<HashMap<String, ConnectionParams>>,
    /// DataView cache (L1/L2 tiered). Always present — uses NoopDataViewCache as fallback.
    cache: Arc<dyn DataViewCache>,
    /// Optional EventBus for cache invalidation events.
    event_bus: Option<Arc<rivers_core::EventBus>>,
    /// Optional connection pool router (D2). When `Some`, DataView calls
    /// route through `ConnectionAcquirer::acquire(datasource_id)` so they
    /// reuse pooled connections. When `None`, the executor falls back to
    /// `factory.connect(...)` per call (legacy/test path).
    ///
    /// Production wiring (`bundle_loader::load`) always installs an
    /// acquirer; the `Option` exists so unit tests + the older transactional
    /// constructor remain callable without wiring a pool.
    acquirer: Option<Arc<dyn ConnectionAcquirer>>,
}

impl DataViewExecutor {
    /// Create a new executor with a registry, driver factory, datasource params, and cache.
    ///
    /// No `ConnectionAcquirer` is installed; calls to [`Self::execute`] will
    /// fall back to `factory.connect(...)` per request. Production callers
    /// should chain [`Self::with_acquirer`] to route through the pool.
    pub fn new(
        registry: DataViewRegistry,
        factory: Arc<DriverFactory>,
        datasource_params: Arc<HashMap<String, ConnectionParams>>,
        cache: Arc<dyn DataViewCache>,
    ) -> Self {
        Self {
            registry,
            factory,
            datasource_params,
            cache,
            event_bus: None,
            acquirer: None,
        }
    }

    /// Install a `ConnectionAcquirer` so DataView calls route through the
    /// per-datasource pool. Returns `self` for builder-style chaining.
    pub fn with_acquirer(mut self, acquirer: Arc<dyn ConnectionAcquirer>) -> Self {
        self.acquirer = Some(acquirer);
        self
    }

    /// Install a `ConnectionAcquirer` after construction.
    pub fn set_acquirer(&mut self, acquirer: Arc<dyn ConnectionAcquirer>) {
        self.acquirer = Some(acquirer);
    }

    /// Whether a `ConnectionAcquirer` has been installed (testing helper).
    pub fn has_acquirer(&self) -> bool {
        self.acquirer.is_some()
    }

    /// Find a DataView whose name ends with the given suffix.
    ///
    /// Delegates to the underlying registry. Used by host callbacks
    /// to resolve unqualified names to namespaced entries.
    pub fn find_by_suffix(&self, suffix: &str) -> Option<String> {
        self.registry.find_by_suffix(suffix)
    }

    /// Return the datasource name a DataView is configured to execute on.
    ///
    /// Used by `ctx.transaction()`'s cross-datasource enforcement
    /// (spec §6.2): inside a transaction callback, `ctx.dataview("foo")`
    /// must reject if `foo`'s datasource differs from the transaction's.
    /// This lookup is pure registry introspection — no connection is
    /// acquired, no query is built.
    pub fn datasource_for(&self, name: &str) -> Option<String> {
        self.registry.get(name).map(|c| c.datasource.clone())
    }

    /// Set the EventBus for cache invalidation events.
    pub fn set_event_bus(&mut self, event_bus: Arc<rivers_core::EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// Execute a named DataView with the given parameters.
    ///
    /// Flow: registry lookup → param validation → build query →
    /// factory.connect() → conn.execute() → return QueryResult.
    ///
    /// If `txn_conn` is `Some`, the provided connection is used directly
    /// (transaction path) and cache population is skipped.
    ///
    /// Equivalent to [`Self::execute_with_timeout`] with `timeout_ms = None`
    /// (no enforced per-request budget). Existing call sites use this form
    /// and remain unchanged.
    pub async fn execute(
        &self,
        name: &str,
        params: HashMap<String, QueryValue>,
        method: &str,
        trace_id: &str,
        txn_conn: Option<&mut Box<dyn rivers_driver_sdk::Connection>>,
    ) -> Result<DataViewResponse, DataViewError> {
        self.execute_with_timeout(name, params, method, trace_id, txn_conn, None).await
    }

    /// Execute a named DataView with an explicit per-request timeout (D3 / P1-10).
    ///
    /// The timeout, when `Some(ms)` with `ms > 0`, encompasses BOTH the
    /// connection acquire (pool checkout) and the driver query execution —
    /// it is a request-level budget, not just a query-level one. On elapse,
    /// the inner future is dropped (cancelling any in-flight acquire/execute,
    /// freeing the request worker) and the call returns
    /// [`DataViewError::Timeout`] carrying the datasource id and the
    /// configured budget.
    ///
    /// `None` or `Some(0)` disables the timeout (current behavior preserved).
    /// The transaction path (`txn_conn = Some(...)`) does not apply the
    /// timeout — transactions own their own deadline at the orchestrator level.
    pub async fn execute_with_timeout(
        &self,
        name: &str,
        params: HashMap<String, QueryValue>,
        method: &str,
        trace_id: &str,
        txn_conn: Option<&mut Box<dyn rivers_driver_sdk::Connection>>,
        timeout_ms: Option<u64>,
    ) -> Result<DataViewResponse, DataViewError> {
        let start = Instant::now();
        let is_transaction = txn_conn.is_some();

        // 1. Registry lookup
        let config = self
            .registry
            .get(name)
            .ok_or_else(|| DataViewError::NotFound {
                name: name.to_string(),
            })?;

        // 2. Parameter validation (use HTTP method for per-method query/param resolution)
        let request = DataViewRequestBuilder::new(name)
            .method(method)
            .params(params)
            .trace_id(trace_id)
            .build_for(config)?;

        // 3. Cache check — skip entirely if view has no caching config
        let view_caching = config.caching.as_ref();
        if !request.cache_bypass && view_caching.is_some() {
            let key = cache_key(name, &request.parameters);
            match self.cache.get(name, &request.parameters).await {
                Ok(Some(cached)) => {
                    tracing::debug!(dataview = %name, cache_key = %key, "cache hit");
                    return Ok(build_response(cached, start, true, trace_id.to_string()));
                }
                Ok(None) => {
                    tracing::debug!(dataview = %name, cache_key = %key, "cache miss");
                }
                Err(e) => {
                    tracing::warn!(dataview = %name, error = %e, "cache get failed, proceeding without cache");
                }
            }
        }

        // 4. Build query from config + validated params
        let mut query = build_query(config, &request.parameters, &request.method);

        // 5. Resolve datasource → connection params
        let ds_params = self
            .datasource_params
            .get(&config.datasource)
            .ok_or_else(|| DataViewError::Pool(format!(
                "datasource '{}' not configured for dataview '{}'",
                config.datasource, name
            )))?;

        // 6. Get driver name from datasource config and connect
        // The DriverFactory resolves driver_name → DatabaseDriver → Connection
        let driver_name = ds_params
            .options
            .get("driver")
            .map(|s| s.as_str())
            .unwrap_or(&config.datasource);

        // 6a. Translate $name parameters to driver's native format
        if let Some(driver) = self.factory.get_driver(driver_name) {
            let style = driver.param_style();
            if style != rivers_driver_sdk::ParamStyle::None {
                let (rewritten, ordered) = rivers_driver_sdk::translate_params(
                    &query.statement,
                    &query.parameters,
                    style,
                );
                query.statement = rewritten;
                // For positional styles, rebuild params in order
                if style == rivers_driver_sdk::ParamStyle::DollarPositional
                    || style == rivers_driver_sdk::ParamStyle::QuestionPositional
                {
                    // Use zero-padded numeric keys so alphabetical sort in
                    // build_params preserves positional order: "001", "002", ...
                    query.parameters.clear();
                    for (i, (_k, v)) in ordered.into_iter().enumerate() {
                        query.parameters.insert(format!("{:03}", i + 1), v);
                    }
                }
            }
        }

        // Build the acquire+execute future. The whole future is then either
        // awaited directly (no timeout) or wrapped in `tokio::time::timeout`
        // (D3 / P1-10) so a slow datasource cannot tie up the request worker
        // indefinitely. The timeout encompasses BOTH pool acquisition and
        // driver query execution — a request-level budget, not just a
        // query-level one.
        //
        // Transaction path: skip the timeout wrapper. Transactions own their
        // own deadline at the orchestrator level and the caller-provided
        // connection is not ours to cancel mid-flight.
        let acquire_and_execute = async {
            if let Some(ref acquirer) = self.acquirer {
                // Pool path (D2) — `acquire` resolves the datasource id to a
                // pool and returns an RAII guard. Single checkout for the
                // whole call; dropped automatically once `guard` falls out
                // of scope at the end of this async block.
                //
                // `has_pool` lets us route broker datasources (which have no
                // pool registered) through the legacy direct-connect path.
                let datasource_id = config.datasource.as_str();
                if acquirer.has_pool(datasource_id).await {
                    match acquirer.acquire(datasource_id).await {
                        Ok(mut guard) => {
                            let conn = guard.conn_mut();
                            let r = if config.prepared && conn.has_prepared(&query.statement) {
                                conn.execute_prepared(&query).await
                            } else if config.prepared {
                                if let Err(e) = conn.prepare(&query.statement).await {
                                    return Err(DataViewError::Driver(format!("prepare: {e}")));
                                }
                                conn.execute_prepared(&query).await
                            } else {
                                conn.execute(&query).await
                            };
                            Ok(InnerOutcome::Query(r))
                        }
                        Err(e) => Err(DataViewError::Pool(format!("pool acquire failed: {e}"))),
                    }
                } else {
                    // No pool registered → direct-connect path (broker or pre-wired test).
                    match self.connect_and_execute_or_broker(
                        driver_name, ds_params, &query, config, name, start, trace_id,
                    ).await? {
                        FactoryOutcome::Query(r) => Ok(InnerOutcome::Query(r)),
                        FactoryOutcome::BrokerResponse(resp) => Ok(InnerOutcome::BrokerResponse(resp)),
                    }
                }
            } else {
                // No acquirer installed — legacy direct-connect path. We log
                // at WARN once-per-call so it's noticeable in production but
                // doesn't break test fixtures that drive the executor
                // without a pool.
                tracing::warn!(
                    dataview = %name,
                    datasource = %config.datasource,
                    "DataViewExecutor has no ConnectionAcquirer installed; falling back to factory.connect (per-call handshake)"
                );
                match self.connect_and_execute_or_broker(
                    driver_name, ds_params, &query, config, name, start, trace_id,
                ).await? {
                    FactoryOutcome::Query(r) => Ok(InnerOutcome::Query(r)),
                    FactoryOutcome::BrokerResponse(resp) => Ok(InnerOutcome::BrokerResponse(resp)),
                }
            }
        };

        let inner_outcome = if let Some(conn) = txn_conn {
            // Transaction path — use provided connection, skip caching AND
            // skip the timeout wrapper. Transactions own their own deadline.
            let r = if config.prepared && conn.has_prepared(&query.statement) {
                conn.execute_prepared(&query).await
            } else if config.prepared {
                conn.prepare(&query.statement).await
                    .map_err(|e| DataViewError::Driver(format!("prepare: {e}")))?;
                conn.execute_prepared(&query).await
            } else {
                conn.execute(&query).await
            };
            InnerOutcome::Query(r)
        } else {
            // Apply request-level timeout (D3 / P1-10) when configured.
            // `None` or `Some(0)` → no timeout (preserves prior behavior).
            // Any positive value → enforced via tokio::time::timeout. The
            // inner future is dropped on elapse, which cancels any in-flight
            // acquire / driver call and frees the request worker.
            match timeout_ms {
                Some(ms) if ms > 0 => {
                    match tokio::time::timeout(Duration::from_millis(ms), acquire_and_execute).await {
                        Ok(inner) => inner?,
                        Err(_elapsed) => {
                            tracing::warn!(
                                dataview = %name,
                                datasource = %config.datasource,
                                timeout_ms = ms,
                                "dataview request timeout — acquire+execute exceeded budget"
                            );
                            return Err(DataViewError::Timeout {
                                datasource_id: config.datasource.clone(),
                                timeout_ms: ms,
                            });
                        }
                    }
                }
                _ => acquire_and_execute.await?,
            }
        };

        // Hoist the broker-response short-circuit out of the timeout block so
        // both the inner-pool branch and the inner-direct branch can return
        // it. (Broker produce builds its own DataViewResponse and skips the
        // remaining max_rows / cache / schema validation pipeline.)
        let execute_result = match inner_outcome {
            InnerOutcome::Query(r) => r,
            InnerOutcome::BrokerResponse(resp) => return Ok(resp),
        };

        let mut query_result = execute_result
            .map_err(|e| DataViewError::Driver(e.to_string()))?;

        // Enforce max_rows limit
        if config.max_rows > 0 && query_result.rows.len() > config.max_rows {
            tracing::warn!(
                dataview = %name,
                returned = query_result.rows.len(),
                max_rows = config.max_rows,
                "result truncated to max_rows"
            );
            query_result.rows.truncate(config.max_rows);
            query_result.affected_rows = config.max_rows as u64;
        }

        // Validate result against schema if configured
        if config.validate_result {
            if let Some(ref schema_path) = config.return_schema {
                validate_query_result(&query_result, schema_path)?;
            }
        }

        // Cache populate on success — skip for transaction queries
        if !is_transaction && !request.cache_bypass && view_caching.is_some() {
            let ttl_override = view_caching.map(|c| c.ttl_seconds);
            if let Err(e) = self.cache.set(name, &request.parameters, &query_result, ttl_override).await {
                tracing::warn!(dataview = %name, error = %e, "cache set failed");
            }
        }

        // Cache invalidation — invalidate listed DataViews on success
        self.run_cache_invalidation(name, &config.invalidates, trace_id).await;

        Ok(build_response(Arc::new(query_result), start, false, trace_id.to_string()))
    }

    /// Helper: factory.connect + execute, with the broker-produce fallback
    /// preserved (used by both the no-pool-registered branch and the
    /// no-acquirer-installed branch of `execute`).
    #[allow(clippy::too_many_arguments)]
    async fn connect_and_execute_or_broker(
        &self,
        driver_name: &str,
        ds_params: &ConnectionParams,
        query: &Query,
        config: &DataViewConfig,
        name: &str,
        start: Instant,
        trace_id: &str,
    ) -> Result<FactoryOutcome, DataViewError> {
        match self.factory.connect(driver_name, ds_params).await {
            Ok(mut conn) => {
                let r = if config.prepared && conn.has_prepared(&query.statement) {
                    conn.execute_prepared(query).await
                } else if config.prepared {
                    conn.prepare(&query.statement).await
                        .map_err(|e| DataViewError::Driver(format!("prepare: {e}")))?;
                    conn.execute_prepared(query).await
                } else {
                    conn.execute(query).await
                };
                Ok(FactoryOutcome::Query(r))
            }
            Err(DriverError::UnknownDriver(_)) => {
                // Broker produce path — transactions don't apply to message brokers.
                let invalidates = config.invalidates.clone();
                let response = self.execute_broker_produce(driver_name, ds_params, query, start, trace_id).await?;
                self.run_cache_invalidation(name, &invalidates, trace_id).await;
                Ok(FactoryOutcome::BrokerResponse(response))
            }
            Err(e) => Err(DataViewError::Pool(format!("connection failed: {e}"))),
        }
    }

    /// Execute a DDL statement or admin operation (ApplicationInit context only).
    ///
    /// Checks the DDL whitelist (Gate 3) before calling `Connection::ddl_execute()`.
    /// Only used by application init handlers.
    pub async fn execute_ddl(
        &self,
        datasource_name: &str,
        query: &Query,
        app_id: &str,
        ddl_whitelist: &[String],
        _trace_id: &str,
    ) -> Result<QueryResult, DataViewError> {
        // Resolve datasource → connection params
        let ds_params = self
            .datasource_params
            .get(datasource_name)
            .ok_or_else(|| DataViewError::Pool(format!(
                "datasource '{}' not configured",
                datasource_name
            )))?;

        // Gate 3: whitelist check
        let database = if ds_params.database.is_empty() { datasource_name } else { &ds_params.database };
        if !rivers_core_config::config::security::is_ddl_permitted(database, app_id, ddl_whitelist) {
            return Err(DataViewError::Driver(format!(
                "DDL operation not permitted: '{}' not in ddl_whitelist for app {}",
                database, app_id
            )));
        }

        // Connect and execute DDL
        let driver_name = ds_params
            .options
            .get("driver")
            .map(|s| s.as_str())
            .unwrap_or(datasource_name);

        let mut conn = self.factory.connect(driver_name, ds_params).await
            .map_err(|e| DataViewError::Pool(format!("connection failed: {e}")))?;

        let result = conn
            .ddl_execute(query)
            .await
            .map_err(|e| DataViewError::Driver(e.to_string()))?;

        tracing::info!(
            datasource = %datasource_name,
            database = %database,
            app_id = %app_id,
            statement_prefix = %query.statement.chars().take(40).collect::<String>(),
            "DDL executed"
        );

        Ok(result)
    }

    /// Invalidate cache entries for listed DataViews and emit EventBus event.
    ///
    /// Called after successful write DataView execution when `config.invalidates` is non-empty.
    async fn run_cache_invalidation(&self, source_view: &str, invalidates: &[String], trace_id: &str) {
        if invalidates.is_empty() {
            return;
        }
        for target_view in invalidates {
            self.cache.invalidate(Some(target_view.as_str())).await;
            tracing::info!(
                source = %source_view,
                target = %target_view,
                "cache invalidated"
            );
        }

        // Emit CacheInvalidation event for observability
        if let Some(ref event_bus) = self.event_bus {
            let event = rivers_core::event::Event::new(
                rivers_core::eventbus::events::CACHE_INVALIDATION,
                serde_json::json!({
                    "source_view": source_view,
                    "invalidated": invalidates,
                }),
            ).with_trace_id(trace_id);
            event_bus.publish(&event).await;
        }
    }

    /// Broker produce path — create producer, publish message, return result.
    ///
    /// Called when the driver name is a broker (not a database driver).
    /// The DataView's `query` field = destination topic/queue.
    /// The request body (via parameters) = message payload serialized as JSON.
    async fn execute_broker_produce(
        &self,
        driver_name: &str,
        ds_params: &ConnectionParams,
        query: &Query,
        start: Instant,
        trace_id: &str,
    ) -> Result<DataViewResponse, DataViewError> {
        let broker_driver = self
            .factory
            .get_broker_driver(driver_name)
            .ok_or_else(|| DataViewError::Pool(format!(
                "no database or broker driver registered for '{driver_name}'"
            )))?;

        // Minimal config — only needed for create_producer connection setup
        let broker_config = BrokerConsumerConfig {
            group_prefix: String::new(),
            app_id: String::new(),
            datasource_id: query.target.clone(),
            node_id: String::new(),
            reconnect_ms: 5000,
            subscriptions: Vec::new(),
        };

        let mut producer = broker_driver
            .create_producer(ds_params, &broker_config)
            .await
            .map_err(|e| DataViewError::Pool(format!("broker producer failed: {e}")))?;

        // Build outbound message: destination = query statement (topic/queue name),
        // payload = parameters serialized as JSON bytes
        let payload = serde_json::to_vec(&query.parameters)
            .map_err(|e| DataViewError::Driver(format!("payload serialization: {e}")))?;

        let message = OutboundMessage {
            destination: query.statement.clone(),
            payload,
            headers: std::collections::HashMap::new(),
            key: None,
            reply_to: None,
        };

        producer
            .publish(message)
            .await
            .map_err(|e| DataViewError::Driver(e.to_string()))?;

        let _ = producer.close().await;

        // Return success with affected_rows = 1
        let query_result = QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        };

        Ok(build_response(Arc::new(query_result), start, false, trace_id.to_string()))
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &DataViewRegistry {
        &self.registry
    }

    /// Get the DataViewConfig for a named DataView (used for circuit breaker checks).
    pub fn get_dataview_config(&self, name: &str) -> Option<&DataViewConfig> {
        self.registry.get(name)
    }

    /// List configured datasource names and their driver type.
    ///
    /// Returns one entry per configured datasource, with the datasource name
    /// and the driver name extracted from the connection params options.
    pub fn datasource_info(&self) -> Vec<serde_json::Value> {
        let mut result: Vec<_> = self
            .datasource_params
            .iter()
            .map(|(name, params)| {
                let driver = params.options.get("driver").cloned().unwrap_or_default();
                serde_json::json!({"name": name, "driver": driver})
            })
            .collect();
        result.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });
        result
    }

    /// Get a reference to the driver factory.
    pub fn factory(&self) -> &Arc<DriverFactory> {
        &self.factory
    }

    /// Get a reference to the datasource connection params.
    pub fn datasource_params(&self) -> &Arc<HashMap<String, ConnectionParams>> {
        &self.datasource_params
    }

    /// Look up connection params for a datasource by exact name.
    pub fn datasource_params_get(&self, name: &str) -> Option<&ConnectionParams> {
        self.datasource_params.get(name)
    }

    /// Look up connection params by suffix match (e.g., `:canary-sqlite`).
    ///
    /// Used by host callbacks to resolve unqualified datasource names
    /// against namespaced entries like `sql:canary-sqlite`.
    pub fn datasource_params_by_suffix(&self, suffix: &str) -> Option<&ConnectionParams> {
        self.datasource_params
            .iter()
            .find(|(k, _)| k.ends_with(suffix))
            .map(|(_, v)| v)
    }

    /// Look up the driver name for a datasource.
    ///
    /// Checks the datasource name in the registry to find the associated driver.
    pub fn driver_for_datasource(&self, datasource_name: &str) -> Option<String> {
        // The driver is stored in the options map under "driver" key,
        // or can be inferred from the DataView config's datasource reference.
        self.datasource_params
            .get(datasource_name)
            .and_then(|p| p.options.get("driver").cloned())
    }

    /// List all configured datasource names.
    pub fn datasource_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.datasource_params.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coerce_uuid_valid() {
        let v = QueryValue::String("550e8400-e29b-41d4-a716-446655440000".into());
        assert!(coerce_param_type(&v, "uuid").is_some());
    }

    #[test]
    fn coerce_uuid_invalid() {
        let v = QueryValue::String("not-a-uuid".into());
        assert!(coerce_param_type(&v, "uuid").is_none());
    }

    #[test]
    fn coerce_date_valid() {
        let v = QueryValue::String("2026-04-15".into());
        assert!(coerce_param_type(&v, "date").is_some());
    }

    #[test]
    fn coerce_date_invalid() {
        let v = QueryValue::String("04/15/2026".into());
        assert!(coerce_param_type(&v, "date").is_none());
    }

    #[test]
    fn coerce_array_from_csv() {
        let v = QueryValue::String("a,b,c".into());
        match coerce_param_type(&v, "array") {
            Some(QueryValue::Array(arr)) => assert_eq!(arr.len(), 3),
            other => panic!("expected array, got {:?}", other),
        }
    }

    #[test]
    fn coerce_decimal_as_float() {
        let v = QueryValue::String("19.99".into());
        assert!(matches!(coerce_param_type(&v, "decimal"), Some(QueryValue::Float(_))));
    }

    // ── validate_query_result hard-fail tests (H10 / T2-2) ────────

    #[test]
    fn validate_query_result_missing_schema_file_errors() {
        let result = rivers_driver_sdk::types::QueryResult::empty();
        let err = validate_query_result(&result, "schemas/does_not_exist.json")
            .expect_err("missing schema file must hard-fail, not silently pass");
        match err {
            DataViewError::SchemaFileNotFound { path } => {
                assert!(
                    path.contains("schemas/does_not_exist.json"),
                    "error should reference the missing path, got: {}",
                    path
                );
            }
            other => panic!("expected SchemaFileNotFound, got {:?}", other),
        }
    }

    #[test]
    fn validate_query_result_malformed_schema_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let schema_path = dir.path().join("broken.schema.json");
        std::fs::write(&schema_path, "{not valid json").unwrap();
        let result = rivers_driver_sdk::types::QueryResult::empty();

        let err = validate_query_result(&result, schema_path.to_str().unwrap())
            .expect_err("malformed schema JSON must hard-fail, not silently pass");
        match err {
            DataViewError::SchemaFileParseError { reason, .. } => {
                // serde_json error string mentions the parse failure
                assert!(
                    !reason.is_empty(),
                    "parse error reason should be populated"
                );
            }
            other => panic!("expected SchemaFileParseError, got {:?}", other),
        }
    }

    #[test]
    fn validate_query_result_valid_schema_passes() {
        let dir = tempfile::TempDir::new().unwrap();
        let schema_path = dir.path().join("contact.schema.json");
        std::fs::write(
            &schema_path,
            r#"{"fields":[{"name":"id","required":true},{"name":"name","required":true}]}"#,
        )
        .unwrap();

        let mut row = std::collections::HashMap::new();
        row.insert("id".to_string(), QueryValue::Integer(1));
        row.insert("name".to_string(), QueryValue::String("Alice".into()));
        let result = rivers_driver_sdk::types::QueryResult {
            rows: vec![row],
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        };

        validate_query_result(&result, schema_path.to_str().unwrap())
            .expect("valid schema with all required fields should pass");
    }

    #[test]
    fn validate_query_result_missing_required_field_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let schema_path = dir.path().join("contact.schema.json");
        std::fs::write(
            &schema_path,
            r#"{"fields":[{"name":"id","required":true},{"name":"email","required":true}]}"#,
        )
        .unwrap();

        let mut row = std::collections::HashMap::new();
        row.insert("id".to_string(), QueryValue::Integer(1));
        // email missing
        let result = rivers_driver_sdk::types::QueryResult {
            rows: vec![row],
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        };

        let err = validate_query_result(&result, schema_path.to_str().unwrap())
            .expect_err("row missing a required field should fail validation");
        assert!(matches!(err, DataViewError::Schema { .. }));
    }
}

/// Validate query result rows against a schema file's required fields.
///
/// Loads the schema JSON, extracts `fields[].name` where `required = true`,
/// and checks that every result row contains those fields.
///
/// Hard-fails (H10 / T2-2) when the schema file is missing or its JSON is
/// malformed: a typo'd `return_schema` path used to silently bypass result
/// validation, serving untrusted driver output to clients. Bundle-load
/// existence checks (`validate_existence::validate_schema_files`) catch the
/// common case at validate time; this guard provides defense in depth for
/// on-disk corruption between load and request.
///
/// `schema_path` is expected to be bundle-relative (the bundle loader
/// normalizes paths before this point), so the surfaced error message does
/// not leak absolute deploy paths. The `None` case for `return_schema` is a
/// bundle-author choice and never reaches this function.
fn validate_query_result(
    result: &rivers_driver_sdk::types::QueryResult,
    schema_path: &str,
) -> Result<(), DataViewError> {
    // Load and parse schema file — hard-fail on missing or malformed.
    let schema_json = std::fs::read_to_string(schema_path).map_err(|e| {
        DataViewError::SchemaFileNotFound {
            path: format!("{}: {}", schema_path, e),
        }
    })?;
    let schema: serde_json::Value =
        serde_json::from_str(&schema_json).map_err(|e| DataViewError::SchemaFileParseError {
            path: schema_path.to_string(),
            reason: e.to_string(),
        })?;

    // Extract required field names from schema
    let required_fields: Vec<&str> = schema
        .get("fields")
        .and_then(|f| f.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter(|f| f.get("required").and_then(|r| r.as_bool()).unwrap_or(false))
                .filter_map(|f| f.get("name").and_then(|n| n.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if required_fields.is_empty() {
        return Ok(());
    }

    // Check each result row for required fields
    for (i, row) in result.rows.iter().enumerate() {
        for field in &required_fields {
            if row.get(*field).is_none() {
                return Err(DataViewError::Schema {
                    reason: format!(
                        "row {}: missing required field '{}'",
                        i, field
                    ),
                });
            }
        }
    }

    Ok(())
}
