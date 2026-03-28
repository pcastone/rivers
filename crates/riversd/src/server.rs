//! HTTP server entry point and router construction.
//!
//! Per `rivers-httpd-spec.md` §1-3, §13.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use sha2::Digest;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::middleware as axum_middleware;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use tokio_stream::StreamExt as _;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

use rivers_runtime::rivers_core::ServerConfig;
use rivers_runtime::rivers_core::EventBus;

use crate::backpressure::BackpressureState;
use crate::cors::CorsConfig;
use crate::deployment::DeploymentManager;
use crate::error_response;
use crate::health::UptimeTracker;
use crate::hot_reload::{FileWatcher, HotReloadState};
use crate::middleware;
use crate::process_pool::ProcessPoolManager;
use crate::shutdown::ShutdownCoordinator;
use crate::sse::SseRouteManager;
use crate::static_files;
use crate::view_engine;
use crate::websocket::WebSocketRouteManager;

use rivers_runtime::DataViewExecutor;

// ── LogController ─────────────────────────────────────────────────

/// Runtime log level controller.
///
/// Type-erases the `tracing_subscriber::reload::Handle` via a closure so
/// `AppContext` can store it without carrying complex generic parameters.
/// Created during tracing setup in main.rs and injected into AppContext.
pub struct LogController {
    initial_filter: String,
    current_filter: std::sync::RwLock<String>,
    reload_fn: Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
}

impl LogController {
    /// Create a new controller.
    ///
    /// `initial` is the initial filter directive string (e.g. `"info"`).
    /// `reload_fn` accepts a new filter string and applies it to the
    /// underlying tracing subscriber.
    pub fn new(
        initial: impl Into<String>,
        reload_fn: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let initial = initial.into();
        Self {
            current_filter: std::sync::RwLock::new(initial.clone()),
            initial_filter: initial,
            reload_fn: Box::new(reload_fn),
        }
    }

    /// Return the current active filter directive.
    pub fn current(&self) -> String {
        self.current_filter
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Apply a new filter directive.
    pub fn set(&self, filter: &str) -> Result<(), String> {
        (self.reload_fn)(filter)?;
        *self.current_filter
            .write()
            .unwrap_or_else(|e| e.into_inner()) = filter.to_string();
        Ok(())
    }

    /// Reset to the initial filter directive.
    pub fn reset(&self) -> Result<(), String> {
        let initial = self.initial_filter.clone();
        (self.reload_fn)(&initial)?;
        *self.current_filter
            .write()
            .unwrap_or_else(|e| e.into_inner()) = initial;
        Ok(())
    }
}

// ── AppContext ─────────────────────────────────────────────────────

/// Shared application context — passed to all request handlers.
///
/// Per spec §2 step 16: all subsystems wired together.
///
/// # Planned Decomposition (after Wave 5)
///
/// This struct will be split into logical sub-structs:
///   - `AppContext.security`  → lockbox_resolver, keystore_resolver, csrf_manager, admin_auth_config, session_manager
///   - `AppContext.storage`   → storage_engine, event_bus
///   - `AppContext.routing`   → view_router, dataview_executor, graphql_schema
///   - `AppContext.engines`   → pool, driver_factory
///   - `AppContext.streaming` → sse_manager, ws_manager
///   - `AppContext.lifecycle` → shutdown, uptime, deployment_manager, hot_reload_state, config_path, loaded_bundle, guard_view_id, shutdown_tx
///   - `AppContext.config`    → config, log_controller
#[derive(Clone)]
pub struct AppContext {
    pub config: ServerConfig,
    pub shutdown: Arc<ShutdownCoordinator>,
    pub uptime: Arc<UptimeTracker>,
    /// ProcessPool for CodeComponent execution.
    pub pool: Arc<ProcessPoolManager>,
    /// View router built from deployed app configs.
    pub view_router: Arc<tokio::sync::RwLock<Option<view_engine::ViewRouter>>>,
    /// DataView executor for resolving DataView queries.
    pub dataview_executor: Arc<tokio::sync::RwLock<Option<DataViewExecutor>>>,
    /// Deployment manager for tracking deployment lifecycle.
    pub deployment_manager: Arc<DeploymentManager>,
    /// Runtime log level controller — wired from main.rs tracing setup.
    pub log_controller: Option<Arc<LogController>>,
    /// Hot reload state — `Some` in dev mode, `None` in production.
    /// Per spec §16: config file watcher swaps view routes without restart.
    pub hot_reload_state: Option<Arc<HotReloadState>>,
    /// Config file path — used to initialize hot reload file watcher.
    pub config_path: Option<std::path::PathBuf>,
    /// Loaded bundle — used by services discovery endpoint.
    pub loaded_bundle: Option<Arc<rivers_runtime::LoadedBundle>>,
    /// LockBox resolver — resolves credential names to metadata (no values in memory).
    pub lockbox_resolver: Option<Arc<rivers_runtime::rivers_core::lockbox::LockBoxResolver>>,
    /// Application keystore resolver — holds unlocked keystores scoped by app.
    pub keystore_resolver: Option<Arc<crate::keystore::KeystoreResolver>>,
    /// In-process EventBus — pub/sub with priority-tiered dispatch.
    /// Per spec §11: wired to broker bridges, message consumers, and middleware.
    pub event_bus: Arc<EventBus>,
    /// Internal KV storage — session, cache, polling, ctx.store backends.
    /// `None` until configured; waves 1/3/5 depend on this being `Some`.
    pub storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    /// Session manager — cookie-based sessions backed by StorageEngine.
    /// `None` until StorageEngine is available.
    pub session_manager: Option<Arc<crate::session::SessionManager>>,
    /// CSRF manager — double-submit cookie pattern backed by StorageEngine.
    /// `None` until StorageEngine is available.
    pub csrf_manager: Option<Arc<crate::csrf::CsrfManager>>,
    /// Detected guard view ID from bundle scan.
    /// `None` if no guard view is configured.
    pub guard_view_id: Option<String>,
    /// Pre-built admin RBAC config — built once at startup instead of per-request (AN11.4).
    pub admin_auth_config: Option<crate::admin::AdminAuthConfig>,
    /// SSE route manager — per-view broadcast channels.
    pub sse_manager: Arc<SseRouteManager>,
    /// WebSocket route manager — per-view broadcast hubs and connection registries.
    pub ws_manager: Arc<WebSocketRouteManager>,
    /// GraphQL dynamic schema — built from DataView resolver mappings at bundle load.
    /// `None` when GraphQL is disabled or no bundle is loaded.
    pub graphql_schema: Arc<tokio::sync::RwLock<Option<async_graphql::dynamic::Schema>>>,
    /// DriverFactory — shared with host callbacks for cdylib engine access.
    pub driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
    /// Shutdown sender — triggers graceful shutdown when sent `true`.
    pub shutdown_tx: Option<Arc<tokio::sync::watch::Sender<bool>>>,
}

impl AppContext {
    pub fn new(config: ServerConfig, shutdown: Arc<ShutdownCoordinator>) -> Self {
        let pool = Arc::new(ProcessPoolManager::from_config(
            &config.runtime.process_pools,
        ));
        Self {
            config,
            shutdown,
            uptime: Arc::new(UptimeTracker::new()),
            pool,
            view_router: Arc::new(tokio::sync::RwLock::new(None)),
            dataview_executor: Arc::new(tokio::sync::RwLock::new(None)),
            deployment_manager: Arc::new(DeploymentManager::new()),
            log_controller: None,
            hot_reload_state: None,
            config_path: None,
            loaded_bundle: None,
            lockbox_resolver: None,
            keystore_resolver: None,
            event_bus: Arc::new(EventBus::new()),
            storage_engine: None,
            session_manager: None,
            csrf_manager: None,
            guard_view_id: None,
            admin_auth_config: None,
            sse_manager: Arc::new(SseRouteManager::new()),
            ws_manager: Arc::new(WebSocketRouteManager::new()),
            graphql_schema: Arc::new(tokio::sync::RwLock::new(None)),
            driver_factory: None,
            shutdown_tx: None,
        }
    }
}

// ── Router Construction ───────────────────────────────────────────

/// Build the main server router.
///
/// Per spec §3: route registration order —
/// health → gossip → graphql → views → static.
pub fn build_main_router(ctx: AppContext) -> Router {
    let shutdown = ctx.shutdown.clone();
    let timeout_secs = ctx.config.base.request_timeout_seconds;
    let cors_config = Arc::new(CorsConfig {
        enabled: ctx.config.security.cors_enabled,
        allowed_origins: ctx.config.security.cors_allowed_origins.clone(),
        allowed_methods: ctx.config.security.cors_allowed_methods.clone(),
        allowed_headers: ctx.config.security.cors_allowed_headers.clone(),
        allow_credentials: ctx.config.security.cors_allow_credentials,
    });

    // Backpressure state — per spec §11
    let bp_config = &ctx.config.base.backpressure;
    let backpressure = BackpressureState::new(
        bp_config.queue_depth,
        bp_config.queue_timeout_ms,
        bp_config.enabled,
    );

    // Route registration order per spec §3
    let mut app = Router::new()
        // 1. Health endpoints
        .route("/health", get(health_handler))
        .route("/health/verbose", get(health_verbose_handler))
        // 2. Gossip endpoint (always registered per spec §3)
        .route("/gossip/receive", axum::routing::post(gossip_receive_handler));

    // 3. GraphQL routes — mount if schema is built
    if ctx.config.graphql.enabled {
        let gql_schema = ctx.graphql_schema.clone();
        // Try to get the schema synchronously — it was built at bundle load
        let maybe_schema = gql_schema.try_read().ok().and_then(|g| g.clone());
        if let Some(schema) = maybe_schema {
            let gql_config = crate::graphql::GraphqlConfig::from(&ctx.config.graphql);
            let gql_path = gql_config.path.clone();
            let introspection = gql_config.introspection;

            let schema_for_post = schema.clone();
            let post_handler = axum::routing::post(move |req: async_graphql_axum::GraphQLRequest| {
                let schema = schema_for_post.clone();
                async move {
                    let resp = schema.execute(req.into_inner()).await;
                    async_graphql_axum::GraphQLResponse::from(resp)
                }
            });
            app = app.route(&gql_path, post_handler);

            if introspection {
                let playground_path = format!("{}/playground", gql_path.trim_end_matches('/'));
                let playground_handler = axum::routing::get(|| async {
                    axum::response::Html(
                        async_graphql::http::playground_source(
                            async_graphql::http::GraphQLPlaygroundConfig::new("/graphql"),
                        ),
                    )
                });
                app = app.route(&playground_path, playground_handler);
            }

            tracing::info!(path = %gql_path, "GraphQL endpoint mounted");
        }
    }

    // 4. View routes + 5. Static file fallback
    // Combined fallback: tries view dispatch first, then static files.
    // View routes are matched dynamically after bundle deployment.
    let app = app
        .fallback(combined_fallback_handler)
        .with_state(ctx);

    // Middleware stack per spec §4 (layers apply in reverse — last = outermost)
    //
    // Innermost to outermost:
    // 10. request_observer
    // 9. timeout
    // 8. backpressure
    // 7. shutdown_guard
    // 6. rate_limit (pass-through — wired per-view at dispatch time)
    // 5. session (per-view — checked at view dispatch, not globally)
    // 4. security_headers
    // 3. trace_id
    // 2. body_limit (16 MiB)
    // 1. cors (covers all responses including errors)
    // 0. compression (outermost)
    app.layer(axum_middleware::from_fn(
            middleware::request_observer_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            timeout_secs,
            middleware::timeout_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            backpressure,
            crate::backpressure::backpressure_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            shutdown,
            middleware::shutdown_guard_middleware,
        ))
        .layer(axum_middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(axum_middleware::from_fn(middleware::trace_id_middleware))
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024)) // 16 MiB
        .layer(axum_middleware::from_fn_with_state(
            cors_config,
            middleware::cors_middleware,
        ))
        .layer(CompressionLayer::new())
}

