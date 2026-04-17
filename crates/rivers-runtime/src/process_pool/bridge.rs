//! Serialization bridge — converts TaskContext to/from SerializedTaskContext.

use std::collections::HashMap;
use std::sync::Arc;

use crate::rivers_core::{DriverFactory, StorageEngine};
use crate::DataViewExecutor;

use super::types::*;

// ── Serialization Bridge (BB1) ────────────────────────────────────

impl From<&TaskContext> for crate::rivers_engine_sdk::SerializedTaskContext {
    fn from(ctx: &TaskContext) -> Self {
        Self {
            datasource_tokens: ctx.datasources.iter()
                .map(|(k, v)| (k.clone(), match v {
                    DatasourceToken::Pooled { pool_id } => pool_id.clone(),
                    DatasourceToken::Direct { driver, root } => {
                        format!("direct://{}?root={}", driver, root.display())
                    }
                }))
                .collect(),
            dataview_tokens: ctx.dataviews.iter()
                .map(|(k, v)| (k.clone(), v.0.clone()))
                .collect(),
            datasource_configs: ctx.datasource_configs.iter()
                .map(|(k, v)| (k.clone(), crate::rivers_engine_sdk::SerializedDatasource {
                    driver_name: v.driver_name.clone(),
                    host: v.params.host.clone(),
                    port: v.params.port,
                    database: v.params.database.clone(),
                    username: v.params.username.clone(),
                    options: v.params.options.clone(),
                }))
                .collect(),
            http_enabled: ctx.http.is_some(),
            env: ctx.env.clone(),
            entrypoint: crate::rivers_engine_sdk::SerializedEntrypoint {
                module: ctx.entrypoint.module.clone(),
                function: ctx.entrypoint.function.clone(),
                language: ctx.entrypoint.language.clone(),
            },
            args: ctx.args.clone(),
            trace_id: ctx.trace_id.clone(),
            app_id: ctx.app_id.clone(),
            node_id: ctx.node_id.clone(),
            runtime_env: ctx.runtime_env.clone(),
            storage_available: ctx.storage.is_some(),
            store_namespace: Some(format!("app:{}", ctx.app_id)),
            lockbox_available: {
                #[cfg(feature = "lockbox")]
                { ctx.lockbox.is_some() }
                #[cfg(not(feature = "lockbox"))]
                { false }
            },
            keystore_available: {
                #[cfg(feature = "keystore")]
                { ctx.keystore.is_some() }
                #[cfg(not(feature = "keystore"))]
                { false }
            },
            inline_source: ctx.args.get("_source").and_then(|v| v.as_str()).map(|s| s.to_string()),
            prefetched_data: HashMap::new(),
            libs: ctx.libs.iter()
                .map(|l| crate::rivers_engine_sdk::SerializedLib {
                    name: l.name.clone(),
                    content: l.content.clone(),
                })
                .collect(),
        }
    }
}

impl From<crate::rivers_engine_sdk::SerializedTaskResult> for TaskResult {
    fn from(r: crate::rivers_engine_sdk::SerializedTaskResult) -> Self {
        Self {
            value: r.value,
            duration_ms: r.duration_ms,
        }
    }
}

/// Builder for TaskContext.
pub struct TaskContextBuilder {
    datasources: HashMap<String, DatasourceToken>,
    dataviews: HashMap<String, DataViewToken>,
    libs: Vec<ResolvedLib>,
    http: Option<HttpToken>,
    env: HashMap<String, String>,
    storage: Option<Arc<dyn StorageEngine>>,
    driver_factory: Option<Arc<DriverFactory>>,
    datasource_configs: HashMap<String, ResolvedDatasource>,
    dataview_executor: Option<Arc<DataViewExecutor>>,
    #[cfg(feature = "lockbox")]
    lockbox: Option<Arc<crate::rivers_core::lockbox::LockBoxResolver>>,
    #[cfg(feature = "lockbox")]
    lockbox_keystore_path: Option<std::path::PathBuf>,
    #[cfg(feature = "lockbox")]
    lockbox_identity: Option<String>,
    #[cfg(feature = "keystore")]
    keystore: Option<Arc<rivers_keystore_engine::AppKeystore>>,
    entrypoint: Option<Entrypoint>,
    args: serde_json::Value,
    trace_id: String,
    app_id: String,
    node_id: String,
    runtime_env: String,
}

impl TaskContextBuilder {
    /// Create a new builder with default values.
    pub fn new() -> Self {
        Self {
            datasources: HashMap::new(),
            dataviews: HashMap::new(),
            libs: Vec::new(),
            http: None,
            env: HashMap::new(),
            storage: None,
            driver_factory: None,
            datasource_configs: HashMap::new(),
            dataview_executor: None,
            #[cfg(feature = "lockbox")]
            lockbox: None,
            #[cfg(feature = "lockbox")]
            lockbox_keystore_path: None,
            #[cfg(feature = "lockbox")]
            lockbox_identity: None,
            #[cfg(feature = "keystore")]
            keystore: None,
            entrypoint: None,
            args: serde_json::Value::Null,
            trace_id: String::new(),
            app_id: String::new(),
            node_id: String::new(),
            runtime_env: "dev".to_string(),
        }
    }

