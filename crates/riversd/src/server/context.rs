//! `LogController` and `AppContext` — shared application state.

use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::rivers_core::ServerConfig;
use rivers_runtime::rivers_core::EventBus;
use rivers_runtime::DataViewExecutor;

use crate::deployment::DeploymentManager;
use crate::health::UptimeTracker;
use crate::hot_reload::HotReloadState;
use crate::process_pool::ProcessPoolManager;
use crate::shutdown::ShutdownCoordinator;
use crate::sse::SseRouteManager;
use crate::websocket::WebSocketRouteManager;

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
    /// Server configuration.
    pub config: ServerConfig,
    /// Graceful shutdown coordinator.
    pub shutdown: Arc<ShutdownCoordinator>,
    /// Server uptime tracker.
    pub uptime: Arc<UptimeTracker>,
    /// ProcessPool for CodeComponent execution.
    pub pool: Arc<ProcessPoolManager>,
    /// View router built from deployed app configs.
    pub view_router: Arc<tokio::sync::RwLock<Option<crate::view_engine::ViewRouter>>>,
    /// DataView executor for resolving DataView queries.
    pub dataview_executor: Arc<tokio::sync::RwLock<Option<Arc<DataViewExecutor>>>>,
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
    /// Apps that failed to load — keyed by entry_point path prefix (e.g., "/canary-fleet/nosql"),
    /// value is the human-readable error message for the 503 response.
    pub failed_apps: Arc<std::sync::RwLock<HashMap<String, String>>>,
    /// Circuit breaker registry — app-level manual DataView traffic control.
    pub circuit_breaker_registry: Arc<crate::circuit_breaker::BreakerRegistry>,
}

impl AppContext {
    /// Create a new application context with default subsystem state.
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
            failed_apps: Arc::new(std::sync::RwLock::new(HashMap::new())),
            circuit_breaker_registry: Arc::new(crate::circuit_breaker::BreakerRegistry::new()),
        }
    }
}