/// Build the admin server router.
///
/// Per spec §1, §15: admin server has subset middleware + auth.
pub fn build_admin_router(ctx: AppContext) -> Router {
    let timeout_secs = ctx.config.base.request_timeout_seconds;
    let auth_ctx = ctx.clone();

    let app = Router::new()
        // Status/info endpoints
        .route("/admin/status", get(admin_status_handler))
        .route("/admin/drivers", get(admin_drivers_handler))
        .route("/admin/datasources", get(admin_datasources_handler))
        // Deployment lifecycle endpoints per spec §15.6
        .route("/admin/deploy", axum::routing::post(admin_deploy_handler))
        .route("/admin/deploy/test", axum::routing::post(admin_deploy_test_handler))
        .route("/admin/deploy/approve", axum::routing::post(admin_deploy_approve_handler))
        .route("/admin/deploy/reject", axum::routing::post(admin_deploy_reject_handler))
        .route("/admin/deploy/promote", axum::routing::post(admin_deploy_promote_handler))
        .route("/admin/deployments", get(admin_deployments_handler))
        // Log management endpoints per spec §15.8
        .route("/admin/log/levels", get(admin_log_levels_handler))
        .route("/admin/log/set", axum::routing::post(admin_log_set_handler))
        .route("/admin/log/reset", axum::routing::post(admin_log_reset_handler))
        // Shutdown endpoint
        .route("/admin/shutdown", axum::routing::post(admin_shutdown_handler))
        .with_state(ctx);

    // Admin middleware: admin_auth → timeout → security_headers → trace_id → body_limit
    app.layer(axum_middleware::from_fn_with_state(
            timeout_secs,
            middleware::timeout_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            auth_ctx,
            admin_auth_middleware,
        ))
        .layer(axum_middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(axum_middleware::from_fn(middleware::trace_id_middleware))
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024))
}

// ── Route Handlers ────────────────────────────────────────────────

