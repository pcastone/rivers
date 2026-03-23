//! View Layer — REST view routing, handler pipeline, and response serialization.
//!
//! Per `rivers-view-layer-spec.md` §1-5, §12-13.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use rivers_runtime::view::{ApiViewConfig, HandlerConfig};

// ── ParsedRequest ────────────────────────────────────────────────

/// A parsed HTTP request, ready for view handler consumption.
///
/// Per spec §4.3, technology-path-spec §E1.6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub query_params: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: serde_json::Value,
    pub path_params: HashMap<String, String>,
}

impl ParsedRequest {
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            query_params: HashMap::new(),
            headers: HashMap::new(),
            body: serde_json::Value::Null,
            path_params: HashMap::new(),
        }
    }
}

// ── ViewContext ───────────────────────────────────────────────────

/// Application KV store handle — wraps StorageEngine with app namespace.
///
/// Per technology-path-spec §2.4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreHandle {
    pub app_id: String,
}

impl StoreHandle {
    /// Reserved namespace prefixes that handlers cannot use.
    const RESERVED_PREFIXES: &'static [&'static str] =
        &["session:", "csrf:", "cache:", "raft:", "rivers:"];

    pub fn new(app_id: String) -> Self {
        Self { app_id }
    }

    /// Check if a key uses a reserved namespace prefix.
    pub fn is_reserved_key(key: &str) -> bool {
        Self::RESERVED_PREFIXES.iter().any(|p| key.starts_with(p))
    }
}

/// Shared context for a view execution, passed through all pipeline stages.
///
/// Per technology-path-spec §E1.1: enriched ViewContext with app identity,
/// pre-fetched data map, mutable response payload, and store handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewContext {
    pub request: ParsedRequest,
    pub trace_id: String,
    pub session: Option<serde_json::Value>,
    /// Application ID.
    pub app_id: String,
    /// Node identifier.
    pub node_id: String,
    /// Environment: "dev" | "staging" | "prod".
    pub env: String,
    /// Pre-fetched DataView results keyed by DataView name.
    pub data: HashMap<String, serde_json::Value>,
    /// Mutable response payload (replaces former `sources["primary"]`).
    pub resdata: serde_json::Value,
    /// Application KV store handle.
    pub store: StoreHandle,
}

impl ViewContext {
    pub fn new(
        request: ParsedRequest,
        trace_id: String,
        app_id: String,
        node_id: String,
        env: String,
    ) -> Self {
        let store = StoreHandle::new(app_id.clone());
        Self {
            request,
            trace_id,
            session: None,
            app_id,
            node_id,
            env,
            data: HashMap::new(),
            resdata: serde_json::Value::Null,
            store,
        }
    }
}

// ── View Router ──────────────────────────────────────────────────

/// A registered view route for matching incoming requests.
#[derive(Debug, Clone)]
pub struct ViewRoute {
    pub view_id: String,
    pub method: String,
    pub path_pattern: String,
    /// Path segments for matching. Each is either a literal or a parameter (starts with '{').
    pub segments: Vec<PathSegment>,
    pub config: ApiViewConfig,
    /// App entry point — used to namespace DataView lookups.
    pub app_entry_point: String,
}

/// A single segment of a URL path pattern.
#[derive(Debug, Clone)]
pub enum PathSegment {
    /// Literal path segment (exact match).
    Literal(String),
    /// Path parameter like `{id}`.
    Param(String),
}

/// Build a namespaced path: `/[prefix]/<bundle>/<app>/<view_path>`.
///
/// Per spec §3.1: all app routes are namespaced by bundle and entry point.
pub fn build_namespaced_path(
    route_prefix: Option<&str>,
    bundle_name: &str,
    entry_point: &str,
    view_path: &str,
) -> String {
    let view_path = view_path.trim_start_matches('/');
    match route_prefix.filter(|p| !p.is_empty()) {
        Some(prefix) => {
            let prefix = prefix.trim_matches('/');
            if view_path.is_empty() {
                format!("/{prefix}/{bundle_name}/{entry_point}")
            } else {
                format!("/{prefix}/{bundle_name}/{entry_point}/{view_path}")
            }
        }
        None => {
            if view_path.is_empty() {
                format!("/{bundle_name}/{entry_point}")
            } else {
                format!("/{bundle_name}/{entry_point}/{view_path}")
            }
        }
    }
}

/// The view router — matches incoming requests to registered view routes.
pub struct ViewRouter {
    routes: Vec<ViewRoute>,
}

