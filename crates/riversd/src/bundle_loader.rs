//! Bundle loading and wiring extracted from server.rs (AN13.2).
//!
//! Loads a Rivers bundle from disk, resolves LockBox credentials,
//! registers DataViews, builds ConnectionParams, wires broker bridges
//! and MessageConsumer handlers, and detects guard views.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use zeroize::Zeroize;

use rivers_runtime::rivers_core::ServerConfig;
use rivers_runtime::DataViewExecutor;

use crate::server::{AppContext, ServerError, register_all_drivers};
use crate::view_engine;

// ── SSE Trigger Handler ─────────────────────────────────────────────

/// EventHandler that pushes an SSE event when a trigger event fires on the EventBus.
///
/// Registered per trigger-event per SSE view during bundle loading.
#[allow(dead_code)]
struct SseTriggerHandler {
    channel: Arc<crate::sse::SseChannel>,
    view_id: String,
}

#[async_trait]
impl rivers_runtime::rivers_core::eventbus::EventHandler for SseTriggerHandler {
    async fn handle(&self, event: &rivers_runtime::rivers_core::event::Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sse_event = crate::sse::SseEvent::typed(
            event.event_type.clone(),
            serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string()),
        );
        // Ignore NoActiveClients — no subscribers connected yet is fine
        let _ = self.channel.push(sse_event);
        Ok(())
    }

    fn name(&self) -> &str {
        "SseTriggerHandler"
    }
}