/// Health check — returns 200 with simple status.
///
/// Per spec §14.1: always 200, basic status.
async fn health_handler(request: Request) -> impl IntoResponse {
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
async fn health_verbose_handler(
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
async fn gossip_receive_handler() -> impl IntoResponse {
    // Gossip processing deferred to RPS epic
    StatusCode::OK
}

// ── View Dispatch ─────────────────────────────────────────────────

/// Pre-matched route data extracted under a single RwLock acquisition.
///
/// Eliminates the double-lock pattern where `combined_fallback_handler`
/// matched the route, dropped the lock, then `view_dispatch_handler`
/// re-acquired the lock and re-matched.
struct MatchedRoute {
    config: rivers_runtime::view::ApiViewConfig,
    app_entry_point: String,
    path_params: HashMap<String, String>,
    guard_view_path: Option<String>,
    /// View ID from the router — needed by SSE/WS/Polling to look up per-route managers.
    view_id: String,
}

/// Combined fallback: tries view routes first, then static files.
///
/// Per spec §3: route registration order is views before static.
/// Views are matched dynamically via `ViewRouter` against loaded app config.
/// Unmatched requests fall through to static file serving.
async fn combined_fallback_handler(
    State(ctx): State<AppContext>,
    request: Request,
) -> axum::response::Response {
    let path = request.uri().path().to_string();

    // Phase 1: Check if any registered view matches this request.
    // Single RwLock acquire — extract all needed data in one pass (AN11.1).
    let method = request.method().to_string();
    let matched: Option<MatchedRoute> = {
        let router_guard = ctx.view_router.read().await;
        if let Some(ref router) = *router_guard {
            if let Some((route, path_params)) = router.match_route(&method, &path) {
                let config = route.config.clone();
                let app_entry_point = route.app_entry_point.clone();
                let guard_view_path = ctx.guard_view_id.as_ref().and_then(|gid| {
                    router.routes().iter()
                        .find(|r| r.view_id == *gid)
                        .map(|r| r.path_pattern.clone())
                });
                let view_id = route.view_id.clone();
                Some(MatchedRoute { config, app_entry_point, path_params, guard_view_path, view_id })
            } else {
                None
            }
        } else {
            None
        }
    }; // read lock dropped here

    if let Some(matched) = matched {
        return view_dispatch_handler(State(ctx), request, matched).await.into_response();
    }

    // Phase 2: Check for /services discovery endpoint on app-main
    if path.ends_with("/services") {
        return services_discovery_handler(State(ctx), request).await.into_response();
    }

    // Phase 3: Static file fallback
    static_file_handler(State(ctx), request).await.into_response()
}

/// View dispatch handler — routes API requests to the ViewEngine.
///
/// Per spec §3: view routes are registered before static file fallback.
/// Accepts pre-matched route data from `combined_fallback_handler` to
/// avoid a second RwLock acquisition and re-match (AN11.1).
///
/// Branches on `view_type` BEFORE body extraction so that WebSocket views
/// preserve the raw request for upgrade, and SSE views skip body parsing.
async fn view_dispatch_handler(
    State(ctx): State<AppContext>,
    request: Request,
    matched: MatchedRoute,
) -> impl IntoResponse {
    let view_type = matched.config.view_type.as_str();

    // ── Dispatch switch: branch by view_type before body extraction ──
    match view_type {
        "ServerSentEvents" => {
            return execute_sse_view(ctx, request, matched).await;
        }
        "Websocket" => {
            return execute_ws_view(ctx, request, matched).await;
        }
        _ => {
            // Rest (default) or streaming Rest — falls through to body extraction below
        }
    }

    // ── REST path: extract method, headers, body ──
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query: HashMap<String, String> = request
        .uri()
        .query()
        .map(|q| parse_query_string(q))
        .unwrap_or_default();
    let headers: HashMap<String, String> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|val| (k.to_string(), val.to_string())))
        .collect();

    // Check if this is a streaming REST view before consuming the body
    let is_streaming = matched.config.streaming.unwrap_or(false);

    // Extract body for non-GET/HEAD requests
    let body = if method != "GET" && method != "HEAD" {
        let bytes = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await;
        match bytes {
            Ok(b) if !b.is_empty() => serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        }
    } else {
        serde_json::Value::Null
    };

    let trace_id = uuid::Uuid::new_v4().to_string();

    // Use pre-matched route data — no second RwLock acquire needed (AN11.1)
    let mut config = matched.config;
    let app_entry_point = matched.app_entry_point;
    let path_params = matched.path_params;
    let guard_view_path = matched.guard_view_path;

    // Namespace the dataview reference so it resolves to the correct app's dataview
    if !app_entry_point.is_empty() {
        if let rivers_runtime::view::HandlerConfig::Dataview { ref mut dataview } = config.handler {
            *dataview = format!("{}:{}", app_entry_point, dataview);
        }
    }

    let parsed = view_engine::ParsedRequest {
        method: method.clone(),
        path: path.clone(),
        query_params: query,
        headers,
        body,
        path_params,
    };

    // ── Streaming REST dispatch ──
    if is_streaming {
        return execute_streaming_rest_view(&ctx, parsed, &config, &trace_id).await;
    }

    // ── Standard REST dispatch ──
    let mut view_ctx = view_engine::ViewContext::new(
        parsed,
        trace_id.clone(),
        String::new(), // app_id — populated after bundle deployment
        String::new(), // node_id
        "dev".to_string(),
    );

    // ── Steps 1-3: Security pipeline (AN13.3) ──────────────
    let security = crate::security_pipeline::run_security_pipeline(
        &ctx, &config,
        &view_ctx.request.headers,
        &method,
        &trace_id,
        guard_view_path.as_deref(),
    ).await;
    let session_id = match security {
        Ok(outcome) => {
            view_ctx.session = outcome.session;
            outcome.session_id
        }
        Err(resp) => return resp,
    };

    // ── Execute the view with the ProcessPool and DataViewExecutor ──
    let dv_guard = ctx.dataview_executor.read().await;
    let dv_ref = dv_guard.as_ref();
    let view_result = view_engine::execute_rest_view(&mut view_ctx, &config, Some(&ctx.pool), dv_ref).await;
    drop(dv_guard);

    // ── Step 4: Build response with session/CSRF cookies ────
    let mut set_cookies: Vec<String> = Vec::new();

    // If this is a guard view and the result succeeded, check for session creation
    if config.guard {
        if let Ok(ref result) = view_result {
            // Guard view returned allow=true with session_claims → create session
            if result.body.get("allow").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(claims) = result.body.get("session_claims").cloned() {
                    if let Some(ref mgr) = ctx.session_manager {
                        let subject = claims.get("subject")
                            .or(claims.get("username"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("anonymous")
                            .to_string();
                        match mgr.create_session(subject, claims.clone()).await {
                            Ok(session) => {
                                // Set session cookie
                                set_cookies.push(
                                    crate::session::build_set_cookie(
                                        &session.session_id,
                                        &ctx.config.security.session,
                                    ),
                                );
                                // Generate CSRF token for the new session
                                if let Some(ref csrf_mgr) = ctx.csrf_manager {
                                    if let Ok(csrf_token) = csrf_mgr.get_or_rotate_token(
                                        &session.session_id,
                                        ctx.config.security.session.ttl_s,
                                    ).await {
                                        set_cookies.push(
                                            crate::csrf::build_csrf_cookie(
                                                &csrf_token,
                                                &ctx.config.security.csrf,
                                            ),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "failed to create session from guard result");
                            }
                        }
                    }
                }
            }
        }
    }

    // For all responses with an active session, rotate CSRF cookie
    if set_cookies.is_empty() {
        if let (Some(ref csrf_mgr), Some(ref sid)) = (&ctx.csrf_manager, &session_id) {
            if view_ctx.session.is_some() {
                if let Ok(csrf_token) = csrf_mgr.get_or_rotate_token(
                    sid,
                    ctx.config.security.session.ttl_s,
                ).await {
                    set_cookies.push(
                        crate::csrf::build_csrf_cookie(
                            &csrf_token,
                            &ctx.config.security.csrf,
                        ),
                    );
                }
            }
        }
    }

    // Build the final HTTP response
    match view_result {
        Ok(result) => {
            let (status, resp_headers, body_str) =
                view_engine::serialize_view_result(&result);
            let mut builder = axum::response::Response::builder().status(status);
            for (k, v) in &resp_headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            for cookie in &set_cookies {
                builder = builder.header("set-cookie", cookie.as_str());
            }
            builder
                .body(axum::body::Body::from(body_str))
                .unwrap_or_else(|_| {
                    error_response::internal_error("response body construction failed")
                        .with_trace_id(trace_id.clone())
                        .into_axum_response()
                })
                .into_response()
        }
        Err(e) => {
            error_response::map_view_error(&e, Some(&trace_id))
                .into_axum_response()
        }
    }
}

// ── Streaming Response Helper ──────────────────────────────────────

/// Build a streaming HTTP response from an mpsc receiver of string chunks.
///
/// Used by SSE, Streaming REST, and WebSocket views to return chunked responses.
fn build_streaming_response(
    content_type: &str,
    rx: tokio::sync::mpsc::Receiver<String>,
) -> axum::response::Response {
    use tokio_stream::wrappers::ReceiverStream;

    let stream = ReceiverStream::new(rx)
        .map(|chunk| Ok::<_, std::convert::Infallible>(chunk));
    let body = axum::body::Body::from_stream(stream);

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type)
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(body)
        .unwrap_or_else(|_| {
            error_response::internal_error("streaming response construction failed")
                .into_axum_response()
        })
}

// ── SSE View Handler ───────────────────────────────────────────────

/// Execute an SSE view — subscribe to the SSE channel and stream events.
///
/// Per spec §7: SSE views return `text/event-stream` with per-client push loop.
async fn execute_sse_view(
    ctx: AppContext,
    request: Request,
    matched: MatchedRoute,
) -> axum::response::Response {
    let view_id = matched.view_id.clone();
    let trace_id = uuid::Uuid::new_v4().to_string();

    // Look up the SSE channel for this view
    let channel = match ctx.sse_manager.get(&view_id).await {
        Some(ch) => ch,
        None => {
            tracing::warn!(view_id = %view_id, "SSE channel not registered");
            return error_response::internal_error("SSE channel not configured")
                .with_trace_id(trace_id)
                .into_axum_response();
        }
    };

    // Subscribe this client
    let mut sse_rx = match channel.subscribe() {
        Ok(rx) => rx,
        Err(e) => {
            return error_response::service_unavailable(e.to_string())
                .with_trace_id(trace_id)
                .into_axum_response();
        }
    };

    // Extract Last-Event-ID for reconnection
    let last_event_id = request
        .headers()
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Create mpsc channel for streaming response
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);

    // Replay missed events from buffer if client is reconnecting with Last-Event-ID
    if let Some(ref last_id) = last_event_id {
        let missed = channel.replay_since(last_id);
        for event in missed {
            let wire = event.to_wire_format();
            if tx.send(wire).await.is_err() {
                channel.unsubscribe();
                return error_response::internal_error("client disconnected during replay")
                    .with_trace_id(trace_id)
                    .into_axum_response();
            }
        }
        tracing::debug!(view_id = %view_id, last_event_id = %last_id, "SSE replay complete");
    }

    // Extract session ID from request for revalidation
    let session_id = {
        let cookie_name = &ctx.config.security.session.cookie.name;
        let cookie_hdr = request.headers().get("cookie").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let auth_hdr = request.headers().get("authorization").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        crate::session::extract_session_id(cookie_hdr.as_deref(), auth_hdr.as_deref(), cookie_name)
    };

    // Spawn per-client relay task: broadcast receiver → mpsc sender → HTTP stream
    let channel_for_cleanup = channel.clone();
    let revalidation_interval = matched.config.session_revalidation_interval_s;
    let session_mgr = ctx.session_manager.clone();
    let view_id_clone = view_id.clone();
    tokio::spawn(async move {
        // Optional session revalidation timer
        let mut revalidation_tick = revalidation_interval.map(|secs| {
            tokio::time::interval(tokio::time::Duration::from_secs(secs))
        });
        // Skip the first immediate tick
        if let Some(ref mut tick) = revalidation_tick {
            tick.tick().await;
        }

        loop {
            tokio::select! {
                msg = sse_rx.recv() => {
                    match msg {
                        Ok(event) => {
                            let wire = event.to_wire_format();
                            if tx.send(wire).await.is_err() {
                                break; // Client disconnected
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            let comment = format!(": lagged {} events\n\n", n);
                            if tx.send(comment).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                _ = async {
                    if let Some(ref mut tick) = revalidation_tick {
                        tick.tick().await
                    } else {
                        std::future::pending::<tokio::time::Instant>().await
                    }
                } => {
                    // Session revalidation tick — validate against StorageEngine
                    if let (Some(ref mgr), Some(ref sid)) = (&session_mgr, &session_id) {
                        match mgr.validate_session(sid).await {
                            Ok(Some(_)) => {} // Session still valid
                            Ok(None) | Err(_) => {
                                tracing::info!(
                                    view_id = %view_id_clone,
                                    "SSE session expired — closing connection"
                                );
                                let _ = tx.send(": session expired\n\n".to_string()).await;
                                break;
                            }
                        }
                    }
                }
            }
        }
        channel_for_cleanup.unsubscribe();
    });

    tracing::debug!(view_id = %view_id, "SSE client connected");
    build_streaming_response("text/event-stream", rx)
}

// ── Streaming REST View Handler ────────────────────────────────────

/// Execute a streaming REST view — returns chunked NDJSON or SSE response.
///
/// Per spec: streaming views use CodeComponent handlers that produce chunks.
async fn execute_streaming_rest_view(
    ctx: &AppContext,
    _parsed: view_engine::ParsedRequest,
    config: &rivers_runtime::view::ApiViewConfig,
    trace_id: &str,
) -> axum::response::Response {
    use crate::streaming::{StreamingConfig, StreamingFormat, StreamChunk, run_streaming_generator, poison_chunk_ndjson, poison_chunk_sse};
    use crate::process_pool::Entrypoint;

    // Determine streaming format
    let format = StreamingFormat::from_str_opt(config.streaming_format.as_deref())
        .unwrap_or(StreamingFormat::Ndjson);
    let content_type = format.content_type();
    let stream_timeout_ms = config.stream_timeout_ms.unwrap_or(120_000);

    let streaming_config = StreamingConfig {
        format: format.clone(),
        stream_timeout_ms,
    };

    // Extract CodeComponent entrypoint
    let entrypoint = match &config.handler {
        rivers_runtime::view::HandlerConfig::Codecomponent { language, module, entrypoint, .. } => {
            Entrypoint {
                module: module.clone(),
                function: entrypoint.clone(),
                language: language.clone(),
            }
        }
        _ => {
            return error_response::internal_error("streaming views require CodeComponent handler")
                .with_trace_id(trace_id.to_string())
                .into_axum_response();
        }
    };

    // Create channels: generator → formatter → HTTP stream
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<StreamChunk>(64);
    let (wire_tx, wire_rx) = tokio::sync::mpsc::channel::<String>(64);

    // Spawn generator task — owns chunk_tx, drops it when done
    let pool = ctx.pool.clone();
    let trace_owned = trace_id.to_string();
    let gen_handle = tokio::spawn(async move {
        run_streaming_generator(
            &pool,
            &entrypoint,
            &streaming_config,
            chunk_tx,
            &trace_owned,
        )
        .await
    });

    // Spawn formatter task — reads chunks, formats, writes to wire
    let fmt2 = format;
    tokio::spawn(async move {
        while let Some(chunk) = chunk_rx.recv().await {
            let wire = match fmt2 {
                StreamingFormat::Ndjson => chunk.to_ndjson(),
                StreamingFormat::Sse => chunk.to_sse(None),
            };
            if wire_tx.send(wire).await.is_err() {
                return; // Client disconnected
            }
        }
        // Generator is done (chunk_tx dropped). Check for error.
        if let Ok(Err(e)) = gen_handle.await {
            let poison = match fmt2 {
                StreamingFormat::Ndjson => poison_chunk_ndjson(&e.to_string()),
                StreamingFormat::Sse => poison_chunk_sse(&e.to_string()),
            };
            let _ = wire_tx.send(poison).await;
        }
    });

    build_streaming_response(content_type, wire_rx)
}

// ── WebSocket View Handler ─────────────────────────────────────────

/// Execute a WebSocket view — upgrade HTTP to WebSocket connection.
///
/// Per spec §6: bidirectional connection with read/write loop and lifecycle hooks.
async fn execute_ws_view(
    ctx: AppContext,
    request: Request,
    matched: MatchedRoute,
) -> axum::response::Response {
    use axum::extract::ws::WebSocketUpgrade;

    let view_id = matched.view_id.clone();
    let trace_id = uuid::Uuid::new_v4().to_string();
    let config = matched.config.clone();

    // Extract WebSocketUpgrade from the request parts (before body consumption)
    let (mut parts, _body) = request.into_parts();
    let ws_upgrade: WebSocketUpgrade = match <WebSocketUpgrade as FromRequestParts<()>>::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!(view_id = %view_id, error = %e, "WebSocket upgrade failed");
            return error_response::bad_request(format!("WebSocket upgrade failed: {}", e))
                .with_trace_id(trace_id)
                .into_axum_response();
        }
    };

    // Determine WebSocket mode
    let ws_mode = crate::websocket::WebSocketMode::from_str_opt(
        config.websocket_mode.as_deref(),
    );

    let ctx_clone = ctx.clone();
    ws_upgrade
        .on_upgrade(move |socket| {
            handle_ws_connection(ctx_clone, socket, view_id, config, ws_mode, trace_id)
        })
        .into_response()
}

/// Handle an active WebSocket connection — read/write loop with lifecycle hooks.
///
/// Uses a single-owner pattern: the socket stays in one task that alternates
/// between reading client frames and draining outbound messages (no split needed).
async fn handle_ws_connection(
    ctx: AppContext,
    mut socket: axum::extract::ws::WebSocket,
    view_id: String,
    config: rivers_runtime::view::ApiViewConfig,
    ws_mode: crate::websocket::WebSocketMode,
    trace_id: String,
) {
    use axum::extract::ws::Message;
    use crate::websocket::{
        WebSocketMode, ConnectionId, ConnectionInfo, WebSocketMessage,
        WsRateLimiter, BinaryFrameTracker, dispatch_ws_lifecycle, execute_ws_on_stream,
    };
    use crate::process_pool::Entrypoint;

    let conn_id = ConnectionId::new();
    tracing::info!(
        view_id = %view_id,
        connection_id = %conn_id.0,
        mode = ?ws_mode,
        "WebSocket connected"
    );

    // Rate limiter (per-connection)
    let rate_limiter = WsRateLimiter::new(
        config.rate_limit_per_minute,
        config.rate_limit_burst_size,
    );

    let binary_tracker = BinaryFrameTracker::new();

    // Dispatch on_connect lifecycle hook
    if let Some(ref hooks) = config.ws_hooks {
        if let Some(ref on_connect) = hooks.on_connect {
            match dispatch_ws_lifecycle(
                &ctx.pool,
                &on_connect.module,
                &on_connect.entrypoint,
                &conn_id.0,
                None,
                None,
                &trace_id,
            )
            .await
            {
                Ok(reply) if !reply.is_null() => {
                    let reply_str = serde_json::to_string(&reply).unwrap_or_default();
                    let _ = socket.send(Message::Text(reply_str.into())).await;
                }
                Err(e) => {
                    tracing::warn!(connection_id = %conn_id.0, error = %e, "onConnect hook failed");
                }
                _ => {}
            }
        }
    }

    // Publish WebSocket connected event
    {
        let event = rivers_runtime::rivers_core::Event::new(
            rivers_runtime::rivers_core::eventbus::events::WEBSOCKET_CONNECTED,
            serde_json::json!({"connection_id": conn_id.0, "view_id": view_id}),
        ).with_trace_id(&trace_id);
        ctx.event_bus.publish(&event).await;
    }

    // Subscribe to broadcast or register in Direct mode
    let mut broadcast_rx: Option<tokio::sync::broadcast::Receiver<WebSocketMessage>> = None;
    match ws_mode {
        WebSocketMode::Broadcast => {
            if let Some(hub) = ctx.ws_manager.get_broadcast(&view_id).await {
                broadcast_rx = hub.subscribe().ok();
            }
        }
        WebSocketMode::Direct => {
            let info = ConnectionInfo {
                id: conn_id.clone(),
                view_id: view_id.clone(),
                connected_at: chrono::Utc::now(),
                session_id: None,
                path_params: HashMap::new(),
            };
            if let Some(registry) = ctx.ws_manager.get_direct(&view_id).await {
                broadcast_rx = registry.register(info).await.ok();
            }
        }
    }

    // Extract on_stream entrypoint if configured
    let on_stream_ep = config.on_stream.as_ref().map(|os| Entrypoint {
        module: os.module.clone(),
        function: os.entrypoint.clone(),
        language: "javascript".to_string(),
    });

    // Extract on_message hook if configured
    let on_message_hook = config.ws_hooks.as_ref().and_then(|h| h.on_message.clone());

    // Main loop: owns the socket, alternates between recv and outbound sends
    loop {
        tokio::select! {
            // Read from client
            msg_opt = socket.recv() => {
                let msg = match msg_opt {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break, // Connection error or closed
                };

                match msg {
                    Message::Text(text) => {
                        // Rate limit check
                        if let Some(ref rl) = rate_limiter {
                            if !rl.check() {
                                tracing::debug!(connection_id = %conn_id.0, "WS rate limited");
                                continue;
                            }
                        }

                        // Publish message-in EventBus event
                        {
                            let event = rivers_runtime::rivers_core::Event::new(
                                rivers_runtime::rivers_core::eventbus::events::WEBSOCKET_MESSAGE_IN,
                                serde_json::json!({"connection_id": conn_id.0, "view_id": view_id}),
                            ).with_trace_id(&trace_id);
                            ctx.event_bus.publish(&event).await;
                        }

                        // Dispatch on_message lifecycle hook if configured
                        if let Some(ref hook) = on_message_hook {
                            let message_val: serde_json::Value =
                                serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text.to_string()));
                            match dispatch_ws_lifecycle(
                                &ctx.pool,
                                &hook.module,
                                &hook.entrypoint,
                                &conn_id.0,
                                Some(&message_val),
                                None,
                                &trace_id,
                            ).await {
                                Ok(reply) if !reply.is_null() => {
                                    let reply_str = serde_json::to_string(&reply).unwrap_or_default();
                                    if socket.send(Message::Text(reply_str.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(connection_id = %conn_id.0, error = %e, "onMessage hook failed");
                                }
                                _ => {}
                            }
                        }

                        // Dispatch on_stream handler if configured
                        if let Some(ref ep) = on_stream_ep {
                            let message_val: serde_json::Value =
                                serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text.to_string()));
                            match execute_ws_on_stream(
                                &ctx.pool,
                                ep,
                                &message_val,
                                &conn_id,
                                &trace_id,
                            )
                            .await
                            {
                                Ok(Some(reply)) => {
                                    let reply_str = serde_json::to_string(&reply)
                                        .unwrap_or_else(|_| "null".to_string());
                                    if socket.send(Message::Text(reply_str.into())).await.is_err() {
                                        break;
                                    }
                                }
                                Ok(None) => {} // No reply
                                Err(e) => {
                                    tracing::warn!(
                                        connection_id = %conn_id.0,
                                        error = %e,
                                        "on_stream handler failed"
                                    );
                                }
                            }
                        }
                    }
                    Message::Binary(_) => {
                        if binary_tracker.record_binary_frame() {
                            tracing::warn!(
                                connection_id = %conn_id.0,
                                view_id = %view_id,
                                "binary WebSocket frame received (not supported)"
                            );
                        }
                    }
                    Message::Close(_) => break,
                    _ => {} // Ping/Pong handled by axum
                }
            }

            // Drain broadcast messages
            bcast = async {
                match broadcast_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match bcast {
                    Ok(ws_msg) => {
                        if socket.send(Message::Text(ws_msg.payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

        }
    }

    // Dispatch on_disconnect lifecycle hook
    if let Some(ref hooks) = config.ws_hooks {
        if let Some(ref on_disconnect) = hooks.on_disconnect {
            match dispatch_ws_lifecycle(
                &ctx.pool,
                &on_disconnect.module,
                &on_disconnect.entrypoint,
                &conn_id.0,
                None,
                None,
                &trace_id,
            )
            .await
            {
                Ok(reply) if !reply.is_null() => {
                    // In Broadcast mode, broadcast the farewell to remaining peers
                    if ws_mode == WebSocketMode::Broadcast {
                        if let Some(hub) = ctx.ws_manager.get_broadcast(&view_id).await {
                            let reply_str = serde_json::to_string(&reply).unwrap_or_default();
                            let _ = hub.broadcast(WebSocketMessage::text(reply_str));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(connection_id = %conn_id.0, error = %e, "onDisconnect hook failed");
                }
                _ => {}
            }
        }
    }

    // Publish WebSocket disconnected event
    {
        let event = rivers_runtime::rivers_core::Event::new(
            rivers_runtime::rivers_core::eventbus::events::WEBSOCKET_DISCONNECTED,
            serde_json::json!({"connection_id": conn_id.0, "view_id": view_id}),
        ).with_trace_id(&trace_id);
        ctx.event_bus.publish(&event).await;
    }

    // Unregister from Direct mode registry
    if ws_mode == WebSocketMode::Direct {
        if let Some(registry) = ctx.ws_manager.get_direct(&view_id).await {
            registry.unregister(&conn_id.0).await;
        }
    }

    tracing::info!(
        view_id = %view_id,
        connection_id = %conn_id.0,
        "WebSocket disconnected"
    );
}

/// Parse a URL query string into key-value pairs.
fn parse_query_string(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?;
            let value = parts.next().unwrap_or("");
            let key = percent_encoding::percent_decode_str(key)
                .decode_utf8_lossy()
                .into_owned();
            let value = percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned();
            Some((key, value))
        })
        .collect()
}

/// Static file handler — serves files from the configured root directory.
///
/// Per spec §7-8: path resolution, ETag, Cache-Control, SPA fallback.
/// Returns 404 if static files are not enabled.
async fn static_file_handler(State(ctx): State<AppContext>, request: Request) -> impl IntoResponse {
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
async fn services_discovery_handler(
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

// Admin handlers moved to crate::admin_handlers (AN13.1)
use crate::admin_handlers::{
    admin_status_handler, admin_drivers_handler, admin_datasources_handler,
    admin_deploy_handler, admin_deploy_test_handler,
    admin_deploy_approve_handler, admin_deploy_reject_handler,
    admin_deploy_promote_handler, admin_deployments_handler,
    admin_log_levels_handler, admin_log_set_handler, admin_log_reset_handler,
    admin_shutdown_handler,
};


// ── Admin Auth Middleware ────────────────────────────────────────

/// Admin API authentication middleware.
///
/// Per spec §15.3, §18.3: Ed25519 signature verification.
/// Bypassed when `--no-admin-auth` flag is set (AdminApiConfig.no_auth).
async fn admin_auth_middleware(
    State(ctx): State<AppContext>,
    request: Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use crate::admin_auth;

    // Check --no-admin-auth flag
    if ctx.config.base.admin_api.no_auth.unwrap_or(false) {
        return next.run(request).await;
    }

    // Check if public_key is configured
    let public_key_hex = match &ctx.config.base.admin_api.public_key {
        Some(pk) => pk.clone(),
        None => {
            unreachable!("startup validation guarantees public_key is present");
        }
    };

    // Parse the configured public key
    let public_key = match admin_auth::parse_public_key(&public_key_hex) {
        Ok(pk) => pk,
        Err(e) => {
            tracing::error!(target: "rivers.admin", "invalid admin public key: {e}");
            return error_response::internal_error("admin auth misconfigured")
                .into_axum_response();
        }
    };

    // Extract signature and timestamp headers
    let signature_hex = request.headers()
        .get("x-rivers-signature")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let timestamp = request.headers()
        .get("x-rivers-timestamp")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match (signature_hex, timestamp) {
        (Some(sig_hex), Some(ts)) => {
            // Validate timestamp freshness (300_000 millisecond window)
            if let Err(e) = admin_auth::validate_timestamp(&ts, 300_000) {
                return error_response::unauthorized(e.to_string())
                    .into_axum_response();
            }

            // Decode signature from hex
            let sig_bytes = match hex::decode(&sig_hex) {
                Ok(b) => b,
                Err(_) => {
                    return error_response::unauthorized("invalid signature encoding")
                        .into_axum_response();
                }
            };

            // Consume body, compute SHA-256 hash, then reconstruct the request
            let method = request.method().as_str().to_string();
            let path = request.uri().path().to_string();

            let (parts, body) = request.into_parts();
            let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
                Ok(b) => b,
                Err(_) => {
                    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
                }
            };
            let body_hash = hex::encode(sha2::Sha256::digest(&bytes));
            let request = Request::from_parts(parts, axum::body::Body::from(bytes));

            if let Err(_) = admin_auth::verify_admin_signature(
                &public_key, &method, &path, &ts, &body_hash, &sig_bytes,
            ) {
                return error_response::unauthorized("signature verification failed")
                    .into_axum_response();
            }

            // ── IP allowlist check ──────────────────────────────
            let ip_allowlist = &ctx.config.security.admin_ip_allowlist;
            if !ip_allowlist.is_empty() {
                let remote_ip = request
                    .extensions()
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|ci| ci.0.ip().to_string())
                    .unwrap_or_default();
                if let Err(_) = crate::admin::check_ip_allowlist(&remote_ip, ip_allowlist) {
                    return error_response::forbidden("IP not in admin allowlist")
                        .into_axum_response();
                }
            }

            // ── RBAC permission check ───────────────────────────
            // Map request path to required permission
            let required_permission = path_to_admin_permission(&path);
            if let Some(perm) = required_permission {
                // Derive identity from the verified public key fingerprint
            let identity = hex::encode(sha2::Sha256::digest(public_key.as_bytes()));
                let admin_auth_config = ctx.admin_auth_config.as_ref()
                    .unwrap_or(&*DEFAULT_ADMIN_AUTH_CONFIG);
                if let Err(_) = crate::admin::check_permission(&identity, &perm, admin_auth_config) {
                    return error_response::forbidden("permission denied")
                        .into_axum_response();
                }
            }

            next.run(request).await
        }
        _ => {
            error_response::unauthorized("missing X-Rivers-Signature or X-Rivers-Timestamp header")
                .into_axum_response()
        }
    }
}

/// Map an admin API path to the required `AdminPermission`.
///
/// Returns `None` for unknown paths (middleware will allow through).
fn path_to_admin_permission(path: &str) -> Option<crate::admin::AdminPermission> {
    use crate::admin::AdminPermission;
    match path {
        "/admin/status" | "/admin/drivers" | "/admin/datasources" => Some(AdminPermission::StatusRead),
        "/admin/deploy" | "/admin/deploy/test" => Some(AdminPermission::DeployWrite),
        "/admin/deploy/approve" | "/admin/deploy/reject" => Some(AdminPermission::DeployApprove),
        "/admin/deploy/promote" => Some(AdminPermission::DeployPromote),
        "/admin/deployments" => Some(AdminPermission::DeployRead),
        p if p.starts_with("/admin/log") => Some(AdminPermission::Admin),
        "/admin/shutdown" => Some(AdminPermission::Admin),
        _ => None,
    }
}

/// Fallback for when `admin_auth_config` is not initialized (e.g. tests).
static DEFAULT_ADMIN_AUTH_CONFIG: std::sync::LazyLock<crate::admin::AdminAuthConfig> =
    std::sync::LazyLock::new(crate::admin::AdminAuthConfig::default);

/// Build an `AdminAuthConfig` from server config for RBAC checks.
///
/// Called once at startup and stored in `AppContext.admin_auth_config` (AN11.4).
/// Bridges from `rivers_runtime::rivers_core::config::RbacConfig` to the admin module's
/// `AdminAuthConfig` which `check_permission` expects.
fn build_admin_auth_config_for_rbac(config: &ServerConfig) -> crate::admin::AdminAuthConfig {
    use crate::admin::{AdminAuthConfig, AdminPermission};

    let mut auth_config = AdminAuthConfig::default();
    if let Some(ref rbac) = config.base.admin_api.rbac {
        // Convert role → Vec<String> to role → Vec<AdminPermission>
        for (role, perms) in &rbac.roles {
            let permissions: Vec<AdminPermission> = perms
                .iter()
                .filter_map(|p| match p.as_str() {
                    "status_read" => Some(AdminPermission::StatusRead),
                    "deploy_write" => Some(AdminPermission::DeployWrite),
                    "deploy_approve" => Some(AdminPermission::DeployApprove),
                    "deploy_promote" => Some(AdminPermission::DeployPromote),
                    "deploy_read" => Some(AdminPermission::DeployRead),
                    "admin" => Some(AdminPermission::Admin),
                    _ => {
                        tracing::warn!(permission = %p, role = %role, "unknown admin permission, skipping");
                        None
                    }
                })
                .collect();
            auth_config.roles.insert(role.clone(), permissions);
        }
        auth_config.identity_roles = rbac.bindings.clone();
    }
    auth_config.no_auth = config.base.admin_api.no_auth.unwrap_or(false);
    auth_config
}

// ── Driver Registration ──────────────────────────────────────────

/// Register all drivers — built-in and plugin — into a `DriverFactory`.
///
/// Centralises the 15+ individual registration calls so that both
/// bundle-load and future code paths share one inventory.
///
/// Drivers listed in `ignore` are skipped with an INFO log.
pub fn register_all_drivers(
    factory: &mut rivers_runtime::rivers_core::DriverFactory,
    ignore: &[String],
) {
    // Built-in drivers — statically linked when feature is enabled
    #[cfg(feature = "static-builtin-drivers")]
    {
        rivers_runtime::rivers_core::register_builtin_drivers(factory);
    }

    // Static plugin drivers (only when compiled with "static-plugins" feature)
    #[cfg(feature = "static-plugins")]
    {
        use std::sync::Arc as A;
        let static_plugins: Vec<(&str, Box<dyn FnOnce(&mut rivers_runtime::rivers_core::DriverFactory)>)> = vec![
            ("cassandra",      Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_cassandra::CassandraDriver)); })),
            ("couchdb",        Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_couchdb::CouchDBDriver)); })),
            ("mongodb",        Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_mongodb::MongoDriver)); })),
            ("elasticsearch",  Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_elasticsearch::ElasticsearchDriver)); })),
            ("influxdb",       Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_influxdb::InfluxDriver)); })),
            ("ldap",           Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_ldap::LdapDriver)); })),
            ("kafka",          Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_kafka::KafkaDriver)); })),
            ("rabbitmq",       Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_rabbitmq::RabbitMqDriver)); })),
            ("nats",           Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_nats::NatsDriver)); })),
            ("rivers-exec",    Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_exec::ExecDriver)); })),
        ];
        for (name, register_fn) in static_plugins {
            if ignore.iter().any(|i| i == name) {
                tracing::info!(driver = name, "driver ignored per [plugins].ignore config");
            } else {
                register_fn(factory);
            }
        }
    }

    // Dynamic drivers from lib/ directory (builtin drivers dylib)
    let lib_dir = std::path::Path::new("lib");
    if lib_dir.is_dir() {
        let results = rivers_runtime::rivers_core::driver_factory::load_plugins(lib_dir, factory);
        for result in &results {
            match result {
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Success { path, driver_names } => {
                    // Check if any loaded driver names are in the ignore list
                    let ignored: Vec<&str> = driver_names.iter()
                        .filter(|d| ignore.iter().any(|i| i == *d))
                        .map(|d| d.as_str())
                        .collect();
                    if !ignored.is_empty() {
                        tracing::info!(path = %path, drivers = ?ignored, "driver library loaded but drivers ignored per config");
                    } else {
                        tracing::info!(path = %path, drivers = ?driver_names, "loaded driver library");
                    }
                }
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Failed { path, reason } => {
                    tracing::warn!(path = %path, reason = %reason, "failed to load driver library");
                }
            }
        }
    }

    // Dynamic plugin drivers from plugins/ directory
    let plugin_dir = std::path::Path::new("plugins");
    if plugin_dir.is_dir() {
        let results = rivers_runtime::rivers_core::driver_factory::load_plugins(plugin_dir, factory);
        for result in &results {
            match result {
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Success { path, driver_names } => {
                    let ignored: Vec<&str> = driver_names.iter()
                        .filter(|d| ignore.iter().any(|i| i == *d))
                        .map(|d| d.as_str())
                        .collect();
                    if !ignored.is_empty() {
                        tracing::info!(path = %path, drivers = ?ignored, "plugin loaded but drivers ignored per config");
                    } else {
                        tracing::info!(path = %path, drivers = ?driver_names, "loaded driver plugin");
                    }
                }
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Failed { path, reason } => {
                    tracing::warn!(path = %path, reason = %reason, "failed to load driver plugin");
                }
            }
        }
    }

    if !ignore.is_empty() {
        tracing::info!(ignored = ?ignore, "drivers ignored — bundles referencing these will fail validation");
    }
}

