//! TaskContext auto-enrichment — wires shared capabilities into every dispatch.
//!
//! Dispatch sites only set per-request fields such as entrypoint, args, and trace IDs.
//! This module snapshots the shared capabilities currently available from `AppContext`
//! so every handler gets the same storage, datasource, dataview, lockbox, and keystore
//! wiring from one place.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use rivers_runtime::process_pool::TaskContextBuilder;
use rivers_runtime::rivers_core::{DriverFactory, StorageEngine};
use rivers_runtime::DataViewExecutor;

#[derive(Clone, Default)]
struct SharedTaskCapabilities {
    storage_engine: Option<Arc<dyn StorageEngine>>,
    driver_factory: Option<Arc<DriverFactory>>,
    dataview_executor: Option<Arc<DataViewExecutor>>,
    lockbox_resolver: Option<Arc<rivers_runtime::rivers_core::lockbox::LockBoxResolver>>,
    lockbox_keystore_path: Option<PathBuf>,
    lockbox_identity: Option<String>,
    keystore_resolver: Option<Arc<crate::keystore::KeystoreResolver>>,
}

static SHARED_TASK_CAPABILITIES: LazyLock<RwLock<SharedTaskCapabilities>> =
    LazyLock::new(|| RwLock::new(SharedTaskCapabilities::default()));

/// Snapshot the capabilities currently available from `AppContext`.
///
/// Called during startup and hot reload so dispatch sites can stay simple.
pub fn sync_from_app_context(ctx: &crate::server::AppContext) {
    let (lockbox_keystore_path, lockbox_identity) = match ctx.config.lockbox.as_ref() {
        Some(config) => {
            let identity = match rivers_runtime::rivers_core::lockbox::resolve_key_source(config) {
                Ok(identity) => Some(identity.trim().to_string()),
                Err(error) => {
                    tracing::warn!(error = %error, "task enrichment: lockbox identity unavailable");
                    None
                }
            };
            (config.path.as_ref().map(PathBuf::from), identity)
        }
        None => (None, None),
    };

    let dataview_executor = ctx
        .dataview_executor
        .try_read()
        .ok()
        .and_then(|guard| guard.clone());

    let capabilities = SharedTaskCapabilities {
        storage_engine: ctx.storage_engine.clone(),
        driver_factory: ctx.driver_factory.clone(),
        dataview_executor,
        lockbox_resolver: ctx.lockbox_resolver.clone(),
        lockbox_keystore_path,
        lockbox_identity,
        keystore_resolver: ctx.keystore_resolver.clone(),
    };

    *SHARED_TASK_CAPABILITIES
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = capabilities;
}

/// Extract the app ID from a qualified identifier such as `app:view_id`.
pub fn app_id_from_qualified_name(name: &str) -> &str {
    name.split_once(':').map(|(app_id, _)| app_id).unwrap_or("")
}

/// Enrich a TaskContextBuilder with all capabilities available from shared state.
///
/// New capabilities wired here automatically become available to every dispatch site.
pub fn enrich(mut builder: TaskContextBuilder, app_id: &str) -> TaskContextBuilder {
    if !app_id.is_empty() {
        builder = builder.app_id(app_id.into());
    }

    let capabilities = SHARED_TASK_CAPABILITIES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();

    if let Some(storage_engine) = capabilities.storage_engine {
        builder = builder.storage(storage_engine);
    }

    if let Some(driver_factory) = capabilities.driver_factory {
        builder = builder.driver_factory(driver_factory);
    }

    if let Some(dataview_executor) = capabilities.dataview_executor {
        builder = builder.dataview_executor(dataview_executor);
    }

    if let (Some(lockbox_resolver), Some(lockbox_keystore_path), Some(lockbox_identity)) = (
        capabilities.lockbox_resolver,
        capabilities.lockbox_keystore_path,
        capabilities.lockbox_identity,
    ) {
        builder = builder.lockbox(lockbox_resolver, lockbox_keystore_path, lockbox_identity);
    }

    if !app_id.is_empty() {
        if let Some(resolver) = capabilities.keystore_resolver {
            if let Some(keystore) = resolver.get_for_entry_point(app_id) {
                builder = builder.keystore(keystore.clone());
            }
        }
    }

    builder
}

