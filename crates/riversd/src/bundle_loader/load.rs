//! Bundle loading, LockBox resolution, driver setup, and DataView wiring.
//!
//! Contains the main `load_and_wire_bundle` entry point which orchestrates
//! the full bundle loading pipeline, delegating streaming/event wiring
//! to [`super::wire::wire_streaming_and_events`].

use std::collections::HashMap;
use std::sync::Arc;

use zeroize::Zeroize;

use rivers_runtime::rivers_core::ServerConfig;
use rivers_runtime::DataViewExecutor;

use rivers_runtime::bundle::InitHandlerConfig;

use crate::process_pool::ProcessPoolManager;
use crate::server::{AppContext, ServerError, register_all_drivers};
use crate::view_engine;
use super::reload::build_cache_policy_from_bundle;

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

    // ── Keystore: resolve master keys and unlock keystores ──
    let mut ks_resolver = crate::keystore::KeystoreResolver::new();
    for app in &bundle.apps {
        let entry_point = app
            .manifest
            .entry_point
            .as_deref()
            .unwrap_or(&app.manifest.app_name);

        for ks_decl in &app.resources.keystores {
            // 1. Check that the keystore is configured in app.toml
            let ks_config = app.config.data.keystore.get(&ks_decl.name)
                .ok_or_else(|| ServerError::Config(format!(
                    "keystore '{}' declared in resources.toml but not configured in app.toml [data.keystore.{}]",
                    ks_decl.name, ks_decl.name,
                )))?;

            // 2. Resolve the keystore file path (relative to app dir)
            let ks_path = app.app_dir.join(&ks_config.path);
            if !ks_path.exists() {
                if ks_decl.required {
                    return Err(ServerError::Config(format!(
                        "keystore '{}' file not found: {}",
                        ks_decl.name, ks_path.display(),
                    )));
                } else {
                    tracing::warn!(keystore = %ks_decl.name, path = %ks_path.display(), "keystore: file not found (optional, skipping)");
                    continue;
                }
            }

            // 3. Resolve master key from LockBox
            let lb_config = config.lockbox.as_ref().ok_or_else(|| {
                ServerError::Config(format!(
                    "keystore '{}' requires lockbox alias '{}' but [lockbox] is not configured",
                    ks_decl.name, ks_decl.lockbox,
                ))
            })?;

            let lb_resolver = ctx.lockbox_resolver.as_ref().ok_or_else(|| {
                ServerError::Config(format!(
                    "keystore '{}' requires lockbox but no lockbox resolver available",
                    ks_decl.name,
                ))
            })?;

            let metadata = lb_resolver.resolve(&ks_decl.lockbox)
                .ok_or_else(|| ServerError::Config(format!(
                    "keystore '{}': lockbox alias '{}' not found",
                    ks_decl.name, ks_decl.lockbox,
                )))?;

            let keystore_path = std::path::Path::new(
                lb_config.path.as_deref().unwrap_or(""),
            );
            let identity_str = rivers_runtime::rivers_core::lockbox::resolve_key_source(lb_config)
                .map_err(|e| ServerError::Config(format!("lockbox key: {e}")))?;
            let mut resolved = rivers_runtime::rivers_core::lockbox::fetch_secret_value(
                metadata,
                keystore_path,
                identity_str.trim(),
            ).map_err(|e| ServerError::Config(format!(
                "keystore '{}': failed to fetch master key from lockbox: {e}",
                ks_decl.name,
            )))?;

            // The resolved value IS the Age identity string for the keystore
            let master_key = resolved.value.clone();
            resolved.value.zeroize();

            // 4. Load and decrypt the keystore
            let keystore = rivers_keystore_engine::AppKeystore::load(&ks_path, &master_key)
                .map_err(|e| ServerError::Config(format!(
                    "keystore '{}': failed to unlock: {e}",
                    ks_decl.name,
                )))?;

            let scoped_name = format!("{}:{}", entry_point, ks_decl.name);
            tracing::info!(keystore = %ks_decl.name, app = %entry_point, keys = keystore.keys.len(), "keystore: unlocked");
            ks_resolver.insert(scoped_name, keystore);
        }
    }
    if !ks_resolver.is_empty() {
        ctx.keystore_resolver = Some(Arc::new(ks_resolver));
    }

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

    // Build DataView cache — L1 always active, L2 only when StorageEngine available.
    let cache_policy = build_cache_policy_from_bundle(&bundle);
    let mut tiered = rivers_runtime::tiered_cache::TieredDataViewCache::new(cache_policy.clone());
    if let Some(ref engine) = ctx.storage_engine {
        tiered = tiered.with_storage(engine.clone());
    } else if cache_policy.l2_enabled {
        tracing::warn!("DataView cache: L2 enabled in config but no StorageEngine available — L2 disabled");
    }
    let cache: Arc<dyn rivers_runtime::tiered_cache::DataViewCache> = Arc::new(tiered);
    if cache_policy.l2_enabled && ctx.storage_engine.is_some() {
        tracing::info!("DataView cache: L1 + L2 enabled (L1 max: {} MB)", cache_policy.l1_max_bytes / (1024 * 1024));
    } else if cache_policy.l1_enabled {
        tracing::info!("DataView cache: L1 enabled (max: {} MB)", cache_policy.l1_max_bytes / (1024 * 1024));
    }

    let ds_params = Arc::new(ds_params);
    let mut executor = DataViewExecutor::new(registry, factory.clone(), ds_params.clone(), cache);
    executor.set_event_bus(ctx.event_bus.clone());
    let executor = Arc::new(executor);
    *ctx.dataview_executor.write().await = Some(executor.clone());
    ctx.driver_factory = Some(factory.clone());

    // ── Phase 1.5: Run application init handlers (DDL security spec) ──
    for app in &bundle.apps {
        if let Some(ref init_config) = app.manifest.init {
            let app_id = &app.manifest.app_id;
            let app_name = &app.manifest.app_name;

            tracing::info!(
                app_id = %app_id,
                app_name = %app_name,
                module = %init_config.module,
                entrypoint = %init_config.entrypoint,
                "init handler started"
            );

            let start = std::time::Instant::now();

            // Dispatch init handler via ProcessPool
            let init_result = dispatch_init_handler(
                &ctx.pool,
                app,
                init_config,
                &executor,
                &config.security.ddl_whitelist,
                config.base.init_timeout_s,
            ).await;

            let duration_ms = start.elapsed().as_millis() as u64;

            match init_result {
                Ok(()) => {
                    tracing::info!(
                        app_id = %app_id,
                        app_name = %app_name,
                        duration_ms = duration_ms,
                        "init handler completed"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        app_id = %app_id,
                        app_name = %app_name,
                        error = %e,
                        duration_ms = duration_ms,
                        "init handler failed — app entering FAILED state"
                    );
                    return Err(ServerError::Config(format!(
                        "init handler failed for app '{}': {}",
                        app_name, e
                    )));
                }
            }
        }
    }

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

    // ── Phase 2: Wire streaming and event handlers ──
    super::wire::wire_streaming_and_events(
        ctx, &bundle, &factory, &ds_params, shutdown_rx,
    ).await?;

    // Store bundle for services discovery
    ctx.loaded_bundle = Some(Arc::new(bundle));

    tracing::info!(
        path = %bundle_path,
        apps = app_count,
        views = view_count,
        dataviews = dv_count,
        "bundle loaded"
    );

    crate::task_enrichment::sync_from_app_context(ctx);

    Ok(())
}

