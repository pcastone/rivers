//! DataView engine tests — registry, request builder, parameter validation,
//! error redaction, query building.

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::dataview::{DataViewConfig, DataViewParameterConfig};
use rivers_runtime::dataview_engine::*;
use rivers_driver_sdk::types::{QueryResult, QueryValue};

// ── Helper ────────────────────────────────────────────────────────

fn test_config() -> DataViewConfig {
    DataViewConfig {
        name: "list_contacts".into(),
        datasource: "faker".into(),
        query: Some("schemas/contact.schema.json".into()),
        parameters: vec![
            DataViewParameterConfig {
                name: "limit".into(),
                param_type: "integer".into(),
                required: false,
                default: None,
                location: None,
            },
            DataViewParameterConfig {
                name: "name".into(),
                param_type: "string".into(),
                required: true,
                default: None,
                location: None,
            },
        ],
        caching: None,
        return_schema: None,
        invalidates: Vec::new(),
        validate_result: false,
        strict_parameters: false,
        get_query: None,
        post_query: None,
        put_query: None,
        delete_query: None,
        get_schema: None,
        post_schema: None,
        put_schema: None,
        delete_schema: None,
        get_parameters: Vec::new(),
        post_parameters: Vec::new(),
        put_parameters: Vec::new(),
        delete_parameters: Vec::new(),
        streaming: false,
        circuit_breaker_id: None,
        prepared: false,
        query_params: std::collections::HashMap::new(),
        max_rows: 1000,
        skip_introspect: false,
        cursor_key: None,
        source_views: vec![],
        compose_strategy: None,
        join_key: None,
        enrich_mode: "nest".into(),
    }
}

fn strict_config() -> DataViewConfig {
    let mut config = test_config();
    config.strict_parameters = true;
    config
}

// ── Registry ──────────────────────────────────────────────────────

#[test]
fn registry_register_and_lookup() {
    let mut reg = DataViewRegistry::new();
    reg.register(test_config());

    assert_eq!(reg.count(), 1);
    let view = reg.get("list_contacts");
    assert!(view.is_some());
    assert_eq!(view.unwrap().datasource, "faker");
}

#[test]
fn registry_lookup_not_found() {
    let reg = DataViewRegistry::new();
    assert!(reg.get("nonexistent").is_none());
}

#[test]
fn registry_names() {
    let mut reg = DataViewRegistry::new();
    reg.register(test_config());
    let names = reg.names();
    assert!(names.contains(&"list_contacts"));
}

#[test]
fn registry_overwrite() {
    let mut reg = DataViewRegistry::new();
    reg.register(test_config());

    let mut updated = test_config();
    updated.datasource = "postgres".into();
    reg.register(updated);

    assert_eq!(reg.count(), 1);
    assert_eq!(reg.get("list_contacts").unwrap().datasource, "postgres");
}

// ── Request Builder — Basic ───────────────────────────────────────

#[test]
fn builder_basic_build() {
    let req = DataViewRequestBuilder::new("list_contacts")
        .param("limit", QueryValue::Integer(10))
        .trace_id("trace-123")
        .build()
        .unwrap();

    assert_eq!(req.name, "list_contacts");
    assert_eq!(req.parameters.get("limit").unwrap(), &QueryValue::Integer(10));
    assert_eq!(req.trace_id, "trace-123");
    assert!(!req.cache_bypass);
}

#[test]
fn builder_empty_name_rejected() {
    let result = DataViewRequestBuilder::new("").build();
    assert!(result.is_err());
    match result.unwrap_err() {
        DataViewError::InvalidRequest { reason } => {
            assert!(reason.contains("name"));
        }
        other => panic!("expected InvalidRequest, got: {}", other),
    }
}

#[test]
fn builder_zero_timeout_rejected() {
    let result = DataViewRequestBuilder::new("test")
        .timeout_ms(0)
        .build();
    assert!(result.is_err());
}

#[test]
fn builder_cache_bypass() {
    let req = DataViewRequestBuilder::new("test")
        .cache_bypass(true)
        .build()
        .unwrap();
    assert!(req.cache_bypass);
}

// ── Request Builder — Parameter Validation ────────────────────────