impl ViewRouter {
    /// Build a router from a loaded bundle with namespaced paths.
    ///
    /// Per spec §3.1: routes are `/<prefix>/<bundle>/<app>/<view>`.
    pub fn from_bundle(
        bundle: &rivers_runtime::LoadedBundle,
        route_prefix: Option<&str>,
    ) -> Self {
        let bundle_name = &bundle.manifest.bundle_name;
        let mut routes = Vec::new();

        for app in &bundle.apps {
            let entry_point = app
                .manifest
                .entry_point
                .as_deref()
                .unwrap_or(&app.manifest.app_name);

            for (id, config) in &app.config.api.views {
                if config.view_type == "MessageConsumer" {
                    continue;
                }

                let view_path = match &config.path {
                    Some(p) => p.clone(),
                    None => continue,
                };

                let full_path = build_namespaced_path(
                    route_prefix,
                    bundle_name,
                    entry_point,
                    &view_path,
                );

                let method = config
                    .method
                    .as_deref()
                    .unwrap_or("GET")
                    .to_uppercase();

                let segments = parse_path_pattern(&full_path);

                // Key with entry_point to prevent collisions across apps
                let qualified_id = format!("{entry_point}:{id}");

                if config.allow_outbound_http {
                    let module = match &config.handler {
                        HandlerConfig::Codecomponent { module, .. } => module.as_str(),
                        _ => "<none>",
                    };
                    tracing::warn!(
                        target: "rivers.security",
                        view = %qualified_id,
                        module = %module,
                        "view declares allow_outbound_http"
                    );
                }

                routes.push(ViewRoute {
                    view_id: qualified_id,
                    method,
                    path_pattern: full_path,
                    segments,
                    config: config.clone(),
                    app_entry_point: entry_point.to_string(),
                });
            }
        }

        Self { routes }
    }

    /// Build a router from a set of view configs (flat, no namespacing — legacy).
    pub fn from_views(views: &HashMap<String, ApiViewConfig>) -> Self {
        let mut routes = Vec::new();

        for (id, config) in views {
            // X2.3: Warn at startup for views that declare allow_outbound_http
            if config.allow_outbound_http {
                let module = match &config.handler {
                    HandlerConfig::Codecomponent { module, .. } => module.as_str(),
                    _ => "<none>",
                };
                tracing::warn!(
                    target: "rivers.security",
                    view = %id,
                    module = %module,
                    "view declares allow_outbound_http — Rivers.http will be available in handler"
                );
            }

            // MessageConsumer views have no HTTP route
            if config.view_type == "MessageConsumer" {
                continue;
            }

            let path = match &config.path {
                Some(p) => p.clone(),
                None => continue,
            };

            let method = config
                .method
                .as_deref()
                .unwrap_or("GET")
                .to_uppercase();

            let segments = parse_path_pattern(&path);

            routes.push(ViewRoute {
                view_id: id.clone(),
                method,
                path_pattern: path,
                segments,
                config: config.clone(),
                app_entry_point: String::new(),
            });
        }

        Self { routes }
    }

    /// Match an incoming request to a view route.
    ///
    /// Returns the matched route and extracted path parameters.
    pub fn match_route(
        &self,
        method: &str,
        path: &str,
    ) -> Option<(&ViewRoute, HashMap<String, String>)> {
        let request_segments: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        for route in &self.routes {
            if route.method != method.to_uppercase() {
                continue;
            }

            if route.segments.len() != request_segments.len() {
                continue;
            }

            let mut params = HashMap::new();
            let mut matched = true;

            for (seg, req_seg) in route.segments.iter().zip(request_segments.iter()) {
                match seg {
                    PathSegment::Literal(lit) => {
                        if lit != req_seg {
                            matched = false;
                            break;
                        }
                    }
                    PathSegment::Param(name) => {
                        params.insert(name.clone(), req_seg.to_string());
                    }
                }
            }

            if matched {
                return Some((route, params));
            }
        }

        None
    }

    /// Get all registered routes.
    pub fn routes(&self) -> &[ViewRoute] {
        &self.routes
    }
}

/// Parse a path pattern like "/api/orders/{id}" into segments.
fn parse_path_pattern(pattern: &str) -> Vec<PathSegment> {
    pattern
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                PathSegment::Param(seg[1..seg.len() - 1].to_string())
            } else if let Some(stripped) = seg.strip_prefix(':') {
                // Also support :param syntax
                PathSegment::Param(stripped.to_string())
            } else {
                PathSegment::Literal(seg.to_string())
            }
        })
        .collect()
}

// ── Parameter Mapping ────────────────────────────────────────────

