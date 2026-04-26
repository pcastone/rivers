//! Host context — subsystem references for host callbacks, set once after server init.

use std::sync::{Arc, OnceLock};

use rivers_engine_sdk::HostCallbacks;

use super::host_callbacks;

// ── Host Context (OnceLock subsystem references) ────────────────

/// Subsystem references for host callbacks. Set once after server init.
pub(super) struct HostContext {
    pub(super) dataview_executor: Arc<tokio::sync::RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>>,
    pub(super) storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    pub(super) driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
    pub(super) http_client: reqwest::Client,
    pub(super) rt_handle: tokio::runtime::Handle,
}

pub(super) static HOST_CONTEXT: OnceLock<HostContext> = OnceLock::new();

/// Application keystore for dynamic engine callbacks (App Keystore feature).
/// Separate OnceLock because keystore resolution happens per-app and may
/// occur after the main host context is wired.
pub(super) static HOST_KEYSTORE: OnceLock<Arc<rivers_keystore_engine::AppKeystore>> = OnceLock::new();

/// DDL whitelist — authorizes specific database+app pairs for DDL execution.
/// Set once during server startup from `config.security.ddl_whitelist`.
pub(super) static DDL_WHITELIST: OnceLock<Vec<String>> = OnceLock::new();

/// Maps entry_point names to manifest app_id UUIDs.
/// Used by DDL whitelist check — the ProcessPool uses entry_point as app_id,
/// but the whitelist format is `{database}@{appId}` with the manifest UUID.
pub(super) static APP_ID_MAP: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();

/// Wire host subsystem references so callbacks can reach DataViewExecutor,
/// StorageEngine, DriverFactory, and HTTP client. Called once during server
/// startup after all subsystems are initialized.
pub fn set_host_context(
    dataview_executor: Arc<tokio::sync::RwLock<Option<Arc<rivers_runtime::DataViewExecutor>>>>,
    storage_engine: Option<Arc<dyn rivers_runtime::rivers_core::storage::StorageEngine>>,
    driver_factory: Option<Arc<rivers_runtime::rivers_core::DriverFactory>>,
) {
    let _ = HOST_CONTEXT.set(HostContext {
        dataview_executor,
        storage_engine,
        driver_factory,
        http_client: reqwest::Client::new(),
        rt_handle: tokio::runtime::Handle::current(),
    });
}

/// Set the application keystore for dynamic engine callbacks.
/// Called after `set_host_context` when an app has [[keystores]] declared.
pub fn set_host_keystore(keystore: Arc<rivers_keystore_engine::AppKeystore>) {
    let _ = HOST_KEYSTORE.set(keystore);
}

/// Set the DDL whitelist for host callback gating.
/// Called once during server startup alongside `set_host_context`.
pub fn set_ddl_whitelist(whitelist: Vec<String>) {
    let _ = DDL_WHITELIST.set(whitelist);
}

/// Set the entry_point → manifest app_id (UUID) mapping.
/// Called once during bundle loading so DDL whitelist checks can
/// resolve the ProcessPool's entry_point-based app_id to the UUID
/// used in whitelist entries.
pub fn set_app_id_map(map: std::collections::HashMap<String, String>) {
    let _ = APP_ID_MAP.set(map);
}

/// Read the configured DDL whitelist, if one was set during startup.
///
/// Returns `None` when `set_ddl_whitelist` has not been called (e.g. tests
/// that don't wire one). Returns `Some(vec)` otherwise — the vec may be
/// empty when the operator configured no entries.
///
/// Mirrors the read pattern in `host_callbacks::host_ddl_execute` so the V8
/// in-process callback (`ctx.ddl()`) and the dynamic-engine callback share
/// a single source of whitelist state — there must not be two stores.
pub fn ddl_whitelist() -> Option<Vec<String>> {
    DDL_WHITELIST.get().cloned()
}

/// Resolve a ProcessPool entry_point name to the manifest app_id (UUID).
///
/// The ProcessPool dispatches with entry_point as `app_id`, but the DDL
/// whitelist is keyed by the manifest UUID (`database@uuid`). When no
/// mapping is configured, callers should fall back to the entry_point
/// itself — same behavior as `host_callbacks::host_ddl_execute`.
pub fn app_id_for_entry_point(entry_point: &str) -> Option<String> {
    APP_ID_MAP.get().and_then(|m| m.get(entry_point).cloned())
}

// ── Host Callback Implementations ───────────────────────────────
//
// NOTE: The callbacks below return JSON over FFI boundaries using `{"error": ...}`
// format. This is an FFI protocol contract with cdylib engine plugins (V8, WASM).
// Do NOT replace with ErrorResponse — changing the shape would break dynamic
// engine plugins that parse these responses.

/// Build the `HostCallbacks` table with all callback functions wired.
pub fn build_host_callbacks() -> HostCallbacks {
    HostCallbacks {
        dataview_execute: Some(host_callbacks::host_dataview_execute),
        store_get: Some(host_callbacks::host_store_get),
        store_set: Some(host_callbacks::host_store_set),
        store_del: Some(host_callbacks::host_store_del),
        datasource_build: Some(host_callbacks::host_datasource_build),
        http_request: Some(host_callbacks::host_http_request),
        log_message: Some(host_callbacks::host_log_message),
        free_buffer: Some(host_callbacks::host_free_buffer),
        keystore_has: Some(host_callbacks::host_keystore_has),
        keystore_info: Some(host_callbacks::host_keystore_info),
        crypto_encrypt: Some(host_callbacks::host_crypto_encrypt),
        crypto_decrypt: Some(host_callbacks::host_crypto_decrypt),
        ddl_execute: Some(host_callbacks::host_ddl_execute),
        db_begin: Some(host_callbacks::host_db_begin),
        db_commit: Some(host_callbacks::host_db_commit),
        db_rollback: Some(host_callbacks::host_db_rollback),
        db_batch: Some(host_callbacks::host_db_batch),
    }
}
