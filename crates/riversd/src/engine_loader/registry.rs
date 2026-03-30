//! Global engine registry — populated at startup, read on every task dispatch.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use super::loaded_engine::LoadedEngine;

/// Global engine registry — populated at startup, read on every task dispatch.
static ENGINE_REGISTRY: OnceLock<RwLock<HashMap<String, LoadedEngine>>> = OnceLock::new();

pub(super) fn registry() -> &'static RwLock<HashMap<String, LoadedEngine>> {
    ENGINE_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Get an engine by name from the global registry.
pub fn get_engine(name: &str) -> Option<std::sync::RwLockReadGuard<'static, HashMap<String, LoadedEngine>>> {
    let guard = registry().read().ok()?;
    if guard.contains_key(name) {
        Some(guard)
    } else {
        None
    }
}

/// Check if an engine is loaded.
pub fn is_engine_available(name: &str) -> bool {
    registry().read().ok()
        .map(|r| r.contains_key(name))
        .unwrap_or(false)
}

/// Execute a task on a named engine.
///
/// Acquires a read lock on the registry, finds the engine, and calls execute.
/// This function is designed to be called from `spawn_blocking`.
pub fn execute_on_engine(
    name: &str,
    ctx: &rivers_engine_sdk::SerializedTaskContext,
) -> Result<rivers_engine_sdk::SerializedTaskResult, String> {
    let guard = registry().read()
        .map_err(|_| "engine registry lock poisoned".to_string())?;
    let engine = guard.get(name)
        .ok_or_else(|| format!("engine '{}' not loaded", name))?;
    engine.execute(ctx)
}

/// List all loaded engine names.
pub fn loaded_engines() -> Vec<String> {
    registry().read().ok()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default()
}