/// Apply parameter mapping from request to DataView parameters.
///
/// Per spec §5.2 and AMD-4: parameter_mapping.query, .path, and .body subtables.
pub fn apply_parameter_mapping(
    request: &ParsedRequest,
    config: &ApiViewConfig,
) -> HashMap<String, serde_json::Value> {
    let mut params = HashMap::new();

    if let Some(ref mapping) = config.parameter_mapping {
        // Map query parameters
        for (http_param, dv_param) in &mapping.query {
            if let Some(value) = request.query_params.get(http_param) {
                params.insert(dv_param.clone(), serde_json::Value::String(value.clone()));
            }
        }

        // Map path parameters
        for (http_param, dv_param) in &mapping.path {
            if let Some(value) = request.path_params.get(http_param) {
                params.insert(dv_param.clone(), serde_json::Value::String(value.clone()));
            }
        }

        // Map body parameters (for write operations)
        for (body_field, dv_param) in &mapping.body {
            if let Some(value) = request.body.get(body_field) {
                params.insert(dv_param.clone(), value.clone());
            }
        }
    }

    params
}

// ── JSON → QueryValue Conversion ─────────────────────────────────

/// Convert a serde_json::Value to a QueryValue for DataView parameter passing.
pub fn json_value_to_query_value(value: &serde_json::Value) -> rivers_runtime::rivers_driver_sdk::types::QueryValue {
    use rivers_runtime::rivers_driver_sdk::types::QueryValue;
    match value {
        serde_json::Value::Null => QueryValue::Null,
        serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                QueryValue::Float(f)
            } else {
                QueryValue::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => QueryValue::String(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            QueryValue::Json(value.clone())
        }
    }
}

// ── Pipeline Execution ───────────────────────────────────────────

/// Result of executing a view handler pipeline.
#[derive(Debug, Serialize)]
pub struct ViewResult {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: serde_json::Value,
}

impl Default for ViewResult {
    fn default() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: serde_json::Value::Null,
        }
    }
}

