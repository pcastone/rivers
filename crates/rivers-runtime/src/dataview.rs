//! DataView configuration and engine stub.
//!
//! Per `rivers-data-layer-spec.md` §6, §7, §12.3.
//! CRUD per-method fields per technology path spec §5, §7.2, §13.3.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

/// Configuration for a single DataView parameter.
///
/// Per spec: parameters use `[[data.dataviews.*.parameters]]` array-of-tables
/// with explicit `name` field (not named subtables).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataViewParameterConfig {
    /// Parameter name (used in query template substitution).
    pub name: String,

    /// Type name: "string", "integer", "float", "decimal", "boolean", "array", "uuid", "date". Default: "string".
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,

    /// Whether the caller must supply this parameter. Default: false.
    #[serde(default)]
    pub required: bool,

    /// Default value for this parameter when not supplied by the caller.
    #[serde(default)]
    pub default: Option<serde_json::Value>,

    /// Source location: "path", "query", "body", "header".
    /// Used by HTTP driver for outbound parameter placement.
    #[serde(default)]
    pub location: Option<String>,
}

fn default_param_type() -> String {
    "string".to_string()
}

/// DataView caching policy.
///
/// Per spec: cache uses `ttl_seconds` (integer), not `ttl`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataViewCachingConfig {
    /// Time-to-live in seconds. 0 disables caching.
    pub ttl_seconds: u64,

    /// Whether L1 in-process cache is enabled. Default: true.
    #[serde(default = "default_true")]
    pub l1_enabled: bool,

    /// Max L1 memory in bytes. Default: 150 MB.
    #[serde(default = "default_l1_max_bytes")]
    pub l1_max_bytes: usize,

    /// Hard cap on L1 entry count. Default: 100,000.
    #[serde(default = "default_l1_max_entries")]
    pub l1_max_entries: usize,

    /// Whether L2 StorageEngine-backed cache is enabled. Default: false.
    #[serde(default)]
    pub l2_enabled: bool,

    /// Max serialized value size for L2 storage in bytes. Default: 128 KB.
    #[serde(default = "default_l2_max_bytes")]
    pub l2_max_value_bytes: usize,
}

fn default_true() -> bool {
    true
}

fn default_l1_max_bytes() -> usize {
    150 * 1024 * 1024 // 150 MB
}

fn default_l1_max_entries() -> usize {
    100_000
}

fn default_l2_max_bytes() -> usize {
    131_072
}

/// Configuration for a named, parameterized DataView.
///
/// A DataView maps a logical query name to a datasource + query template.
/// The DataView engine resolves parameters, checks cache, and dispatches
/// to the pool manager at execution time.
///
/// ## Backward compatibility
///
/// The original fields `query`, `parameters`, and `return_schema` are retained
/// as backward-compatible aliases that map to their `get_*` equivalents:
///
/// - `query` -> `get_query`
/// - `return_schema` -> `get_schema`
/// - `parameters` -> `get_parameters`
///
/// When both the legacy field and the per-method field are present, the
/// per-method field takes precedence.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataViewConfig {
    /// DataView name (unique within the app).
    pub name: String,
    /// Target datasource name.
    pub datasource: String,

    // ── Legacy / backward-compatible fields ──────────────────────────
    // These map to the GET variants when per-method fields are absent.

    /// Legacy query field — aliases `get_query` for backward compatibility.
    #[serde(default)]
    pub query: Option<String>,

    /// Legacy parameters — aliases `get_parameters` for backward compatibility.
    #[serde(default)]
    pub parameters: Vec<DataViewParameterConfig>,

    /// Legacy return schema — aliases `get_schema` for backward compatibility.
    pub return_schema: Option<String>,

    // ── Per-method queries (tech path spec §5.1) ─────────────────────

    /// GET query string (overrides legacy `query`).
    #[serde(default)]
    pub get_query: Option<String>,

    /// POST query string.
    #[serde(default)]
    pub post_query: Option<String>,

    /// PUT query string.
    #[serde(default)]
    pub put_query: Option<String>,

    /// DELETE query string.
    #[serde(default)]
    pub delete_query: Option<String>,

    // ── Per-method schemas (tech path spec §7.2) ─────────────────────

    /// GET return schema path (overrides legacy `return_schema`).
    #[serde(default)]
    pub get_schema: Option<String>,

    /// POST return schema path.
    #[serde(default)]
    pub post_schema: Option<String>,

    /// PUT return schema path.
    #[serde(default)]
    pub put_schema: Option<String>,

    /// DELETE return schema path.
    #[serde(default)]
    pub delete_schema: Option<String>,

    // ── Per-method parameters (tech path spec §5.2) ──────────────────

    /// GET-specific parameters (overrides legacy `parameters` if non-empty).
    #[serde(default)]
    pub get_parameters: Vec<DataViewParameterConfig>,

    /// POST-specific parameters.
    #[serde(default)]
    pub post_parameters: Vec<DataViewParameterConfig>,

    /// PUT-specific parameters.
    #[serde(default)]
    pub put_parameters: Vec<DataViewParameterConfig>,

    /// DELETE-specific parameters.
    #[serde(default)]
    pub delete_parameters: Vec<DataViewParameterConfig>,

    // ── Streaming support (tech path spec §13.3) ─────────────────────

    /// When true, this DataView supports streaming responses.
    #[serde(default)]
    pub streaming: bool,

    /// Optional circuit breaker ID. DataViews sharing the same ID are tripped/reset together.
    #[serde(default, rename = "circuitBreakerId")]
    pub circuit_breaker_id: Option<String>,

    /// Enable prepared statement caching for this DataView's queries.
    #[serde(default)]
    pub prepared: bool,

    /// Static query parameters appended to every outbound HTTP driver request.
    #[serde(default)]
    pub query_params: HashMap<String, String>,

    // ── Existing flags ───────────────────────────────────────────────

    /// Per-view caching policy (L1/L2 tiered cache).
    #[serde(default)]
    pub caching: Option<DataViewCachingConfig>,

    /// List of DataView names whose cache entries should be invalidated
    /// when this DataView executes successfully.
    ///
    /// Typically configured on write DataViews to invalidate read caches.
    /// Empty by default — no invalidation occurs.
    #[serde(default)]
    pub invalidates: Vec<String>,

    /// Whether to validate results against the active schema.
    #[serde(default)]
    pub validate_result: bool,

    /// When true, reject unknown parameters instead of ignoring them.
    #[serde(default)]
    pub strict_parameters: bool,

    /// Maximum rows returned from a query. 0 = no limit. Default: 1000.
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,

    /// When true, skip schema introspection for this DataView at startup.
    /// Use for mutation DataViews (INSERT/UPDATE/DELETE) whose queries cannot
    /// be wrapped in a LIMIT 0 subquery.
    #[serde(default)]
    pub skip_introspect: bool,
}