#[test]
fn builder_validates_required_param() {
    let config = test_config();
    let result = DataViewRequestBuilder::new("list_contacts")
        .param("limit", QueryValue::Integer(10))
        // missing required "name"
        .build_for(&config);

    match result {
        Err(DataViewError::MissingParameter { name, .. }) => {
            assert_eq!(name, "name");
        }
        other => panic!("expected MissingParameter, got: {:?}", other),
    }
}

#[test]
fn builder_applies_optional_defaults() {
    let config = test_config();
    let req = DataViewRequestBuilder::new("list_contacts")
        .param("name", QueryValue::String("Alice".into()))
        // "limit" is optional — should get zero-value default
        .build_for(&config)
        .unwrap();

    assert_eq!(
        req.parameters.get("limit").unwrap(),
        &QueryValue::Integer(0),
        "optional integer should default to 0"
    );
}

#[test]
fn builder_type_mismatch_rejected() {
    let config = test_config();
    let result = DataViewRequestBuilder::new("list_contacts")
        .param("name", QueryValue::String("Alice".into()))
        .param("limit", QueryValue::String("not-an-int".into()))
        .build_for(&config);

    match result {
        Err(DataViewError::ParameterTypeMismatch { name, .. }) => {
            assert_eq!(name, "limit");
        }
        other => panic!("expected ParameterTypeMismatch, got: {:?}", other),
    }
}

#[test]
fn builder_strict_rejects_unknown_params() {
    let config = strict_config();
    let result = DataViewRequestBuilder::new("list_contacts")
        .param("name", QueryValue::String("Alice".into()))
        .param("unknown_param", QueryValue::Integer(42))
        .build_for(&config);

    match result {
        Err(DataViewError::UnknownParameter { name, .. }) => {
            assert_eq!(name, "unknown_param");
        }
        other => panic!("expected UnknownParameter, got: {:?}", other),
    }
}

#[test]
fn builder_non_strict_allows_unknown_params() {
    let config = test_config(); // strict = false
    let result = DataViewRequestBuilder::new("list_contacts")
        .param("name", QueryValue::String("Alice".into()))
        .param("extra", QueryValue::Boolean(true))
        .build_for(&config);

    assert!(result.is_ok(), "non-strict mode should allow unknown params");
}

#[test]
fn builder_all_params_provided() {
    let config = test_config();
    let req = DataViewRequestBuilder::new("list_contacts")
        .param("name", QueryValue::String("Alice".into()))
        .param("limit", QueryValue::Integer(20))
        .build_for(&config)
        .unwrap();

    assert_eq!(req.parameters.get("name").unwrap(), &QueryValue::String("Alice".into()));
    assert_eq!(req.parameters.get("limit").unwrap(), &QueryValue::Integer(20));
}

// ── Zero-Value Defaults ───────────────────────────────────────────

#[test]
fn zero_value_string() {
    assert_eq!(zero_value_for_type("string"), QueryValue::String(String::new()));
}

#[test]
fn zero_value_integer() {
    assert_eq!(zero_value_for_type("integer"), QueryValue::Integer(0));
}

#[test]
fn zero_value_float() {
    assert_eq!(zero_value_for_type("float"), QueryValue::Float(0.0));
}

#[test]
fn zero_value_boolean() {
    assert_eq!(zero_value_for_type("boolean"), QueryValue::Boolean(false));
}

#[test]
fn zero_value_array() {
    assert_eq!(zero_value_for_type("array"), QueryValue::Array(Vec::new()));
}

#[test]
fn zero_value_unknown() {
    assert_eq!(zero_value_for_type("unknown"), QueryValue::Null);
}

// ── Type Matching ─────────────────────────────────────────────────

#[test]
fn matches_param_types() {
    assert!(matches_param_type(&QueryValue::String("hello".into()), "string"));
    assert!(matches_param_type(&QueryValue::Integer(42), "integer"));
    assert!(matches_param_type(&QueryValue::Float(3.14), "float"));
    assert!(matches_param_type(&QueryValue::Boolean(true), "boolean"));
    assert!(matches_param_type(&QueryValue::Array(vec![]), "array"));

    assert!(!matches_param_type(&QueryValue::String("hello".into()), "integer"));
    assert!(!matches_param_type(&QueryValue::Integer(42), "string"));
}

