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
use std::time::Instant;

use rivers_core::DriverFactory;
use rivers_driver_sdk::broker::{BrokerConsumerConfig, OutboundMessage};
use rivers_driver_sdk::error::DriverError;
use rivers_driver_sdk::types::{Query, QueryResult, QueryValue};
use rivers_driver_sdk::ConnectionParams;

use crate::dataview::DataViewConfig;
use crate::tiered_cache::{cache_key, DataViewCache};

// ── DataView Errors ───────────────────────────────────────────────

/// Errors from DataView operations.
#[derive(Debug, thiserror::Error)]
pub enum DataViewError {
    #[error("dataview not found: '{name}'")]
    NotFound { name: String },

    #[error("missing required parameter '{name}' for dataview '{dataview}'")]
    MissingParameter { name: String, dataview: String },

    #[error("parameter type mismatch for '{name}': expected {expected}, got {actual}")]
    ParameterTypeMismatch {
        name: String,
        expected: String,
        actual: String,
    },

    #[error("unknown parameter '{name}' for dataview '{dataview}' (strict mode)")]
    UnknownParameter { name: String, dataview: String },

    #[error("schema validation failed: {reason}")]
    Schema { reason: String },

    #[error("pool error: {0}")]
    Pool(String),

    #[error("driver error: {0}")]
    Driver(String),

    #[error("invalid request: {reason}")]
    InvalidRequest { reason: String },

    #[error("unsupported schema attribute: {attribute} for driver {driver}")]
    UnsupportedSchemaAttribute { attribute: String, driver: String },

    #[error("schema file not found: {path}")]
    SchemaFileNotFound { path: String },

    #[error("schema file parse error: {path}: {reason}")]
    SchemaFileParseError { path: String, reason: String },

    #[error("unknown faker method: {method}")]
    UnknownFakerMethod { method: String },

    #[error("cache error: {0}")]
    Cache(String),
}

// ── DataView Request / Response ───────────────────────────────────

/// A request to execute a DataView.
///
/// Per spec §6.3.
#[derive(Debug, Clone)]
pub struct DataViewRequest {
    pub name: String,
    pub method: String,
    pub parameters: HashMap<String, QueryValue>,
    pub timeout_ms: Option<u64>,
    pub trace_id: String,
    pub cache_bypass: bool,
}

