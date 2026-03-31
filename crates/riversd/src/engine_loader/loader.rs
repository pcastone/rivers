//! Engine loading — scans directories for engine shared libraries and loads them.

use std::path::Path;

use rivers_engine_sdk::{ENGINE_ABI_VERSION, HostCallbacks};

use super::loaded_engine::LoadedEngine;
use super::registry::registry;

/// Load result for a single engine.
#[derive(Debug)]
pub enum EngineLoadResult {
    /// Engine loaded successfully.
    Success {
        /// Engine name (e.g. "v8", "wasm").
        name: String,
        /// Filesystem path to the shared library.
        path: String,
    },
    /// Engine failed to load.
    Failed {
        /// Filesystem path to the shared library.
        path: String,
        /// Reason for the failure.
        reason: String,
    },
}

/// Scan a directory for engine shared libraries and load them.
///
/// Looks for files matching `librivers_v8.*` and `librivers_wasm.*`.
/// Each is loaded, ABI-checked, and initialized.
pub fn load_engines(lib_dir: &Path, callbacks: &HostCallbacks) -> Vec<EngineLoadResult> {
    let mut results = Vec::new();

    if !lib_dir.is_dir() {
        tracing::info!(dir = %lib_dir.display(), "engine lib directory not found — no engines loaded");
        return results;
    }

    let entries = match std::fs::read_dir(lib_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(dir = %lib_dir.display(), error = %e, "failed to read engine lib directory");
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        // Match librivers_v8.* or librivers_wasm.*
        let engine_name = if file_name.starts_with("librivers_v8") {
            "v8"
        } else if file_name.starts_with("librivers_wasm") {
            "wasm"
        } else {
            continue; // Not an engine library
        };

        let path_str = path.display().to_string();
        match load_single_engine(&path, engine_name, callbacks) {
            Ok(engine) => {
                tracing::info!(engine = engine_name, path = %path_str, "engine loaded");
                let mut registry = registry().write().unwrap();
                registry.insert(engine_name.to_string(), engine);
                results.push(EngineLoadResult::Success {
                    name: engine_name.to_string(),
                    path: path_str,
                });
            }
            Err(reason) => {
                tracing::warn!(engine = engine_name, path = %path_str, reason = %reason, "engine load failed");
                results.push(EngineLoadResult::Failed {
                    path: path_str,
                    reason,
                });
            }
        }
    }

    results
}

/// Load a single engine shared library.
fn load_single_engine(
    path: &Path,
    name: &str,
    _callbacks: &HostCallbacks,
) -> Result<LoadedEngine, String> {
    // Load the library
    let lib = unsafe {
        libloading::Library::new(path)
            .map_err(|e| format!("dlopen failed: {}", e))?
    };

    // Check ABI version
    let abi_version: u32 = unsafe {
        let func: libloading::Symbol<unsafe extern "C" fn() -> u32> = lib
            .get(b"_rivers_engine_abi_version")
            .map_err(|e| format!("missing _rivers_engine_abi_version: {}", e))?;
        func()
    };

    if abi_version != ENGINE_ABI_VERSION {
        return Err(format!(
            "ABI version mismatch: engine has {}, expected {}",
            abi_version, ENGINE_ABI_VERSION
        ));
    }

    // Resolve execute function
    let execute_fn = unsafe {
        let func: libloading::Symbol<
            unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32
        > = lib
            .get(b"_rivers_engine_execute")
            .map_err(|e| format!("missing _rivers_engine_execute: {}", e))?;
        *func
    };

    // Resolve cancel function (optional)
    let cancel_fn = unsafe {
        lib.get::<unsafe extern "C" fn(usize) -> i32>(b"_rivers_engine_cancel")
            .ok()
            .map(|f| *f)
    };

    // Call init (optional)
    unsafe {
        if let Ok(init_fn) = lib.get::<unsafe extern "C" fn() -> i32>(b"_rivers_engine_init") {
            let result = init_fn();
            if result != 0 {
                return Err(format!("_rivers_engine_init returned {}", result));
            }
        }
    }

    Ok(LoadedEngine {
        name: name.to_string(),
        _library: lib,
        execute_fn,
        cancel_fn,
    })
}
