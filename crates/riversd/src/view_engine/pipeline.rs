//! View pipeline — parameter mapping, execution, and response serialization.

use std::collections::HashMap;

use rivers_runtime::view::{ApiViewConfig, HandlerConfig};

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder};

use super::types::{ViewContext, ViewError, ViewResult};
use super::validation::{execute_on_error_handlers, parse_handler_view_result};

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
                let builder = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone());
                let builder = crate::task_enrichment::enrich(builder, &ctx.app_id);
                let task_ctx = builder
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
                    let builder = crate::task_enrichment::enrich(builder, &ctx.app_id);
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
                let builder = crate::task_enrichment::enrich(builder, &ctx.app_id);
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
                let builder = TaskContextBuilder::new()
                    .entrypoint(entrypoint)
                    .args(args)
                    .trace_id(ctx.trace_id.clone());
                let builder = crate::task_enrichment::enrich(builder, &ctx.app_id);
                let task_ctx = builder
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