/// Execute the full view handler pipeline for a REST view.
///
/// Per spec §4: pre_process → handlers → post_process + on_error.
///
/// When `pool` is `Some`, CodeComponent pipeline stages are dispatched
/// to the ProcessPool.  When `None`, CodeComponent stages are stubbed.
///
/// When `executor` is `Some`, DataView handlers execute real queries
/// through the DataViewExecutor.  When `None`, DataView handlers return stub data.
pub async fn execute_rest_view(
    ctx: &mut ViewContext,
    config: &ApiViewConfig,
    pool: Option<&ProcessPoolManager>,
    executor: Option<&rivers_runtime::DataViewExecutor>,
) -> Result<ViewResult, ViewError> {
    let inner = async {
        // ── Pre_process (observer, fire-and-forget) ─────────────────
        if let (Some(pool), Some(ref handlers)) = (pool, &config.event_handlers) {
            for handler in &handlers.pre_process {
                let entrypoint = Entrypoint {
                    module: handler.module.clone(),
                    function: handler.entrypoint.clone(),
                    language: "javascript".into(),
                };
                let args = serde_json::json!({
                    "request": ctx.request,
                    "trace_id": ctx.trace_id,
                    "session": ctx.session,
                    "data": ctx.data,
                    "resdata": ctx.resdata,
                });
                let task_ctx = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone())
                    .build()
                    .map_err(|e| ViewError::Pipeline(format!("pre_process build: {e}")))?;
                // Fire and forget — pre_process is observer
                let _ = pool.dispatch("default", task_ctx).await;
            }
        }

        // ── Primary execution ───────────────────────────────────────
        match &config.handler {
            HandlerConfig::Dataview { dataview } => {
                // DataView handler: apply parameter mapping, execute DataView
                let params = apply_parameter_mapping(&ctx.request, config);

                if let Some(exec) = executor {
                    // Convert JSON params to QueryValue for the executor
                    let query_params: std::collections::HashMap<String, rivers_runtime::rivers_driver_sdk::types::QueryValue> =
                        params.iter()
                            .map(|(k, v)| (k.clone(), json_value_to_query_value(v)))
                            .collect();

                    let response = exec
                        .execute(dataview, query_params, &ctx.request.method, &ctx.trace_id)
                        .await
                        .map_err(|e| ViewError::Handler(format!("dataview '{}': {}", dataview, e)))?;

                    // Set resdata to the query result rows
                    ctx.resdata = serde_json::to_value(&response.query_result.rows)
                        .unwrap_or(serde_json::Value::Null);
                } else {
                    // DataView execution stub — executor not available
                    ctx.resdata = serde_json::json!({
                        "_stub": true,
                        "_dataview": dataview,
                        "_params": params,
                    });
                }
            }
            HandlerConfig::Codecomponent {
                module,
                entrypoint,
                language,
                ..
            } => {
                if let Some(pool) = pool {
                    let entry = Entrypoint {
                        module: module.clone(),
                        function: entrypoint.clone(),
                        language: language.clone(),
                    };
                    let args = serde_json::json!({
                        "request": ctx.request,
                        "session": ctx.session,
                        "data": ctx.data,
                        "_source": null,
                    });
                    let mut builder = TaskContextBuilder::new()
                        .entrypoint(entry)
                        .args(args)
                        .trace_id(ctx.trace_id.clone());
                    // X2.3: Wire allow_outbound_http → HttpToken capability
                    if config.allow_outbound_http {
                        builder = builder.http(crate::process_pool::HttpToken);
                    }
                    let task_ctx = builder
                        .build()
                        .map_err(|e| ViewError::Handler(format!("codecomponent build: {e}")))?;

                    let result = pool
                        .dispatch("default", task_ctx)
                        .await
                        .map_err(|e| ViewError::Handler(format!("codecomponent dispatch: {e}")))?;

                    // Check for handler result envelope { status, headers, body }
                    if let Some(view_result) = parse_handler_view_result(&result.value) {
                        return Ok(view_result);
                    }
                    ctx.resdata = result.value;
                } else {
                    ctx.resdata =
                        serde_json::json!({ "_stub": true, "_handler": "codecomponent" });
                }
            }
            HandlerConfig::None { .. } => {
                // Null datasource pattern: no primary DataView execution.
                // The view runs only CodeComponent pipeline stages
                // (pre_process, handlers, post_process).
                // resdata stays null — pipeline stages populate it.
                ctx.resdata = serde_json::Value::Null;
            }
        }

        // ── Handlers (ordered chain) ────────────────────────────────
        if let (Some(pool), Some(ref handlers)) = (pool, &config.event_handlers) {
            for handler in &handlers.handlers {
                let entrypoint = Entrypoint {
                    module: handler.module.clone(),
                    function: handler.entrypoint.clone(),
                    language: "javascript".into(),
                };
                let args = serde_json::json!({
                    "request": ctx.request,
                    "trace_id": ctx.trace_id,
                    "session": ctx.session,
                    "data": ctx.data,
                    "resdata": ctx.resdata,
                });
                let task_ctx = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone())
                    .build()
                    .map_err(|e| ViewError::Pipeline(format!("handler build: {e}")))?;

                match pool.dispatch("default", task_ctx).await {
                    Ok(result) => {
                        // If result is non-null, update ctx.resdata
                        if !result.value.is_null() {
                            ctx.resdata = result.value;
                        }
                    }
                    Err(e) => {
                        return Err(ViewError::Pipeline(format!("handler dispatch: {e}")));
                    }
                }
            }
        }

        // ── Post_process (observer, fire-and-forget) ────────────────
        if let (Some(pool), Some(ref handlers)) = (pool, &config.event_handlers) {
            for handler in &handlers.post_process {
                let entrypoint = Entrypoint {
                    module: handler.module.clone(),
                    function: handler.entrypoint.clone(),
                    language: "javascript".into(),
                };
                let args = serde_json::json!({
                    "request": ctx.request,
                    "trace_id": ctx.trace_id,
                    "session": ctx.session,
                    "data": ctx.data,
                    "resdata": ctx.resdata,
                });
                let task_ctx = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone())
                    .build()
                    .map_err(|e| ViewError::Pipeline(format!("post_process build: {e}")))?;
                // Fire and forget — post_process is observer
                let _ = pool.dispatch("default", task_ctx).await;
            }
        }

        // Build response from ctx.resdata
        let body = ctx.resdata.clone();

        Ok(ViewResult {
            status: 200,
            headers: HashMap::new(),
            body,
        })
    };

    // ── on_error wrapping ───────────────────────────────────────
    match inner.await {
        Ok(result) => Ok(result),
        Err(err) => {
            // Try on_error handlers if pool is available
            if let (Some(pool), Some(ref handlers)) = (pool, &config.event_handlers) {
                if !handlers.on_error.is_empty() {
                    if let Some(recovery) =
                        execute_on_error_handlers(pool, &handlers.on_error, ctx, &err).await
                    {
                        return Ok(recovery);
                    }
                }
            }
            Err(err)
        }
    }
}

// ── Response Serialization ───────────────────────────────────────

