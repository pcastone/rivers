use std::collections::HashMap;

use rivers_runtime::view::{ApiViewConfig, HandlerConfig, ParameterMappingConfig};
use riversd::view_engine::{
    apply_parameter_mapping, execute_rest_view, serialize_view_result, validate_views,
    ParsedRequest, ViewContext, ViewResult, ViewRouter,
};

// ── Helper ───────────────────────────────────────────────────────

fn rest_view(method: &str, path: &str, dataview: &str) -> ApiViewConfig {
    ApiViewConfig {
        view_type: "Rest".to_string(),
        path: Some(path.to_string()),
        method: Some(method.to_string()),
        handler: HandlerConfig::Dataview {
            dataview: dataview.to_string(),
        },
        handlers: None,
        max_body_mb: None,
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        event_handlers: None,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
        polling: None,
        tools: HashMap::new(),
        resources: HashMap::new(),
        prompts: HashMap::new(),
        instructions: None,
        session: None,
        federation: vec![],
        response_headers: None,
        schedule: None,
        interval_seconds: None,
        overlap_policy: None,
        max_concurrent: None,
            guard_view: None,
    }
}

fn codecomponent_view(method: &str, path: &str) -> ApiViewConfig {
    ApiViewConfig {
        view_type: "Rest".to_string(),
        path: Some(path.to_string()),
        method: Some(method.to_string()),
        handler: HandlerConfig::Codecomponent {
            language: "javascript".to_string(),
            module: "handler.js".to_string(),
            entrypoint: "onRequest".to_string(),
            resources: vec![],
        },
        handlers: None,
        max_body_mb: None,
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        event_handlers: None,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
        polling: None,
        tools: HashMap::new(),
        resources: HashMap::new(),
        prompts: HashMap::new(),
        instructions: None,
        session: None,
        federation: vec![],
        response_headers: None,
        schedule: None,
        interval_seconds: None,
        overlap_policy: None,
        max_concurrent: None,
            guard_view: None,
    }
}

// ── ParsedRequest ────────────────────────────────────────────────

#[test]
fn parsed_request_new_defaults() {
    let req = ParsedRequest::new("GET", "/api/test");
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/api/test");
    assert!(req.query_params.is_empty());
    assert!(req.headers.is_empty());
    assert_eq!(req.body, serde_json::Value::Null);
    assert!(req.path_params.is_empty());
}

// ── ViewContext ──────────────────────────────────────────────────

#[test]
fn view_context_new_defaults() {
    let req = ParsedRequest::new("GET", "/");
    let ctx = ViewContext::new(
        req,
        "trace-123".to_string(),
        "app-1".to_string(),
        String::new(),
        "node-1".to_string(),
        "dev".to_string(),
    );
    assert_eq!(ctx.trace_id, "trace-123");
    assert_eq!(ctx.app_id, "app-1");
    assert_eq!(ctx.node_id, "node-1");
    assert_eq!(ctx.env, "dev");
    assert!(ctx.data.is_empty());
    assert_eq!(ctx.resdata, serde_json::Value::Null);
    assert!(ctx.session.is_none());
    assert_eq!(ctx.store.app_id, "app-1");
}

// ── ViewRouter ──────────────────────────────────────────────────

#[test]
fn router_matches_literal_path() {
    let mut views = HashMap::new();
    views.insert("list".to_string(), rest_view("GET", "/api/contacts", "list_contacts"));

    let router = ViewRouter::from_views(&views);
    let result = router.match_route("GET", "/api/contacts");
    assert!(result.is_some());
    let (route, params) = result.unwrap();
    assert_eq!(route.view_id, "list");
    assert!(params.is_empty());
}

#[test]
fn router_matches_param_path_braces() {
    let mut views = HashMap::new();
    views.insert("detail".to_string(), rest_view("GET", "/api/contacts/{id}", "get_contact"));

    let router = ViewRouter::from_views(&views);
    let result = router.match_route("GET", "/api/contacts/42");
    assert!(result.is_some());
    let (route, params) = result.unwrap();
    assert_eq!(route.view_id, "detail");
    assert_eq!(params.get("id").unwrap(), "42");
}

#[test]
fn router_matches_param_path_colon() {
    let mut views = HashMap::new();
    views.insert("detail".to_string(), rest_view("GET", "/api/contacts/:id", "get_contact"));

    let router = ViewRouter::from_views(&views);
    let result = router.match_route("GET", "/api/contacts/abc");
    assert!(result.is_some());
    let (_, params) = result.unwrap();
    assert_eq!(params.get("id").unwrap(), "abc");
}

#[test]
fn router_rejects_wrong_method() {
    let mut views = HashMap::new();
    views.insert("create".to_string(), rest_view("POST", "/api/contacts", "create_contact"));

    let router = ViewRouter::from_views(&views);
    assert!(router.match_route("GET", "/api/contacts").is_none());
}

