//! Full validation pipeline — orchestrates Layers 1-4.
//!
//! Per `rivers-bundle-validation-spec.md` §5: Gate 1 (offline validation).
//!
//! This is the top-level entry point that `riverpackage validate` calls.
//! It runs all four layers in sequence and produces a [`ValidationReport`].

use std::path::PathBuf;

use crate::validate_crossref::validate_crossref;
use crate::validate_engine::{self, EngineConfig, EngineHandles};
use crate::validate_existence::validate_existence;
use crate::validate_result::{
    error_codes, ValidationReport, ValidationResult,
    LAYER_LOGICAL_CROSS_REFERENCES, LAYER_RESOURCE_EXISTENCE,
    LAYER_STRUCTURAL_TOML, LAYER_SYNTAX_VERIFICATION,
};
use crate::validate_structural::validate_structural;
use crate::validate_syntax::validate_syntax;

/// Configuration for the full validation pipeline.
pub struct ValidationConfig {
    /// Path to the bundle directory on disk.
    pub bundle_dir: PathBuf,
    /// Optional engine dylib configuration for Layer 4.
    pub engines: Option<EngineConfig>,
}

/// Gate 1: offline validation (riverpackage).
///
/// Runs Layers 1-3. Layer 4 is not yet implemented and emits a W003 warning.
///
/// The pipeline always runs Layer 1 first. If structural validation finds
/// errors, Layers 2-3 still run (if the bundle can be loaded) to provide
/// as much feedback as possible in a single pass.
pub fn validate_bundle_full(config: &ValidationConfig) -> ValidationReport {
    // Try to extract bundle name/version from manifest for the report header.
    let (bundle_name, bundle_version) = read_bundle_meta(&config.bundle_dir);
    let mut report = ValidationReport::new(&bundle_name, &bundle_version);

    // Layer 1: Structural TOML
    let structural_results = validate_structural(&config.bundle_dir);
    for r in structural_results {
        report.add_result(LAYER_STRUCTURAL_TOML, r);
    }

    // Try to load bundle for Layers 2-4.
    match crate::loader::load_bundle(&config.bundle_dir) {
        Ok(bundle) => {
            // Layer 2: Resource Existence
            let existence_results = validate_existence(&config.bundle_dir, &bundle);
            for r in existence_results {
                report.add_result(LAYER_RESOURCE_EXISTENCE, r);
            }

            // Layer 3: Cross-Reference
            let crossref_results = validate_crossref(&bundle);
            for r in crossref_results {
                report.add_result(LAYER_LOGICAL_CROSS_REFERENCES, r);
            }

            // Layer 4: Syntax Verification
            let (engines, engine_warnings) = load_engine_handles(&config.engines);
            for warning in engine_warnings {
                report.add_result(
                    LAYER_SYNTAX_VERIFICATION,
                    ValidationResult::warn(error_codes::W002, warning),
                );
            }
            if !engines.any_available() && config.engines.is_none() {
                report.add_result(
                    LAYER_SYNTAX_VERIFICATION,
                    ValidationResult::warn(
                        error_codes::W003,
                        "Layer 4 skipped — riversd.toml not found, engine dylibs not configured",
                    ),
                );
            }
            let syntax_results = validate_syntax(&config.bundle_dir, &bundle, &engines);
            for r in syntax_results {
                report.add_result(LAYER_SYNTAX_VERIFICATION, r);
            }
        }
        Err(e) => {
            // Bundle couldn't be loaded — add error and skip Layers 2-4.
            report.add_result(
                LAYER_STRUCTURAL_TOML,
                ValidationResult::fail(
                    error_codes::S001,
                    "manifest.toml",
                    format!("bundle load failed: {e}"),
                ),
            );
        }
    }

    report
}

// ── Gate 2: Live Validation ─────────────────────────────────────

/// Trait for checking whether a LockBox alias exists.
pub trait LockBoxChecker {
    /// Returns true if the given alias exists in the LockBox keystore.
    fn alias_exists(&self, alias: &str) -> bool;
}

/// Trait for checking whether a service app is running.
pub trait ServiceHealthChecker {
    /// Returns true if the app with the given appId is in RUNNING state.
    fn is_running(&self, app_id: &str) -> bool;
}