/// Load a bundle from disk and wire all subsystems into AppContext.
///
/// This function performs:
/// - TOML bundle parsing
/// - DataView registration (namespaced by app entry_point)
/// - ConnectionParams construction per datasource
/// - LockBox credential resolution (with zeroize)
/// - DriverFactory setup with all drivers
/// - DataViewExecutor + ViewRouter construction
/// - Broker bridge spawning + MessageConsumer wiring
/// - Guard view detection
/// - Session manager construction (delegated to caller)
pub async fn load_and_wire_bundle(
    ctx: &mut AppContext,
    config: &ServerConfig,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), ServerError> {
    let bundle_path = match config.bundle_path {
        Some(ref bp) => bp.clone(),
        None => return Ok(()),
    };

    let path = std::path::Path::new(&bundle_path);
    let bundle = match rivers_runtime::load_bundle(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(path = %bundle_path, error = %e, "failed to load bundle");
            return Err(ServerError::Config(format!(
                "bundle_path '{}' could not be loaded: {}",
                bundle_path, e
            )));
        }
    };

    // ── AT3.2 (A): Validate bundle before wiring ──
    if let Err(errors) = rivers_runtime::validate_bundle(&bundle) {
        let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        tracing::error!(path = %bundle_path, "bundle validation failed: {}", msg);
        return Err(ServerError::Config(format!("bundle validation failed: {}", msg)));
    }

    let mut registry = rivers_runtime::DataViewRegistry::new();
    let mut ds_params: HashMap<String, rivers_runtime::rivers_driver_sdk::ConnectionParams> = HashMap::new();
    let mut view_count = 0usize;

    // ── LockBox: collect credential references from all datasources ──
    let mut lockbox_refs: Vec<(&str, &str)> = Vec::new();
    for app in &bundle.apps {
        for ds in app.config.data.datasources.values() {
            if let Some(ref cred_src) = ds.credentials_source {
                lockbox_refs.push((&ds.name, cred_src.as_str()));
            }
        }
    }
    let lb_references = rivers_runtime::rivers_core::lockbox::collect_lockbox_references(&lockbox_refs);

    // ── LockBox: resolve if references exist ──
    let lockbox_resolved: HashMap<String, rivers_runtime::rivers_core::lockbox::EntryMetadata> =
        if !lb_references.is_empty() {
            let lb_config = config.lockbox.as_ref().ok_or_else(|| {
                ServerError::Config(
                    "lockbox credentials referenced but [lockbox] not configured".into(),
                )
            })?;
            let (resolver, resolved) =
                rivers_runtime::rivers_core::lockbox::startup_resolve(lb_config, &lb_references)
                    .map_err(|e| ServerError::Config(format!("lockbox: {e}")))?;
            ctx.lockbox_resolver = Some(Arc::new(resolver));
            tracing::info!(entries = resolved.len(), "lockbox: credentials resolved");
            resolved
        } else {
            HashMap::new()
        };

    for app in &bundle.apps {
        let entry_point = app
            .manifest
            .entry_point
            .as_deref()
            .unwrap_or(&app.manifest.app_name);

        // Count views for logging
        view_count += app.config.api.views.len();

        // Register dataviews — namespaced by entry_point to prevent collisions
        for dv in app.config.data.dataviews.values() {
            let mut namespaced_dv = dv.clone();
            namespaced_dv.name = format!("{}:{}", entry_point, dv.name);
            namespaced_dv.datasource = format!("{}:{}", entry_point, dv.datasource);
            registry.register(namespaced_dv);
        }

        // Build ConnectionParams — namespaced: "postgres:pg"
        for ds in app.config.data.datasources.values() {
            let mut params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
                host: ds.host.clone().unwrap_or_default(),
                port: ds.port.unwrap_or(0),
                database: ds.database.clone().unwrap_or_default(),
                username: ds.username.clone().unwrap_or_default(),
                password: String::new(),
                options: HashMap::new(),
            };
            params.options.insert("driver".into(), ds.driver.clone());
            // Pass extra config as options (driver-specific)
            for (k, v) in &ds.extra {
                params.options.insert(k.clone(), v.clone());
            }
            // Pass write_batch config to driver via options
            if let Some(ref wb) = ds.write_batch {
                if wb.enabled {
                    params.options.insert("write_batch_enabled".into(), "true".into());
                    params.options.insert("write_batch_max_size".into(), wb.max_size.to_string());
                    params.options.insert("write_batch_flush_interval_ms".into(), wb.flush_interval_ms.to_string());
                    tracing::info!(
                        datasource = %ds.name,
                        max_size = wb.max_size,
                        flush_interval_ms = wb.flush_interval_ms,
                        "write batching enabled"
                    );
                }
            }

            // ── LockBox: fetch credential if datasource has lockbox reference ──
            if !ds.nopassword {
                if let Some(metadata) = lockbox_resolved.get(&ds.name) {
                    let lb_config = config.lockbox.as_ref().unwrap();
                    let keystore_path = std::path::Path::new(
                        lb_config.path.as_deref().unwrap_or(""),
                    );
                    let identity_str = rivers_runtime::rivers_core::lockbox::resolve_key_source(lb_config)
                        .map_err(|e| ServerError::Config(format!("lockbox key: {e}")))?;
                    let mut resolved = rivers_runtime::rivers_core::lockbox::fetch_secret_value(
                        metadata,
                        keystore_path,
                        identity_str.trim(),
                    )
                    .map_err(|e| ServerError::Config(format!("lockbox fetch: {e}")))?;
                    params.password = resolved.value.clone();
                    // Zeroize per SHAPE-5
                    resolved.value.zeroize();
                    tracing::info!(datasource = %ds.name, "lockbox: credential loaded");
                }
            }

            let namespaced_key = format!("{}:{}", entry_point, ds.name);
            ds_params.insert(namespaced_key, params);
        }
    }

    // Build DriverFactory with all drivers (built-in + plugins)
    let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
    register_all_drivers(&mut factory, &config.plugins.ignore);

    let app_count = bundle.apps.len();
    let dv_count = registry.count();

    // ── AT3.4 (D): Validate driver names against registered drivers ──
    {
        let mut known: Vec<&str> = factory.driver_names();
        known.extend(factory.broker_driver_names());
        let driver_errors = rivers_runtime::validate_known_drivers(&bundle, &known);

        // Check if any unknown drivers are in the ignore list — those are hard failures.
        // A bundle that references an explicitly ignored driver cannot be loaded.
        let ignored = &config.plugins.ignore;
        let ignored_in_bundle: Vec<String> = if !ignored.is_empty() {
            // Collect driver names from all datasources in the bundle
            bundle.apps.iter()
                .flat_map(|app| app.config.data.datasources.values())
                .map(|ds| ds.driver.clone())
                .filter(|d| ignored.iter().any(|i| i == d))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };

        if !ignored_in_bundle.is_empty() {
            let msg = format!(
                "bundle requires ignored drivers: {} — remove from [plugins].ignore or remove datasources from bundle",
                ignored_in_bundle.join(", ")
            );
            tracing::error!("{}", msg);
            return Err(ServerError::Config(msg));
        }

        if !driver_errors.is_empty() {
            let msg = driver_errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
            tracing::warn!("driver validation: {}", msg);
            // Warn but don't block — unknown drivers may be loaded via plugins later
        }
    }

    let factory = Arc::new(factory);

    // Build DataView cache from StorageEngine if available.
    // Aggregate caching policy from all DataView configs — use the most permissive values.
    let cache_policy = build_cache_policy_from_bundle(&bundle);
    let cache: Option<Arc<dyn rivers_runtime::tiered_cache::DataViewCache>> =
        ctx.storage_engine.as_ref().map(|engine| {
            let tiered = rivers_runtime::tiered_cache::TieredDataViewCache::new(
                cache_policy.clone(),
            )
            .with_storage(engine.clone());
            Arc::new(tiered) as Arc<dyn rivers_runtime::tiered_cache::DataViewCache>
        });
    if cache_policy.l2_enabled {
        tracing::info!("DataView cache: L1 + L2 enabled (max L1 entries: {})", cache_policy.l1_max_entries);
    } else if cache_policy.l1_enabled {
        tracing::info!("DataView cache: L1 enabled (max entries: {})", cache_policy.l1_max_entries);
    }

    let ds_params = Arc::new(ds_params);
    let mut executor = DataViewExecutor::new(registry, factory.clone(), ds_params.clone(), cache);
    executor.set_event_bus(ctx.event_bus.clone());
    *ctx.dataview_executor.write().await = Some(executor);
    ctx.driver_factory = Some(factory.clone());

    // Build namespaced router: /<prefix>/<bundle>/<app>/<view>
    let router = view_engine::ViewRouter::from_bundle(
        &bundle,
        config.route_prefix.as_deref(),
    );
    *ctx.view_router.write().await = Some(router);

    // ── AR2.3: Build GraphQL schema from DataViews if enabled ──
    if config.graphql.enabled {
        let executor_ref = ctx.dataview_executor.clone();
        let guard = executor_ref.read().await;
        if let Some(ref exec) = *guard {
            let dv_names = exec.registry().names();
            let resolvers = crate::graphql::build_resolver_mappings_from_dataviews(&dv_names);
            drop(guard); // Release read lock before building schema

            // Scan views for CodeComponent mutations
            let mut mutation_mappings = Vec::new();
            for app in &bundle.apps {
                let ep = app.manifest.entry_point.as_deref()
                    .unwrap_or(&app.manifest.app_name);
                mutation_mappings.extend(
                    crate::graphql::build_mutation_mappings_from_views(&app.config.api.views, ep)
                );
            }

            // Scan views for subscription topics (SSE trigger events)
            let mut subscription_mappings = Vec::new();
            for app in &bundle.apps {
                subscription_mappings.extend(
                    crate::graphql::build_subscription_mappings_from_views(&app.config.api.views)
                );
            }

            let gql_config = crate::graphql::GraphqlConfig::from(&config.graphql);
            match crate::graphql::build_schema_with_executor(
                &gql_config,
                &resolvers,
                ctx.dataview_executor.clone(),
                &mutation_mappings,
                ctx.pool.clone(),
                &subscription_mappings,
                ctx.event_bus.clone(),
            ) {
                Ok(schema) => {
                    *ctx.graphql_schema.write().await = Some(schema);
                    tracing::info!(
                        path = %config.graphql.path,
                        resolvers = resolvers.len(),
                        mutations = mutation_mappings.len(),
                        "GraphQL schema built"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "GraphQL schema build failed — endpoint disabled");
                }
            }
        } else {
            drop(guard);
        }
    }

    // ── AM1.2: Validate protected views require StorageEngine ──
    // ── AM1.3: Detect guard view, reject multiples ──
    {
        let mut all_views: HashMap<String, rivers_runtime::view::ApiViewConfig> = HashMap::new();
        for app in &bundle.apps {
            for (id, view_cfg) in &app.config.api.views {
                all_views.insert(id.clone(), view_cfg.clone());
            }
        }

        // AM1.2: If any view has auth != "none" and no StorageEngine, reject
        let has_protected = all_views.values().any(|v| {
            !crate::guard::is_public_view(v)
        });
        if has_protected && ctx.storage_engine.is_none() {
            return Err(ServerError::Config(
                "protected views require [storage_engine] to be configured".into(),
            ));
        }

        // AM1.3: Detect guard view — at most one allowed
        let detection = crate::guard::detect_guard_view(&all_views);
        if !detection.errors.is_empty() {
            return Err(ServerError::Config(
                detection.errors.join("; "),
            ));
        }
        if let Some(ref guard_id) = detection.guard_view_id {
            tracing::info!(guard_view = %guard_id, "guard view detected");
        }
        ctx.guard_view_id = detection.guard_view_id;
    }

    // ── AL2: Wire broker consumer bridges + MessageConsumer handlers ──
    // Scan apps for broker datasources and MessageConsumer views.
    let mut broker_bridge_count = 0usize;
    let mut consumer_count = 0usize;

    for app in &bundle.apps {
        let entry_point = app.manifest.entry_point.as_deref()
            .unwrap_or(&app.manifest.app_name);

        // AL2.1: Find broker datasources in this app's resources
        for ds in &app.resources.datasources {
            if let Some(broker_driver) = factory.get_broker_driver(&ds.driver) {
                // AL2.2: Create broker consumer and spawn bridge
                let namespaced_key = format!("{}:{}", entry_point, ds.name);
                if let Some(params) = ds_params.get(&namespaced_key) {
                    // Collect subscriptions from MessageConsumer views targeting this datasource
                    let mut subscriptions = Vec::new();
                    for (view_id, view_cfg) in &app.config.api.views {
                        if view_cfg.view_type == "MessageConsumer" {
                            // Use the view_id as the topic name for the subscription
                            subscriptions.push(
                                rivers_runtime::rivers_driver_sdk::broker::BrokerSubscription {
                                    topic: view_id.clone(),
                                    event_name: Some(view_id.clone()),
                                },
                            );
                        }
                    }

                    if subscriptions.is_empty() {
                        continue;
                    }

                    // Read consumer config from the full DatasourceConfig (app.toml)
                    let full_ds_config = app.config.data.datasources.get(&ds.name);
                    let consumer_cfg = full_ds_config.and_then(|d| d.consumer.as_ref());

                    let group_prefix = consumer_cfg
                        .and_then(|c| c.group_prefix.as_deref())
                        .unwrap_or("rivers")
                        .to_string();
                    let reconnect_ms = consumer_cfg
                        .map(|c| c.reconnect_ms)
                        .unwrap_or(5000);

                    // Build failure policy from config (default: Drop)
                    let failure_policy = consumer_cfg
                        .and_then(|c| c.subscriptions.first())
                        .and_then(|s| s.on_failure.as_ref())
                        .map(|fp| {
                            let mode = match fp.mode.as_str() {
                                "dead_letter" => rivers_runtime::rivers_driver_sdk::broker::FailureMode::DeadLetter,
                                "requeue" => rivers_runtime::rivers_driver_sdk::broker::FailureMode::Requeue,
                                "redirect" => rivers_runtime::rivers_driver_sdk::broker::FailureMode::Redirect,
                                _ => rivers_runtime::rivers_driver_sdk::broker::FailureMode::Drop,
                            };
                            rivers_runtime::rivers_driver_sdk::broker::FailurePolicy {
                                mode,
                                destination: fp.destination.clone(),
                                handlers: Vec::new(),
                            }
                        })
                        .unwrap_or(rivers_runtime::rivers_driver_sdk::broker::FailurePolicy {
                            mode: rivers_runtime::rivers_driver_sdk::broker::FailureMode::Drop,
                            destination: None,
                            handlers: Vec::new(),
                        });

                    // Warn if manual ack mode is configured (not yet supported)
                    if let Some(cfg) = consumer_cfg {
                        for sub in &cfg.subscriptions {
                            if sub.ack_mode == "manual" {
                                tracing::warn!(
                                    datasource = %ds.name,
                                    topic = %sub.topic,
                                    "ack_mode='manual' is not yet supported — using 'auto'"
                                );
                            }
                        }
                    }

                    let broker_config = rivers_runtime::rivers_driver_sdk::broker::BrokerConsumerConfig {
                        group_prefix,
                        app_id: app.manifest.app_id.clone(),
                        datasource_id: ds.name.clone(),
                        node_id: "node-0".to_string(),
                        reconnect_ms,
                        subscriptions,
                    };

                    match broker_driver.create_consumer(params, &broker_config).await {
                        Ok(consumer) => {
                            let bridge = crate::broker_bridge::BrokerConsumerBridge::new(
                                consumer,
                                ctx.event_bus.clone(),
                                failure_policy,
                                &ds.name,
                                reconnect_ms,
                                shutdown_rx.clone(),
                            );
                            tokio::spawn(bridge.run());
                            broker_bridge_count += 1;
                            tracing::info!(
                                datasource = %ds.name,
                                driver = %ds.driver,
                                "broker bridge started"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                datasource = %ds.name,
                                error = %e,
                                "broker consumer creation failed — bridge not started"
                            );
                        }
                    }
                }
            }
        }

        // AL2.3: Build MessageConsumerRegistry and subscribe handlers
        let mc_registry = crate::message_consumer::MessageConsumerRegistry::from_views(
            &app.config.api.views,
        );
        if !mc_registry.is_empty() {
            consumer_count += mc_registry.len();
            crate::message_consumer::subscribe_message_consumers(
                &mc_registry,
                &ctx.event_bus,
                ctx.pool.clone(),
            )
            .await;
        }
    }

    if broker_bridge_count > 0 || consumer_count > 0 {
        tracing::info!(
            bridges = broker_bridge_count,
            consumers = consumer_count,
            "broker pipeline wired"
        );
    }

    // ── AL3: Wire datasource event handlers ──
    // Read event_handlers from DatasourceConfig and log configured handlers.
    for app in &bundle.apps {
        for ds in app.config.data.datasources.values() {
            if let Some(ref handlers) = ds.event_handlers {
                for handler in &handlers.on_connection_failed {
                    tracing::info!(
                        datasource = %ds.name,
                        module = %handler.module,
                        entrypoint = %handler.entrypoint,
                        event = "on_connection_failed",
                        "datasource event handler registered"
                    );
                }
                for handler in &handlers.on_pool_exhausted {
                    tracing::info!(
                        datasource = %ds.name,
                        module = %handler.module,
                        entrypoint = %handler.entrypoint,
                        event = "on_pool_exhausted",
                        "datasource event handler registered"
                    );
                }
            }
        }
    }

    // ── Wire SSE and WebSocket view managers ──
    let mut sse_count = 0usize;
    let mut ws_count = 0usize;

    for app in &bundle.apps {
        let entry_point = app.manifest.entry_point.as_deref()
            .unwrap_or(&app.manifest.app_name);

        for (view_id, view_cfg) in &app.config.api.views {
            let qualified_id = format!("{}:{}", entry_point, view_id);

            match view_cfg.view_type.as_str() {
                "ServerSentEvents" => {
                    let tick_ms = view_cfg.sse_tick_interval_ms.unwrap_or(0);
                    let triggers = view_cfg.sse_trigger_events.clone();
                    let max_conns = view_cfg.max_connections;

                    let buffer_size = view_cfg.sse_event_buffer_size.unwrap_or(100);
                    let channel = ctx.sse_manager.register_with_buffer(
                        qualified_id.clone(),
                        max_conns,
                        tick_ms,
                        triggers.clone(),
                        buffer_size,
                    ).await;

                    // Subscribe trigger events to EventBus → push to SSE channel
                    for event_name in &triggers {
                        let ch = channel.clone();
                        let handler = Arc::new(SseTriggerHandler {
                            channel: ch,
                            view_id: qualified_id.clone(),
                        });
                        ctx.event_bus.subscribe(
                            event_name.clone(),
                            handler,
                            rivers_runtime::rivers_core::eventbus::HandlerPriority::Handle,
                        ).await;
                    }

                    // Spawn channel-level push loop for SSE views
                    if view_cfg.polling.is_some() || tick_ms > 0 {
                        let ch = channel.clone();
                        let vid = qualified_id.clone();

                        if let Some(ref polling) = view_cfg.polling {
                            // Real DataView polling with StorageEngine persistence
                            let executor: Arc<dyn crate::polling::PollDataViewExecutor> = Arc::new(
                                crate::polling::DataViewPollExecutor::new(ctx.dataview_executor.clone())
                            );
                            let storage = ctx.storage_engine.clone();
                            let strategy = Some(crate::polling::DiffStrategy::from_str_opt(
                                Some(polling.diff_strategy.as_str())
                            ));
                            let poll_tick_ms = polling.tick_interval_ms;

                            tokio::spawn(async move {
                                crate::sse::drive_sse_push_loop(
                                    ch, poll_tick_ms, vid,
                                    Some(executor), storage, strategy,
                                ).await;
                            });
                        } else {
                            // Heartbeat mode — no DataView polling
                            tokio::spawn(async move {
                                crate::sse::drive_sse_push_loop(ch, tick_ms, vid, None, None, None).await;
                            });
                        }
                    }

                    sse_count += 1;
                    tracing::info!(
                        view_id = %qualified_id,
                        tick_ms = tick_ms,
                        triggers = triggers.len(),
                        "SSE channel registered"
                    );
                }
                "Websocket" => {
                    let mode = crate::websocket::WebSocketMode::from_str_opt(
                        view_cfg.websocket_mode.as_deref(),
                    );
                    let max_conns = view_cfg.max_connections;

                    match mode {
                        crate::websocket::WebSocketMode::Broadcast => {
                            ctx.ws_manager.register_broadcast(
                                qualified_id.clone(),
                                max_conns,
                            ).await;
                        }
                        crate::websocket::WebSocketMode::Direct => {
                            ctx.ws_manager.register_direct(
                                qualified_id.clone(),
                                max_conns,
                            ).await;
                        }
                    }

                    ws_count += 1;
                    tracing::info!(
                        view_id = %qualified_id,
                        mode = ?mode,
                        "WebSocket route registered"
                    );
                }
                _ => {}
            }
        }
    }

    if sse_count > 0 || ws_count > 0 {
        tracing::info!(
            sse_channels = sse_count,
            ws_routes = ws_count,
            "streaming views wired"
        );
    }

    // Store bundle for services discovery
    ctx.loaded_bundle = Some(Arc::new(bundle));

    tracing::info!(
        path = %bundle_path,
        apps = app_count,
        views = view_count,
        dataviews = dv_count,
        "bundle loaded"
    );

    Ok(())
}

