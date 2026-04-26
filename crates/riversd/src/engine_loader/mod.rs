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
mod dyn_transaction_map;

pub use loaded_engine::LoadedEngine;
pub use registry::{get_engine, is_engine_available, execute_on_engine, loaded_engines};
pub use loader::{EngineLoadResult, load_engines};
pub use host_context::{set_host_context, set_host_keystore, set_ddl_whitelist, set_app_id_map, build_host_callbacks, ddl_whitelist, app_id_for_entry_point};

/// Shared host-callback timeout budget. See
/// `host_context::HOST_CALLBACK_TIMEOUT_MS`. Re-exported at the engine_loader
/// boundary so V8 (`process_pool/v8_engine`) and the dyn-engine cdylib path
/// share a single source of truth.
pub(crate) use host_context::HOST_CALLBACK_TIMEOUT_MS;
