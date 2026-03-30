//! Basic route handlers — health, gossip, static files, services discovery.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use crate::error_response;
use crate::static_files;
use crate::view_engine;

use super::context::AppContext;

// ── Route Handlers ────────────────────────────────────────────────

/// Health check — returns 200 with simple status.
///
/// Per spec §14.1: always 200, basic status.
pub(super) async fn health_handler(request: Request) -> impl IntoResponse {
    // simulate_delay_ms support per spec §14.3
    if let Some(delay) = crate::health::parse_simulate_delay(request.uri().query()) {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    Json(crate::health::HealthResponse::ok(
        "riversd".to_string(),
        "default".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    ))
}

/// Verbose health — returns extended status info.
///
/// Per spec §14.2: pool snapshots, cluster state, uptime.
pub(super) async fn health_verbose_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> impl IntoResponse {
    if let Some(delay) = crate::health::parse_simulate_delay(request.uri().query()) {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    // Run datasource connectivity probes
    let datasource_probes = {
        let exec_guard = ctx.dataview_executor.read().await;
        if let Some(ref executor) = *exec_guard {
            let factory = executor.factory().clone();
            let params = executor.datasource_params().clone();
            drop(exec_guard); // release lock before probing

            let mut probes = Vec::new();
            for (name, ds_params) in params.iter() {
                let driver_name = ds_params.options.get("driver")
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");

                let start = std::time::Instant::now();
                let probe = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    factory.connect(driver_name, ds_params),
                ).await {
                    Ok(Ok(_conn)) => crate::health::DatasourceProbeResult {
                        name: name.clone(),
                        driver: driver_name.to_string(),
                        status: "ok".to_string(),
                        latency_ms: start.elapsed().as_millis() as u64,
                        error: None,
                    },
                    Ok(Err(e)) => crate::health::DatasourceProbeResult {
                        name: name.clone(),
                        driver: driver_name.to_string(),
                        status: "error".to_string(),
                        latency_ms: start.elapsed().as_millis() as u64,
                        error: Some(e.to_string()),
                    },
                    Err(_) => crate::health::DatasourceProbeResult {
                        name: name.clone(),
                        driver: driver_name.to_string(),
                        status: "error".to_string(),
                        latency_ms: 5000,
                        error: Some("probe timeout (5s)".to_string()),
                    },
                };
                probes.push(probe);
            }
            probes.sort_by(|a, b| a.name.cmp(&b.name));
            probes
        } else {
            drop(exec_guard);
            Vec::new()
        }
    };

    Json(crate::health::VerboseHealthResponse {
        status: "ok",
        service: "riversd".to_string(),
        environment: "default".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        draining: ctx.shutdown.is_draining(),
        inflight_requests: ctx.shutdown.inflight_count() as u64,
        uptime_seconds: ctx.uptime.uptime_seconds(),
        pool_snapshots: Vec::new(), // populated when pool manager is wired
        datasource_probes,
    })
}

/// Gossip receive endpoint.
pub(super) async fn gossip_receive_handler() -> impl IntoResponse {
    // Gossip processing deferred to RPS epic
    StatusCode::OK
}

/// Static file handler — serves files from the configured root directory.
///
/// Per spec §7-8: path resolution, ETag, Cache-Control, SPA fallback.
/// Returns 404 if static files are not enabled.
pub(super) async fn static_file_handler(State(ctx): State<AppContext>, request: Request) -> impl IntoResponse {
    if !ctx.config.static_files.enabled {
        return error_response::not_found("not found").into_axum_response();
    }

    let path = request.uri().path();
    let if_none_match = request
        .headers()
        .get("if-none-match")
        .and_then(|v| v.to_str().ok());

    static_files::serve_static_file(&ctx.config.static_files, path, if_none_match).await
}

// ── Services Discovery Handler ───────────────────────────────────

/// Services discovery endpoint — returns JSON list of available services.
///
/// Per spec §3.2 / §7.2: app-main exposes `/<bundle>/<main>/services`
/// so the SPA knows where to find app-services.
pub(super) async fn services_discovery_handler(
    State(ctx): State<AppContext>,
    _request: Request,
) -> impl IntoResponse {
    let bundle = match &ctx.loaded_bundle {
        Some(b) => b,
        None => return Json(serde_json::json!([])).into_response(),
    };

    let bundle_name = &bundle.manifest.bundle_name;
    let prefix = ctx.config.route_prefix.as_deref();

    // Build service list from all app-main's declared services
    let mut services = Vec::new();
    for app in &bundle.apps {
        if app.manifest.app_type != "app-main" {
            continue;
        }
        // For each service dependency in this app-main's resources
        for svc_dep in &app.resources.services {
            // Find the target app in the bundle by appId
            if let Some(target_app) = bundle.apps.iter().find(|a| a.manifest.app_id == svc_dep.app_id) {
                let entry_point = target_app
                    .manifest
                    .entry_point
                    .as_deref()
                    .unwrap_or(&target_app.manifest.app_name);
                let url = view_engine::build_namespaced_path(prefix, bundle_name, entry_point, "");
                services.push(serde_json::json!({
                    "name": svc_dep.name,
                    "url": url,
                }));
            }
        }
    }

    Json(serde_json::json!(services)).into_response()
}