#[cfg(test)]
fn replace_shared_capabilities(
    capabilities: SharedTaskCapabilities,
) -> SharedTaskCapabilities {
    let mut guard = SHARED_TASK_CAPABILITIES
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::replace(&mut *guard, capabilities)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use rivers_runtime::process_pool::Entrypoint;
    use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;
    use rivers_runtime::tiered_cache::NoopDataViewCache;
    use rivers_runtime::{DataViewExecutor, DataViewRegistry};

    use super::*;

    fn make_lockbox_entry(name: &str, value: &str) -> rivers_runtime::rivers_core::lockbox::KeystoreEntry {
        rivers_runtime::rivers_core::lockbox::KeystoreEntry {
            name: name.to_string(),
            value: value.to_string(),
            entry_type: "string".to_string(),
            aliases: Vec::new(),
            created: Utc::now(),
            updated: Utc::now(),
            driver: None,
            username: None,
            hosts: Vec::new(),
            database: None,
        }
    }

    fn make_dataview_executor() -> Arc<DataViewExecutor> {
        let registry = DataViewRegistry::new();
        let factory = Arc::new(DriverFactory::new());
        let params = Arc::new(std::collections::HashMap::new());
        let cache = Arc::new(NoopDataViewCache);
        Arc::new(DataViewExecutor::new(registry, factory, params, cache))
    }

    #[tokio::test]
    async fn sync_from_app_context_wires_shared_capabilities() {
        let previous = replace_shared_capabilities(SharedTaskCapabilities::default());

        let lockbox_dir = tempfile::tempdir().unwrap();
        let lockbox_path = lockbox_dir.path().join("test.rkeystore");
        let lockbox_identity = "AGE-SECRET-KEY-TASK-ENRICHMENT-TEST".to_string();
        let env_var = "RIVERS_TASK_ENRICHMENT_LOCKBOX_TEST";
        // SAFETY: the test uses a unique variable name and restores it before exit.
        unsafe { std::env::set_var(env_var, &lockbox_identity); }

        let mut config = rivers_runtime::rivers_core::config::ServerConfig::default();
        config.lockbox = Some(rivers_runtime::rivers_core::lockbox::LockBoxConfig {
            path: Some(lockbox_path.display().to_string()),
            key_source: "env".to_string(),
            key_env_var: env_var.to_string(),
            ..Default::default()
        });

        let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new());
        let mut ctx = crate::server::AppContext::new(config, shutdown);
        ctx.storage_engine = Some(Arc::new(InMemoryStorageEngine::new()));
        ctx.driver_factory = Some(Arc::new(DriverFactory::new()));
        ctx.lockbox_resolver = Some(Arc::new(
            rivers_runtime::rivers_core::lockbox::LockBoxResolver::from_entries(&[
                make_lockbox_entry("crypto/hmac", "secret"),
            ])
            .unwrap(),
        ));

        let mut keystore_resolver = crate::keystore::KeystoreResolver::new();
        keystore_resolver.insert(
            "test-app:primary".to_string(),
            rivers_keystore_engine::AppKeystore {
                version: 1,
                keys: Vec::new(),
            },
        );
        ctx.keystore_resolver = Some(Arc::new(keystore_resolver));

        let executor = make_dataview_executor();
        *ctx.dataview_executor.write().await = Some(executor.clone());

        sync_from_app_context(&ctx);

        let task = enrich(
            TaskContextBuilder::new()
                .entrypoint(Entrypoint {
                    module: "handlers/test.js".to_string(),
                    function: "handle".to_string(),
                    language: "javascript".to_string(),
                })
                .args(serde_json::json!({"ok": true}))
                .trace_id("trace-1".to_string()),
            "test-app",
        )
        .build()
        .unwrap();

        assert_eq!(task.app_id, "test-app");
        assert!(task.storage.is_some());
        assert!(task.driver_factory.is_some());
        assert!(task.dataview_executor.is_some());
        assert!(task.lockbox.is_some());
        assert_eq!(task.lockbox_keystore_path.as_deref(), Some(lockbox_path.as_path()));
        assert_eq!(task.lockbox_identity.as_deref(), Some(lockbox_identity.trim()));
        assert!(task.keystore.is_some());

        // SAFETY: this test is the only one using this unique variable name.
        unsafe { std::env::remove_var(env_var); }
        replace_shared_capabilities(previous);
    }

    #[test]
    fn app_id_from_qualified_name_handles_namespaced_ids() {
        assert_eq!(app_id_from_qualified_name("orders:create"), "orders");
        assert_eq!(app_id_from_qualified_name("plain-view"), "");
    }
}