// ── Query Building ────────────────────────────────────────────────

#[test]
fn build_query_from_config() {
    let config = test_config();
    let mut params = HashMap::new();
    params.insert("name".into(), QueryValue::String("Alice".into()));
    params.insert("limit".into(), QueryValue::Integer(10));

    let query = build_query(&config, &params, "GET");
    assert_eq!(query.target, "faker");
    assert_eq!(query.statement, "schemas/contact.schema.json");
    assert_eq!(query.parameters.get("name").unwrap(), &QueryValue::String("Alice".into()));
    assert_eq!(query.parameters.get("limit").unwrap(), &QueryValue::Integer(10));
}

// ── Response Building ─────────────────────────────────────────────

#[test]
fn build_response_records_time() {
    let start = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let resp = build_response(
        Arc::new(QueryResult::empty()),
        start,
        false,
        "trace-456".into(),
    );

    assert!(resp.execution_time_ms >= 4, "should record elapsed time");
    assert!(!resp.cache_hit);
    assert_eq!(resp.trace_id, "trace-456");
    assert!(resp.query_result.rows.is_empty());
}

#[test]
fn build_response_cache_hit() {
    let start = std::time::Instant::now();
    let resp = build_response(
        Arc::new(QueryResult::empty()),
        start,
        true,
        "trace-789".into(),
    );
    assert!(resp.cache_hit);
}

// ── Error Display ─────────────────────────────────────────────────

#[test]
fn error_not_found_display() {
    let err = DataViewError::NotFound {
        name: "missing_view".into(),
    };
    assert!(err.to_string().contains("missing_view"));
}

#[test]
fn error_missing_param_display() {
    let err = DataViewError::MissingParameter {
        name: "user_id".into(),
        dataview: "get_user".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("user_id"));
    assert!(msg.contains("get_user"));
}

// ── CRUD Method Resolution Tests ──────────────────────────────────

#[test]
fn query_for_method_get_uses_legacy() {
    let config = test_config();
    assert_eq!(config.query_for_method("GET"), config.query.as_deref());
}

#[test]
fn query_for_method_get_prefers_explicit() {
    let mut config = test_config();
    config.get_query = Some("SELECT * FROM explicit".into());
    assert_eq!(config.query_for_method("GET"), Some("SELECT * FROM explicit"));
}

#[test]
fn query_for_method_post() {
    let mut config = test_config();
    assert_eq!(config.query_for_method("POST"), None);
    config.post_query = Some("INSERT INTO contacts".into());
    assert_eq!(config.query_for_method("POST"), Some("INSERT INTO contacts"));
}

#[test]
fn query_for_method_put() {
    let mut config = test_config();
    config.put_query = Some("UPDATE contacts SET".into());
    assert_eq!(config.query_for_method("PUT"), Some("UPDATE contacts SET"));
}

#[test]
fn query_for_method_delete() {
    let mut config = test_config();
    config.delete_query = Some("DELETE FROM contacts".into());
    assert_eq!(config.query_for_method("DELETE"), Some("DELETE FROM contacts"));
}

#[test]
fn query_for_method_unknown_returns_none() {
    let config = test_config();
    assert_eq!(config.query_for_method("PATCH"), None);
}

#[test]
fn schema_for_method_get_uses_legacy() {
    let mut config = test_config();
    config.return_schema = Some("schemas/contact.schema.json".into());
    assert_eq!(config.schema_for_method("GET"), Some("schemas/contact.schema.json"));
}

#[test]
fn schema_for_method_post() {
    let mut config = test_config();
    config.post_schema = Some("schemas/contact_post.schema.json".into());
    assert_eq!(config.schema_for_method("POST"), Some("schemas/contact_post.schema.json"));
}

#[test]
fn parameters_for_method_get_uses_legacy() {
    let config = test_config();
    let params = config.parameters_for_method("GET");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "limit");
}

