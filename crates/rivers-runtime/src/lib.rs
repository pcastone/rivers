//! Rivers Runtime — facade crate and application-level types.
//!
//! In **static mode** this is an `rlib` that re-exports the four upstream
//! crates (`rivers-core`, `rivers-core-config`, `rivers-driver-sdk`,
//! `rivers-engine-sdk`). In **dynamic mode** it becomes the single Rust
//! `dylib` shared by `riversd`, engines, and plugins.
//!
//! Beyond re-exports, this crate owns the application-layer types:
//! bundle/app manifests, datasource and DataView configuration, the schema
//! system, config validation, TOML loading, environment overrides, the
//! tiered DataView cache, and the ProcessPool bridge types.

#![warn(missing_docs)]

// Re-export upstream crates so consumers link against rivers-runtime only.
// In static mode this is an rlib facade; in dynamic mode it becomes a dylib.
pub use rivers_core;
pub use rivers_core_config;
pub use rivers_driver_sdk;
pub use rivers_engine_sdk;

/// ProcessPool shared types — `TaskContext`, `TaskResult`, `Worker` trait.
pub mod process_pool;
/// Bundle and app manifest types (`BundleManifest`, `AppManifest`).
pub mod bundle;
/// Datasource configuration (`DatasourceConfig`, `PoolConfig`, etc.).
pub mod datasource;
/// DataView configuration and engine trait.
pub mod dataview;
/// DataView execution facade — request dispatch and error types.
pub mod dataview_engine;
/// Environment-variable overrides applied at startup.
pub mod env_override;
/// TOML file loading for server config and app bundles.
pub mod loader;
/// Pre-compiled handler source cache (bundle load time — spec §3.4).
pub mod module_cache;
/// Runtime-constructed DataViews for internal/synthetic endpoints.
pub mod pseudo_dataview;
/// JSON-schema system — field types, validation, and driver attributes.
pub mod schema;
/// Two-tier (L1 in-memory / L2 storage-engine) DataView result cache.
pub mod tiered_cache;
/// Config validation — structural and cross-reference checks.
pub mod validate;
/// Bundle validation pipeline — result types, error codes, report builder.
pub mod validate_result;
/// Bundle validation pipeline — text and JSON output formatters, Levenshtein helper.
pub mod validate_format;
/// Bundle validation pipeline — Layer 1: structural TOML validation.
pub mod validate_structural;
/// Layer 2 — Resource existence validation (files, directories, config files).
pub mod validate_existence;
/// Bundle validation pipeline — Layer 3: logical cross-reference checks.
pub mod validate_crossref;
/// Bundle validation pipeline — engine dylib FFI loading for Layer 4.
pub mod validate_engine;
/// Bundle validation pipeline — Layer 4: syntax verification (schemas, imports, compile checks).
pub mod validate_syntax;
/// Full validation pipeline — orchestrates Layers 1-4 for `riverpackage validate`.
pub mod validate_pipeline;
/// API view (endpoint) configuration types.
pub mod view;
/// Rivers home directory and config discovery.
pub mod home;

pub use bundle::{app_config_schema, bundle_manifest_schema, AppConfig, AppManifest, BundleManifest, KeystoreDataConfig, ResourceKeystore, ResourcesConfig};
pub use datasource::DatasourceConfig;
pub use dataview::{DataViewConfig, DataViewEngine, DataViewParameterConfig};
pub use env_override::apply_environment_overrides;
pub use loader::{load_bundle, load_server_config, LoadedApp, LoadedBundle};
pub use pseudo_dataview::{DatasourceBuilder, PseudoDataView, PseudoDataViewError};
pub use validate::{validate_app_config, validate_bundle, validate_known_drivers, validate_server_config};
pub use dataview_engine::{DataViewError, DataViewExecutor, DataViewRegistry};
pub use view::ApiViewConfig;
pub use validate_result::{ValidationReport, ValidationResult, ValidationStatus, ValidationSeverity, ValidationSummary, LayerResults, error_codes};
pub use validate_format::{format_text, format_json, levenshtein_distance, did_you_mean, suggest_key};
pub use validate_structural::validate_structural;
pub use validate_crossref::validate_crossref;
pub use validate_existence::validate_existence;
pub use validate_pipeline::{validate_bundle_full, validate_bundle_live, ValidationConfig, LockBoxChecker, ServiceHealthChecker};
pub use validate_engine::{EngineConfig, EngineHandles, EngineHandle};
