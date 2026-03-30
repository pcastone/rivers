//! View validation — config validation, error handlers, session revalidation.

use std::collections::HashMap;

use rivers_runtime::view::{ApiViewConfig, HandlerConfig, HandlerStageConfig};

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder};

use super::types::{ViewContext, ViewError, ViewResult};

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

        let builder = TaskContextBuilder::new()
            .entrypoint(entrypoint)
            .args(args)
            .trace_id(ctx.trace_id.clone());
        let builder = crate::task_enrichment::enrich(builder, &ctx.app_id);
        let task_ctx = match builder.build() {
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

    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(trace_id.to_string());
    let builder = crate::task_enrichment::enrich(builder, "");
    let task_ctx = builder
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
pub(super) fn parse_handler_view_result(value: &serde_json::Value) -> Option<ViewResult> {
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
