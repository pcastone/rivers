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
/// Runtime-constructed DataViews for internal/synthetic endpoints.
pub mod pseudo_dataview;
/// JSON-schema system — field types, validation, and driver attributes.
pub mod schema;
/// Two-tier (L1 in-memory / L2 storage-engine) DataView result cache.
pub mod tiered_cache;
/// Config validation — structural and cross-reference checks.
pub mod validate;
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
pub use dataview_engine::{DataViewExecutor, DataViewRegistry};
pub use view::ApiViewConfig;
