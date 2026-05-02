//! Admin API route handlers extracted from server.rs (AN13.1).
//!
//! Contains all `/admin/*` endpoint handlers, shared helpers,
//! and the `register_all_drivers` function they depend on.

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::error_response;
use crate::server::{AppContext, register_all_drivers};

// ── Shared Admin Helpers ─────────────────────────────────────────

/// Parse the request body as JSON, returning `Value::Null` on failure.
///
/// Used by admin handlers to avoid repeating the body-read + deserialize pattern.
pub async fn parse_json_body(request: Request, limit: usize) -> serde_json::Value {
    axum::body::to_bytes(request.into_body(), limit)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Default body size limit for admin handlers (16 MiB).
pub const ADMIN_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Return a 503 error response indicating the log controller is not initialized.
pub fn log_controller_unavailable() -> Response {
    error_response::service_unavailable("log controller not initialized")
        .into_axum_response()
}

// ── Admin Route Handlers ─────────────────────────────────────────

/// Admin status endpoint.
pub async fn admin_status_handler(State(ctx): State<AppContext>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "draining": ctx.shutdown.is_draining(),
        "inflight": ctx.shutdown.inflight_count(),
    }))
}

/// Admin drivers endpoint.
///
/// Per spec §15.5: list registered driver names and types.
pub async fn admin_drivers_handler(State(_ctx): State<AppContext>) -> impl IntoResponse {
    // Compute the driver list once — it never changes at runtime.
    use std::sync::LazyLock;
    static DRIVER_LIST: LazyLock<serde_json::Value> = LazyLock::new(|| {
        let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
        register_all_drivers(&mut factory, &[]);

        let mut drivers: Vec<serde_json::Value> = Vec::new();
        for name in factory.driver_names() {
            drivers.push(serde_json::json!({"name": name, "type": "database"}));
        }
        for name in factory.broker_driver_names() {
            drivers.push(serde_json::json!({"name": name, "type": "broker"}));
        }
        let count = drivers.len();
        serde_json::json!({ "drivers": drivers, "count": count })
    });

    Json(DRIVER_LIST.clone())
}

/// Admin datasources endpoint.
///
/// Per spec §15.5: list configured datasources from loaded app config.
pub async fn admin_datasources_handler(State(ctx): State<AppContext>) -> impl IntoResponse {
    let exec = ctx.dataview_executor.read().await;
    if let Some(executor) = exec.as_ref() {
        let datasources = executor.datasource_info();
        let count = datasources.len();
        Json(serde_json::json!({
            "datasources": datasources,
            "count": count,
        }))
    } else {
        Json(serde_json::json!({
            "datasources": [],
            "count": 0,
            "note": "no bundle deployed",
        }))
    }
}