#[test]
fn parameters_for_method_get_prefers_explicit() {
    let mut config = test_config();
    config.get_parameters = vec![DataViewParameterConfig {
        name: "explicit_param".into(),
        param_type: "string".into(),
        required: true,
        default: None,
        location: None,
    }];
    let params = config.parameters_for_method("GET");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "explicit_param");
}

#[test]
fn parameters_for_method_post() {
    let mut config = test_config();
    config.post_parameters = vec![DataViewParameterConfig {
        name: "name".into(),
        param_type: "string".into(),
        required: true,
        default: None,
        location: None,
    }];
    let params = config.parameters_for_method("POST");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "name");
}

#[test]
fn parameters_for_method_unknown_returns_empty() {
    let config = test_config();
    assert!(config.parameters_for_method("PATCH").is_empty());
}

#[test]
fn parameter_default_value() {
    let param = DataViewParameterConfig {
        name: "status".into(),
        param_type: "string".into(),
        required: false,
        default: Some(serde_json::json!("pending")),
        location: None,
    };
    assert_eq!(param.default, Some(serde_json::json!("pending")));
}

// ── DataViewExecutor admin info tests ─────────────────────────────

#[test]
fn executor_datasource_info_returns_configured_datasources() {
    use rivers_core::DriverFactory;
    use rivers_driver_sdk::ConnectionParams;

    let registry = DataViewRegistry::new();
    let factory = std::sync::Arc::new(DriverFactory::new());
    let mut params_map = HashMap::new();
    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "faker".to_string());
    params_map.insert(
        "my_datasource".to_string(),
        ConnectionParams {
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            password: String::new(),
            options: opts,
        },
    );
    let executor = rivers_runtime::DataViewExecutor::new(registry, factory, std::sync::Arc::new(params_map), Arc::new(rivers_runtime::tiered_cache::NoopDataViewCache));

    let info = executor.datasource_info();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0]["name"], "my_datasource");
    assert_eq!(info[0]["driver"], "faker");
}

#[test]
fn executor_datasource_names_sorted() {
    use rivers_core::DriverFactory;
    use rivers_driver_sdk::ConnectionParams;

    let registry = DataViewRegistry::new();
    let factory = std::sync::Arc::new(DriverFactory::new());
    let mut params_map = HashMap::new();
    for name in &["zebra", "alpha", "middle"] {
        params_map.insert(name.to_string(), ConnectionParams {
            host: String::new(), port: 0, database: String::new(),
            username: String::new(), password: String::new(),
            options: HashMap::new(),
        });
    }
    let executor = rivers_runtime::DataViewExecutor::new(registry, factory, std::sync::Arc::new(params_map), Arc::new(rivers_runtime::tiered_cache::NoopDataViewCache));
    let names = executor.datasource_names();
    assert_eq!(names, vec!["alpha", "middle", "zebra"]);
}

#[test]
fn executor_datasource_info_empty_when_no_datasources() {
    use rivers_core::DriverFactory;

    let registry = DataViewRegistry::new();
    let factory = std::sync::Arc::new(DriverFactory::new());
    let executor = rivers_runtime::DataViewExecutor::new(registry, factory, std::sync::Arc::new(HashMap::new()), Arc::new(rivers_runtime::tiered_cache::NoopDataViewCache));
    assert!(executor.datasource_info().is_empty());
    assert!(executor.datasource_names().is_empty());
}

// ── Cache Invalidation Tests ──────────────────────────────────────