// ── Hot Reload: Rebuild views and DataViews ──────────────────────

/// Summary of a hot reload rebuild.
#[derive(Debug)]
pub struct ReloadSummary {
    pub apps: usize,
    pub views: usize,
    pub dataviews: usize,
}

/// Re-parse the bundle and rebuild DataView registry + ViewRouter.
///
/// Does NOT re-resolve LockBox credentials or re-create connection pools.
/// Existing `ds_params` and `DriverFactory` are reused. Only the DataView
/// registry, view router, and GraphQL schema (if enabled) are rebuilt.
///
/// This is the hot-reload-safe subset of `load_and_wire_bundle`.
pub async fn rebuild_views_and_dataviews(
    ctx: &AppContext,
    config: &ServerConfig,
    bundle_path: &str,
) -> Result<ReloadSummary, ServerError> {
    let path = std::path::Path::new(bundle_path);
    let bundle = rivers_runtime::load_bundle(path).map_err(|e| {
        ServerError::Config(format!("hot reload: bundle parse failed: {}", e))
    })?;

    // ── AT3.3 (B): Validate bundle on hot reload ──
    if let Err(errors) = rivers_runtime::validate_bundle(&bundle) {
        let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        tracing::warn!("hot reload: bundle validation failed: {}", msg);
        return Err(ServerError::Config(format!("hot reload: bundle validation failed: {}", msg)));
    }

    let mut registry = rivers_runtime::DataViewRegistry::new();
    let mut view_count = 0usize;

    for app in &bundle.apps {
        let entry_point = app.manifest.entry_point.as_deref()
            .unwrap_or(&app.manifest.app_name);

        view_count += app.config.api.views.len();

        for dv in app.config.data.dataviews.values() {
            let mut namespaced_dv = dv.clone();
            namespaced_dv.name = format!("{}:{}", entry_point, dv.name);
            namespaced_dv.datasource = format!("{}:{}", entry_point, dv.datasource);
            registry.register(namespaced_dv);
        }
    }

    let app_count = bundle.apps.len();
    let dv_count = registry.count();

    // Reuse existing factory and ds_params from the current executor
    let (factory, ds_params, cache) = {
        let guard = ctx.dataview_executor.read().await;
        match guard.as_ref() {
            Some(_exec) => {
                // We can't extract factory/ds_params/cache directly since they're private.
                // Instead, we'll build a new executor from scratch.
                // The factory and ds_params are stable across reloads.
                drop(guard);
                // Re-create factory with all drivers
                let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
                crate::server::register_all_drivers(&mut factory, &config.plugins.ignore);
                let factory = Arc::new(factory);

                // Build cache from StorageEngine if available
                let cache_policy = build_cache_policy_from_bundle(&bundle);
                let cache: Option<Arc<dyn rivers_runtime::tiered_cache::DataViewCache>> =
                    ctx.storage_engine.as_ref().map(|engine| {
                        let tiered = rivers_runtime::tiered_cache::TieredDataViewCache::new(
                            cache_policy,
                        )
                        .with_storage(engine.clone());
                        Arc::new(tiered) as Arc<dyn rivers_runtime::tiered_cache::DataViewCache>
                    });

                // Reuse existing ds_params by reading from the existing executor
                // Since we can't access private fields, we need to rebuild ds_params too.
                // This is acceptable — ds_params don't change on reload (new datasources require restart).
                let mut ds_params: HashMap<String, rivers_runtime::rivers_driver_sdk::ConnectionParams> = HashMap::new();
                for app in &bundle.apps {
                    let entry_point = app.manifest.entry_point.as_deref()
                        .unwrap_or(&app.manifest.app_name);
                    for ds in app.config.data.datasources.values() {
                        let mut params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
                            host: ds.host.clone().unwrap_or_default(),
                            port: ds.port.unwrap_or(0),
                            database: ds.database.clone().unwrap_or_default(),
                            username: ds.username.clone().unwrap_or_default(),
                            password: String::new(), // Passwords not re-resolved on reload
                            options: HashMap::new(),
                        };
                        params.options.insert("driver".into(), ds.driver.clone());
                        for (k, v) in &ds.extra {
                            params.options.insert(k.clone(), v.clone());
                        }
                        let namespaced_key = format!("{}:{}", entry_point, ds.name);
                        ds_params.insert(namespaced_key, params);
                    }
                }

                (factory, Arc::new(ds_params), cache)
            }
            None => {
                return Err(ServerError::Config(
                    "hot reload: no existing executor to rebuild from".into(),
                ));
            }
        }
    };

    // Build new executor
    let mut executor = DataViewExecutor::new(registry, factory, ds_params, cache);
    executor.set_event_bus(ctx.event_bus.clone());
    *ctx.dataview_executor.write().await = Some(executor);

    // Rebuild view router
    let router = view_engine::ViewRouter::from_bundle(&bundle, config.route_prefix.as_deref());
    *ctx.view_router.write().await = Some(router);

    // Rebuild GraphQL schema if enabled
    if config.graphql.enabled {
        let guard = ctx.dataview_executor.read().await;
        if let Some(ref exec) = *guard {
            let dv_names = exec.registry().names();
            let resolvers = crate::graphql::build_resolver_mappings_from_dataviews(&dv_names);
            drop(guard);

            // Scan views for CodeComponent mutations
            let mut mutation_mappings = Vec::new();
            for app in &bundle.apps {
                let ep = app.manifest.entry_point.as_deref()
                    .unwrap_or(&app.manifest.app_name);
                mutation_mappings.extend(
                    crate::graphql::build_mutation_mappings_from_views(&app.config.api.views, ep)
                );
            }

            // Scan views for subscription topics
            let mut subscription_mappings = Vec::new();
            for app in &bundle.apps {
                subscription_mappings.extend(
                    crate::graphql::build_subscription_mappings_from_views(&app.config.api.views)
                );
            }

            let gql_config = crate::graphql::GraphqlConfig::from(&config.graphql);
            match crate::graphql::build_schema_with_executor(
                &gql_config,
                &resolvers,
                ctx.dataview_executor.clone(),
                &mutation_mappings,
                ctx.pool.clone(),
                &subscription_mappings,
                ctx.event_bus.clone(),
            ) {
                Ok(schema) => {
                    *ctx.graphql_schema.write().await = Some(schema);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "hot reload: GraphQL schema rebuild failed");
                }
            }
        } else {
            drop(guard);
        }
    }

    tracing::info!(
        apps = app_count,
        views = view_count,
        dataviews = dv_count,
        "hot reload: views and dataviews rebuilt"
    );

    Ok(ReloadSummary {
        apps: app_count,
        views: view_count,
        dataviews: dv_count,
    })
}

