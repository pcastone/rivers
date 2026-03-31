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
    }
}
