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

    // ── Gate 2, Layer 1: Structural TOML validation ──
    let structural_results = rivers_runtime::validate_structural(path);
    let structural_errors: Vec<_> = structural_results
        .iter()
        .filter(|r| r.status == rivers_runtime::ValidationStatus::Fail)
        .collect();
    if !structural_errors.is_empty() {
        let msg = structural_errors
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        tracing::error!(path = %bundle_path, "structural validation failed: {}", msg);
        return Err(ServerError::Config(format!(
            "structural validation failed: {}", msg
        )));
    }
    for r in &structural_results {
        if r.status == rivers_runtime::ValidationStatus::Warn {
            tracing::warn!(path = %bundle_path, "{}", r.message);
        }
    }

    // ── AT3.2 (A): Validate bundle before wiring ──
    if let Err(errors) = rivers_runtime::validate_bundle(&bundle) {
        let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        tracing::error!(path = %bundle_path, "bundle validation failed: {}", msg);
        return Err(ServerError::Config(format!("bundle validation failed: {}", msg)));
    }

    // ── Resolve module paths to absolute ──
    // CodeComponent handler modules are relative to their app directory.
    // Resolve them now so the V8/WASM engines can read them from any CWD.
    let mut bundle = bundle;
    for app in &mut bundle.apps {
        let app_dir = app.app_dir.clone();
        for view_cfg in app.config.api.views.values_mut() {
            resolve_handler_module(&app_dir, &mut view_cfg.handler);
            if let Some(ref mut os) = view_cfg.on_stream {
                let resolved = app_dir.join(&os.module);
                os.module = resolved.to_string_lossy().to_string();
            }
            if let Some(ref mut eh) = view_cfg.event_handlers {
                for h in eh.pre_process.iter_mut()
                    .chain(eh.handlers.iter_mut())
                    .chain(eh.post_process.iter_mut())
                {
                    let resolved = app_dir.join(&h.module);
                    h.module = resolved.to_string_lossy().to_string();
                }
            }
            if let Some(ref mut polling) = view_cfg.polling {
                if let Some(ref mut oc) = polling.on_change {
                    let resolved = app_dir.join(&oc.module);
                    oc.module = resolved.to_string_lossy().to_string();
                }
            }
        }
    }

    // ── Phase 2 (spec §2.6–2.7): compile every `.ts`/`.js` under every app's
    //    `libraries/` at load time, populate the process-global module cache,
    //    and install it atomically. Any compile failure aborts bundle load.
    let module_cache = crate::process_pool::module_cache::populate_module_cache(&bundle)
        .map_err(|e| ServerError::Config(format!("module cache population failed: {e}")))?;
    tracing::info!(
        modules = module_cache.len(),
        "bundle: module cache populated"
    );
    crate::process_pool::module_cache::install_module_cache(module_cache);
    // B3 / P1-8: arm production-strict cache enforcement now that the
    // validated bundle's modules are installed. Subsequent V8 dispatches that
    // miss the cache will hard-fail unless the operator opted into dev-mode
    // via `RIVERS_DEV_MODULE_CACHE=permissive`.
    crate::process_pool::module_cache::arm_production_strict();

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

        // Register app with per-app log router (no-op if router not configured)
        // Use entry_point (not app_name) — must match TASK_APP_NAME used by V8 callbacks.
        if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
            let app_name = entry_point;
            if let Err(e) = router.register(app_name) {
                tracing::warn!(app = %app_name, error = %e, "failed to create app log file");
            } else {
                tracing::info!(app = %app_name, "app log file created");
            }
        }

        // Register dataviews — namespaced by entry_point to prevent collisions
        for dv in app.config.data.dataviews.values() {
            let mut namespaced_dv = dv.clone();
            namespaced_dv.name = format!("{}:{}", entry_point, dv.name);
            namespaced_dv.datasource = format!("{}:{}", entry_point, dv.datasource);
            registry.register(namespaced_dv);
        }

        // ── Build circuit breaker registry from DataView config (circuit-breaker-spec §3) ──
        let app_id = &app.manifest.app_id;
        for (dv_name, dv_config) in &app.config.data.dataviews {
            if let Some(ref breaker_id) = dv_config.circuit_breaker_id {
                ctx.circuit_breaker_registry
                    .register(app_id, breaker_id.clone(), dv_name.clone())
                    .await;
            }
        }

        // Restore persisted breaker state from StorageEngine (circuit-breaker-spec §3, REG-3)
        if let Some(ref storage) = ctx.storage_engine {
            for entry in ctx.circuit_breaker_registry.list_for_app(app_id).await {
                let key = format!("breaker:{}:{}", app_id, entry.breaker_id);
                match storage.get("rivers", &key).await {
                    Ok(Some(bytes)) => {
                        if let Ok(state_str) = String::from_utf8(bytes) {
                            if state_str.trim() == "open" {
                                ctx.circuit_breaker_registry
                                    .set_state(app_id, &entry.breaker_id, crate::circuit_breaker::BreakerState::Open)
                                    .await;
                                tracing::info!(breaker = %entry.breaker_id, "restored breaker state: OPEN");
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            breaker = %entry.breaker_id,
                            error = %e,
                            "failed to read persisted breaker state, starting CLOSED"
                        );
                    }
                }
            }
        }

        // Log breaker summary for this app
        for entry in ctx.circuit_breaker_registry.list_for_app(app_id).await {
            tracing::info!(
                breaker = %entry.breaker_id,
                state = ?entry.state,
                dataviews = entry.dataviews.len(),
                "breaker loaded"
            );
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

    // Warn if [plugins] dir points to a real directory — cdylib plugins are disabled
    if !config.plugins.dir.is_empty() && std::path::Path::new(&config.plugins.dir).is_dir() {
        tracing::warn!(
            dir = %config.plugins.dir,
            "[plugins] dir is deprecated — cdylib driver plugins disabled in this version. \
             All drivers are compiled statically. Plugin ABI v2 will re-enable dynamic loading."
        );
    }

    // Build DriverFactory with all drivers (built-in + static-plugins)
    let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
    register_all_drivers(&mut factory, &config.plugins.ignore);

    let app_count = bundle.apps.len();
    let dv_count = registry.count();

    // ── AT3.4 (D): Validate driver names against registered drivers ──
    let failed_app_names: std::collections::HashSet<String>;
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

        // Group driver errors by app name and block affected apps (503 at request time)
        failed_app_names = if !driver_errors.is_empty() {
            // Parse "[app_name] datasource ..." and "unknown driver 'drv'" from each error
            let mut by_app: HashMap<String, Vec<String>> = HashMap::new();
            for err in &driver_errors {
                let s = err.to_string();
                // Error format: "config error: [app_name] datasource ..."
                // Find the bracketed app name anywhere in the string.
                let app_name = s.find('[')
                    .and_then(|start| s[start+1..].find(']').map(|end| &s[start+1..start+1+end]))
                    .unwrap_or("unknown")
                    .to_string();
                by_app.entry(app_name).or_default().push(s);
            }

            let bundle_name = &bundle.manifest.bundle_name;
            let route_prefix = config.route_prefix.as_deref();
            let mut failed_prefixes: HashMap<String, String> = HashMap::new();
            let mut failed_names: std::collections::HashSet<String> = std::collections::HashSet::new();

            for (app_name, errors) in &by_app {
                // Collect missing driver names for the message
                let missing_drivers: Vec<String> = errors.iter().filter_map(|s| {
                    s.find("unknown driver '").map(|start| {
                        let rest = &s[start + 16..];
                        rest.split('\'').next().unwrap_or("?").to_string()
                    })
                }).collect();
                let driver_list = missing_drivers.join(", ");

                let error_msg = format!(
                    "app '{}' is unavailable — missing driver(s): {}",
                    app_name, driver_list
                );

                tracing::error!(app_name = %app_name, drivers = %driver_list, "app blocked — missing drivers");

                // Write structured JSON to per-app log
                if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
                    let json = serde_json::json!({
                        "event": "app_blocked",
                        "app_name": app_name,
                        "missing_drivers": missing_drivers,
                        "errors": errors,
                    });
                    router.write(app_name, &json.to_string());
                }

                // Build path prefix for 503 matching
                let entry_point = bundle.apps.iter()
                    .find(|a| &a.manifest.app_name == app_name)
                    .and_then(|a| a.manifest.entry_point.as_deref())
                    .unwrap_or(app_name.as_str());

                let prefix = match route_prefix.filter(|p| !p.is_empty()) {
                    Some(pfx) => format!("/{}/{}/{}", pfx.trim_matches('/'), bundle_name, entry_point),
                    None => format!("/{}/{}", bundle_name, entry_point),
                };

                failed_prefixes.insert(prefix, error_msg);
                failed_names.insert(app_name.clone());
            }

            // Store in ctx.failed_apps
            if let Ok(mut map) = ctx.failed_apps.write() {
                map.extend(failed_prefixes);
            }

            failed_names
        } else {
            std::collections::HashSet::new()
        };
    }

    let factory = Arc::new(factory);

    // ── D2: Register one ConnectionPool per datasource ────────────────
    //
    // Pools key by the same namespaced datasource id used everywhere else
    // in the loader (`entry_point:ds_name`). Pool config uses defaults
    // (max_size=10, idle=300s, lifetime=30min) — per-app pool configuration
    // is a future feature. Brokers (which lack a registered DatabaseDriver)
    // are skipped silently — the executor falls back to the legacy
    // direct-connect path for those datasources.
    for (ds_id, ds_params) in ds_params.iter() {
        let driver_name = ds_params
            .options
            .get("driver")
            .map(|s| s.as_str())
            .unwrap_or(ds_id.as_str());
        let Some(driver) = factory.get_driver(driver_name) else {
            tracing::debug!(
                datasource = %ds_id,
                driver = %driver_name,
                "skipping pool registration — no DatabaseDriver registered (broker or unknown)"
            );
            continue;
        };
        let pool = Arc::new(crate::pool::ConnectionPool::new(
            ds_id.clone(),
            crate::pool::PoolConfig::default(),
            driver.clone(),
            ds_params.clone(),
            ctx.event_bus.clone(),
        ));
        if let Err(e) = ctx.pool_manager.add_pool(pool).await {
            tracing::warn!(
                datasource = %ds_id,
                error = %e,
                "pool registration failed (continuing with degraded direct-connect path)"
            );
        } else {
            tracing::info!(
                datasource = %ds_id,
                driver = %driver_name,
                "pool registered"
            );
        }
    }

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

    // ── Schema introspection (schema-introspection-spec §4) ──────────
    // For each SQL datasource with `introspect = true`, acquire a connection
    // and execute a LIMIT 0 wrapper query per DataView to validate query
    // syntax at startup rather than at first request.
    {
        let mut all_mismatches: Vec<crate::schema_introspection::SchemaMismatch> = Vec::new();

        for app in &bundle.apps {
            let entry_point = app
                .manifest
                .entry_point
                .as_deref()
                .unwrap_or(&app.manifest.app_name);

            for (ds_name, ds_config) in &app.config.data.datasources {
                // Skip if introspection disabled on this datasource
                if !ds_config.introspect {
                    tracing::debug!(
                        datasource = %ds_name,
                        app = %entry_point,
                        "schema introspection skipped (introspect = false)"
                    );
                    continue;
                }

                // Check if driver supports introspection
                let driver = match factory.get_driver(&ds_config.driver) {
                    Some(d) => d,
                    None => continue,
                };
                if !driver.supports_introspection() {
                    tracing::debug!(
                        datasource = %ds_name,
                        driver = %ds_config.driver,
                        "schema introspection skipped — driver does not support introspection"
                    );
                    continue;
                }

                // Retrieve the already-built ConnectionParams for this datasource
                let namespaced_ds = format!("{}:{}", entry_point, ds_name);
                let params = match ds_params.get(&namespaced_ds) {
                    Some(p) => p,
                    None => {
                        tracing::warn!(
                            datasource = %ds_name,
                            app = %entry_point,
                            "schema introspection skipped — no connection params found"
                        );
                        continue;
                    }
                };

                // Try to connect for introspection
                let mut conn = match driver.connect(params).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            datasource = %ds_name,
                            app = %entry_point,
                            error = %e,
                            "schema introspection skipped — cannot connect"
                        );
                        continue;
                    }
                };

                // Introspect each DataView on this datasource
                for (dv_name, dv_config) in &app.config.data.dataviews {
                    if dv_config.datasource != *ds_name {
                        continue;
                    }

                    // Use the GET query for introspection (primary read query)
                    let query_str = match dv_config.query_for_method("GET") {
                        Some(q) if !q.is_empty() => q.to_string(),
                        _ => continue,
                    };

                    // Wrap in LIMIT 0 to get column metadata without returning rows
                    let limit_query = rivers_runtime::rivers_driver_sdk::Query {
                        operation: "select".to_string(),
                        target: String::new(),
                        statement: format!(
                            "SELECT * FROM ({}) AS _introspect LIMIT 0",
                            query_str
                        ),
                        parameters: std::collections::HashMap::new(),
                    };

                    match conn.execute(&limit_query).await {
                        Ok(result) => {
                            if let Some(ref columns) = result.column_names {
                                tracing::debug!(
                                    dataview = %dv_name,
                                    app = %entry_point,
                                    columns = ?columns,
                                    "introspected {} column(s)",
                                    columns.len()
                                );

                                // Load schema and compare field names against actual columns
                                let schema_ref = dv_config.get_schema.as_deref()
                                    .or(dv_config.return_schema.as_deref());
                                if let Some(schema_path) = schema_ref {
                                    let full_path = app.app_dir.join(schema_path);
                                    match std::fs::read_to_string(&full_path) {
                                        Ok(content) => {
                                            match rivers_runtime::schema::parse_schema(&content, schema_path) {
                                                Ok(schema) => {
                                                    let field_names: Vec<String> = schema.fields
                                                        .iter()
                                                        .map(|f| f.name.clone())
                                                        .collect();
                                                    let mismatches = crate::schema_introspection::check_fields_against_columns(
                                                        &format!("{}:{}", entry_point, dv_name),
                                                        &field_names,
                                                        columns,
                                                    );
                                                    all_mismatches.extend(mismatches);
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        dataview = %dv_name,
                                                        schema = %schema_path,
                                                        error = %e,
                                                        "schema parse failed, skipping field comparison"
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::debug!(
                                                dataview = %dv_name,
                                                schema = %schema_path,
                                                error = %e,
                                                "schema file not found, skipping field comparison"
                                            );
                                        }
                                    }
                                }
                            } else {
                                tracing::debug!(
                                    dataview = %dv_name,
                                    app = %entry_point,
                                    "introspection query succeeded (no column metadata returned)"
                                );
                            }
                        }
                        Err(e) => {
                            all_mismatches.push(crate::schema_introspection::SchemaMismatch {
                                dataview_name: format!("{}:{}", entry_point, dv_name),
                                field_name: "(query)".to_string(),
                                available_columns: vec![],
                                suggestion: Some(format!("query error at startup: {}", e)),
                            });
                        }
                    }
                }
            }
        }

        if !all_mismatches.is_empty() {
            let msg = crate::schema_introspection::format_introspection_errors(&all_mismatches);
            tracing::error!("{}", msg);
            return Err(ServerError::Config(msg));
        }
    }

    let ds_params = Arc::new(ds_params);
    let mut executor = DataViewExecutor::new(registry, factory.clone(), ds_params.clone(), cache);
    executor.set_event_bus(ctx.event_bus.clone());
    // D2: route DataView execution through the per-datasource pool.
    // The acquirer is the same `PoolManager` we just registered pools on.
    executor.set_acquirer(ctx.pool_manager.clone() as Arc<dyn rivers_runtime::ConnectionAcquirer>);
    let executor = Arc::new(executor);
    *ctx.dataview_executor.write().await = Some(executor.clone());
    ctx.driver_factory = Some(factory.clone());

    // ── Wire HOST_CONTEXT before init handlers ──────────────────────
    //
    // Init handlers need HOST_CONTEXT for ctx.ddl() host callbacks.
    // OnceLock ensures this is safe to call early — subsequent calls
    // in lifecycle.rs are harmless no-ops.
    crate::engine_loader::set_host_context(
        ctx.dataview_executor.clone(),
        ctx.storage_engine.clone(),
        ctx.driver_factory.clone(),
    );

    // Wire DDL whitelist before init handlers so Gate 3 is active
    let ddl_warnings = rivers_runtime::rivers_core_config::config::security::validate_ddl_whitelist(
        &config.security.ddl_whitelist,
    );
    for w in &ddl_warnings {
        tracing::warn!(target: "rivers.security", "{}", w);
    }
    crate::engine_loader::set_ddl_whitelist(config.security.ddl_whitelist.clone());

    // Build entry_point → manifest app_id (UUID) map for DDL whitelist resolution
    let mut app_id_map = std::collections::HashMap::new();
    for app in &bundle.apps {
        let entry_point = app.manifest.entry_point.as_deref()
            .unwrap_or(&app.manifest.app_name)
            .to_string();
        app_id_map.insert(entry_point, app.manifest.app_id.clone());
    }
    crate::engine_loader::set_app_id_map(app_id_map);

    // ── Phase 1.5: Run application init handlers (DDL security spec) ──
    for app in &bundle.apps {
        if failed_app_names.contains(&app.manifest.app_name) {
            tracing::info!(app_name = %app.manifest.app_name, "skipping init handler — app blocked due to missing drivers");
            continue;
        }
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
        &failed_app_names,
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

        // AM1.2 (P0-1 / A2.2): If any view has auth != "none", a session
        // manager must be configured. session_manager initializes only when
        // storage_engine is present, so this check also catches the missing-
        // storage case while making the actual security boundary explicit.
        if let Err(e) = check_protected_views_have_session(
            &all_views,
            ctx.session_manager.is_some(),
            ctx.storage_engine.is_some(),
        ) {
            return Err(ServerError::Config(e));
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

    #[cfg(feature = "metrics")]
    crate::server::metrics::set_loaded_apps(app_count);

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
    executor: &Arc<DataViewExecutor>,
    ddl_whitelist: &[String],
    timeout_s: u64,
) -> Result<(), String> {
    use crate::process_pool::Entrypoint;

    let entry_point = app
        .manifest
        .entry_point
        .as_deref()
        .unwrap_or(&app.manifest.app_name);

    // Validate module path containment — prevent path traversal
    let module_path = rivers_runtime::bundle::validate_module_path(
        &app.app_dir,
        &init_config.module,
    ).map_err(|e| format!("init handler: {e}"))?;

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

    // Wire shared capabilities (storage, driver_factory, lockbox, keystore)
    // C1.2: ApplicationInit is the ONLY task_kind that may call ctx.ddl().
    let builder = crate::task_enrichment::enrich(
        builder,
        entry_point,
        rivers_runtime::process_pool::TaskKind::ApplicationInit,
    );

    // Override dataview_executor with the exact instance passed in — the global
    // shared state may not be synced yet during initial bundle load.
    let builder = builder.dataview_executor(Arc::clone(executor));

    // DDL whitelist (Gate 3) is enforced at the host callback level via DDL_WHITELIST
    // OnceLock — set at startup by set_ddl_whitelist(). Log if whitelist is active.
    if !ddl_whitelist.is_empty() {
        tracing::debug!(
            app_id = %app.manifest.app_id,
            whitelist_entries = ddl_whitelist.len(),
            "init handler: DDL whitelist active (Gate 3)"
        );
    }

    let task_ctx = builder.build().map_err(|e| format!("failed to build init task context: {e}"))?;

    let timeout = std::time::Duration::from_secs(timeout_s);
    match tokio::time::timeout(timeout, pool.dispatch("default", task_ctx)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(format!("init handler execution failed: {e}")),
        Err(_) => Err(format!("init handler timed out after {}s", timeout_s)),
    }
}

/// Resolve a `HandlerConfig::Codecomponent` module path to absolute.
fn resolve_handler_module(
    app_dir: &std::path::Path,
    handler: &mut rivers_runtime::view::HandlerConfig,
) {
    if let rivers_runtime::view::HandlerConfig::Codecomponent { module, .. } = handler {
        let resolved = app_dir.join(&*module);
        *module = resolved.to_string_lossy().to_string();
    }
}

/// **P0-1 / A2.2**: validate that a bundle declaring any non-public view
/// has a session manager (and therefore storage engine) configured.
///
/// Returns `Ok(())` if the bundle is safe to load; `Err(message)` otherwise.
/// Error message names the offending view and the missing dependency.
///
/// Extracted as a free function so the rule is unit-testable without staging
/// a disk bundle and full lifecycle.
fn check_protected_views_have_session(
    views: &HashMap<String, rivers_runtime::view::ApiViewConfig>,
    has_session_manager: bool,
    has_storage_engine: bool,
) -> Result<(), String> {
    let protected_view_id = views
        .iter()
        .find(|(_, v)| !crate::guard::is_public_view(v))
        .map(|(id, _)| id.clone());

    let Some(view_id) = protected_view_id else {
        return Ok(());
    };

    if has_session_manager {
        return Ok(());
    }

    let reason = if !has_storage_engine {
        "no storage engine configured"
    } else {
        "session manager not initialized"
    };
    Err(format!(
        "protected view '{view_id}' requires session management ({reason}); \
         either set auth=\"none\" or configure [storage_engine] and [security.session]"
    ))
}

#[cfg(test)]
mod check_protected_views_tests {
    use super::*;
    use rivers_runtime::view::ApiViewConfig;

    fn view_json(value: serde_json::Value) -> ApiViewConfig {
        serde_json::from_value(value).expect("valid ApiViewConfig")
    }

    fn protected() -> ApiViewConfig {
        view_json(serde_json::json!({
            "view_type": "Rest",
            "path": "/api/protected",
            "method": "GET",
            "handler": { "type": "dataview", "dataview": "noop" }
        }))
    }

    fn public_auth_none() -> ApiViewConfig {
        view_json(serde_json::json!({
            "view_type": "Rest",
            "path": "/api/public",
            "method": "GET",
            "auth": "none",
            "handler": { "type": "dataview", "dataview": "noop" }
        }))
    }

    #[test]
    fn rejects_protected_view_when_session_manager_missing() {
        let mut views = HashMap::new();
        views.insert("protected_view".to_string(), protected());
        let err = check_protected_views_have_session(&views, false, false)
            .expect_err("must reject");
        assert!(err.contains("protected_view"), "names offending view: {err}");
        assert!(err.contains("no storage engine"), "blames missing storage: {err}");
    }

    #[test]
    fn rejects_with_storage_present_but_session_missing() {
        let mut views = HashMap::new();
        views.insert("v".to_string(), protected());
        let err = check_protected_views_have_session(&views, false, true)
            .expect_err("must reject");
        assert!(err.contains("session manager not initialized"), "{err}");
    }

    #[test]
    fn allows_protected_view_when_session_manager_present() {
        let mut views = HashMap::new();
        views.insert("v".to_string(), protected());
        check_protected_views_have_session(&views, true, true)
            .expect("ok when session manager present");
    }

    #[test]
    fn allows_public_views_with_no_session_manager() {
        let mut views = HashMap::new();
        views.insert("p".to_string(), public_auth_none());
        check_protected_views_have_session(&views, false, false)
            .expect("auth=none requires no session");
    }

    #[test]
    fn allows_empty_view_set() {
        let views = HashMap::new();
        check_protected_views_have_session(&views, false, false)
            .expect("nothing to protect");
    }

    #[test]
    fn rejects_when_mixed_views_include_one_protected() {
        let mut views = HashMap::new();
        views.insert("p".to_string(), public_auth_none());
        views.insert("admin".to_string(), protected());
        let err = check_protected_views_have_session(&views, false, false)
            .expect_err("one protected view is enough");
        // The first protected view found is named (HashMap order isn't stable
        // but only "admin" is protected, so it must be that one).
        assert!(err.contains("admin"), "{err}");
    }
}