/// Serialize a ViewResult into an axum response.
///
/// Per spec §5.3: primary sources serialized to JSON.
/// CodeComponent can return { status, headers, body } envelope.
pub fn serialize_view_result(result: &ViewResult) -> (u16, HashMap<String, String>, String) {
    let body_str = serde_json::to_string(&result.body).unwrap_or_else(|_| "null".to_string());

    let mut headers = result.headers.clone();
    headers
        .entry("content-type".to_string())
        .or_insert_with(|| "application/json; charset=utf-8".to_string());

    (result.status, headers, body_str)
}

// ── Validation ───────────────────────────────────────────────────

/// Validate view configurations at load time.
///
/// Per spec §13.
pub fn validate_views(
    views: &HashMap<String, ApiViewConfig>,
    available_dataviews: &[String],
) -> Vec<String> {
    let mut errors = Vec::new();

    for (id, config) in views {
        // DataView handler on non-REST view (allowed when polling is configured)
        if matches!(config.handler, HandlerConfig::Dataview { .. })
            && config.view_type != "Rest"
            && config.polling.is_none()
        {
            errors.push(format!(
                "view '{}': dataview handler is only supported for view_type=Rest (or SSE/WS with polling)",
                id
            ));
        }

        // None handler must have at least one pipeline stage defined
        if matches!(config.handler, HandlerConfig::None { .. }) {
            let has_pipeline = config
                .event_handlers
                .as_ref()
                .map(|eh| {
                    !eh.pre_process.is_empty()
                        || !eh.handlers.is_empty()
                        || !eh.post_process.is_empty()
                })
                .unwrap_or(false);
            if !has_pipeline {
                errors.push(format!(
                    "view '{}': handler type 'none' requires at least one pipeline event handler",
                    id
                ));
            }
        }

        // WebSocket must be GET
        if config.view_type == "Websocket" {
            if let Some(ref method) = config.method {
                if method.to_uppercase() != "GET" {
                    errors.push(format!(
                        "view '{}': method must be GET when view_type=Websocket",
                        id
                    ));
                }
            }
        }

        // SSE must be GET
        if config.view_type == "ServerSentEvents" {
            if let Some(ref method) = config.method {
                if method.to_uppercase() != "GET" {
                    errors.push(format!(
                        "view '{}': method must be GET when view_type=ServerSentEvents",
                        id
                    ));
                }
            }
            // SSE with on_stream is invalid
            if config.on_stream.is_some() {
                errors.push(format!(
                    "view '{}': on_stream is not valid for ServerSentEvents views",
                    id
                ));
            }
        }

        // MessageConsumer must not declare a path
        if config.view_type == "MessageConsumer" && config.path.is_some() {
            errors.push(format!(
                "view '{}': MessageConsumer views must not declare a path",
                id
            ));
        }

        // DataView reference must exist
        if let HandlerConfig::Dataview { ref dataview } = config.handler {
            if !available_dataviews.contains(&dataview.to_string()) {
                errors.push(format!(
                    "view '{}': unknown dataview '{}'",
                    id, dataview
                ));
            }
        }

        // Streaming validation: requires CodeComponent handler, no pipeline stages, Rest only
        if config.streaming.unwrap_or(false) {
            let has_pipeline = config
                .event_handlers
                .as_ref()
                .map(|eh| {
                    !eh.pre_process.is_empty()
                        || !eh.handlers.is_empty()
                        || !eh.post_process.is_empty()
                })
                .unwrap_or(false);
            let is_codecomponent = matches!(config.handler, HandlerConfig::Codecomponent { .. });

            let streaming_errors = crate::streaming::validate_streaming(
                id,
                &config.view_type,
                true,
                has_pipeline,
                is_codecomponent,
            );
            errors.extend(streaming_errors);
        }

        // Polling validation: SSE/WS only, tick > 0, change_detect requires handler
        if let Some(ref polling) = config.polling {
            if config.view_type != "ServerSentEvents" && config.view_type != "Websocket" {
                errors.push(format!(
                    "view '{}': polling is only supported for ServerSentEvents and Websocket views",
                    id
                ));
            }
            if polling.tick_interval_ms == 0 {
                errors.push(format!(
                    "view '{}': polling.tick_interval_ms must be greater than 0",
                    id
                ));
            }
            if polling.diff_strategy == "change_detect" && polling.change_detect.is_none() {
                errors.push(format!(
                    "view '{}': polling.diff_strategy=change_detect requires a change_detect handler",
                    id
                ));
            }
        }

        // Allow DataView handler on SSE/WS views when polling is configured
        // (overrides the earlier "dataview handler is only supported for view_type=Rest" check)

        // Rate limit must be > 0 if specified
        if let Some(rpm) = config.rate_limit_per_minute {
            if rpm == 0 {
                errors.push(format!(
                    "view '{}': rate_limit_per_minute must be greater than 0",
                    id
                ));
            }
        }
    }

    errors
}