#[test]
fn router_rejects_wrong_path() {
    let mut views = HashMap::new();
    views.insert("list".to_string(), rest_view("GET", "/api/contacts", "list_contacts"));

    let router = ViewRouter::from_views(&views);
    assert!(router.match_route("GET", "/api/orders").is_none());
}

#[test]
fn router_rejects_segment_count_mismatch() {
    let mut views = HashMap::new();
    views.insert("detail".to_string(), rest_view("GET", "/api/contacts/{id}", "get_contact"));

    let router = ViewRouter::from_views(&views);
    // Too many segments
    assert!(router.match_route("GET", "/api/contacts/42/extra").is_none());
    // Too few segments
    assert!(router.match_route("GET", "/api").is_none());
}

#[test]
fn router_method_case_insensitive() {
    let mut views = HashMap::new();
    views.insert("list".to_string(), rest_view("GET", "/api/contacts", "list_contacts"));

    let router = ViewRouter::from_views(&views);
    assert!(router.match_route("get", "/api/contacts").is_some());
}

#[test]
fn router_defaults_to_get_method() {
    let mut config = rest_view("GET", "/api/test", "test_dv");
    config.method = None; // should default to GET

    let mut views = HashMap::new();
    views.insert("test".to_string(), config);

    let router = ViewRouter::from_views(&views);
    assert!(router.match_route("GET", "/api/test").is_some());
}

#[test]
fn router_skips_message_consumer_views() {
    let mut config = rest_view("GET", "/api/test", "test_dv");
    config.view_type = "MessageConsumer".to_string();

    let mut views = HashMap::new();
    views.insert("consumer".to_string(), config);

    let router = ViewRouter::from_views(&views);
    assert!(router.routes().is_empty());
}

#[test]
fn router_skips_views_without_path() {
    let mut config = rest_view("GET", "/api/test", "test_dv");
    config.path = None;

    let mut views = HashMap::new();
    views.insert("no_path".to_string(), config);

    let router = ViewRouter::from_views(&views);
    assert!(router.routes().is_empty());
}

#[test]
fn router_multiple_params() {
    let mut views = HashMap::new();
    views.insert(
        "nested".to_string(),
        rest_view("GET", "/api/{org}/contacts/{id}", "org_contact"),
    );

    let router = ViewRouter::from_views(&views);
    let result = router.match_route("GET", "/api/acme/contacts/42");
    assert!(result.is_some());
    let (_, params) = result.unwrap();
    assert_eq!(params.get("org").unwrap(), "acme");
    assert_eq!(params.get("id").unwrap(), "42");
}

// ── Parameter Mapping ───────────────────────────────────────────

#[test]
fn parameter_mapping_query_params() {
    let mut req = ParsedRequest::new("GET", "/api/contacts");
    req.query_params
        .insert("page".to_string(), "2".to_string());
    req.query_params
        .insert("size".to_string(), "10".to_string());

    let mut config = rest_view("GET", "/api/contacts", "list_contacts");
    config.parameter_mapping = Some(ParameterMappingConfig {
        query: {
            let mut m = HashMap::new();
            m.insert("page".to_string(), "offset_page".to_string());
            m.insert("size".to_string(), "page_size".to_string());
            m
        },
        path: HashMap::new(),
        body: HashMap::new(),
        header: HashMap::new(),
    });

    let params = apply_parameter_mapping(&req, &config);
    assert_eq!(params.get("offset_page").unwrap(), "2");
    assert_eq!(params.get("page_size").unwrap(), "10");
}

#[test]
fn parameter_mapping_path_params() {
    let mut req = ParsedRequest::new("GET", "/api/contacts/42");
    req.path_params.insert("id".to_string(), "42".to_string());

    let mut config = rest_view("GET", "/api/contacts/{id}", "get_contact");
    config.parameter_mapping = Some(ParameterMappingConfig {
        query: HashMap::new(),
        path: {
            let mut m = HashMap::new();
            m.insert("id".to_string(), "contact_id".to_string());
            m
        },
        body: HashMap::new(),
        header: HashMap::new(),
    });

    let params = apply_parameter_mapping(&req, &config);
    assert_eq!(params.get("contact_id").unwrap(), "42");
}

#[test]
fn parameter_mapping_missing_params_skipped() {
    let req = ParsedRequest::new("GET", "/api/contacts");

    let mut config = rest_view("GET", "/api/contacts", "list_contacts");
    config.parameter_mapping = Some(ParameterMappingConfig {
        query: {
            let mut m = HashMap::new();
            m.insert("missing".to_string(), "dv_param".to_string());
            m
        },
        path: HashMap::new(),
        body: HashMap::new(),
        header: HashMap::new(),
    });

    let params = apply_parameter_mapping(&req, &config);
    assert!(params.is_empty());
}

