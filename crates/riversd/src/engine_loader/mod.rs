//! Dynamic engine loader — loads V8/Wasmtime engine shared libraries at startup.
//!
//! Scans the `[engines].dir` directory for `librivers_*.dylib` (macOS) or
//! `librivers_*.so` (Linux), loads each via `libloading`, checks ABI version,
//! and initializes with host callback function pointers.

mod loaded_engine;
mod registry;
mod loader;
mod host_context;
mod host_callbacks;

pub use loaded_engine::LoadedEngine;
pub use registry::{get_engine, is_engine_available, execute_on_engine, loaded_engines};
pub use loader::{EngineLoadResult, load_engines};
pub use host_context::{set_host_context, set_host_keystore, set_ddl_whitelist, set_app_id_map, build_host_callbacks, ddl_whitelist, app_id_for_entry_point};