    /// Add a datasource token.
    pub fn datasource(mut self, name: String, token: DatasourceToken) -> Self {
        self.datasources.insert(name, token);
        self
    }

    /// Add a DataView token.
    pub fn dataview(mut self, name: String, token: DataViewToken) -> Self {
        self.dataviews.insert(name, token);
        self
    }

    /// Add a pre-resolved library module.
    pub fn lib(mut self, lib: ResolvedLib) -> Self {
        self.libs.push(lib);
        self
    }

    /// Enable outbound HTTP capability.
    pub fn http(mut self, token: HttpToken) -> Self {
        self.http = Some(token);
        self
    }

    /// Set the StorageEngine backend for `ctx.store`.
    pub fn storage(mut self, engine: Arc<dyn StorageEngine>) -> Self {
        self.storage = Some(engine);
        self
    }

    /// Set the DriverFactory for `ctx.datasource().build()`.
    pub fn driver_factory(mut self, factory: Arc<DriverFactory>) -> Self {
        self.driver_factory = Some(factory);
        self
    }

    /// Add a resolved datasource config for dynamic connection building.
    pub fn datasource_config(mut self, name: String, config: ResolvedDatasource) -> Self {
        self.datasource_configs.insert(name, config);
        self
    }

    /// Set the DataViewExecutor for `ctx.dataview()` dynamic execution.
    pub fn dataview_executor(mut self, executor: Arc<DataViewExecutor>) -> Self {
        self.dataview_executor = Some(executor);
        self
    }

    /// Set the LockBox resolver, keystore path, and identity for secret access.
    #[cfg(feature = "lockbox")]
    pub fn lockbox(
        mut self,
        resolver: Arc<crate::rivers_core::lockbox::LockBoxResolver>,
        keystore_path: std::path::PathBuf,
        identity: String,
    ) -> Self {
        self.lockbox = Some(resolver);
        self.lockbox_keystore_path = Some(keystore_path);
        self.lockbox_identity = Some(identity);
        self
    }

    /// Set the application keystore for encryption/decryption operations.
    #[cfg(feature = "keystore")]
    pub fn keystore(mut self, ks: Arc<rivers_keystore_engine::AppKeystore>) -> Self {
        self.keystore = Some(ks);
        self
    }

    /// Add an environment variable.
    pub fn env_var(mut self, key: String, value: String) -> Self {
        self.env.insert(key, value);
        self
    }

    /// Set the module and function to invoke.
    pub fn entrypoint(mut self, entrypoint: Entrypoint) -> Self {
        self.entrypoint = Some(entrypoint);
        self
    }

    /// Set the JSON arguments for the handler function.
    pub fn args(mut self, args: serde_json::Value) -> Self {
        self.args = args;
        self
    }

    /// Set the distributed trace ID.
    pub fn trace_id(mut self, trace_id: String) -> Self {
        self.trace_id = trace_id;
        self
    }

    /// Set the application ID.
    pub fn app_id(mut self, id: String) -> Self {
        self.app_id = id;
        self
    }

    /// Set the Rivers node ID.
    pub fn node_id(mut self, id: String) -> Self {
        self.node_id = id;
        self
    }

    /// Set the runtime environment name (e.g. "dev", "staging", "prod").
    pub fn runtime_env(mut self, env: String) -> Self {
        self.runtime_env = env;
        self
    }

    /// Build the `TaskContext`. Fails if entrypoint is not set.
    pub fn build(self) -> Result<TaskContext, TaskError> {
        let entrypoint = self
            .entrypoint
            .ok_or_else(|| TaskError::Capability("entrypoint is required".to_string()))?;

        Ok(TaskContext {
            datasources: self.datasources,
            dataviews: self.dataviews,
            libs: self.libs,
            http: self.http,
            env: self.env,
            entrypoint,
            args: self.args,
            trace_id: self.trace_id,
            app_id: self.app_id,
            node_id: self.node_id,
            runtime_env: self.runtime_env,
            storage: self.storage,
            driver_factory: self.driver_factory,
            datasource_configs: self.datasource_configs,
            dataview_executor: self.dataview_executor,
            #[cfg(feature = "lockbox")]
            lockbox: self.lockbox,
            #[cfg(feature = "lockbox")]
            lockbox_keystore_path: self.lockbox_keystore_path,
            #[cfg(feature = "lockbox")]
            lockbox_identity: self.lockbox_identity,
            #[cfg(feature = "keystore")]
            keystore: self.keystore,
        })
    }
}

impl Default for TaskContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}