/// Dispatch an application init handler via the ProcessPool.
///
/// The init handler runs in ApplicationInit context with access to
/// `ctx.ddl()`, `ctx.admin()`, and `ctx.query()`. DDL operations
/// are gated by the whitelist (Gate 3).
async fn dispatch_init_handler(
    pool: &ProcessPoolManager,
    app: &rivers_runtime::LoadedApp,
    init_config: &InitHandlerConfig,
    _executor: &Arc<DataViewExecutor>,
    _ddl_whitelist: &[String],
    timeout_s: u64,
) -> Result<(), String> {
    use crate::process_pool::Entrypoint;

    let entry_point = app
        .manifest
        .entry_point
        .as_deref()
        .unwrap_or(&app.manifest.app_name);

    let module_path = app.app_dir
        .join("libraries")
        .join(&init_config.module);

    if !module_path.exists() {
        return Err(format!(
            "init handler module not found: {}",
            module_path.display()
        ));
    }

    let entrypoint = Entrypoint {
        module: module_path.to_string_lossy().to_string(),
        function: init_config.entrypoint.clone(),
        language: "javascript".to_string(),
    };

    let args = serde_json::json!({
        "context": "ApplicationInit",
        "app_id": app.manifest.app_id,
        "app_name": app.manifest.app_name,
    });

    let builder = rivers_runtime::process_pool::TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .trace_id(format!("init-{}", entry_point));

    let task_ctx = builder.build().map_err(|e| format!("failed to build init task context: {e}"))?;

    let _ = timeout_s; // TODO: enforce timeout via tokio::time::timeout

    pool.dispatch("default", task_ctx)
        .await
        .map(|_| ())
        .map_err(|e| format!("init handler execution failed: {e}"))
}
