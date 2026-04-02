//! Bundle and app manifest configuration types.
//!
//! Per `rivers-application-spec.md` §4-6.
//!
//! Note: The spec uses JSON for manifests and resources, but the address-book
//! reference implementation uses TOML. We support TOML parsing with serde,
//! which works for both formats via the appropriate deserializer.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::datasource::DatasourceConfig;
use crate::dataview::DataViewConfig;
use crate::view::ApiViewConfig;

// ── Bundle manifest ─────────────────────────────────────────────────

/// Bundle-level `manifest.toml` at the root of a bundle directory.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BundleManifest {
    /// Human-readable bundle name.
    #[serde(alias = "bundleName")]
    pub bundle_name: String,

    /// Semantic version of this bundle.
    #[serde(alias = "bundleVersion")]
    pub bundle_version: String,

    /// Source repository URL or reference.
    pub source: Option<String>,

    /// List of app directory names contained in this bundle.
    pub apps: Vec<String>,
}

// ── App manifest ────────────────────────────────────────────────────

/// Per-app `manifest.toml` inside `{app-name}/manifest.toml`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AppManifest {
    /// Human-readable application name.
    #[serde(alias = "appName")]
    pub app_name: String,

    /// Optional description.
    pub description: Option<String>,
    /// App version string.
    pub version: Option<String>,

    /// "app-main" | "app-service"
    #[serde(rename = "type")]
    pub app_type: String,

    /// Stable UUID — generate once, never regenerate.
    #[serde(alias = "appId")]
    pub app_id: String,

    /// Route name for this app — used as a URL segment: `/<bundle>/<entryPoint>/...`
    /// e.g. "service", "main", "orders"
    #[serde(alias = "entryPoint")]
    pub entry_point: Option<String>,

    /// Deprecated alias for `entry_point`.
    #[serde(alias = "appEntryPoint")]
    pub app_entry_point: Option<String>,

    /// Source repository URL or reference.
    pub source: Option<String>,

    /// SPA configuration (app-main only).
    pub spa: Option<SpaConfig>,

    /// Application init handler — runs once at startup before the app enters RUNNING.
    /// Used for DDL operations (schema migrations) with whitelisted datasources.
    pub init: Option<InitHandlerConfig>,
}

/// Application init handler configuration.
///
/// Declared in the app manifest. Runs once during startup in ApplicationInit context.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InitHandlerConfig {
    /// Path to CodeComponent module relative to `libraries/`.
    pub module: String,
    /// Exported function name.
    pub entrypoint: String,
}

/// SPA serving config in the app manifest.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SpaConfig {
    /// Root directory containing the SPA build output.
    pub root: String,
    /// Index file name. Default: "index.html".
    #[serde(default = "default_index")]
    pub index_file: String,
    /// Enable SPA fallback (serve index for unmatched routes). Default: false.
    #[serde(default)]
    pub fallback: bool,
    /// Cache-Control max-age header value in seconds.
    pub max_age: Option<u64>,
}

fn default_index() -> String {
    "index.html".to_string()
}

// ── Resources config ────────────────────────────────────────────────

/// `resources.toml` — declares datasources and service dependencies.
///
/// Per `rivers-application-spec.md` §6.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ResourcesConfig {
    /// Datasource declarations.
    #[serde(default)]
    pub datasources: Vec<ResourceDatasource>,

    /// Application keystore declarations.
    #[serde(default)]
    pub keystores: Vec<ResourceKeystore>,

    /// Inter-app service dependencies.
    #[serde(default)]
    pub services: Vec<ServiceDependency>,
}

/// A datasource declaration in `resources.toml`.
///
/// This is a lightweight reference — the full config lives in `app.toml`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ResourceDatasource {
    /// Datasource name (matches `[data.datasources.{name}]` in app.toml).
    pub name: String,
    /// Driver name: "postgres", "mysql", "sqlite", "http", "faker", etc.
    pub driver: String,
    /// LockBox alias for credentials (e.g. "lockbox://db/myapp-postgres").
    pub lockbox: Option<String>,
    /// If true, no password/credentials required (e.g. faker driver).
    #[serde(default)]
    pub nopassword: bool,
    /// Build-time type hint for validation tools.
    #[serde(rename = "x-type")]
    pub x_type: Option<String>,
    /// Whether this datasource is required for app startup. Default: true.
    #[serde(default = "default_true")]
    pub required: bool,
}