/// Gate 2: offline + live checks (riversd deploy-time).
///
/// Runs the same Layers 1-3 as Gate 1, then adds live infrastructure checks:
/// - L001: LockBox alias existence
/// - L002: Driver name registration
/// - L004: x-type vs registered driver type
/// - L005: Required service health
pub fn validate_bundle_live(
    config: &ValidationConfig,
    known_drivers: &[&str],
    lockbox: Option<&dyn LockBoxChecker>,
    services: Option<&dyn ServiceHealthChecker>,
) -> ValidationReport {
    // First run the offline validation (Layers 1-3, skip Layer 4 engine for deploy)
    let mut report = validate_bundle_full(config);

    // Then add live checks
    if let Ok(bundle) = crate::loader::load_bundle(&config.bundle_dir) {
        let live_layer = "live_checks";

        for app in &bundle.apps {
            let app_name = &app.manifest.app_name;

            // L002: Driver registration check
            for ds in &app.resources.datasources {
                if !known_drivers.contains(&ds.driver.as_str()) {
                    report.add_result(
                        live_layer,
                        ValidationResult::fail(
                            error_codes::L002,
                            &format!("{}/resources.toml", app_name),
                            format!(
                                "driver '{}' not registered in this riversd instance",
                                ds.driver
                            ),
                        )
                        .with_app(app_name),
                    );
                }
            }

            // L001: LockBox alias existence
            if let Some(lb) = lockbox {
                for ds in &app.resources.datasources {
                    if let Some(ref alias) = ds.lockbox {
                        if !alias.is_empty() && !lb.alias_exists(alias) {
                            report.add_result(
                                live_layer,
                                ValidationResult::fail(
                                    error_codes::L001,
                                    &format!("{}/resources.toml", app_name),
                                    format!(
                                        "lockbox alias '{}' not found in keystore for datasource '{}'",
                                        alias, ds.name
                                    ),
                                )
                                .with_app(app_name),
                            );
                        }
                    }
                }
            }

            // L004: x-type vs registered driver
            for ds in &app.resources.datasources {
                if let Some(ref xt) = ds.x_type {
                    if xt != &ds.driver && known_drivers.contains(&ds.driver.as_str()) {
                        report.add_result(
                            live_layer,
                            ValidationResult::fail(
                                error_codes::L004,
                                &format!("{}/resources.toml", app_name),
                                format!(
                                    "x-type '{}' does not match registered driver '{}' for datasource '{}'",
                                    xt, ds.driver, ds.name
                                ),
                            )
                            .with_app(app_name),
                        );
                    }
                }
            }

            // L005: Required service health
            if let Some(svc_checker) = services {
                for svc in &app.resources.services {
                    if svc.required && !svc_checker.is_running(&svc.app_id) {
                        report.add_result(
                            live_layer,
                            ValidationResult::fail(
                                error_codes::L005,
                                &format!("{}/resources.toml", app_name),
                                format!(
                                    "required service '{}' (appId: {}) is not running",
                                    svc.name, svc.app_id
                                ),
                            )
                            .with_app(app_name),
                        );
                    }
                }
            }
        }
    }

    report
}

/// Load engine handles from an optional `EngineConfig`.
fn load_engine_handles(config: &Option<EngineConfig>) -> (EngineHandles, Vec<String>) {
    match config {
        Some(cfg) => validate_engine::load_engines(cfg),
        None => (EngineHandles::none(), Vec::new()),
    }
}

/// Read bundle name and version from manifest.toml without full load_bundle.
///
/// Falls back to directory name and "0.0.0" if the manifest is missing or
/// unparseable.
fn read_bundle_meta(bundle_dir: &std::path::Path) -> (String, String) {
    let manifest_path = bundle_dir.join("manifest.toml");
    if let Ok(content) = std::fs::read_to_string(&manifest_path) {
        if let Ok(val) = toml::from_str::<toml::Value>(&content) {
            let name = val
                .get("bundleName")
                .or_else(|| val.get("bundle_name"))
                .or_else(|| val.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let version = val
                .get("bundleVersion")
                .or_else(|| val.get("bundle_version"))
                .or_else(|| val.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or("0.0.0")
                .to_string();
            return (name, version);
        }
    }

    // Fallback: use directory name.
    let name = bundle_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    (name, "0.0.0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_nonexistent_bundle_has_errors() {
        let config = ValidationConfig {
            bundle_dir: PathBuf::from("/tmp/nonexistent_bundle_dir_99999"),
            engines: None,
        };
        let report = validate_bundle_full(&config);
        assert!(report.has_errors(), "nonexistent bundle should have errors");
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn validate_bundle_full_has_w003_for_valid_bundle() {
        // W003 is only emitted when the bundle loads but no engines are configured
        let bundle_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../..", "/address-book-bundle");
        let path = std::path::Path::new(bundle_dir);
        if !path.exists() {
            return; // skip if bundle not present
        }
        let config = ValidationConfig {
            bundle_dir: path.to_path_buf(),
            engines: None,
        };
        let report = validate_bundle_full(&config);
        assert!(report.has_warnings(), "should have W003 warning");
        let layer4 = &report.layers["syntax_verification"];
        assert!(
            layer4.results.iter().any(|r| r.error_code.as_deref() == Some("W003")),
            "Layer 4 should contain W003 warning"
        );
    }

    #[test]
    fn validate_address_book_bundle() {
        let bundle_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../..", "/address-book-bundle");
        let path = std::path::Path::new(bundle_dir);
        if !path.exists() {
            return; // skip if bundle not present
        }
        let config = ValidationConfig {
            bundle_dir: path.to_path_buf(),
            engines: None,
        };
        let report = validate_bundle_full(&config);
        // The address book bundle should not have errors (warnings are OK). (warnings are OK).
        assert!(
            !report.has_errors(),
            "address-book-bundle should pass validation.\nReport:\n{}",
            crate::validate_format::format_text(&report),
        );
        assert_eq!(report.bundle_name, "address-book");
        assert_eq!(report.bundle_version, "1.0.0");
    }

    #[test]
    fn read_bundle_meta_falls_back() {
        let (name, version) = read_bundle_meta(std::path::Path::new("/tmp/nonexistent_99999"));
        assert_eq!(name, "nonexistent_99999");
        assert_eq!(version, "0.0.0");
    }
}