// ── Server Entry Point ────────────────────────────────────────────

// ── HTTP Redirect Server ─────────────────────────────────────────

/// Spawn a plain HTTP server on `redirect_port` that issues 301 → HTTPS main server.
///
/// Per spec §4: no middleware, preserves Host + query string.
/// Bind failure → log WARN and return None (not an error).
/// When `tls.redirect = false` → caller skips this entirely.
pub async fn maybe_spawn_http_redirect_server(
    _base_port: u16,
    redirect_port: u16,
    shutdown_rx: watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    let addr: std::net::SocketAddr = match format!("0.0.0.0:{redirect_port}").parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(port = redirect_port, error = %e, "invalid redirect address");
            return None;
        }
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                port = redirect_port,
                error = %e,
                "HTTP redirect server: bind failed, redirect will not run"
            );
            return None;
        }
    };
    tracing::info!(port = redirect_port, "HTTP redirect server listening");
    Some(tokio::spawn(run_http_redirect_server(
        listener,
        redirect_port,
        shutdown_rx,
    )))
}

/// Inner redirect server loop: issues 301 → HTTPS for all incoming requests.
///
/// Per spec §4: preserves Host header and path+query string.
/// No middleware stack — just the redirect.
pub async fn run_http_redirect_server(
    listener: tokio::net::TcpListener,
    https_port: u16,
    shutdown_rx: watch::Receiver<bool>,
) {
    use axum::extract::Request as AxumRequest;
    use axum::http::StatusCode;

    let app = Router::new().fallback(move |req: AxumRequest| {
        let uri = req.uri().clone();
        let host = req
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost")
            .split(':')
            .next()
            .unwrap_or("localhost")
            .to_string();
        let port_suffix = if https_port == 443 {
            String::new()
        } else {
            format!(":{https_port}")
        };
        let path_and_query = uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .to_string();
        let target = format!("https://{host}{port_suffix}{path_and_query}");
        async move {
            axum::response::Response::builder()
                .status(StatusCode::MOVED_PERMANENTLY)
                .header("location", &target)
                .body(axum::body::Body::empty())
                .unwrap_or_else(|_| {
                    axum::response::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(axum::body::Body::from("redirect failed"))
                        .expect("fallback response")
                })
        }
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await
        .ok();
}

// ── Plain HTTP (no-ssl) Entry Point ───────────────────────────────

/// Start the main server in plain HTTP mode (`--no-ssl` debug path).
///
/// Binds on `port` (from --port flag, defaulting to redirect_port or 80).
/// Main server TLS validation is skipped. Admin server TLS rules unchanged.
/// Per spec §1.1: emits a prominent WARN at startup.
pub async fn run_server_no_ssl(
    config: ServerConfig,
    port: u16,
    shutdown_rx: watch::Receiver<bool>,
    shutdown_tx: Option<Arc<tokio::sync::watch::Sender<bool>>>,
) -> Result<(), ServerError> {
    tracing::warn!("--no-ssl: TLS is DISABLED for this session — do not use in production");

    // Validate only admin TLS (main TLS is skipped per --no-ssl).
    validate_server_tls(&config, true)
        .map_err(|e| ServerError::Config(e))?;

    let addr: std::net::SocketAddr = format!("{}:{}", config.base.host, port)
        .parse()
        .map_err(|e| ServerError::Config(format!("invalid address: {e}")))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| ServerError::Bind(format!("bind failed: {e}")))?;

    tracing::info!(addr = %addr, "server listening (plain HTTP — no-ssl mode)");

    // Build context same as TLS path
    let shutdown = Arc::new(ShutdownCoordinator::new());
    let mut ctx = AppContext::new(config.clone(), shutdown.clone());
    ctx.shutdown_tx = shutdown_tx;

    // Build RBAC config once at startup (AN11.4)
    if config.base.admin_api.enabled {
        ctx.admin_auth_config = Some(build_admin_auth_config_for_rbac(&config));
    }

    // Auto-load bundle from config if bundle_path is set
    crate::bundle_loader::load_and_wire_bundle(&mut ctx, &config, shutdown_rx.clone()).await?;

    let router = build_main_router(ctx);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await
        .map_err(|e| ServerError::Serve(format!("server error: {e}")))
}

/// Primary server entry point.
///
/// Per spec §2 — `run_server_with_listener_with_control`.
/// Accepts a pre-bound TcpListener (for test harness injection) and
/// a shutdown watch channel for programmatic shutdown.
pub async fn run_server_with_listener_with_control(
    config: ServerConfig,
    listener: TcpListener,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ServerError> {
    run_server_with_listener_and_log(config, listener, shutdown_rx, None, None).await
}

/// Primary server entry point with optional log controller.
///
/// Split out so that main.rs can inject a real `LogController` while
/// tests pass `None` to skip tracing reload setup.
pub async fn run_server_with_listener_and_log(
    config: ServerConfig,
    listener: TcpListener,
    shutdown_rx: watch::Receiver<bool>,
    log_controller: Option<Arc<LogController>>,
    shutdown_tx: Option<Arc<tokio::sync::watch::Sender<bool>>>,
) -> Result<(), ServerError> {
    // Step 2: Config validation — TLS is mandatory (no_ssl=false)
    validate_server_tls(&config, false)
        .map_err(|e| ServerError::Config(e))?;

    // Step 4-15: Initialize subsystems
    let shutdown = Arc::new(ShutdownCoordinator::new());
    let mut ctx = AppContext::new(config.clone(), shutdown.clone());
    ctx.log_controller = log_controller;
    ctx.shutdown_tx = shutdown_tx;

    // Build RBAC config once at startup instead of per-request (AN11.4)
    if config.base.admin_api.enabled {
        ctx.admin_auth_config = Some(build_admin_auth_config_for_rbac(&config));
    }

    // Step 17: Router is built per-connection in the TLS accept loop below.

    // Initialize runtime wiring (DataView engine, StorageEngine, gossip)
    crate::runtime::initialize_runtime(&ctx.pool, &ctx.config).await;

    // Initialize StorageEngine if configured (prerequisite for sessions, cache, polling)
    if config.storage_engine.backend != "none" {
        match rivers_runtime::rivers_core::storage::create_storage_engine(&config.storage_engine) {
            Ok(engine) => {
                let engine: Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine> = Arc::from(engine);
                // Sentinel claim for single-node enforcement (SHAPE-8)
                if config.storage_engine.backend == "redis" {
                    let node_id = config.app_id.as_deref().unwrap_or("node-0");
                    if let Err(e) = rivers_runtime::rivers_core::storage::claim_sentinel(&*engine, node_id).await {
                        return Err(ServerError::Config(format!(
                            "sentinel claim failed (another node active?): {e}"
                        )));
                    }
                    tracing::info!(node_id, "sentinel claimed");
                }
                // Spawn background sweep task
                let sweep_interval = config.storage_engine.sweep_interval_s;
                if sweep_interval > 0 {
                    rivers_runtime::rivers_core::storage::spawn_sweep_task(engine.clone(), sweep_interval);
                }
                ctx.storage_engine = Some(engine);
                tracing::info!(backend = %config.storage_engine.backend, "storage engine initialized");
            }
            Err(e) => {
                tracing::warn!(error = %e, "storage engine initialization failed — running without persistence");
            }
        }
    }

    // Initialize SessionManager and CsrfManager if StorageEngine is available
    if let Some(ref engine) = ctx.storage_engine {
        ctx.session_manager = Some(Arc::new(crate::session::SessionManager::new(
            engine.clone(),
            config.security.session.clone(),
        )));
        ctx.csrf_manager = Some(Arc::new(crate::csrf::CsrfManager::new(
            engine.clone(),
            config.security.csrf.clone(),
        )));
        tracing::info!("session manager and CSRF manager initialized");
    }

    // AM1.7: Validate session cookie http_only=true enforcement
    config.security.session.cookie.validate()
        .map_err(|e| ServerError::Config(e))?;

    // Register LogHandler on EventBus (Observe tier, wildcard subscriber)
    {
        let log_handler = Arc::new(rivers_runtime::rivers_core::logging::LogHandler::from_config(
            &config.base.logging,
            config.app_id.clone().unwrap_or_default(),
            "node-0".to_string(),
        ));
        log_handler.register(&ctx.event_bus).await;
        tracing::debug!("EventBus LogHandler registered");
    }

    // BB6: Load engine shared libraries from lib/ directory
    {
        let engine_dir = std::path::Path::new(&config.engines.dir);
        let callbacks = crate::engine_loader::build_host_callbacks();
        let results = crate::engine_loader::load_engines(engine_dir, &callbacks);
        let loaded: Vec<&str> = results.iter().filter_map(|r| match r {
            crate::engine_loader::EngineLoadResult::Success { name, .. } => Some(name.as_str()),
            _ => None,
        }).collect();
        if !loaded.is_empty() {
            tracing::info!(engines = ?loaded, "dynamic engines loaded from {}", engine_dir.display());
        }
    }

    // Auto-load bundle from config if bundle_path is set (AN13.2)
    crate::bundle_loader::load_and_wire_bundle(&mut ctx, &config, shutdown_rx.clone()).await?;

    // Wire host callbacks for cdylib engines — needs DataViewExecutor, StorageEngine, DriverFactory
    crate::engine_loader::set_host_context(
        ctx.dataview_executor.clone(),
        ctx.storage_engine.clone(),
        ctx.driver_factory.clone(),
    );

    // Wire shared keystore resolver for static engines (V8/WASM) — fallback when
    // TaskContext.keystore is None (which is always the case since dispatch sites
    // don't call .keystore() on the builder).
    if let Some(ref resolver) = ctx.keystore_resolver {
        crate::process_pool::set_keystore_resolver(resolver.clone());
    }

    // Step 18: Maybe spawn admin server
    let admin_handle = if config.base.admin_api.enabled {
        // SHAPE-25: validate TLS and access control before binding
        crate::tls::validate_admin_tls_config(&config.base.admin_api.tls)
            .map_err(|e| ServerError::Config(e))?;
        validate_admin_access_control(&config.base.admin_api)
            .map_err(|e| ServerError::Config(e))?;

        let admin_port = config.base.admin_api.port.unwrap_or(9090);
        let admin_addr: SocketAddr = format!("{}:{}", config.base.admin_api.host, admin_port)
            .parse()
            .map_err(|e| ServerError::Config(format!("invalid admin address: {}", e)))?;

        // Build admin TLS acceptor (auto-gen if certs absent)
        let admin_tls = config.base.admin_api.tls.as_ref()
            .expect("admin TLS validated above; presence guaranteed");
        let (admin_cert_path, admin_key_path) = match (&admin_tls.server_cert, &admin_tls.server_key) {
            (Some(cert), Some(key)) => (cert.clone(), key.clone()),
            (None, None) => {
                let data_dir = config.data_dir.as_deref().unwrap_or("data");
                let app_id = config.app_id.as_deref().unwrap_or("default");
                crate::tls::maybe_autogen_admin_tls_cert(data_dir, app_id)
                    .map_err(|e| ServerError::Config(e))?
            }
            _ => return Err(ServerError::Config("cert and key must both be specified or both absent".into())),
        };
        let admin_acceptor = crate::tls::load_tls_acceptor(&admin_cert_path, &admin_key_path, false, "tls12")
            .map_err(|e| ServerError::Config(e))?;

        let admin_listener = TcpListener::bind(admin_addr)
            .await
            .map_err(|e| ServerError::Bind(format!("admin server bind failed: {}", e)))?;

        tracing::info!(addr = %admin_addr, "admin server listening (TLS)");

        let admin_ctx = ctx.clone();
        let admin_shutdown_rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = admin_listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                let acceptor = admin_acceptor.clone();
                                let app = build_admin_router(admin_ctx.clone());
                                tokio::spawn(async move {
                                    match acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            let io = hyper_util::rt::TokioIo::new(tls_stream);
                                            let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                                let app = app.clone();
                                                async move {
                                                    use tower::ServiceExt;
                                                    app.oneshot(req).await
                                                }
                                            });
                                            if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                                                hyper_util::rt::TokioExecutor::new(),
                                            )
                                            .serve_connection(io, service)
                                            .await
                                            {
                                                tracing::debug!(addr = %addr, error = %e, "admin TLS connection error");
                                            }
                                        }
                                        Err(e) => {
                                            tracing::debug!(addr = %addr, error = %e, "admin TLS handshake failed");
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "admin accept error");
                            }
                        }
                    }
                    _ = shutdown_signal(admin_shutdown_rx.clone()) => break,
                }
            }
        }))
    } else {
        None
    };

    // Step 19: Maybe spawn HTTP redirect server (SHAPE-22)
    let redirect_handle = if let Some(ref tls) = config.base.tls {
        if tls.redirect {
            maybe_spawn_http_redirect_server(
                config.base.port,
                tls.redirect_port,
                shutdown_rx.clone(),
            )
            .await
        } else {
            None
        }
    } else {
        None
    };

    // Step 20: Maybe spawn hot reload watcher (dev mode — when config_path is set)
    let _hot_reload_watcher = if let Some(ref path) = ctx.config_path {
        let hr_state = Arc::new(HotReloadState::new(
            config.clone(),
            Some(path.clone()),
        ));
        ctx.hot_reload_state = Some(hr_state.clone());

        // Step 20b: Spawn hot reload listener that rebuilds views/DataViews on config change
        {
            let mut version_rx = hr_state.subscribe();
            let reload_ctx = ctx.clone();
            let reload_hr = hr_state.clone();
            tokio::spawn(async move {
                loop {
                    if version_rx.changed().await.is_err() {
                        // Sender dropped — hot reload state is gone
                        break;
                    }
                    let version = *version_rx.borrow();
                    tracing::info!(version, "hot reload: config change detected, rebuilding views");

                    let reload_config = reload_hr.current_config().await;
                    if let Some(bundle_path) = reload_config.bundle_path.as_deref() {
                        match crate::bundle_loader::rebuild_views_and_dataviews(
                            &reload_ctx,
                            &reload_config,
                            bundle_path,
                        ).await {
                            Ok(summary) => {
                                tracing::info!(
                                    version,
                                    apps = summary.apps,
                                    views = summary.views,
                                    dataviews = summary.dataviews,
                                    "hot reload: rebuild complete"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    version,
                                    error = %e,
                                    "hot reload: rebuild failed"
                                );
                            }
                        }
                    }
                }
            });
        }

        maybe_spawn_hot_reload_watcher(Some(path), hr_state)
    } else {
        None
    };

    let local_addr = listener
        .local_addr()
        .map_err(|e| ServerError::Bind(format!("failed to get local addr: {}", e)))?;
    tracing::info!(addr = %local_addr, "main server listening");

    // Step 21: Serve — TLS is mandatory (validated above).
    let shutdown_clone = shutdown.clone();

    // Build TLS acceptor from [base.tls] — mandatory, auto-gen if cert/key absent.
    let tls = config.base.tls.as_ref()
        .expect("TLS config validated above; presence guaranteed");

    let (cert_path, key_path) = match (&tls.cert, &tls.key) {
        (Some(cert), Some(key)) => (cert.clone(), key.clone()),
        (None, None) => {
            let data_dir = config.data_dir.as_deref().unwrap_or("data");
            let app_id = config.app_id.as_deref().unwrap_or("default");
            crate::tls::maybe_autogen_tls_cert(&tls.x509, data_dir, app_id)
                .map_err(|e| ServerError::Config(e))?
        }
        _ => unreachable!("validated above"),
    };

    let http2_enabled = config.base.http2.enabled;
    let min_tls_version = config.base.tls.as_ref()
        .map(|t| t.engine.min_version.as_str())
        .unwrap_or("tls12");
    let acceptor = crate::tls::load_tls_acceptor(&cert_path, &key_path, http2_enabled, min_tls_version)
        .map_err(|e| ServerError::Config(e))?;

    let h2_max_streams = config.base.http2.max_concurrent_streams.unwrap_or(250);
    let h2_window_size = config.base.http2.initial_window_size.unwrap_or(1_048_576); // 1MB

    if http2_enabled {
        tracing::info!(
            max_streams = h2_max_streams,
            window_size = h2_window_size,
            "TLS enabled — serving HTTPS with HTTP/2 (ALPN: h2, http/1.1)"
        );
    } else {
        tracing::info!("TLS enabled — serving HTTPS");
    }

    // Manual TLS accept loop with graceful shutdown
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        let acceptor = acceptor.clone();
                        let app = build_main_router(ctx.clone());
                        tokio::spawn(async move {
                            match acceptor.accept(stream).await {
                                Ok(tls_stream) => {
                                    let io = hyper_util::rt::TokioIo::new(tls_stream);
                                    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                        let app = app.clone();
                                        async move {
                                            use tower::ServiceExt;
                                            app.oneshot(req).await
                                        }
                                    });
                                    let mut builder = hyper_util::server::conn::auto::Builder::new(
                                        hyper_util::rt::TokioExecutor::new(),
                                    );
                                    if http2_enabled {
                                        builder.http2()
                                            .max_concurrent_streams(h2_max_streams)
                                            .initial_stream_window_size(h2_window_size)
                                            .initial_connection_window_size(h2_window_size);
                                    }
                                    if let Err(e) = builder
                                    .serve_connection(io, service)
                                    .await
                                    {
                                        tracing::debug!(addr = %addr, error = %e, "TLS connection error");
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!(addr = %addr, error = %e, "TLS handshake failed");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "accept error");
                    }
                }
            }
            _ = shutdown_signal(shutdown_rx.clone()) => {
                tracing::info!("shutdown signal received");
                break;
            }
        }
    }

    // Graceful shutdown sequence per spec §13
    shutdown_clone.mark_draining();
    shutdown_clone.wait_for_drain().await;

    // Abort admin server if running
    if let Some(handle) = admin_handle {
        handle.abort();
    }

    // Abort HTTP redirect server if running
    if let Some(handle) = redirect_handle {
        handle.abort();
    }

    tracing::info!("server shutdown complete");
    Ok(())
}