/// A keystore declaration in `resources.toml`.
///
/// Per `rivers-feature-request-app-keystore.md` §5.1.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ResourceKeystore {
    /// Keystore name — must match a `[data.keystore.<name>]` section in app.toml.
    pub name: String,
    /// Lockbox alias for the master key that encrypts this keystore at rest.
    pub lockbox: String,
    /// Whether this keystore is required for the app to start.
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

/// A service dependency declared in `resources.toml`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ServiceDependency {
    /// Logical service name used in code references.
    pub name: String,
    /// UUID of the depended-upon app.
    #[serde(alias = "appId")]
    pub app_id: String,
    /// Whether this dependency is required for app startup. Default: true.
    #[serde(default = "default_true")]
    pub required: bool,
}

// ── App config (app.toml) ───────────────────────────────────────────

/// Top-level `app.toml` — combines datasource configs, dataviews, views,
/// and static file settings for a single app.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AppConfig {
    /// `[data]` section — datasources, DataViews, keystores.
    #[serde(default)]
    pub data: AppDataConfig,

    /// `[api]` section — view (endpoint) definitions.
    #[serde(default)]
    pub api: AppApiConfig,

    /// Optional static file serving configuration.
    #[serde(default)]
    pub static_files: Option<AppStaticFilesConfig>,
}

/// `[data]` section of app.toml.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AppDataConfig {
    /// Per-datasource config: `[data.datasources.{id}]`
    #[serde(default)]
    pub datasources: HashMap<String, DatasourceConfig>,

    /// Named DataViews: `[data.dataviews.{id}]`
    #[serde(default)]
    pub dataviews: HashMap<String, DataViewConfig>,

    /// Per-keystore config: `[data.keystore.{name}]`
    #[serde(default)]
    pub keystore: HashMap<String, KeystoreDataConfig>,
}

/// Keystore configuration in `app.toml` under `[data.keystore.<name>]`.
///
/// Per `rivers-feature-request-app-keystore.md` §5.2.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KeystoreDataConfig {
    /// Path to the keystore file, relative to app directory.
    pub path: String,
}

/// `[api]` section of app.toml.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AppApiConfig {
    /// Named views: `[api.views.{id}]`
    #[serde(default)]
    pub views: HashMap<String, ApiViewConfig>,
}

/// App-level static file config in app.toml.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AppStaticFilesConfig {
    /// Whether static file serving is active.
    #[serde(default)]
    pub enabled: bool,
    /// Root directory for static assets.
    pub root: Option<String>,
    /// Index file name (e.g. "index.html").
    pub index_file: Option<String>,
    /// Enable SPA fallback routing.
    #[serde(default)]
    pub spa_fallback: bool,
}

// ── Schema Generation ────────────────────────────────────────────

/// Generate JSON Schema for `AppConfig` (the `app.toml` format).
pub fn app_config_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(AppConfig)).unwrap_or_default()
}

/// Generate JSON Schema for `BundleManifest` (the bundle `manifest.toml` format).
pub fn bundle_manifest_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(BundleManifest)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resources_with_keystores() {
        let toml_str = r#"
[[datasources]]
name = "db"
driver = "sqlite"
nopassword = true

[[keystores]]
name = "app-keys"
lockbox = "netinventory/keystore-master-key"
required = true
"#;
        let config: ResourcesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.keystores.len(), 1);
        assert_eq!(config.keystores[0].name, "app-keys");
        assert_eq!(config.keystores[0].lockbox, "netinventory/keystore-master-key");
        assert!(config.keystores[0].required);
    }

    #[test]
    fn parse_app_data_with_keystore() {
        let toml_str = r#"
[datasources.db]
name = "db"
driver = "sqlite"

[keystore.app-keys]
path = "data/app.keystore"
"#;
        let config: AppDataConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.keystore.len(), 1);
        assert_eq!(config.keystore["app-keys"].path, "data/app.keystore");
    }

    #[test]
    fn resources_without_keystores_still_parse() {
        let toml_str = r#"
[[datasources]]
name = "db"
driver = "sqlite"
nopassword = true
"#;
        let config: ResourcesConfig = toml::from_str(toml_str).unwrap();
        assert!(config.keystores.is_empty());
    }
}
