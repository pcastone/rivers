//! Application init handler — per-app bootstrap.
//!
//! Per technology-path-spec §11.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Application context provided to init handlers.
///
/// Per spec §11.2: no `env`, no `node_id`, no runtime metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationContext {
    pub app_id: String,
    pub app_name: String,
    pub config: HashMap<String, serde_json::Value>,
}

/// CORS policy set by the init handler.
///
/// Per technology-path-spec §10.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsPolicy {
    pub origins: Vec<String>,
    pub methods: Vec<String>,
    pub headers: Vec<String>,
    pub credentials: bool,
    #[serde(default)]
    pub max_age: Option<u64>,
}

/// Health check function type.
pub type HealthCheckFn = Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>> + Send + Sync>;

/// Shutdown handler function type.
pub type ShutdownFn = Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// App-level configuration set during init.
///
/// Populated by the init handler, consumed by the runtime.
#[derive(Debug)]
pub struct AppInitResult {
    pub cors_policy: Option<CorsPolicy>,
    pub health_check: Option<String>,  // Placeholder — real impl would use fn
    pub shutdown_hooks: Vec<String>,    // Placeholder — real impl would use fn
}

impl Default for AppInitResult {
    fn default() -> Self {
        Self {
            cors_policy: None,
            health_check: None,
            shutdown_hooks: Vec::new(),
        }
    }
}

/// Init handler TOML configuration.
///
/// Per spec §11.3.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct InitHandlerConfig {
    /// Path to the handler module (e.g., "handlers/init.ts").
    pub init_handler: Option<String>,

    /// Entrypoint function name (e.g., "init").
    pub init_entrypoint: Option<String>,

    /// App-level config map from [app.config].
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cors_policy_serializes() {
        let policy = CorsPolicy {
            origins: vec!["http://localhost:3000".into()],
            methods: vec!["GET".into(), "POST".into()],
            headers: vec!["Content-Type".into()],
            credentials: true,
            max_age: Some(3600),
        };
        let json = serde_json::to_value(&policy).unwrap();
        assert_eq!(json["origins"][0], "http://localhost:3000");
        assert_eq!(json["credentials"], true);
        assert_eq!(json["max_age"], 3600);
    }

    #[test]
    fn app_init_result_default() {
        let result = AppInitResult::default();
        assert!(result.cors_policy.is_none());
        assert!(result.health_check.is_none());
        assert!(result.shutdown_hooks.is_empty());
    }

    #[test]
    fn init_handler_config_deserializes() {
        let toml_str = r#"
            init_handler = "handlers/init.ts"
            init_entrypoint = "init"

            [config]
            allowed_origins = ["http://localhost:3000"]
            seed_on_start = false
        "#;
        let config: InitHandlerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.init_handler.as_deref(), Some("handlers/init.ts"));
        assert_eq!(config.init_entrypoint.as_deref(), Some("init"));
        assert!(config.config.contains_key("allowed_origins"));
    }
}