fn default_max_rows() -> usize {
    1000
}

impl DataViewConfig {
    /// Return the query string for the given HTTP method.
    ///
    /// Resolution order:
    /// 1. Per-method field (`get_query`, `post_query`, etc.)
    /// 2. Legacy `query` field (only for GET)
    pub fn query_for_method(&self, method: &str) -> Option<&str> {
        match method.to_uppercase().as_str() {
            "GET" => self
                .get_query
                .as_deref()
                .or(self.query.as_deref()),
            "POST" => self.post_query.as_deref(),
            "PUT" => self.put_query.as_deref(),
            "DELETE" => self.delete_query.as_deref(),
            _ => None,
        }
    }

    /// Return the schema path for the given HTTP method.
    ///
    /// Resolution order:
    /// 1. Per-method field (`get_schema`, `post_schema`, etc.)
    /// 2. Legacy `return_schema` field (only for GET)
    pub fn schema_for_method(&self, method: &str) -> Option<&str> {
        match method.to_uppercase().as_str() {
            "GET" => self
                .get_schema
                .as_deref()
                .or(self.return_schema.as_deref()),
            "POST" => self.post_schema.as_deref(),
            "PUT" => self.put_schema.as_deref(),
            "DELETE" => self.delete_schema.as_deref(),
            _ => None,
        }
    }

    /// Return the parameter list for the given HTTP method.
    ///
    /// Resolution order:
    /// 1. Per-method field (`get_parameters`, `post_parameters`, etc.) if non-empty
    /// 2. Legacy `parameters` field (only for GET)
    pub fn parameters_for_method(&self, method: &str) -> &[DataViewParameterConfig] {
        match method.to_uppercase().as_str() {
            "GET" => {
                if !self.get_parameters.is_empty() {
                    &self.get_parameters
                } else {
                    &self.parameters
                }
            }
            "POST" => &self.post_parameters,
            "PUT" => &self.put_parameters,
            "DELETE" => &self.delete_parameters,
            _ => &[],
        }
    }
}

/// The DataView engine — registry of DataViews, parameter validation,
/// caching, and dispatch to pool manager.
///
/// Stub implementation — Epic 10 (DataView Engine) builds the real one.
pub struct DataViewEngine {
    dataviews: Vec<DataViewConfig>,
}

impl DataViewEngine {
    /// Create a new empty DataView engine.
    pub fn new() -> Self {
        Self {
            dataviews: Vec::new(),
        }
    }

    /// Register a DataView configuration.
    pub fn register(&mut self, config: DataViewConfig) {
        self.dataviews.push(config);
    }

    /// Return the number of registered DataViews.
    pub fn count(&self) -> usize {
        self.dataviews.len()
    }
}

impl Default for DataViewEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataview_config_parses_circuit_breaker_id() {
        let toml_str = r#"
            name = "test"
            datasource = "ds"
            circuitBreakerId = "Warehouse_Transaction"
        "#;
        let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.circuit_breaker_id.as_deref(), Some("Warehouse_Transaction"));
    }

    #[test]
    fn dataview_config_circuit_breaker_id_optional() {
        let toml_str = r#"
            name = "test"
            datasource = "ds"
        "#;
        let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.circuit_breaker_id.is_none());
    }

    #[test]
    fn dataview_config_parses_prepared() {
        let toml_str = r#"
            name = "test"
            datasource = "ds"
            prepared = true
        "#;
        let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.prepared);
    }

    #[test]
    fn dataview_config_prepared_defaults_false() {
        let toml_str = r#"
            name = "test"
            datasource = "ds"
        "#;
        let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.prepared);
    }
}
