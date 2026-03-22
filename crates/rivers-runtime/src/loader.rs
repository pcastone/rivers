//! Configuration file loading and parsing.
//!
//! All Rivers configs are TOML files. This module provides typed
//! loaders that read from disk and deserialize into config structs.

use std::path::Path;

use rivers_core_config::RiversError;
use rivers_core_config::ServerConfig;

use crate::bundle::{AppConfig, AppManifest, BundleManifest, ResourcesConfig};

/// Read a file to string, mapping I/O errors to `RiversError::Io`.
fn read_file(path: &Path) -> Result<String, RiversError> {
    std::fs::read_to_string(path)
        .map_err(|e| RiversError::Io(format!("{}: {}", path.display(), e)))
}

/// Parse a TOML string, mapping parse errors to `RiversError::Config`.
fn parse_toml<T: serde::de::DeserializeOwned>(content: &str, label: &str) -> Result<T, RiversError> {
    toml::from_str(content)
        .map_err(|e| RiversError::Config(format!("{}: {}", label, e)))
}

/// Read a file and parse it as TOML, with app-context-enriched error messages.
fn load_and_parse<T: serde::de::DeserializeOwned>(path: &Path, app_name: &str) -> Result<T, RiversError> {
    let content = read_file(path)?;
    let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("unknown");
    parse_toml(&content, &format!("{} (app '{}')", file_name, app_name))
}

/// Load `riversd.conf` (ServerConfig) from the given path.
pub fn load_server_config(path: &Path) -> Result<ServerConfig, RiversError> {
    let content = read_file(path)?;
    parse_toml(&content, "riversd.conf")
}

/// Load a bundle-level `manifest.toml`.
pub fn load_bundle_manifest(path: &Path) -> Result<BundleManifest, RiversError> {
    let content = read_file(path)?;
    parse_toml(&content, "bundle manifest.toml")
}

/// Load a per-app `manifest.toml`.
pub fn load_app_manifest(path: &Path) -> Result<AppManifest, RiversError> {
    let content = read_file(path)?;
    parse_toml(&content, "app manifest.toml")
}

/// Load a per-app `resources.toml`.
pub fn load_resources_config(path: &Path) -> Result<ResourcesConfig, RiversError> {
    let content = read_file(path)?;
    parse_toml(&content, "resources.toml")
}

/// Load a per-app `app.toml`.
pub fn load_app_config(path: &Path) -> Result<AppConfig, RiversError> {
    let content = read_file(path)?;
    parse_toml(&content, "app.toml")
}

/// Load an entire bundle from a directory path.
///
/// Reads:
/// - `{bundle_dir}/manifest.toml`
/// - For each app in the manifest:
///   - `{bundle_dir}/{app}/manifest.toml`
///   - `{bundle_dir}/{app}/resources.toml`
///   - `{bundle_dir}/{app}/app.toml`
pub fn load_bundle(bundle_dir: &Path) -> Result<LoadedBundle, RiversError> {
    let bundle_manifest = load_bundle_manifest(&bundle_dir.join("manifest.toml"))?;

    let mut apps = Vec::new();
    for app_name in &bundle_manifest.apps {
        let app_dir = bundle_dir.join(app_name);

        let manifest_path = app_dir.join("manifest.toml");
        let manifest: AppManifest = load_and_parse(&manifest_path, app_name)?;

        let resources_path = app_dir.join("resources.toml");
        let resources: ResourcesConfig = load_and_parse(&resources_path, app_name)?;

        let config_path = app_dir.join("app.toml");
        let config: AppConfig = load_and_parse(&config_path, app_name)?;

        apps.push(LoadedApp {
            manifest,
            resources,
            config,
            app_dir,
        });
    }

    Ok(LoadedBundle {
        manifest: bundle_manifest,
        apps,
    })
}

/// A fully loaded bundle — manifest plus all apps.
#[derive(Debug)]
pub struct LoadedBundle {
    pub manifest: BundleManifest,
    pub apps: Vec<LoadedApp>,
}

/// A fully loaded app — manifest, resources, and config.
#[derive(Debug)]
pub struct LoadedApp {
    pub manifest: AppManifest,
    pub resources: ResourcesConfig,
    pub config: AppConfig,
    /// Path to the app directory on disk (for resolving schema file references).
    pub app_dir: std::path::PathBuf,
}