/// Admin deploy — initiate a new bundle deployment.
///
/// Per spec §15.6: accepts bundle payload, returns deployment ID.
pub async fn admin_deploy_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> impl IntoResponse {
    let body_json = parse_json_body(request, ADMIN_BODY_LIMIT).await;

    let bundle_path = body_json
        .get("bundle_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let app_id = body_json
        .get("app_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    tracing::info!(target: "rivers.admin", bundle_path = bundle_path, "deployment initiated");

    let deployment = ctx.deployment_manager.create(app_id, bundle_path.to_string()).await;

    Json(serde_json::json!({
        "status": "accepted",
        "deploy_id": deployment.deploy_id,
        "bundle_path": bundle_path,
        "state": deployment.state,
    }))
}

/// Admin deploy test — run preflight checks for a bundle.
///
/// Per spec §15.6: validates bundle structure, resource resolution.
pub async fn admin_deploy_test_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> Response {
    let body_json = parse_json_body(request, ADMIN_BODY_LIMIT).await;

    let deploy_id = body_json.get("deploy_id").and_then(|v| v.as_str()).unwrap_or("");

    match ctx.deployment_manager.get(deploy_id).await {
        Some(deployment) => {
            // Validate bundle path exists
            let bundle_path = std::path::Path::new(&deployment.bundle_name);
            let validation = if bundle_path.is_dir() {
                match rivers_runtime::load_bundle(bundle_path) {
                    Ok(bundle) => match rivers_runtime::validate_bundle(&bundle) {
                        Ok(()) => serde_json::json!({"valid": true, "errors": []}),
                        Err(errors) => {
                            let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
                            serde_json::json!({"valid": false, "errors": msgs})
                        }
                    },
                    Err(e) => serde_json::json!({"valid": false, "errors": [e.to_string()]}),
                }
            } else {
                serde_json::json!({"valid": false, "errors": ["bundle path not found"]})
            };

            Json(serde_json::json!({
                "status": "test_complete",
                "deploy_id": deploy_id,
                "validation": validation,
            })).into_response()
        }
        None => error_response::not_found(format!("deployment '{}' not found", deploy_id))
            .into_axum_response(),
    }
}

/// Shared logic for deployment state transitions (approve, reject, promote).
///
/// Parses `deploy_id` from the request body, calls `transition()`, and
/// returns a JSON response with the given `status_label`.
pub async fn deploy_transition_handler(
    ctx: AppContext,
    request: Request,
    target_state: crate::admin::DeploymentState,
    status_label: &str,
) -> Response {
    let body_json = parse_json_body(request, ADMIN_BODY_LIMIT).await;
    let deploy_id = body_json.get("deploy_id").and_then(|v| v.as_str()).unwrap_or("");

    match ctx.deployment_manager.transition(deploy_id, target_state).await {
        Ok(()) => {
            let deployment = ctx.deployment_manager.get(deploy_id).await;
            Json(serde_json::json!({
                "status": status_label,
                "deploy_id": deploy_id,
                "state": deployment.map(|d| d.state),
            })).into_response()
        }
        Err(e) => error_response::bad_request(e.to_string())
            .into_axum_response(),
    }
}

/// Admin deploy approve — approve a pending deployment.
///
/// Per spec §15.6: transitions deployment from PENDING to RESOLVING.
pub async fn admin_deploy_approve_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> impl IntoResponse {
    deploy_transition_handler(ctx, request, crate::admin::DeploymentState::Resolving, "approved").await
}

/// Admin deploy reject — reject a pending deployment.
///
/// Per spec §15.6: marks deployment as FAILED.
pub async fn admin_deploy_reject_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> impl IntoResponse {
    deploy_transition_handler(ctx, request, crate::admin::DeploymentState::Failed, "rejected").await
}

/// Admin deploy promote — promote a canary deployment.
///
/// Per spec §15.6: transitions to full traffic (Starting → Running).
pub async fn admin_deploy_promote_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> impl IntoResponse {
    deploy_transition_handler(ctx, request, crate::admin::DeploymentState::Running, "promoted").await
}

/// Admin deployments list — list all active and recent deployments.
///
/// Per spec §15.6: returns deployment state machine entries.
pub async fn admin_deployments_handler(State(ctx): State<AppContext>) -> impl IntoResponse {
    let deployments = ctx.deployment_manager.list().await;
    Json(serde_json::json!({
        "deployments": deployments,
        "count": deployments.len(),
    }))
}

/// Admin log levels — list current log levels.
///
/// Per spec §15.8: runtime log level inspection.
pub async fn admin_log_levels_handler(State(ctx): State<AppContext>) -> impl IntoResponse {
    let current = if let Some(controller) = &ctx.log_controller {
        controller.current()
    } else {
        // Fall back to config level when no reload handle is available (e.g. tests)
        format!("{:?}", ctx.config.base.log_level).to_lowercase()
    };
    Json(serde_json::json!({
        "levels": {
            "global": current,
        },
    }))
}

/// Admin log set — update log levels at runtime.
///
/// Per spec §15.8: dynamic log level adjustment.
pub async fn admin_log_set_handler(State(ctx): State<AppContext>, request: Request) -> Response {
    let body_json = parse_json_body(request, 1024 * 1024).await;

    let target = body_json.get("target").and_then(|v| v.as_str()).unwrap_or("global");
    let level = body_json.get("level").and_then(|v| v.as_str()).unwrap_or("info");

    // Validate level
    let valid = matches!(level, "trace" | "debug" | "info" | "warn" | "error");
    if !valid {
        return error_response::bad_request(
            format!("invalid level '{}' — must be trace, debug, info, warn, or error", level),
        ).into_axum_response();
    }

    // Build filter directive: target=level (or just level for global)
    let filter = if target == "global" {
        level.to_string()
    } else {
        format!("{}={}", target, level)
    };

    match &ctx.log_controller {
        Some(controller) => match controller.set(&filter) {
            Ok(()) => {
                tracing::info!(target: "rivers.admin", log_target = target, level = level, "log level updated");
                Json(serde_json::json!({
                    "status": "updated",
                    "target": target,
                    "level": level,
                    "filter": filter,
                })).into_response()
            }
            Err(e) => error_response::internal_error(
                format!("failed to update log level: {}", e),
            ).into_axum_response(),
        },
        None => log_controller_unavailable(),
    }
}

/// Admin log reset — reset log levels to defaults.
///
/// Per spec §15.8.
pub async fn admin_log_reset_handler(State(ctx): State<AppContext>) -> Response {
    match &ctx.log_controller {
        Some(controller) => match controller.reset() {
            Ok(()) => {
                let current = controller.current();
                tracing::info!(target: "rivers.admin", filter = %current, "log levels reset to defaults");
                Json(serde_json::json!({
                    "status": "reset",
                    "filter": current,
                })).into_response()
            }
            Err(e) => error_response::internal_error(
                format!("failed to reset log level: {}", e),
            ).into_axum_response(),
        },
        None => log_controller_unavailable(),
    }
}

// ── Circuit Breaker Endpoints ────────────────────────────────────

/// List all circuit breakers for a specific app.
/// GET /admin/apps/:app_id/breakers
pub async fn admin_list_breakers_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path(app_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let entries = ctx.circuit_breaker_registry.list_for_app(&app_id).await;
    Json(serde_json::json!(entries))
}

/// Get a single circuit breaker's status.
/// GET /admin/apps/:app_id/breakers/:breaker_id
pub async fn admin_get_breaker_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path((app_id, breaker_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match ctx.circuit_breaker_registry.get(&app_id, &breaker_id).await {
        Some(entry) => Json(serde_json::json!(entry)).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("breaker '{}' not found for app '{}'", breaker_id, app_id)})),
        ).into_response(),
    }
}