/// Spawn a hot-reload file watcher if a config path is available.
///
/// Per spec §2 step 21 / §16: dev mode only, non-fatal on failure.
fn maybe_spawn_hot_reload_watcher(
    config_path: Option<&std::path::Path>,
    state: Arc<HotReloadState>,
) -> Option<FileWatcher> {
    let path = config_path?;
    match FileWatcher::new(path.to_path_buf(), state) {
        Ok(watcher) => Some(watcher),
        Err(e) => {
            tracing::warn!(error = %e, "hot reload watcher failed to start — continuing without");
            None
        }
    }
}

/// Wait for a shutdown signal.
///
/// Per spec §13.1: SIGTERM, SIGINT, or watch channel.
async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    let watch = async move {
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    };

    tokio::select! {
        _ = ctrl_c => {}
        _ = sigterm => {}
        _ = watch => {}
    }
}

/// Validate admin server access control rules.
///
/// SHAPE-25: TLS is mandatory (validated separately via validate_admin_tls_config).
/// Ed25519 public_key is required regardless of bind address.
/// The localhost plain-HTTP exception is removed.
pub fn validate_admin_access_control(
    admin: &rivers_runtime::rivers_core::config::AdminApiConfig,
) -> Result<(), String> {
    if admin.public_key.is_none() {
        return Err(
            "admin API requires public_key to be configured (Ed25519 auth is mandatory)".to_string()
        );
    }
    Ok(())
}