/// Response from a DataView execution.
///
/// Per spec §6.3.
#[derive(Debug, Clone)]
pub struct DataViewResponse {
    pub query_result: QueryResult,
    pub execution_time_ms: u64,
    pub cache_hit: bool,
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
fn json_value_to_query_value(val: &serde_json::Value, param_type: &str) -> Option<QueryValue> {
    match param_type.to_lowercase().as_str() {
        "string" => val.as_str().map(|s| QueryValue::String(s.to_string())),
        "integer" => val.as_i64().map(QueryValue::Integer),
        "float" => val.as_f64().map(QueryValue::Float),
        "boolean" => val.as_bool().map(QueryValue::Boolean),
        _ => Some(QueryValue::String(val.to_string())),
    }
}

pub fn zero_value_for_type(param_type: &str) -> QueryValue {
    match param_type.to_lowercase().as_str() {
        "string" => QueryValue::String(String::new()),
        "integer" => QueryValue::Integer(0),
        "float" => QueryValue::Float(0.0),
        "boolean" => QueryValue::Boolean(false),
        "array" => QueryValue::Array(Vec::new()),
        _ => QueryValue::Null,
    }
}

/// Check if a QueryValue matches an expected parameter type.
pub fn matches_param_type(value: &QueryValue, param_type: &str) -> bool {
    match param_type.to_lowercase().as_str() {
        "string" => matches!(value, QueryValue::String(_)),
        "integer" => matches!(value, QueryValue::Integer(_)),
        "float" => matches!(value, QueryValue::Float(_)),
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
        (QueryValue::String(s), "float") => s.parse::<f64>().ok().map(QueryValue::Float),
        (QueryValue::String(s), "boolean") => match s.as_str() {
            "true" | "1" => Some(QueryValue::Boolean(true)),
            "false" | "0" => Some(QueryValue::Boolean(false)),
            _ => None,
        },
        // Float → Integer (truncate if lossless)
        (QueryValue::Float(f), "integer") => {
            let i = *f as i64;
            if (i as f64 - f).abs() < f64::EPSILON { Some(QueryValue::Integer(i)) } else { None }
        }
        // Integer → Float (always lossless for reasonable values)
        (QueryValue::Integer(i), "float") => Some(QueryValue::Float(*i as f64)),
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
    query_result: QueryResult,
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
    /// Optional DataView cache (L1/L2 tiered).
    cache: Option<Arc<dyn DataViewCache>>,
    /// Optional EventBus for cache invalidation events.
    event_bus: Option<Arc<rivers_core::EventBus>>,
}

impl DataViewExecutor {
    /// Create a new executor with a registry, driver factory, datasource params, and optional cache.
    pub fn new(
        registry: DataViewRegistry,
        factory: Arc<DriverFactory>,
        datasource_params: Arc<HashMap<String, ConnectionParams>>,
        cache: Option<Arc<dyn DataViewCache>>,
    ) -> Self {
        Self {
            registry,
            factory,
            datasource_params,
            cache,
            event_bus: None,
        }
    }

    /// Set the EventBus for cache invalidation events.
    pub fn set_event_bus(&mut self, event_bus: Arc<rivers_core::EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// Execute a named DataView with the given parameters.
    ///
    /// Flow: registry lookup → param validation → build query →
    /// factory.connect() → conn.execute() → return QueryResult.
    pub async fn execute(
        &self,
        name: &str,
        params: HashMap<String, QueryValue>,
        trace_id: &str,
    ) -> Result<DataViewResponse, DataViewError> {
        let start = Instant::now();

        // 1. Registry lookup
        let config = self
            .registry
            .get(name)
            .ok_or_else(|| DataViewError::NotFound {
                name: name.to_string(),
            })?;

        // 2. Parameter validation
        let request = DataViewRequestBuilder::new(name)
            .params(params)
            .trace_id(trace_id)
            .build_for(config)?;

        // 3. Cache check — skip entirely if view has no caching config
        let view_caching = config.caching.as_ref();
        if let Some(ref cache) = self.cache {
            if !request.cache_bypass && view_caching.is_some() {
                let key = cache_key(name, &request.parameters);
                match cache.get(name, &request.parameters).await {
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
        }

        // 4. Build query from config + validated params
        let query = build_query(config, &request.parameters, &request.method);

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

        // Try database driver first; if unknown, fall back to broker driver
        match self.factory.connect(driver_name, ds_params).await {
            Ok(mut conn) => {
                // 6. Execute query via database connection
                let query_result = conn
                    .execute(&query)
                    .await
                    .map_err(|e| DataViewError::Driver(e.to_string()))?;

                // 7. Cache populate on success (unless bypass or no caching config)
                if let Some(ref cache) = self.cache {
                    if !request.cache_bypass && view_caching.is_some() {
                        let ttl_override = view_caching.map(|c| c.ttl_seconds);
                        if let Err(e) = cache.set(name, &request.parameters, &query_result, ttl_override).await {
                            tracing::warn!(dataview = %name, error = %e, "cache set failed");
                        }
                    }
                }

                // 8. Cache invalidation — invalidate listed DataViews on success
                self.run_cache_invalidation(name, &config.invalidates, trace_id).await;

                Ok(build_response(query_result, start, false, trace_id.to_string()))
            }
            Err(DriverError::UnknownDriver(_)) => {
                // 6b. Try broker driver → produce path
                let invalidates = config.invalidates.clone();
                let response = self.execute_broker_produce(driver_name, ds_params, &query, start, trace_id).await?;
                self.run_cache_invalidation(name, &invalidates, trace_id).await;
                Ok(response)
            }
            Err(e) => Err(DataViewError::Pool(format!("connection failed: {e}"))),
        }
    }

    /// Invalidate cache entries for listed DataViews and emit EventBus event.
    ///
    /// Called after successful write DataView execution when `config.invalidates` is non-empty.
    async fn run_cache_invalidation(&self, source_view: &str, invalidates: &[String], trace_id: &str) {
        if invalidates.is_empty() {
            return;
        }
        if let Some(ref cache) = self.cache {
            for target_view in invalidates {
                cache.invalidate(Some(target_view.as_str())).await;
                tracing::info!(
                    source = %source_view,
                    target = %target_view,
                    "cache invalidated"
                );
            }
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
        };

        Ok(build_response(query_result, start, false, trace_id.to_string()))
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &DataViewRegistry {
        &self.registry
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

    /// List all configured datasource names.
    pub fn datasource_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.datasource_params.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }
}
