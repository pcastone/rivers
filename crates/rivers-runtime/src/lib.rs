// Re-export upstream crates so consumers link against rivers-runtime only.
// In static mode this is an rlib facade; in dynamic mode it becomes a dylib.
pub use rivers_core;
pub use rivers_core_config;
pub use rivers_driver_sdk;
pub use rivers_engine_sdk;

pub mod process_pool;
pub mod bundle;
pub mod datasource;
pub mod dataview;
pub mod dataview_engine;
pub mod env_override;
pub mod loader;
pub mod pseudo_dataview;
pub mod schema;
pub mod tiered_cache;
pub mod validate;
pub mod view;

pub use bundle::{app_config_schema, bundle_manifest_schema, AppConfig, AppManifest, BundleManifest, KeystoreDataConfig, ResourceKeystore, ResourcesConfig};
pub use datasource::DatasourceConfig;
pub use dataview::{DataViewConfig, DataViewEngine, DataViewParameterConfig};
pub use env_override::apply_environment_overrides;
pub use loader::{load_bundle, load_server_config, LoadedApp, LoadedBundle};
pub use pseudo_dataview::{DatasourceBuilder, PseudoDataView, PseudoDataViewError};
pub use validate::{validate_app_config, validate_bundle, validate_known_drivers, validate_server_config};
pub use dataview_engine::{DataViewExecutor, DataViewRegistry};
pub use view::ApiViewConfig;