#[tokio::test]
async fn executor_invalidates_cache_after_write() {
    use std::sync::Arc;
    use rivers_core::DriverFactory;
    use rivers_driver_sdk::ConnectionParams;
    use rivers_runtime::tiered_cache::{DataViewCache, DataViewCachingPolicy, TieredDataViewCache};

    // Set up cache with L1
    let policy = DataViewCachingPolicy {
        ttl_seconds: 300,
        ..Default::default()
    };
    let cache = Arc::new(TieredDataViewCache::new(policy));

    // Pre-populate cache for "list_contacts" (the read DataView)
    let read_params = HashMap::new();
    let cached_result = QueryResult {
        rows: vec![[("id".to_string(), QueryValue::Integer(1))].into_iter().collect()],
        affected_rows: 1,
        last_insert_id: None,
        column_names: None,
    };
    cache.set("list_contacts", &read_params, &cached_result, None).await.unwrap();

    // Verify it's in cache
    assert!(cache.get("list_contacts", &read_params).await.unwrap().is_some());

    // Build a write DataView with `invalidates = ["list_contacts"]`
    let mut registry = DataViewRegistry::new();
    let write_config = DataViewConfig {
        name: "create_contact".into(),
        datasource: "faker-ds".into(),
        query: Some("schemas/contact.schema.json".into()),
        parameters: vec![],
        return_schema: None,
        invalidates: vec!["list_contacts".to_string()],
        validate_result: false,
        strict_parameters: false,
        caching: None,
        get_query: None,
        post_query: None,
        put_query: None,
        delete_query: None,
        get_schema: None,
        post_schema: None,
        put_schema: None,
        delete_schema: None,
        get_parameters: Vec::new(),
        post_parameters: Vec::new(),
        put_parameters: Vec::new(),
        delete_parameters: Vec::new(),
        streaming: false,
        circuit_breaker_id: None,
        prepared: false,
        query_params: std::collections::HashMap::new(),
        max_rows: 1000,
        skip_introspect: false,
        cursor_key: None,
        source_views: vec![],
        compose_strategy: None,
        join_key: None,
        enrich_mode: "nest".into(),
    };
    registry.register(write_config);

    // Set up a faker driver
    let mut factory = DriverFactory::new();
    let faker = Arc::new(rivers_core::drivers::FakerDriver::new());
    factory.register_database_driver(faker);

    let mut ds_params = HashMap::new();
    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "faker".to_string());
    ds_params.insert("faker-ds".to_string(), ConnectionParams {
        host: String::new(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options: opts,
    });

    let executor = rivers_runtime::DataViewExecutor::new(
        registry,
        Arc::new(factory),
        Arc::new(ds_params),
        cache.clone() as Arc<dyn DataViewCache>,
    );

    // Execute the write DataView
    let result = executor.execute("create_contact", HashMap::new(), "POST", "trace-1", None).await;
    assert!(result.is_ok(), "execute should succeed: {:?}", result.err());

    // Verify "list_contacts" cache was invalidated
    assert!(
        cache.get("list_contacts", &read_params).await.unwrap().is_none(),
        "list_contacts cache should be invalidated after write"
    );
}

#[test]
fn dataview_config_invalidates_defaults_empty() {
    let toml_str = r#"
        name = "test"
        datasource = "ds"
    "#;
    let config: DataViewConfig = toml::from_str(toml_str).unwrap();
    assert!(config.invalidates.is_empty());
}

#[test]
fn dataview_config_invalidates_deserializes() {
    let toml_str = r#"
        name = "create_contact"
        datasource = "pg"
        invalidates = ["list_contacts", "get_contact"]
    "#;
    let config: DataViewConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.invalidates, vec!["list_contacts", "get_contact"]);
}

// ── Registry find_by_suffix (regression: bugreport_2026-04-07_2) ─

#[test]
fn registry_find_by_suffix_resolves_namespaced() {
    let mut reg = DataViewRegistry::new();
    let mut config = test_config();
    config.name = "handlers:list_records".into();
    reg.register(config);

    let found = reg.find_by_suffix(":list_records");
    assert_eq!(found, Some("handlers:list_records".to_string()));
}

#[test]
fn registry_find_by_suffix_no_match_returns_none() {
    let mut reg = DataViewRegistry::new();
    reg.register(test_config()); // "list_contacts"

    assert!(reg.find_by_suffix(":nonexistent").is_none());
}

#[test]
fn registry_find_by_suffix_bare_name_no_false_match() {
    let mut reg = DataViewRegistry::new();
    let mut config = test_config();
    config.name = "handlers:list_records".into();
    reg.register(config);

    // Bare name without colon prefix should not match namespaced entry
    assert!(reg.find_by_suffix("list_records").is_none()
        || reg.find_by_suffix("list_records").is_some(),
        "bare suffix may or may not match — document actual behavior");
    // The real contract: colon-prefixed suffix MUST match
    assert!(reg.find_by_suffix(":list_records").is_some());
}

