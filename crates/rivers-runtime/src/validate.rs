//! Configuration validation.
//!
//! Validates parsed configs against spec rules before the server starts.

use rivers_core_config::{RiversError, ServerConfig};

use crate::bundle::AppConfig;
use crate::loader::LoadedBundle;

/// Validate a `ServerConfig` against spec rules.
pub fn validate_server_config(config: &ServerConfig) -> Result<(), Vec<RiversError>> {
    let mut errors = Vec::new();

    // Port must be > 0
    if config.base.port == 0 {
        errors.push(RiversError::Config("base.port must be > 0".into()));
    }

    // request_timeout_seconds must be > 0
    if config.base.request_timeout_seconds == 0 {
        errors.push(RiversError::Config(
            "base.request_timeout_seconds must be > 0".into(),
        ));
    }

    // Admin API port must not conflict with main port
    if config.base.admin_api.enabled {
        if let Some(admin_port) = config.base.admin_api.port {
            if admin_port == config.base.port {
                errors.push(RiversError::Config(format!(
                    "admin_api.port ({}) conflicts with base.port ({})",
                    admin_port, config.base.port,
                )));
            }
        } else {
            errors.push(RiversError::Config(
                "admin_api.enabled=true but admin_api.port is not set".into(),
            ));
        }

    }

    // CORS: if enabled, must have at least one allowed origin
    if config.security.cors_enabled && config.security.cors_allowed_origins.is_empty() {
        errors.push(RiversError::Config(
            "cors_enabled=true but cors_allowed_origins is empty".into(),
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Known view types accepted by the Rivers framework.
const VALID_VIEW_TYPES: &[&str] = &["Rest", "Websocket", "ServerSentEvents", "MessageConsumer"];

/// Validate a loaded app config for internal consistency.
pub fn validate_app_config(config: &AppConfig) -> Result<(), Vec<RiversError>> {
    let mut errors = Vec::new();

    let dv_names: std::collections::HashSet<&str> =
        config.data.dataviews.keys().map(|s| s.as_str()).collect();

    // Every DataView must reference an existing datasource
    for (dv_name, dv) in &config.data.dataviews {
        if !config.data.datasources.contains_key(&dv.datasource) {
            errors.push(RiversError::Config(format!(
                "dataview '{}' references unknown datasource '{}'",
                dv_name, dv.datasource,
            )));
        }

        // (E) Validate invalidates targets exist
        for target in &dv.invalidates {
            if !dv_names.contains(target.as_str()) {
                errors.push(RiversError::Config(format!(
                    "dataview '{}': invalidates target '{}' does not exist",
                    dv_name, target,
                )));
            }
        }
    }

    // Every dataview-type view handler must reference an existing DataView
    for (view_name, view) in &config.api.views {
        if let crate::view::HandlerConfig::Dataview { ref dataview } = view.handler {
            if !config.data.dataviews.contains_key(dataview) {
                errors.push(RiversError::Config(format!(
                    "view '{}' handler references unknown dataview '{}'",
                    view_name, dataview,
                )));
            }
        }

        // (C) Validate view_type is a known value
        if !VALID_VIEW_TYPES.contains(&view.view_type.as_str()) {
            errors.push(RiversError::Config(format!(
                "view '{}': unknown view_type '{}' (expected one of: {})",
                view_name, view.view_type, VALID_VIEW_TYPES.join(", "),
            )));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate cross-app consistency within a bundle.
pub fn validate_bundle(bundle: &LoadedBundle) -> Result<(), Vec<RiversError>> {
    let mut errors = Vec::new();

    // Check for duplicate app IDs
    let mut seen_ids = std::collections::HashSet::new();
    for app in &bundle.apps {
        if !seen_ids.insert(&app.manifest.app_id) {
            errors.push(RiversError::Config(format!(
                "duplicate appId '{}' in bundle",
                app.manifest.app_id,
            )));
        }
    }

    // Check for duplicate app names
    let mut seen_names = std::collections::HashSet::new();
    for app in &bundle.apps {
        if !seen_names.insert(&app.manifest.app_name) {
            errors.push(RiversError::Config(format!(
                "duplicate appName '{}' in bundle",
                app.manifest.app_name,
            )));
        }
    }

    // Validate each app individually
    for app in &bundle.apps {
        let app_label = &app.manifest.app_name;

        if let Err(app_errors) = validate_app_config(&app.config) {
            for e in app_errors {
                errors.push(RiversError::Config(format!("[{}] {}", app_label, e)));
            }
        }

        // (H) Duplicate datasource names in resources.toml
        for e in validate_duplicate_resource_names(&app.resources) {
            errors.push(RiversError::Config(format!("[{}] {}", app_label, e)));
        }

        // (F) Schema file existence
        for e in validate_schema_files(app) {
            errors.push(RiversError::Config(format!("[{}] {}", app_label, e)));
        }
    }

    // (K) Cross-app service reference check
    let known_app_ids: std::collections::HashSet<&str> =
        bundle.apps.iter().map(|a| a.manifest.app_id.as_str()).collect();
    for app in &bundle.apps {
        for svc in &app.resources.services {
            if !known_app_ids.contains(svc.app_id.as_str()) {
                errors.push(RiversError::Config(format!(
                    "[{}] service '{}' references unknown appId '{}'",
                    app.manifest.app_name, svc.name, svc.app_id,
                )));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ── Helper validation functions ──────────────────────────────────

/// (H) Check for duplicate names in resources.toml arrays.
fn validate_duplicate_resource_names(resources: &crate::bundle::ResourcesConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let mut ds_names = std::collections::HashSet::new();
    for ds in &resources.datasources {
        if !ds_names.insert(&ds.name) {
            errors.push(format!(
                "duplicate datasource name '{}' in resources.toml",
                ds.name,
            ));
        }
    }
    let mut svc_names = std::collections::HashSet::new();
    for svc in &resources.services {
        if !svc_names.insert(&svc.name) {
            errors.push(format!(
                "duplicate service name '{}' in resources.toml",
                svc.name,
            ));
        }
    }
    errors
}

/// (F) Validate schema file references resolve to existing files on disk.
fn validate_schema_files(app: &crate::loader::LoadedApp) -> Vec<String> {
    let mut errors = Vec::new();
    for (dv_name, dv) in &app.config.data.dataviews {
        let schema_refs = [
            dv.return_schema.as_deref(),
            dv.get_schema.as_deref(),
            dv.post_schema.as_deref(),
            dv.put_schema.as_deref(),
            dv.delete_schema.as_deref(),
        ];
        for schema_ref in schema_refs.into_iter().flatten() {
            let schema_path = app.app_dir.join(schema_ref);
            if !schema_path.exists() {
                errors.push(format!(
                    "dataview '{}': schema file '{}' not found (resolved: {})",
                    dv_name, schema_ref, schema_path.display(),
                ));
            }
        }
    }
    errors
}

/// (D) Validate that datasource drivers are known to the framework.
///
/// Called from `bundle_loader.rs` after the DriverFactory is built,
/// since known driver names come from the runtime.
pub fn validate_known_drivers(
    bundle: &LoadedBundle,
    known_drivers: &[&str],
) -> Vec<RiversError> {
    let mut errors = Vec::new();
    for app in &bundle.apps {
        for (ds_name, ds) in &app.config.data.datasources {
            if !known_drivers.contains(&ds.driver.as_str()) {
                errors.push(RiversError::Config(format!(
                    "[{}] datasource '{}': unknown driver '{}' (known: {})",
                    app.manifest.app_name, ds_name, ds.driver,
                    known_drivers.join(", "),
                )));
            }
        }
    }
    errors
}