/// Build a DataViewCachingPolicy from the aggregate of all DataView caching configs in a bundle.
///
/// Uses the most permissive values: L1/L2 enabled if ANY DataView enables them,
/// max entries = largest configured, TTL = longest configured.
fn build_cache_policy_from_bundle(
    bundle: &rivers_runtime::LoadedBundle,
) -> rivers_runtime::tiered_cache::DataViewCachingPolicy {
    let mut policy = rivers_runtime::tiered_cache::DataViewCachingPolicy::default();

    let mut has_any_caching = false;
    for app in &bundle.apps {
        for dv in app.config.data.dataviews.values() {
            if let Some(ref caching) = dv.caching {
                has_any_caching = true;
                if caching.ttl_seconds > policy.ttl_seconds {
                    policy.ttl_seconds = caching.ttl_seconds;
                }
                if caching.l1_enabled {
                    policy.l1_enabled = true;
                }
                if caching.l1_max_entries > policy.l1_max_entries {
                    policy.l1_max_entries = caching.l1_max_entries;
                }
                if caching.l2_enabled {
                    policy.l2_enabled = true;
                }
                if caching.l2_max_value_bytes > policy.l2_max_value_bytes {
                    policy.l2_max_value_bytes = caching.l2_max_value_bytes;
                }
            }
        }
    }

    if !has_any_caching {
        // No DataView has caching configured — use defaults (L1 only)
        policy.l1_enabled = true;
        policy.l2_enabled = false;
    }

    policy
}
