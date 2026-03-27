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
    #[serde(alias = "bundleName")]
    pub bundle_name: String,

    #[serde(alias = "bundleVersion")]
    pub bundle_version: String,

    pub source: Option<String>,

    /// List of app directory names contained in this bundle.
    pub apps: Vec<String>,
}

// ── App manifest ────────────────────────────────────────────────────

/// Per-app `manifest.toml` inside `{app-name}/manifest.toml`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AppManifest {
    #[serde(alias = "appName")]
    pub app_name: String,

    pub description: Option<String>,
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

    #[serde(alias = "appEntryPoint")]
    pub app_entry_point: Option<String>,

    pub source: Option<String>,

    /// SPA configuration (app-main only).
    pub spa: Option<SpaConfig>,
}

/// SPA serving config in the app manifest.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SpaConfig {
    pub root: String,
    #[serde(default = "default_index")]
    pub index_file: String,
    #[serde(default)]
    pub fallback: bool,
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
    #[serde(default)]
    pub datasources: Vec<ResourceDatasource>,

    #[serde(default)]
    pub keystores: Vec<ResourceKeystore>,

    #[serde(default)]
    pub services: Vec<ServiceDependency>,
}

/// A datasource declaration in `resources.toml`.
///
/// This is a lightweight reference — the full config lives in `app.toml`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ResourceDatasource {
    pub name: String,
    pub driver: String,
    pub lockbox: Option<String>,
    #[serde(default)]
    pub nopassword: bool,
    #[serde(rename = "x-type")]
    pub x_type: Option<String>,
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
    pub name: String,
    #[serde(alias = "appId")]
    pub app_id: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

// ── App config (app.toml) ───────────────────────────────────────────

/// Top-level `app.toml` — combines datasource configs, dataviews, views,
/// and static file settings for a single app.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AppConfig {
    #[serde(default)]
    pub data: AppDataConfig,

    #[serde(default)]
    pub api: AppApiConfig,

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
    #[serde(default)]
    pub enabled: bool,
    pub root: Option<String>,
    pub index_file: Option<String>,
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