/// Trip (open) a circuit breaker.
/// POST /admin/apps/:app_id/breakers/:breaker_id/trip
pub async fn admin_trip_breaker_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path((app_id, breaker_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match ctx.circuit_breaker_registry.trip(&app_id, &breaker_id).await {
        Some(entry) => {
            if let Some(ref storage) = ctx.storage_engine {
                let key = format!("breaker:{}:{}", app_id, breaker_id);
                if let Err(e) = storage.set("rivers", &key, b"open".to_vec(), None).await {
                    tracing::error!(app_id = %app_id, breaker = %breaker_id, error = %e, "failed to persist breaker state");
                }
            }
            tracing::info!(app_id = %app_id, breaker = %breaker_id, "circuit breaker TRIPPED");
            Json(serde_json::json!(entry)).into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("breaker '{}' not found for app '{}'", breaker_id, app_id)})),
        ).into_response(),
    }
}

/// Reset (close) a circuit breaker.
/// POST /admin/apps/:app_id/breakers/:breaker_id/reset
pub async fn admin_reset_breaker_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path((app_id, breaker_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match ctx.circuit_breaker_registry.reset(&app_id, &breaker_id).await {
        Some(entry) => {
            if let Some(ref storage) = ctx.storage_engine {
                let key = format!("breaker:{}:{}", app_id, breaker_id);
                if let Err(e) = storage.set("rivers", &key, b"closed".to_vec(), None).await {
                    tracing::error!(app_id = %app_id, breaker = %breaker_id, error = %e, "failed to persist breaker state");
                }
            }
            tracing::info!(app_id = %app_id, breaker = %breaker_id, "circuit breaker RESET");
            Json(serde_json::json!(entry)).into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("breaker '{}' not found for app '{}'", breaker_id, app_id)})),
        ).into_response(),
    }
}

// ── Bundle Introspection ─────────────────────────────────────────