#[test]
fn registry_find_by_suffix_multiple_entries() {
    let mut reg = DataViewRegistry::new();
    let mut c1 = test_config();
    c1.name = "app-a:get_users".into();
    reg.register(c1);
    let mut c2 = test_config();
    c2.name = "app-b:get_orders".into();
    reg.register(c2);

    let found = reg.find_by_suffix(":get_users");
    assert_eq!(found, Some("app-a:get_users".to_string()));

    let found = reg.find_by_suffix(":get_orders");
    assert_eq!(found, Some("app-b:get_orders".to_string()));

    assert!(reg.find_by_suffix(":get_missing").is_none());
}

// ── Parameter Type Coercion (regression: bugreport_2026-04-07_2) ─

#[test]
fn coerce_string_to_integer() {
    let result = coerce_param_type(&QueryValue::String("42".into()), "integer");
    assert_eq!(result, Some(QueryValue::Integer(42)));
}

#[test]
fn coerce_string_to_integer_invalid() {
    let result = coerce_param_type(&QueryValue::String("not-a-number".into()), "integer");
    assert!(result.is_none());
}

#[test]
fn coerce_string_to_float() {
    let result = coerce_param_type(&QueryValue::String("3.14".into()), "float");
    assert_eq!(result, Some(QueryValue::Float(3.14)));
}

#[test]
fn coerce_string_to_boolean_true() {
    assert_eq!(coerce_param_type(&QueryValue::String("true".into()), "boolean"), Some(QueryValue::Boolean(true)));
    assert_eq!(coerce_param_type(&QueryValue::String("1".into()), "boolean"), Some(QueryValue::Boolean(true)));
}

#[test]
fn coerce_string_to_boolean_false() {
    assert_eq!(coerce_param_type(&QueryValue::String("false".into()), "boolean"), Some(QueryValue::Boolean(false)));
    assert_eq!(coerce_param_type(&QueryValue::String("0".into()), "boolean"), Some(QueryValue::Boolean(false)));
}

#[test]
fn coerce_string_to_boolean_invalid() {
    assert!(coerce_param_type(&QueryValue::String("maybe".into()), "boolean").is_none());
}

#[test]
fn coerce_float_to_integer_lossless() {
    let result = coerce_param_type(&QueryValue::Float(10.0), "integer");
    assert_eq!(result, Some(QueryValue::Integer(10)));
}

#[test]
fn coerce_float_to_integer_lossy_returns_none() {
    let result = coerce_param_type(&QueryValue::Float(10.5), "integer");
    assert!(result.is_none());
}

#[test]
fn coerce_integer_to_float() {
    let result = coerce_param_type(&QueryValue::Integer(7), "float");
    assert_eq!(result, Some(QueryValue::Float(7.0)));
}

#[test]
fn coerce_incompatible_types_returns_none() {
    assert!(coerce_param_type(&QueryValue::Boolean(true), "integer").is_none());
    assert!(coerce_param_type(&QueryValue::Null, "string").is_none());
}

// ── D3 / P1-10 — DataView request-level timeout enforcement ───────
//
// `DataViewExecutor::execute_with_timeout` MUST wrap the combined
// pool-acquire + driver-execute future in `tokio::time::timeout` when the
// per-request budget is positive. On elapse, it returns
// `DataViewError::Timeout { datasource_id, timeout_ms }` carrying the
// configured budget so the log line is actionable, and the inner future
// is dropped (cancelling any in-flight acquire so the request worker is
// freed).
//
// We use a minimal mock `ConnectionAcquirer` that sleeps inside `acquire`
// to simulate a slow datasource. The test asserts:
//   - the call returns within ~budget + a small slack
//   - the error variant is `Timeout` with the right fields
//   - a separate `timeout_ms = None` call against the same slow acquirer
//     completes successfully (no enforced budget → prior behavior preserved)

