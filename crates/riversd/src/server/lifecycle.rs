//! Server lifecycle — entry points, TLS accept loops, HTTP redirect, hot reload.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::watch;

use rivers_runtime::rivers_core::ServerConfig;

use crate::hot_reload::HotReloadState;
use crate::shutdown::ShutdownCoordinator;

use super::admin_auth::build_admin_auth_config_for_rbac;
use super::context::AppContext;
use super::router::{build_main_router, build_admin_router};
use super::validation::{
    validate_admin_access_control,
    validate_server_tls,
    shutdown_signal,
    maybe_spawn_hot_reload_watcher,
    ServerError,
};

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

    // Initialize runtime wiring (DataView engine, StorageEngine, gossip)
    crate::runtime::initialize_runtime(&ctx.pool, &ctx.config).await;

    // Metrics: start Prometheus exporter
    #[cfg(feature = "metrics")]
    if let Some(ref metrics_cfg) = config.metrics {
        if metrics_cfg.enabled {
            let port = metrics_cfg.port.unwrap_or(9091);
            match metrics_exporter_prometheus::PrometheusBuilder::new()
                .with_http_listener(([0, 0, 0, 0], port))
                .install()
            {
                Ok(()) => tracing::info!(port = port, "prometheus metrics exporter started on :{port}"),
                Err(e) => tracing::warn!(error = %e, "failed to start metrics exporter"),
            }
        }
    }

    // Initialize StorageEngine if configured (prerequisite for sessions, cache, polling)
    if config.storage_engine.backend != "none" {
        match rivers_runtime::rivers_core::storage::create_storage_engine(&config.storage_engine) {
            Ok(engine) => {
                let engine: Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine> = Arc::from(engine);
                if config.storage_engine.backend == "redis" {
                    let node_id = config.app_id.as_deref().unwrap_or("node-0");
                    if let Err(e) = rivers_runtime::rivers_core::storage::claim_sentinel(&*engine, node_id).await {
                        return Err(ServerError::Config(format!(
                            "sentinel claim failed (another node active?): {e}"
                        )));
                    }
                    tracing::info!(node_id, "sentinel claimed");
                }
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

    // Validate session cookie http_only=true enforcement
    config.security.session.cookie.validate()
        .map_err(|e| ServerError::Config(e))?;

    // Per-app logging: create router if app_log_dir is configured
    if let Some(ref app_log_dir) = config.base.logging.app_log_dir {
        let router = std::sync::Arc::new(
            rivers_runtime::rivers_core::app_log_router::AppLogRouter::new(
                std::path::Path::new(app_log_dir),
            ),
        );
        rivers_runtime::rivers_core::app_log_router::set_global_router(router);
        tracing::info!(dir = %app_log_dir, "per-app logging enabled");
    }

    // Register LogHandler on EventBus
    {
        let log_handler = Arc::new(rivers_runtime::rivers_core::logging::LogHandler::from_config(
            &config.base.logging,
            config.app_id.clone().unwrap_or_default(),
            "node-0".to_string(),
        ));
        log_handler.register(&ctx.event_bus).await;
    }

    // Auto-load bundle from config if bundle_path is set
    crate::bundle_loader::load_and_wire_bundle(&mut ctx, &config, shutdown_rx.clone()).await?;
    crate::task_enrichment::sync_from_app_context(&ctx);

    // Wire host callbacks for cdylib engines (no-op if already set
    // during bundle load — OnceLock ensures idempotent)
    crate::engine_loader::set_host_context(
        ctx.dataview_executor.clone(),
        ctx.storage_engine.clone(),
        ctx.driver_factory.clone(),
    );
    crate::engine_loader::set_ddl_whitelist(config.security.ddl_whitelist.clone());

    // Wire shared keystore resolver for static engines
    if let Some(ref resolver) = ctx.keystore_resolver {
        crate::process_pool::set_keystore_resolver(resolver.clone());
    }

    // Spawn plain HTTP admin server (--no-ssl mode)
    let admin_handle = if config.base.admin_api.enabled {
        if config.base.admin_api.no_auth != Some(true) {
            validate_admin_access_control(&config.base.admin_api)
                .map_err(|e| ServerError::Config(e))?;
        }

        let admin_port = config.base.admin_api.port.unwrap_or(9090);
        let admin_addr: SocketAddr = format!("{}:{}", config.base.admin_api.host, admin_port)
            .parse()
            .map_err(|e| ServerError::Config(format!("invalid admin address: {}", e)))?;

        let admin_listener = TcpListener::bind(admin_addr)
            .await
            .map_err(|e| ServerError::Bind(format!("admin server bind failed: {}", e)))?;

        tracing::info!(addr = %admin_addr, "admin server listening (plain HTTP — no-ssl mode)");

        let admin_ctx = ctx.clone();
        let admin_shutdown_rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = admin_listener.accept() => {
                        match result {
                            Ok((stream, _addr)) => {
                                let app = build_admin_router(admin_ctx.clone());
                                tokio::spawn(async move {
                                    let io = hyper_util::rt::TokioIo::new(stream);
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
                                        tracing::debug!(error = %e, "admin connection error");
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

    let router = build_main_router(ctx);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await
        .map_err(|e| ServerError::Serve(format!("server error: {e}")))?;

    // Abort admin server if running
    if let Some(handle) = admin_handle {
        handle.abort();
    }

    Ok(())
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
    log_controller: Option<Arc<super::context::LogController>>,
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

    // Metrics: start Prometheus exporter
    #[cfg(feature = "metrics")]
    if let Some(ref metrics_cfg) = config.metrics {
        if metrics_cfg.enabled {
            let port = metrics_cfg.port.unwrap_or(9091);
            match metrics_exporter_prometheus::PrometheusBuilder::new()
                .with_http_listener(([0, 0, 0, 0], port))
                .install()
            {
                Ok(()) => tracing::info!(port = port, "prometheus metrics exporter started on :{port}"),
                Err(e) => tracing::warn!(error = %e, "failed to start metrics exporter"),
            }
        }
    }

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

    // Per-app logging: create router if app_log_dir is configured
    if let Some(ref app_log_dir) = config.base.logging.app_log_dir {
        let router = std::sync::Arc::new(
            rivers_runtime::rivers_core::app_log_router::AppLogRouter::new(
                std::path::Path::new(app_log_dir),
            ),
        );
        rivers_runtime::rivers_core::app_log_router::set_global_router(router);
        tracing::info!(dir = %app_log_dir, "per-app logging enabled");
    }

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
    crate::task_enrichment::sync_from_app_context(&ctx);

    // Wire host callbacks for cdylib engines (no-op if already set
    // during bundle load — OnceLock ensures idempotent)
    crate::engine_loader::set_host_context(
        ctx.dataview_executor.clone(),
        ctx.storage_engine.clone(),
        ctx.driver_factory.clone(),
    );
    crate::engine_loader::set_ddl_whitelist(config.security.ddl_whitelist.clone());

    // Wire shared keystore resolver for static engines (V8/WASM) — fallback when
    // TaskContext.keystore is not present for a given dispatch path.
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

    // Flush per-app logs before exit
    if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
        router.flush_all();
    }

    // Flush and shut down the OTel provider so the last span batch is exported
    // before the process exits. No-op when [telemetry] was not configured.
    crate::telemetry::shutdown();

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