/// Build the JSON payload for `GET /admin/bundle`.
///
/// When no bundle is deployed (`None`), returns a minimal "not deployed" object.
/// When a bundle is loaded (`Some`), iterates all apps and emits datasources,
/// DataView names, view summaries, and MCP surface lists.
pub fn bundle_introspection_json(
    bundle: Option<&std::sync::Arc<rivers_runtime::LoadedBundle>>,
) -> serde_json::Value {
    let Some(bundle) = bundle else {
        return serde_json::json!({
            "bundle_name": "none",
            "bundle_version": "none",
            "apps": [],
            "note": "no bundle deployed",
        });
    };

    let apps: Vec<serde_json::Value> = bundle.apps.iter().map(|app| {
        // Datasource names from resources
        let datasources: Vec<&str> = app.resources.datasources.iter()
            .map(|ds| ds.name.as_str())
            .collect();

        // DataView names from app config
        let dataviews: Vec<&str> = app.config.data.dataviews.keys()
            .map(|k| k.as_str())
            .collect();

        // Views summary + MCP accumulation
        let mut mcp_tools: Vec<&str> = Vec::new();
        let mut mcp_resources: Vec<&str> = Vec::new();
        let mut mcp_prompts: Vec<&str> = Vec::new();

        let views: Vec<serde_json::Value> = app.config.api.views.iter().map(|(name, view)| {
            let handler_label = match &view.handler {
                rivers_runtime::view::HandlerConfig::Dataview { .. } => "dataview",
                rivers_runtime::view::HandlerConfig::Codecomponent { .. } => "codecomponent",
                rivers_runtime::view::HandlerConfig::None {} => "none",
            };

            // Collect MCP surfaces from Mcp-type views
            if view.view_type == "Mcp" {
                for k in view.tools.keys() { mcp_tools.push(k.as_str()); }
                for k in view.resources.keys() { mcp_resources.push(k.as_str()); }
                for k in view.prompts.keys() { mcp_prompts.push(k.as_str()); }
            }

            serde_json::json!({
                "name": name,
                "type": view.view_type,
                "path": view.path.as_deref().unwrap_or(""),
                "method": view.method.as_deref().unwrap_or(""),
                "handler": handler_label,
            })
        }).collect();

        serde_json::json!({
            "app_name": app.manifest.app_name,
            "app_id": app.manifest.app_id,
            "app_type": app.manifest.app_type,
            "datasources": datasources,
            "dataviews": dataviews,
            "views": views,
            "mcp": {
                "tools": mcp_tools,
                "resources": mcp_resources,
                "prompts": mcp_prompts,
            },
        })
    }).collect();

    serde_json::json!({
        "bundle_name": bundle.manifest.bundle_name,
        "bundle_version": bundle.manifest.bundle_version,
        "apps": apps,
    })
}

/// GET /admin/bundle — structured introspection of the loaded bundle.
///
/// Returns all apps, views, DataViews, datasources, and MCP surfaces.
pub async fn admin_bundle_handler(State(ctx): State<AppContext>) -> impl IntoResponse {
    Json(bundle_introspection_json(ctx.loaded_bundle.as_ref()))
}

// ── Audit Stream Endpoint ────────────────────────────────────────

/// GET /admin/audit/stream — SSE stream of structured audit events.
///
/// Requires `[audit] enabled = true` in `riversd.toml`.
/// Returns 503 when audit is disabled.
///
/// Each SSE frame carries a newline-delimited JSON audit event:
/// ```text
/// data: {"event":"handler_invoked","app_id":"...","view":"...","method":"GET","path":"/","duration_ms":5,"status":200}
/// ```
///
/// On `RecvError::Lagged`, skipped events are silently dropped and the stream continues.
/// The stream ends when the broadcast channel is closed (server shutdown).
pub async fn admin_audit_stream_handler(State(ctx): State<AppContext>) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::wrappers::BroadcastStream;
    use tokio_stream::StreamExt as _;

    let bus = match ctx.audit_bus.as_ref() {
        Some(b) => b.clone(),
        None => {
            return axum::response::Response::builder()
                .status(503)
                .header("content-type", "text/plain")
                .body(axum::body::Body::from("audit not enabled"))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()));
        }
    };

    let rx = bus.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(event) => {
                let json = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok::<_, std::convert::Infallible>(Event::default().data(json)))
            }
            // Lagged — drop skipped events, continue streaming
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30)).text(""))
        .into_response()
}

// ── Shutdown Endpoint ────────────────────────────────────────────

/// POST /admin/shutdown
///
/// Body: `{"mode": "graceful"}` or `{"mode": "immediate"}`
///
/// Graceful: marks server as draining, in-flight requests complete, then exits.
/// Immediate: exits the process after response flushes.
pub async fn admin_shutdown_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> impl IntoResponse {
    let body = parse_json_body(request, 1024).await;
    let mode = body
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("graceful");

    match mode {
        "immediate" => {
            tracing::warn!("admin API: immediate shutdown requested");
            let response = Json(serde_json::json!({
                "status": "shutting_down",
                "mode": "immediate"
            }));
            // Spawn exit after short delay to allow response to flush
            tokio::spawn(async {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                std::process::exit(0);
            });
            response.into_response()
        }
        _ => {
            tracing::info!("admin API: graceful shutdown requested");
            ctx.shutdown.mark_draining();
            if let Some(ref tx) = ctx.shutdown_tx {
                let _ = tx.send(true);
            }
            Json(serde_json::json!({
                "status": "shutting_down",
                "mode": "graceful"
            })).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_introspection_none_returns_not_deployed() {
        let result = bundle_introspection_json(None);
        assert_eq!(result["bundle_name"], "none");
        assert!(result["apps"].as_array().unwrap().is_empty());
    }
}
