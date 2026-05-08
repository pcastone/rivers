//! View pipeline — parameter mapping, execution, and response serialization.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use rivers_runtime::view::{ApiViewConfig, HandlerConfig};

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder};

use super::types::{ViewContext, ViewError, ViewResult};
use super::validation::{execute_on_error_handlers, parse_handler_view_result};

/// Default per-observer dispatch cap (G_R4.1, P2-4).
///
/// Pre/post-process observer handlers are awaited so the spec contract
/// (`Rivers.observer.before/after` is non-`spawn`'d) holds, but if a
/// misbehaving handler hangs we MUST NOT extend request latency. The
/// dispatch is wrapped in `tokio::time::timeout(OBSERVER_TIMEOUT, ...)`
/// — on elapsed we log a warning and continue. Configurable via
/// `RIVERS_OBSERVER_TIMEOUT_MS`.
const DEFAULT_OBSERVER_TIMEOUT_MS: u64 = 200;

fn observer_timeout() -> Duration {
    static CACHED: OnceLock<Duration> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let ms = std::env::var("RIVERS_OBSERVER_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_OBSERVER_TIMEOUT_MS);
        Duration::from_millis(ms)
    })
}

/// Dispatch a pre/post-process observer with the configured cap (G_R4.2).
///
/// Returns immediately on timeout — the request must NOT block longer than
/// the cap. A timeout is logged at WARN level; the dispatched task is
/// allowed to continue running in the background (the ProcessPool watchdog
/// owns its CPU budget).
async fn dispatch_observer(
    pool: &ProcessPoolManager,
    task_ctx: rivers_runtime::process_pool::TaskContext,
    stage: &'static str,
    trace_id: &str,
    module: &str,
    entrypoint: &str,
) {
    let timeout = observer_timeout();
    let fut = pool.dispatch("default", task_ctx);
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!(
                target: "rivers.view",
                stage,
                trace_id = %trace_id,
                module = %module,
                entrypoint = %entrypoint,
                error = %e,
                "observer dispatch returned error (request continues)"
            );
        }
        Err(_) => {
            tracing::warn!(
                target: "rivers.view",
                stage,
                trace_id = %trace_id,
                module = %module,
                entrypoint = %entrypoint,
                timeout_ms = timeout.as_millis() as u64,
                "observer exceeded timeout cap (request not delayed)"
            );
        }
    }
}

// ── Parameter Mapping ────────────────────────────────────────────

/// Apply parameter mapping from request to DataView parameters.
///
/// Per spec §5.2 and AMD-4: parameter_mapping.query, .path, and .body subtables.
pub fn apply_parameter_mapping(
    request: &super::types::ParsedRequest,
    config: &ApiViewConfig,
) -> HashMap<String, serde_json::Value> {
    let mut params = HashMap::new();

    if let Some(ref mapping) = config.parameter_mapping {
        // Map query parameters — handle multi-value for array types (spec §5.2)
        for (http_param, dv_param) in &mapping.query {
            // Check queryAll for multi-value (Pattern 1: repeated key)
            if let Some(values) = request.query_all.get(http_param) {
                if values.len() > 1 {
                    // Multiple values — pass as JSON array
                    let arr: Vec<serde_json::Value> = values.iter()
                        .map(|v| serde_json::Value::String(v.clone()))
                        .collect();
                    params.insert(dv_param.clone(), serde_json::Value::Array(arr));
                    continue;
                }
            }
            // Single value
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

        // Map header parameters (spec §4.1)
        for (header_name, dv_param) in &mapping.header {
            if let Some(value) = request.headers.get(header_name) {
                params.insert(dv_param.clone(), serde_json::Value::String(value.clone()));
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
        // ── Pre_process (observer, awaited-with-timeout per G_R4) ────
        // Spec contract: pre/post-process observers are awaited (so handlers
        // have a chance to mutate ctx.data before/after the primary stage)
        // but each dispatch is bounded by `RIVERS_OBSERVER_TIMEOUT_MS`
        // (default 200ms) so a slow observer cannot extend request latency.
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
                let builder = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone());
                let builder = crate::task_enrichment::enrich(
                    builder,
                    &ctx.dv_namespace,
                    rivers_runtime::process_pool::TaskKind::PreProcess,
                );
                let task_ctx = builder
                    .build()
                    .map_err(|e| ViewError::Pipeline(format!("pre_process build: {e}")))?;
                dispatch_observer(
                    pool,
                    task_ctx,
                    "pre_process",
                    &ctx.trace_id,
                    &handler.module,
                    &handler.entrypoint,
                )
                .await;
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
                        .execute(dataview, query_params, &ctx.request.method, &ctx.trace_id, None)
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
                        "_dv_namespace": ctx.dv_namespace,
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
                    // Wire datasources into the task context.
                    // - Filesystem: DatasourceToken::Direct (in-process dispatch).
                    // - Broker: DatasourceToken::Broker + datasource_config (lazy producer).
                    // - All others (SQL, NoSQL, etc.): datasource_config only — used by
                    //   ctx.transaction() and Rivers.db.begin() to open a connection
                    //   on demand. No token needed; driver dispatch is via DriverFactory.
                    builder = crate::task_enrichment::wire_datasources(
                        builder,
                        executor,
                        &ctx.dv_namespace,
                    );
                    let builder = crate::task_enrichment::enrich(
                        builder,
                        &ctx.dv_namespace,
                        rivers_runtime::process_pool::TaskKind::Rest,
                    );
                    let task_ctx = builder
                        .build()
                        .map_err(|e| ViewError::Handler(format!("codecomponent build: {e}")))?;

                    let result = pool
                        .dispatch("default", task_ctx)
                        .await
                        .map_err(|e| match e {
                            // Spec §5.3: preserve the remapped stack so the
                            // error-response serializer can include it in the
                            // `debug.stack` envelope when debug is enabled.
                            rivers_runtime::process_pool::TaskError::HandlerErrorWithStack {
                                message,
                                stack,
                            } => ViewError::HandlerWithStack {
                                message: format!("codecomponent dispatch: {message}"),
                                stack,
                            },
                            // Spec §6 commit-failure: unknown transaction
                            // state must surface as a distinct error class so
                            // the client can pick retry policy correctly.
                            rivers_runtime::process_pool::TaskError::TransactionCommitFailed {
                                datasource,
                                message,
                            } => ViewError::TransactionCommitFailed { datasource, message },
                            other => ViewError::Handler(format!("codecomponent dispatch: {other}")),
                        })?;

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
                let builder = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone());
                let builder = crate::task_enrichment::enrich(
                    builder,
                    &ctx.dv_namespace,
                    rivers_runtime::process_pool::TaskKind::Rest,
                );
                let task_ctx = builder
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

        // ── Post_process (observer, awaited-with-timeout per G_R4) ──
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
                let builder = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone());
                let builder = crate::task_enrichment::enrich(
                    builder,
                    &ctx.dv_namespace,
                    rivers_runtime::process_pool::TaskKind::PostProcess,
                );
                let task_ctx = builder
                    .build()
                    .map_err(|e| ViewError::Pipeline(format!("post_process build: {e}")))?;
                dispatch_observer(
                    pool,
                    task_ctx,
                    "post_process",
                    &ctx.trace_id,
                    &handler.module,
                    &handler.entrypoint,
                )
                .await;
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