/// Validate TLS configuration at startup.
///
/// Per spec §2: `[base.tls]` is required unless `--no-ssl` is active.
/// When `no_ssl = true`, skips all TLS validation.
/// SHAPE-25: admin TLS is always validated when admin_api is enabled (no --no-ssl bypass).
pub fn validate_server_tls(config: &ServerConfig, no_ssl: bool) -> Result<(), String> {
    // SHAPE-25: admin TLS is always required regardless of --no-ssl
    if config.base.admin_api.enabled {
        crate::tls::validate_admin_tls_config(&config.base.admin_api.tls)?;
    }

    if no_ssl {
        return Ok(());
    }

    // Main server TLS checks (skipped when --no-ssl)
    crate::tls::validate_tls_config(&config.base.tls)?;

    if let Some(ref tls) = config.base.tls {
        crate::tls::validate_redirect_port(config.base.port, tls.redirect_port)?;
    }

    if config.base.http2.enabled && config.base.tls.is_none() {
        return Err("HTTP/2 requires TLS: add [base.tls] to your config".to_string());
    }

    Ok(())
}

// ── Error Types ───────────────────────────────────────────────────

/// Server startup/runtime errors.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("config error: {0}")]
    Config(String),

    #[error("bind error: {0}")]
    Bind(String),

    #[error("serve error: {0}")]
    Serve(String),
}