// ── Pipeline Error/Timeout Handlers (D5) ────────────────────

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder};
use rivers_runtime::view::HandlerStageConfig;

/// Execute on_error handlers when primary execution fails.
///
/// Per spec §4: on_error handlers receive the error context and may return
/// a custom error response. Handlers are tried in order; the first one
/// that returns a ViewResult wins.
pub async fn execute_on_error_handlers(
    pool: &ProcessPoolManager,
    handlers: &[HandlerStageConfig],
    ctx: &ViewContext,
    error: &ViewError,
) -> Option<ViewResult> {
    for handler in handlers {
        let entrypoint = Entrypoint {
            module: handler.module.clone(),
            function: handler.entrypoint.clone(),
            language: "javascript".to_string(),
        };

        let args = serde_json::json!({
            "error": error.to_string(),
            "request": {
                "method": ctx.request.method,
                "path": ctx.request.path,
            },
            "data": ctx.data,
            "resdata": ctx.resdata,
            "trace_id": ctx.trace_id,
        });

        let task_ctx = match TaskContextBuilder::new()
            .entrypoint(entrypoint)
            .args(args)
            .trace_id(ctx.trace_id.clone())
            .build()
        {
            Ok(c) => c,
            Err(_) => continue,
        };

        match pool.dispatch("default", task_ctx).await {
            Ok(result) => {
                // Parse the result as a ViewResult envelope
                if let Some(view_result) = parse_handler_view_result(&result.value) {
                    return Some(view_result);
                }
            }
            Err(_) => {
                // on_error handler itself failed — continue to next handler
                continue;
            }
        }
    }

    None
}

// ── Session Revalidation Handler (D6) ───────────────────────