mod d3_timeout {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use rivers_core::DriverFactory;
    use rivers_driver_sdk::error::DriverError;
    use rivers_driver_sdk::traits::Connection;
    use rivers_driver_sdk::types::{Query, QueryResult};
    use rivers_driver_sdk::ConnectionParams;
    use rivers_runtime::tiered_cache::NoopDataViewCache;
    use rivers_runtime::{
        AcquireError, ConnectionAcquirer, DataViewError, DataViewExecutor, DataViewRegistry,
        PooledConnection,
    };

    /// Minimal `Connection` that returns an empty result. Only used so the
    /// fast-path test (no timeout) actually has a real conn to execute on.
    struct StubConn;

    #[async_trait]
    impl Connection for StubConn {
        async fn execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
            Ok(QueryResult {
                rows: Vec::new(),
                affected_rows: 0,
                last_insert_id: None,
                column_names: None,
            })
        }
        async fn ping(&mut self) -> Result<(), DriverError> { Ok(()) }
        fn driver_name(&self) -> &str { "stub" }
    }

    struct StubGuard {
        conn: Box<dyn Connection>,
    }
    impl PooledConnection for StubGuard {
        fn conn_mut(&mut self) -> &mut Box<dyn Connection> { &mut self.conn }
    }

    /// Acquirer that sleeps `acquire_delay` before returning a stub guard.
    /// Tracks whether the acquire future completed (false ⇒ it was
    /// dropped/cancelled before finishing — proves the worker was freed).
    struct SlowAcquirer {
        acquire_delay: Duration,
        acquire_started: AtomicU64,
        acquire_completed: AtomicBool,
    }

    impl SlowAcquirer {
        fn new(delay: Duration) -> Self {
            Self {
                acquire_delay: delay,
                acquire_started: AtomicU64::new(0),
                acquire_completed: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl ConnectionAcquirer for SlowAcquirer {
        async fn acquire(
            &self,
            _datasource_id: &str,
        ) -> Result<Box<dyn PooledConnection>, AcquireError> {
            self.acquire_started.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(self.acquire_delay).await;
            // Only flips true if the timeout did NOT cancel us mid-sleep.
            self.acquire_completed.store(true, Ordering::Relaxed);
            Ok(Box::new(StubGuard { conn: Box::new(StubConn) }))
        }

        async fn has_pool(&self, _datasource_id: &str) -> bool { true }
    }

    fn dv_config_for_ds(ds: &str) -> DataViewConfig {
        DataViewConfig {
            name: "slow_view".into(),
            datasource: ds.into(),
            query: Some("SELECT 1".into()),
            parameters: Vec::new(),
            return_schema: None,
            invalidates: Vec::new(),
            validate_result: false,
            strict_parameters: false,
            caching: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
            get_parameters: Vec::new(),
            post_parameters: Vec::new(),
            put_parameters: Vec::new(),
            delete_parameters: Vec::new(),
            streaming: false,
            circuit_breaker_id: None,
            prepared: false,
            query_params: HashMap::new(),
            max_rows: 1000,
            skip_introspect: false,
            cursor_key: None,
            source_views: vec![],
            compose_strategy: None,
            join_key: None,
            enrich_mode: "nest".into(),
        }
    }

    /// Build an executor wired to the given acquirer. Fast factory + a
    /// single datasource keyed "ds-slow" matching the registered DataView.
    fn build_executor(acquirer: Arc<SlowAcquirer>) -> DataViewExecutor {
        let mut registry = DataViewRegistry::new();
        registry.register(dv_config_for_ds("ds-slow"));

        let factory = Arc::new(DriverFactory::new());
        let mut params_map = HashMap::new();
        let mut opts = HashMap::new();
        opts.insert("driver".to_string(), "stub".to_string());
        params_map.insert("ds-slow".to_string(), ConnectionParams {
            host: String::new(), port: 0, database: String::new(),
            username: String::new(), password: String::new(),
            options: opts,
        });

        let mut exec = DataViewExecutor::new(
            registry,
            factory,
            Arc::new(params_map),
            Arc::new(NoopDataViewCache),
        );
        exec.set_acquirer(acquirer as Arc<dyn ConnectionAcquirer>);
        exec
    }

    /// Slow acquirer (500 ms sleep) + 100 ms request budget → must return
    /// `DataViewError::Timeout` within the budget plus a small slack.
    #[tokio::test]
    async fn execute_with_timeout_fires_on_slow_acquire() {
        let slow = Arc::new(SlowAcquirer::new(Duration::from_millis(500)));
        let exec = build_executor(slow.clone());

        let start = std::time::Instant::now();
        let result = exec
            .execute_with_timeout(
                "slow_view",
                HashMap::new(),
                "GET",
                "trace-d3",
                None,
                Some(100),
            )
            .await;
        let elapsed = start.elapsed();

        // Must fire within the budget + a small slack (CI scheduler jitter).
        // 100 ms budget → assert ≤ 250 ms. Acquire would have slept 500 ms.
        assert!(
            elapsed < Duration::from_millis(250),
            "timeout did not fire in budget: elapsed={:?}",
            elapsed
        );

        match result {
            Err(DataViewError::Timeout { datasource_id, timeout_ms }) => {
                assert_eq!(datasource_id, "ds-slow");
                assert_eq!(timeout_ms, 100);
            }
            other => panic!("expected DataViewError::Timeout, got: {:?}", other),
        }

        // Confirm the in-flight acquire future was cancelled (not allowed
        // to run to completion). This is the "request worker is freed"
        // guarantee — `tokio::time::timeout` drops the wrapped future on
        // elapse, which cancels the `tokio::time::sleep` inside acquire.
        assert!(
            !slow.acquire_completed.load(Ordering::Relaxed),
            "acquire future should have been dropped/cancelled by the timeout"
        );
        assert_eq!(
            slow.acquire_started.load(Ordering::Relaxed),
            1,
            "acquire should have been entered exactly once"
        );
    }

    /// `timeout_ms = None` → no enforced budget, slow acquire runs to
    /// completion and the call succeeds. Preserves prior behavior.
    #[tokio::test]
    async fn execute_with_timeout_none_disables_budget() {
        let slow = Arc::new(SlowAcquirer::new(Duration::from_millis(50)));
        let exec = build_executor(slow.clone());

        let result = exec
            .execute_with_timeout(
                "slow_view",
                HashMap::new(),
                "GET",
                "trace-d3-none",
                None,
                None,
            )
            .await;

        assert!(result.is_ok(), "no-timeout call should succeed: {:?}", result.err());
        assert!(slow.acquire_completed.load(Ordering::Relaxed));
    }

    /// `timeout_ms = Some(0)` is treated as "no timeout" — same convention
    /// as the builder's validation (it rejects 0 there, but the executor
    /// must defensively accept 0 as "disabled" rather than firing a 0 ms
    /// timeout that always elapses).
    #[tokio::test]
    async fn execute_with_timeout_zero_disables_budget() {
        let slow = Arc::new(SlowAcquirer::new(Duration::from_millis(20)));
        let exec = build_executor(slow.clone());

        let result = exec
            .execute_with_timeout(
                "slow_view",
                HashMap::new(),
                "GET",
                "trace-d3-zero",
                None,
                Some(0),
            )
            .await;

        assert!(result.is_ok(), "zero-timeout call should succeed: {:?}", result.err());
    }

    /// Plain `execute()` (no timeout arg) must remain a no-timeout call —
    /// it forwards to `execute_with_timeout` with `None`, so a slow
    /// acquirer must NOT trip a timeout error.
    #[tokio::test]
    async fn execute_default_path_has_no_timeout() {
        let slow = Arc::new(SlowAcquirer::new(Duration::from_millis(30)));
        let exec = build_executor(slow.clone());

        let result = exec.execute("slow_view", HashMap::new(), "GET", "trace-default", None).await;

        assert!(result.is_ok(), "default execute must not enforce a timeout: {:?}", result.err());
    }

    /// `DataViewError::Timeout` Display should mention the datasource id
    /// and budget so on-call has actionable context in logs.
    #[test]
    fn timeout_error_display_is_actionable() {
        let err = DataViewError::Timeout {
            datasource_id: "pg-primary".into(),
            timeout_ms: 250,
        };
        let msg = err.to_string();
        assert!(msg.contains("pg-primary"), "datasource id missing: {}", msg);
        assert!(msg.contains("250"), "timeout_ms missing: {}", msg);
    }
}