// ── Unit Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn run_server_rejects_missing_tls_config() {
        use rivers_runtime::rivers_core::ServerConfig;
        let config = ServerConfig::default();
        let result = super::validate_server_tls(&config, false);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("TLS is required") || msg.contains("[base.tls]"));
    }

    #[test]
    fn run_server_skips_tls_validation_with_no_ssl() {
        use rivers_runtime::rivers_core::ServerConfig;
        let config = ServerConfig::default();
        let result = super::validate_server_tls(&config, true);
        assert!(result.is_ok());
    }

    #[test]
    fn admin_access_control_rejects_no_public_key() {
        use rivers_runtime::rivers_core::config::{AdminApiConfig, AdminTlsConfig};
        let mut admin = AdminApiConfig::default();
        admin.enabled = true;
        admin.public_key = None;
        admin.tls = Some(AdminTlsConfig {
            server_cert: None,
            server_key: None,
            ca_cert: None,
            require_client_cert: false,
        });
        let result = super::validate_admin_access_control(&admin);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("public_key"));
    }

    #[test]
    fn admin_access_control_accepts_with_public_key() {
        use rivers_runtime::rivers_core::config::{AdminApiConfig, AdminTlsConfig};
        let mut admin = AdminApiConfig::default();
        admin.enabled = true;
        admin.public_key = Some("/etc/admin.pub".to_string());
        admin.tls = Some(AdminTlsConfig {
            server_cert: None, server_key: None, ca_cert: None,
            require_client_cert: false,
        });
        let result = super::validate_admin_access_control(&admin);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn redirect_server_responds_with_301() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(super::run_http_redirect_server(
            listener,
            443,
            shutdown_rx,
        ));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/foo?bar=1"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 301);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("https://"), "location was: {location}");
        assert!(location.contains("/foo?bar=1"), "location was: {location}");

        let _ = shutdown_tx.send(true);
        handle.abort();
    }
}