/// Execute on_session_valid handler for periodic session revalidation.
///
/// Per spec: the handler receives the current session claims and returns
/// a boolean indicating whether the session should remain valid.
pub async fn execute_on_session_valid(
    pool: &ProcessPoolManager,
    handler: &HandlerStageConfig,
    session: &serde_json::Value,
    trace_id: &str,
) -> Result<bool, ViewError> {
    let entrypoint = Entrypoint {
        module: handler.module.clone(),
        function: handler.entrypoint.clone(),
        language: "javascript".to_string(),
    };

    let args = serde_json::json!({
        "session": session,
    });

    let task_ctx = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(trace_id.to_string())
        .build()
        .map_err(|e| ViewError::Pipeline(format!("session valid context build: {}", e)))?;

    let result = pool
        .dispatch("default", task_ctx)
        .await
        .map_err(|e| ViewError::Pipeline(format!("session valid dispatch: {}", e)))?;

    // Expect the handler to return { "valid": true/false }
    Ok(result
        .value
        .get("valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// Parse a handler result value as a ViewResult envelope.
///
/// Handlers may return `{ "status": 200, "headers": {...}, "body": ... }`.
fn parse_handler_view_result(value: &serde_json::Value) -> Option<ViewResult> {
    let status = value.get("status")?.as_u64()? as u16;
    let body = value
        .get("body")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let headers = value
        .get("headers")
        .and_then(|h| h.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    Some(ViewResult {
        status,
        headers,
        body,
    })
}

// ── Request-Time Schema Validation ───────────────────────────────

/// Validate input data against the active schema before DataView execution.
///
/// Per driver-schema-validation-spec §2.3: input validated before executor fires.
pub fn validate_input(
    data: &serde_json::Value,
    schema: Option<&rivers_runtime::rivers_driver_sdk::SchemaDefinition>,
    direction: rivers_runtime::rivers_driver_sdk::ValidationDirection,
) -> Result<(), ViewError> {
    if let Some(schema) = schema {
        rivers_runtime::rivers_driver_sdk::validation::validate_fields(data, schema, direction)
            .map_err(|e| ViewError::Validation(e.to_string()))
    } else {
        Ok(())
    }
}

/// Validate output data against the return schema after DataView execution.
///
/// Per driver-schema-validation-spec §2.3: output validated before response.
/// Failures are logged as warnings, not rejected (forward compatibility).
pub fn validate_output(
    data: &serde_json::Value,
    schema: Option<&rivers_runtime::rivers_driver_sdk::SchemaDefinition>,
) -> Option<String> {
    if let Some(schema) = schema {
        match rivers_runtime::rivers_driver_sdk::validation::validate_fields(
            data,
            schema,
            rivers_runtime::rivers_driver_sdk::ValidationDirection::Output,
        ) {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!(target: "rivers.validation", "output validation warning: {}", e);
                Some(e.to_string())
            }
        }
    } else {
        None
    }
}

// ── Error Types ──────────────────────────────────────────────────

/// View execution errors.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("method not allowed: {0}")]
    MethodNotAllowed(String),

    #[error("handler error: {0}")]
    Handler(String),

    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::view::{HandlerStageConfig, ViewEventHandlers};

    fn make_none_handler_view(with_pipeline: bool) -> ApiViewConfig {
        let event_handlers = if with_pipeline {
            Some(ViewEventHandlers {
                pre_process: vec![],
                handlers: vec![HandlerStageConfig {
                    module: "my_module".into(),
                    entrypoint: "handle".into(),
                    key: None,

                    on_failure: None,
                }],
                post_process: vec![],
                on_error: vec![],
            })
        } else {
            None
        };
        ApiViewConfig {
            view_type: "Rest".into(),
            path: Some("/api/computed".into()),
            method: Some("GET".into()),
            handler: HandlerConfig::None {},
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
            event_handlers,
            on_stream: None,
            ws_hooks: None,
            on_event: None,
            polling: None,
        }
    }

    #[tokio::test]
    async fn test_null_datasource_returns_null_primary() {
        let config = make_none_handler_view(true);
        let request = ParsedRequest::new("GET", "/api/computed");
        let mut ctx = ViewContext::new(
            request,
            "trace-1".into(),
            String::new(),
            String::new(),
            String::new(),
        );

        let result = execute_rest_view(&mut ctx, &config, None, None).await.unwrap();

        // Primary source should be null (no DataView executed)
        assert_eq!(result.body, serde_json::Value::Null);
        assert_eq!(result.status, 200);
        // resdata should be Null
        assert_eq!(ctx.resdata, serde_json::Value::Null);
    }

    #[test]
    fn test_null_handler_validation_requires_pipeline() {
        let mut views = HashMap::new();

        // View with None handler but no pipeline stages => error
        views.insert("no_pipeline".into(), make_none_handler_view(false));
        // View with None handler and pipeline stages => ok
        views.insert("with_pipeline".into(), make_none_handler_view(true));

        let errors = validate_views(&views, &[]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("no_pipeline"));
        assert!(errors[0].contains("handler type 'none'"));
    }

    #[test]
    fn test_null_handler_not_counted_as_unknown_dataview() {
        let mut views = HashMap::new();
        views.insert("computed".into(), make_none_handler_view(true));

        // No available dataviews — should not produce "unknown dataview" error
        let errors = validate_views(&views, &[]);
        assert!(errors.is_empty());
    }

    // ── D5: on_error tests ────────────────────────────────────

    #[tokio::test]
    async fn test_on_error_handlers_returns_none_when_pool_unavailable() {
        use crate::process_pool::ProcessPoolManager;

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let handlers = vec![HandlerStageConfig {
            module: "error_handler.js".into(),
            entrypoint: "on_error".into(),
            key: None,
            on_failure: None,
        }];
        let ctx = ViewContext::new(
            ParsedRequest::new("GET", "/test"),
            "trace-1".into(),
            String::new(),
            String::new(),
            String::new(),
        );
        let error = ViewError::Handler("test error".into());

        // Pool returns EngineUnavailable, so handler returns None
        let result = execute_on_error_handlers(&pool, &handlers, &ctx, &error).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_on_error_handlers_empty_list() {
        use crate::process_pool::ProcessPoolManager;

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let ctx = ViewContext::new(
            ParsedRequest::new("GET", "/test"),
            "trace-1".into(),
            String::new(),
            String::new(),
            String::new(),
        );
        let error = ViewError::Handler("test error".into());

        let result = execute_on_error_handlers(&pool, &[], &ctx, &error).await;
        assert!(result.is_none());
    }

    // ── D6: on_session_valid tests ──────────────────────────

    #[tokio::test]
    async fn test_on_session_valid_engine_unavailable() {
        use crate::process_pool::ProcessPoolManager;

        let pool = ProcessPoolManager::from_config(&HashMap::new());
        let handler = HandlerStageConfig {
            module: "session.js".into(),
            entrypoint: "validate".into(),
            key: None,
            on_failure: None,
        };
        let session = serde_json::json!({"user_id": "u-1"});

        let result = execute_on_session_valid(&pool, &handler, &session, "trace-1").await;
        // Should fail because the engine is unavailable
        assert!(result.is_err());
    }

    // ── parse_handler_view_result tests ─────────────────────

    #[test]
    fn test_parse_handler_view_result_valid() {
        let value = serde_json::json!({
            "status": 503,
            "headers": {"x-custom": "val"},
            "body": {"message": "service unavailable"},
        });
        let result = parse_handler_view_result(&value).unwrap();
        assert_eq!(result.status, 503);
        assert_eq!(result.headers.get("x-custom").unwrap(), "val");
        assert_eq!(result.body, serde_json::json!({"message": "service unavailable"}));
    }

    #[test]
    fn test_parse_handler_view_result_missing_status() {
        let value = serde_json::json!({"body": "hello"});
        assert!(parse_handler_view_result(&value).is_none());
    }

    #[test]
    fn test_parse_handler_view_result_minimal() {
        let value = serde_json::json!({"status": 200});
        let result = parse_handler_view_result(&value).unwrap();
        assert_eq!(result.status, 200);
        assert_eq!(result.body, serde_json::Value::Null);
        assert!(result.headers.is_empty());
    }

    // ── StoreHandle tests ───────────────────────────────────────

    #[test]
    fn store_handle_reserved_key_detection() {
        assert!(StoreHandle::is_reserved_key("session:abc"));
        assert!(StoreHandle::is_reserved_key("csrf:token-123"));
        assert!(StoreHandle::is_reserved_key("cache:views:orders"));
        assert!(StoreHandle::is_reserved_key("raft:state"));
        assert!(StoreHandle::is_reserved_key("rivers:internal"));
        assert!(!StoreHandle::is_reserved_key("user:prefs:123"));
        assert!(!StoreHandle::is_reserved_key("mykey"));
    }

    // ── S12: Request-time validation tests ───────────────────────

    fn make_test_schema() -> rivers_runtime::rivers_driver_sdk::SchemaDefinition {
        rivers_runtime::rivers_driver_sdk::SchemaDefinition {
            driver: "postgresql".into(),
            schema_type: "object".into(),
            description: String::new(),
            fields: vec![rivers_runtime::rivers_driver_sdk::SchemaFieldDef {
                name: "name".into(),
                field_type: "string".into(),
                required: true,
                constraints: std::collections::HashMap::new(),
            }],
            extra: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn validate_input_passes_valid_data() {
        let schema = make_test_schema();
        let data = serde_json::json!({"name": "alice"});
        assert!(
            validate_input(&data, Some(&schema), rivers_runtime::rivers_driver_sdk::ValidationDirection::Input)
                .is_ok()
        );
    }

    #[test]
    fn validate_input_rejects_missing_required() {
        let schema = make_test_schema();
        let data = serde_json::json!({});
        assert!(
            validate_input(&data, Some(&schema), rivers_runtime::rivers_driver_sdk::ValidationDirection::Input)
                .is_err()
        );
    }

    #[test]
    fn validate_output_warns_on_failure() {
        let schema = make_test_schema();
        let data = serde_json::json!({});
        let warning = validate_output(&data, Some(&schema));
        assert!(warning.is_some());
    }

    #[test]
    fn validate_output_none_on_success() {
        let schema = make_test_schema();
        let data = serde_json::json!({"name": "alice"});
        let warning = validate_output(&data, Some(&schema));
        assert!(warning.is_none());
    }

    #[test]
    fn validate_input_none_schema_passes() {
        let data = serde_json::json!({"anything": true});
        assert!(
            validate_input(&data, None, rivers_runtime::rivers_driver_sdk::ValidationDirection::Input).is_ok()
        );
    }

    #[test]
    fn validate_output_none_schema_passes() {
        let data = serde_json::json!({"anything": true});
        assert!(validate_output(&data, None).is_none());
    }

    // ── Namespaced path tests (AF4) ─────────────────────────────────

    #[test]
    fn build_namespaced_path_no_prefix() {
        assert_eq!(
            build_namespaced_path(None, "address-book", "service", "contacts"),
            "/address-book/service/contacts"
        );
    }

    #[test]
    fn build_namespaced_path_with_prefix() {
        assert_eq!(
            build_namespaced_path(Some("v1"), "address-book", "service", "contacts/{id}"),
            "/v1/address-book/service/contacts/{id}"
        );
    }

    #[test]
    fn build_namespaced_path_strips_leading_slash() {
        assert_eq!(
            build_namespaced_path(None, "myapp", "api", "/users"),
            "/myapp/api/users"
        );
    }

    #[test]
    fn build_namespaced_path_empty_view() {
        assert_eq!(
            build_namespaced_path(None, "address-book", "main", ""),
            "/address-book/main"
        );
    }

    #[test]
    fn build_namespaced_path_empty_prefix_treated_as_none() {
        assert_eq!(
            build_namespaced_path(Some(""), "address-book", "service", "contacts"),
            "/address-book/service/contacts"
        );
    }
}
