//! TaskContext auto-enrichment — wires shared capabilities into every dispatch.
//!
//! Dispatch sites only set per-request fields such as entrypoint, args, and trace IDs.
//! This module snapshots the shared capabilities currently available from `AppContext`
//! so every handler gets the same storage, datasource, dataview, lockbox, and keystore
//! wiring from one place.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use rivers_runtime::process_pool::{TaskContextBuilder, TaskKind};
use rivers_runtime::rivers_core::{DriverFactory, StorageEngine};
use rivers_runtime::DataViewExecutor;

#[derive(Clone, Default)]
struct SharedTaskCapabilities {
    node_id: String,
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
        node_id: ctx.config.app_id.clone().unwrap_or_else(|| "node-0".to_string()),
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

/// Wire per-app datasource tokens and configs onto a TaskContextBuilder.
///
/// Mirrors the REST primary-handler datasource-wiring loop
/// (`view_engine/pipeline.rs`) so non-REST dispatch paths (MCP, and
/// eventually WebSocket/SSE) get the same view of the app's datasources.
///
/// Without this, `TASK_DS_CONFIGS` is empty for the dispatched handler and
/// every `Rivers.db.execute(...)` call throws `CapabilityError: datasource
/// '<name>' not declared in view config`.
///
/// `executor` may be `None` — in that case the function is a no-op (no
/// datasources to wire). `dv_namespace` is the entry-point slug used to
/// scope `executor.datasource_params()` to a single app.
///
/// CB-P1.13.
pub fn wire_datasources(
    mut builder: TaskContextBuilder,
    executor: Option<&DataViewExecutor>,
    dv_namespace: &str,
) -> TaskContextBuilder {
    let Some(exec) = executor else {
        return builder;
    };
    let ns_prefix = format!("{dv_namespace}:");
    for (key, params) in exec.datasource_params().iter() {
        let Some(ds_name) = key.strip_prefix(&ns_prefix) else {
            continue;
        };
        let driver = params.options.get("driver").map(|s| s.as_str()).unwrap_or("");
        if driver == "filesystem" {
            let token = rivers_runtime::process_pool::DatasourceToken::direct(
                "filesystem",
                std::path::PathBuf::from(&params.database),
            );
            builder = builder.datasource(ds_name.to_string(), token);
        } else if rivers_runtime::process_pool::BROKER_DRIVER_NAMES.contains(&driver) {
            // BR-2026-04-23: broker datasources get a Broker token + full
            // ConnectionParams copy so the worker can lazy-build a BrokerProducer.
            let token = rivers_runtime::process_pool::DatasourceToken::broker(driver);
            builder = builder.datasource(ds_name.to_string(), token.clone());
            let resolved = rivers_runtime::process_pool::ResolvedDatasource {
                driver_name: driver.to_string(),
                params: params.clone(),
            };
            builder = builder.datasource_config(ds_name.to_string(), resolved);
        } else if !driver.is_empty() {
            // SQL / NoSQL / other regular drivers: wire into datasource_configs
            // so ctx.transaction() and Rivers.db.begin() can open a connection
            // by name. No token needed; DriverFactory routes by driver name.
            let resolved = rivers_runtime::process_pool::ResolvedDatasource {
                driver_name: driver.to_string(),
                params: params.clone(),
            };
            builder = builder.datasource_config(ds_name.to_string(), resolved);
        }
    }
    builder
}

/// Wire per-app datasource tokens and configs using the executor cached in
/// `SHARED_TASK_CAPABILITIES`.
///
/// Used by dispatch paths that have no direct handle on `AppContext` (Cron
/// loops live on their own `tokio::task`s spawned at bundle load time). For
/// REST/MCP/SSE/WS, callers still pass the executor explicitly via
/// [`wire_datasources`].
///
/// CB-cron-cap-fix (2026-05-10): introduced so the Cron tick dispatcher can
/// propagate `[api.views.X.handler] resources` into the task capability set
/// the same way REST/MCP do. Without this, `Rivers.db.query` from inside a
/// Cron handler fails with `CapabilityError: datasource '<name>' not
/// declared in view config`.
pub fn wire_datasources_from_shared(
    builder: TaskContextBuilder,
    dv_namespace: &str,
) -> TaskContextBuilder {
    let executor = SHARED_TASK_CAPABILITIES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .dataview_executor
        .clone();
    wire_datasources(builder, executor.as_deref(), dv_namespace)
}

/// Enrich a TaskContextBuilder with all capabilities available from shared state.
///
/// New capabilities wired here automatically become available to every dispatch site.
///
/// **C1.2:** Every caller MUST pass an explicit `task_kind` so host capability
/// gates (e.g. `ctx.ddl()` ApplicationInit-only) work correctly. The compiler
/// is the todo list — adding a new dispatch site without a `task_kind` won't
/// build.
pub fn enrich(
    mut builder: TaskContextBuilder,
    app_id: &str,
    task_kind: TaskKind,
) -> TaskContextBuilder {
    builder = builder.task_kind(task_kind);
    if !app_id.is_empty() {
        builder = builder.app_id(app_id.into());
    }

    let capabilities = SHARED_TASK_CAPABILITIES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();

    if !capabilities.node_id.is_empty() {
        builder = builder.node_id(capabilities.node_id.clone());
    }

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
            TaskKind::Rest,
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

    /// CB-P1.13: helper must wire app-scoped datasources (regular SQL/NoSQL
    /// drivers populate `datasource_configs`; filesystem populates a direct
    /// `DatasourceToken`). Datasources for other apps must be ignored.
    #[tokio::test]
    async fn wire_datasources_populates_per_app_configs() {
        use std::collections::HashMap;
        use rivers_runtime::rivers_driver_sdk::ConnectionParams;

        let registry = DataViewRegistry::new();
        let factory = Arc::new(DriverFactory::new());
        let cache = Arc::new(NoopDataViewCache);

        let mut params: HashMap<String, ConnectionParams> = HashMap::new();
        let mut sql_opts = HashMap::new();
        sql_opts.insert("driver".to_string(), "sqlite".to_string());
        params.insert(
            "myapp:cb_db".to_string(),
            ConnectionParams {
                host: "localhost".into(),
                port: 0,
                database: "/tmp/cb.db".into(),
                username: String::new(),
                password: String::new(),
                options: sql_opts,
            },
        );
        // Other-app datasource must NOT leak.
        let mut other_opts = HashMap::new();
        other_opts.insert("driver".to_string(), "sqlite".to_string());
        params.insert(
            "otherapp:other_db".to_string(),
            ConnectionParams {
                host: "localhost".into(),
                port: 0,
                database: "/tmp/other.db".into(),
                username: String::new(),
                password: String::new(),
                options: other_opts,
            },
        );

        let executor = Arc::new(DataViewExecutor::new(
            registry,
            factory,
            Arc::new(params),
            cache,
        ));

        let builder = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "handlers/h.js".into(),
                function: "handle".into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({}))
            .trace_id("wire-test".into())
            .app_id("myapp".into());
        let builder = wire_datasources(builder, Some(executor.as_ref()), "myapp");
        let task = builder.build().expect("task ctx builds");

        assert!(task.datasource_configs.contains_key("cb_db"),
            "expected cb_db in scope; got keys {:?}", task.datasource_configs.keys().collect::<Vec<_>>());
        assert!(!task.datasource_configs.contains_key("other_db"),
            "other-app datasource leaked into this task");
        assert_eq!(
            task.datasource_configs["cb_db"].driver_name, "sqlite",
            "driver name preserved",
        );
    }