#[test]
fn parameter_mapping_none_returns_empty() {
    let req = ParsedRequest::new("GET", "/api/contacts");
    let config = rest_view("GET", "/api/contacts", "list_contacts");

    let params = apply_parameter_mapping(&req, &config);
    assert!(params.is_empty());
}

// ── Pipeline Execution ──────────────────────────────────────────

#[tokio::test]
async fn execute_rest_view_dataview_handler() {
    let req = ParsedRequest::new("GET", "/api/contacts");
    let mut ctx = ViewContext::new(
        req,
        "trace-1".to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let config = rest_view("GET", "/api/contacts", "list_contacts");

    let result = execute_rest_view(&mut ctx, &config, None, None).await.unwrap();
    assert_eq!(result.status, 200);
    assert!(result.body.get("_stub").is_some());
    assert_eq!(result.body["_dataview"], "list_contacts");
}

#[tokio::test]
async fn execute_rest_view_codecomponent_handler() {
    let req = ParsedRequest::new("POST", "/api/login");
    let mut ctx = ViewContext::new(
        req,
        "trace-2".to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let config = codecomponent_view("POST", "/api/login");

    let result = execute_rest_view(&mut ctx, &config, None, None).await.unwrap();
    assert_eq!(result.status, 200);
    assert!(result.body.get("_stub").is_some());
    assert_eq!(result.body["_handler"], "codecomponent");
}

#[tokio::test]
async fn execute_rest_view_populates_resdata() {
    let req = ParsedRequest::new("GET", "/test");
    let mut ctx = ViewContext::new(
        req,
        "trace-3".to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let config = rest_view("GET", "/test", "test_dv");

    execute_rest_view(&mut ctx, &config, None, None).await.unwrap();
    assert!(!ctx.resdata.is_null());
}

// ── Response Serialization ──────────────────────────────────────

#[test]
fn serialize_view_result_json() {
    let result = ViewResult {
        status: 200,
        headers: HashMap::new(),
        body: serde_json::json!({"name": "Alice"}),
    };

    let (status, headers, body) = serialize_view_result(&result);
    assert_eq!(status, 200);
    assert_eq!(headers.get("content-type").unwrap(), "application/json; charset=utf-8");
    assert!(body.contains("Alice"));
}

#[test]
fn serialize_view_result_preserves_custom_headers() {
    let mut custom_headers = HashMap::new();
    custom_headers.insert("x-custom".to_string(), "value".to_string());

    let result = ViewResult {
        status: 201,
        headers: custom_headers,
        body: serde_json::json!(null),
    };

    let (status, headers, _) = serialize_view_result(&result);
    assert_eq!(status, 201);
    assert_eq!(headers.get("x-custom").unwrap(), "value");
    assert!(headers.contains_key("content-type"));
}

#[test]
fn serialize_view_result_default() {
    let result = ViewResult::default();
    let (status, _, body) = serialize_view_result(&result);
    assert_eq!(status, 200);
    assert_eq!(body, "null");
}

// ── View Validation ─────────────────────────────────────────────

#[test]
fn validate_views_passes_valid_config() {
    let mut views = HashMap::new();
    views.insert("list".to_string(), rest_view("GET", "/api/contacts", "list_contacts"));

    let errors = validate_views(&views, &["list_contacts".to_string()]);
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
}

#[test]
fn validate_views_catches_unknown_dataview() {
    let mut views = HashMap::new();
    views.insert("list".to_string(), rest_view("GET", "/api/contacts", "nonexistent"));

    let errors = validate_views(&views, &["list_contacts".to_string()]);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("unknown dataview"));
}

#[test]
fn validate_views_catches_dataview_on_non_rest() {
    let mut config = rest_view("GET", "/api/test", "test_dv");
    config.view_type = "Websocket".to_string();

    let mut views = HashMap::new();
    views.insert("ws".to_string(), config);

    let errors = validate_views(&views, &["test_dv".to_string()]);
    assert!(errors.iter().any(|e| e.contains("dataview handler is only supported")));
}

#[test]
fn validate_views_catches_websocket_non_get() {
    let mut config = codecomponent_view("POST", "/ws/test");
    config.view_type = "Websocket".to_string();

    let mut views = HashMap::new();
    views.insert("ws".to_string(), config);

    let errors = validate_views(&views, &[]);
    assert!(errors.iter().any(|e| e.contains("method must be GET when view_type=Websocket")));
}

#[test]
fn validate_views_catches_sse_non_get() {
    let mut config = codecomponent_view("POST", "/sse/test");
    config.view_type = "ServerSentEvents".to_string();

    let mut views = HashMap::new();
    views.insert("sse".to_string(), config);

    let errors = validate_views(&views, &[]);
    assert!(errors.iter().any(|e| e.contains("method must be GET when view_type=ServerSentEvents")));
}

#[test]
fn validate_views_catches_message_consumer_with_path() {
    let mut config = codecomponent_view("GET", "/should/not/exist");
    config.view_type = "MessageConsumer".to_string();

    let mut views = HashMap::new();
    views.insert("consumer".to_string(), config);

    let errors = validate_views(&views, &[]);
    assert!(errors.iter().any(|e| e.contains("MessageConsumer views must not declare a path")));
}

#[test]
fn validate_views_catches_zero_rate_limit() {
    let mut config = rest_view("GET", "/api/test", "test_dv");
    config.rate_limit_per_minute = Some(0);

    let mut views = HashMap::new();
    views.insert("limited".to_string(), config);

    let errors = validate_views(&views, &["test_dv".to_string()]);
    assert!(errors.iter().any(|e| e.contains("rate_limit_per_minute must be greater than 0")));
}

#[test]
fn validate_views_multiple_errors() {
    let mut config = rest_view("GET", "/api/test", "nonexistent");
    config.rate_limit_per_minute = Some(0);

    let mut views = HashMap::new();
    views.insert("bad".to_string(), config);

    let errors = validate_views(&views, &[]);
    assert!(errors.len() >= 2);
}

// ── G_R4 (P2-4): observer dispatch is bounded by RIVERS_OBSERVER_TIMEOUT_MS ──
//
// A slow pre_process observer (sleep 500ms) MUST NOT extend request latency
// past the configured cap (~250ms here). The dispatch is awaited but capped
// via tokio::time::timeout — on elapsed we log a warning and continue.

#[tokio::test]
async fn slow_observer_does_not_extend_request_latency() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use rivers_runtime::view::{HandlerStageConfig, ViewEventHandlers};
    use rivers_runtime::rivers_core::storage::{InMemoryStorageEngine, StorageEngine};
    use riversd::process_pool::{ProcessPoolManager};

    // Tighten cap for the test.
    std::env::set_var("RIVERS_OBSERVER_TIMEOUT_MS", "200");
    // OnceLock may have been initialized by an earlier test in this binary;
    // accept either the env value or the default 200ms — both gate the
    // observer well below the 500ms sleep.

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rivers_g_r4_slow_{id}.js"));
    std::fs::write(
        &path,
        r#"
        function slowObserver(ctx) {
            // Busy-loop for ~500ms of real time. setTimeout is not available
            // inside the Rivers V8 isolate, so we burn CPU instead — this
            // genuinely blocks the spawn_blocking worker for the duration.
            const start = Date.now();
            while (Date.now() - start < 500) {
                // intentionally empty
            }
            return null;
        }
        function fastHandler(ctx) {
            return { ok: true };
        }
        "#,
    )
    .unwrap();

    let mut config = codecomponent_view("GET", "/api/slow");
    // Override entrypoint to fastHandler (codecomponent_view uses "onRequest").
    config.handler = HandlerConfig::Codecomponent {
        language: "javascript".into(),
        module: path.to_string_lossy().into(),
        entrypoint: "fastHandler".into(),
        resources: vec![],
    };
    config.event_handlers = Some(ViewEventHandlers {
        pre_process: vec![HandlerStageConfig {
            module: path.to_string_lossy().into(),
            entrypoint: "slowObserver".into(),
            key: None,
            on_failure: None,
        }],
        handlers: vec![],
        post_process: vec![],
        on_error: vec![],
    });

    let req = ParsedRequest::new("GET", "/api/slow");
    // Pre-existing main failure fix: ViewContext::new(req, trace_id,
    // app_id, dv_namespace, ...) — `dv_namespace` is what the
    // canary-sprint RT-CTX-APP-ID fix passes to `enrich`. An empty
    // dv_namespace trips the dispatcher's empty-app_id check at the
    // post-canary-fix code path. Use the same slug as app_id so the
    // observer dispatch reaches the pool.
    let mut ctx = ViewContext::new(
        req,
        "trace-g-r4".into(),
        "test-app".into(),
        "test-app".into(),
        String::new(),
        String::new(),
    );

    let mgr = ProcessPoolManager::from_config(&HashMap::new());
    let _storage: std::sync::Arc<dyn StorageEngine> = std::sync::Arc::new(InMemoryStorageEngine::new());

    let start = std::time::Instant::now();
    let result = execute_rest_view(&mut ctx, &config, Some(&mgr), None).await;
    let elapsed = start.elapsed();
    let _ = std::fs::remove_file(&path);

    assert!(result.is_ok(), "request should succeed despite slow observer: {result:?}");
    assert!(
        elapsed < std::time::Duration::from_millis(450),
        "observer cap not enforced: request took {elapsed:?}"
    );
}