    /// CB-P1.13: when no executor is supplied (e.g. early bootstrap or a
    /// dispatch path without DataView wiring), helper must be a no-op and
    /// must not panic.
    #[test]
    fn wire_datasources_is_noop_without_executor() {
        let builder = TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "handlers/h.js".into(),
                function: "handle".into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({}))
            .trace_id("noop".into())
            .app_id("myapp".into());
        let builder = wire_datasources(builder, None, "myapp");
        let task = builder.build().expect("task ctx builds");
        assert!(task.datasource_configs.is_empty());
    }

    // ── P1.13 follow-up: branch coverage for `wire_datasources` ─────────────
    //
    // The existing tests above cover the SQL/cross-namespace and no-executor
    // branches. The helper has three more branches the original CB-P1.13
    // patch didn't pin. Each one is a path that the V8 worker reads — if any
    // regresses, an MCP-dispatched handler would silently lose access to a
    // capability that worked under REST. These tests exist to fail loudly
    // before that asymmetry slips back in.

    /// Helper: build a single-app executor with a parameterised driver. Used
    /// to keep each branch test focused on one driver class without the
    /// boilerplate of constructing the executor + ConnectionParams inline.
    fn p113_executor_with(
        ds_key: &str,
        driver: &str,
        database: &str,
    ) -> Arc<DataViewExecutor> {
        use std::collections::HashMap;
        use rivers_runtime::rivers_driver_sdk::ConnectionParams;

        let mut opts: HashMap<String, String> = HashMap::new();
        opts.insert("driver".into(), driver.to_string());
        let mut params: HashMap<String, ConnectionParams> = HashMap::new();
        params.insert(
            ds_key.to_string(),
            ConnectionParams {
                host: "localhost".into(),
                port: 0,
                database: database.to_string(),
                username: String::new(),
                password: String::new(),
                options: opts,
            },
        );
        Arc::new(DataViewExecutor::new(
            DataViewRegistry::new(),
            Arc::new(DriverFactory::new()),
            Arc::new(params),
            Arc::new(NoopDataViewCache),
        ))
    }

    fn p113_seed_builder() -> TaskContextBuilder {
        TaskContextBuilder::new()
            .entrypoint(Entrypoint {
                module: "handlers/h.js".into(),
                function: "handle".into(),
                language: "javascript".into(),
            })
            .args(serde_json::json!({}))
            .trace_id("p113-branch".into())
            .app_id("myapp".into())
    }

    /// Filesystem driver: lands as a `DatasourceToken::Direct` token, NOT in
    /// `datasource_configs`. The codecomponent reaches it through the
    /// in-process direct-dispatch path, not the DriverFactory connect cycle.
    #[test]
    fn wire_datasources_filesystem_yields_direct_token_only() {
        let executor = p113_executor_with("myapp:files", "filesystem", "/tmp/probe-data");
        let builder = wire_datasources(p113_seed_builder(), Some(executor.as_ref()), "myapp");
        let task = builder.build().expect("task ctx builds");

        assert!(
            task.datasources.contains_key("files"),
            "filesystem ds must be wired as a DatasourceToken (got keys {:?})",
            task.datasources.keys().collect::<Vec<_>>(),
        );
        assert!(
            !task.datasource_configs.contains_key("files"),
            "filesystem must NOT populate datasource_configs (the worker uses \
             the direct token path, not DriverFactory)",
        );
    }

    /// Broker driver: gets BOTH a `DatasourceToken::Broker` AND a
    /// `ResolvedDatasource` config (per BR-2026-04-23 — the worker uses the
    /// token to route and the config to lazy-build a BrokerProducer).
    #[test]
    fn wire_datasources_broker_yields_token_and_config() {
        let executor = p113_executor_with("myapp:events", "kafka", "events-topic");
        let builder = wire_datasources(p113_seed_builder(), Some(executor.as_ref()), "myapp");
        let task = builder.build().expect("task ctx builds");

        assert!(
            task.datasources.contains_key("events"),
            "broker ds must produce a DatasourceToken so the worker can route writes",
        );
        let resolved = task.datasource_configs.get("events").expect(
            "broker ds must ALSO populate datasource_configs so the worker can \
             lazy-build a producer with the right ConnectionParams",
        );
        assert_eq!(resolved.driver_name, "kafka");
    }

    /// A datasource entry whose `options.driver` is absent or empty must be
    /// silently skipped — no token, no config. Defends against a partial
    /// resources.toml ever inflating the task scope.
    #[test]
    fn wire_datasources_skips_entries_with_empty_driver() {
        use std::collections::HashMap;
        use rivers_runtime::rivers_driver_sdk::ConnectionParams;

        // Build params manually so we can omit the `driver` option entirely.
        let mut params: HashMap<String, ConnectionParams> = HashMap::new();
        params.insert(
            "myapp:no_driver".to_string(),
            ConnectionParams {
                host: String::new(),
                port: 0,
                database: String::new(),
                username: String::new(),
                password: String::new(),
                options: HashMap::new(),
            },
        );
        let executor = Arc::new(DataViewExecutor::new(
            DataViewRegistry::new(),
            Arc::new(DriverFactory::new()),
            Arc::new(params),
            Arc::new(NoopDataViewCache),
        ));

        let builder = wire_datasources(p113_seed_builder(), Some(executor.as_ref()), "myapp");
        let task = builder.build().expect("task ctx builds");

        assert!(
            task.datasources.is_empty() && task.datasource_configs.is_empty(),
            "missing/empty driver must skip the entry entirely; got tokens={:?}, configs={:?}",
            task.datasources.keys().collect::<Vec<_>>(),
            task.datasource_configs.keys().collect::<Vec<_>>(),
        );
    }

    /// CB-cron-cap-fix (2026-05-10): `wire_datasources_from_shared` reads
    /// the executor stashed in `SHARED_TASK_CAPABILITIES` and wires
    /// per-app datasources. Cron dispatch uses this because it has no
    /// direct `AppContext` handle — without it, every Cron handler that
    /// calls `Rivers.db.*` fails with CapabilityError.
    #[test]
    fn wire_datasources_from_shared_uses_snapshot_executor() {
        let executor = p113_executor_with("myapp:cb_db", "sqlite", "/tmp/cb.db");

        let previous = replace_shared_capabilities(SharedTaskCapabilities {
            dataview_executor: Some(executor),
            ..SharedTaskCapabilities::default()
        });

        let task = wire_datasources_from_shared(p113_seed_builder(), "myapp")
            .build()
            .expect("task ctx builds");

        assert!(
            task.datasource_configs.contains_key("cb_db"),
            "shared-state path must wire app-scoped datasources just like the explicit path"
        );

        replace_shared_capabilities(previous);
    }

    /// No executor in shared state → no-op (must not panic). Mirrors the
    /// `None` branch of `wire_datasources`.
    #[test]
    fn wire_datasources_from_shared_is_noop_when_unset() {
        let previous = replace_shared_capabilities(SharedTaskCapabilities::default());

        let task = wire_datasources_from_shared(p113_seed_builder(), "myapp")
            .build()
            .expect("task ctx builds");
        assert!(task.datasource_configs.is_empty());

        replace_shared_capabilities(previous);
    }
}
