//! Parsed source-map cache — `Arc<swc_sourcemap::SourceMap>` per compiled
//! handler, invalidated on hot reload.
//!
//! Per `docs/arch/rivers-javascript-typescript-spec.md §5`. The raw v3 JSON
//! is already stored in `BundleModuleCache::CompiledModule.source_map`;
//! this module layers a lazy-parse cache on top so the
//! `PrepareStackTraceCallback` doesn't re-parse JSON on every exception.
//!
//! Invalidation: `install_module_cache` calls `clear_sourcemap_cache` to
//! match the atomic-swap semantics of the underlying `BundleModuleCache`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use once_cell::sync::OnceCell;
use swc_sourcemap::SourceMap;

use super::super::module_cache::get_module_cache;

static PARSED_SOURCEMAPS: OnceCell<RwLock<HashMap<PathBuf, Arc<SourceMap>>>> = OnceCell::new();

fn slot() -> &'static RwLock<HashMap<PathBuf, Arc<SourceMap>>> {
    PARSED_SOURCEMAPS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Resolve a parsed source map for the given absolute handler path.
///
/// Fast path: read-lock the cache, return a cloned `Arc` if present.
/// Slow path: fetch the raw JSON from `get_module_cache()`, parse it,
/// write-lock, insert, return.
///
/// Returns `None` if the handler isn't in the bundle cache (e.g., inline
/// `_source` test handlers with no on-disk presence) or if the JSON fails
/// to parse (logged; callers fall back to the unmapped frame).
pub fn get_or_parse(path: &Path) -> Option<Arc<SourceMap>> {
    // Fast path
    {
        let guard = slot().read().expect("sourcemap cache lock poisoned");
        if let Some(arc) = guard.get(path) {
            return Some(arc.clone());
        }
    }

    // Slow path — pull the JSON from the bundle module cache
    let module_cache = get_module_cache()?;
    let entry = module_cache.get(path)?;
    if entry.source_map.is_empty() {
        return None;
    }

    let parsed = match SourceMap::from_slice(entry.source_map.as_bytes()) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            tracing::warn!(
                target: "rivers.sourcemap",
                path = %path.display(),
                error = %e,
                "source map parse failed — frames for this module will not be remapped"
            );
            return None;
        }
    };

    let mut guard = slot().write().expect("sourcemap cache lock poisoned");
    // Double-check — another thread may have inserted between the read and
    // write locks.
    if let Some(existing) = guard.get(path) {
        return Some(existing.clone());
    }
    guard.insert(path.to_path_buf(), parsed.clone());
    Some(parsed)
}

/// Invalidate every cached parsed map.
///
/// Called from `install_module_cache` so a hot-reloaded bundle doesn't
/// serve stale source maps.
pub fn clear_sourcemap_cache() {
    if let Some(lock) = PARSED_SOURCEMAPS.get() {
        lock.write().expect("sourcemap cache lock poisoned").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_runtime::module_cache::{BundleModuleCache, CompiledModule};

    /// Minimal valid v3 source map — `mappings` is empty but structurally
    /// parseable by `swc_sourcemap`.
    const MINIMAL_V3: &str = r#"{"version":3,"sources":["test.ts"],"names":[],"mappings":"","file":"test.js"}"#;

    fn install_with_module(path: PathBuf) {
        let mut map = HashMap::new();
        map.insert(
            path.clone(),
            CompiledModule {
                source_path: path,
                compiled_js: String::new(),
                source_map: MINIMAL_V3.into(),
                imports: Vec::new(),
            },
        );
        super::super::super::module_cache::install_module_cache(BundleModuleCache::from_map(map));
    }

    // Both behaviours in one test: the global slot + the shared module_cache
    // make isolated-per-test runs race-prone under cargo's parallel execution.
    // Single serial test covers idempotence AND invalidation cleanly.
    #[test]
    fn sourcemap_cache_idempotence_and_invalidation() {
        clear_sourcemap_cache();
        let path = PathBuf::from("/sourcemap-cache-test/combined.ts");
        install_with_module(path.clone());
        // install_module_cache above invokes clear_sourcemap_cache via the
        // install-hook, so the parsed slot is empty at this point.

        // Idempotence: two calls return the same Arc.
        let first = get_or_parse(&path).expect("first parse");
        let second = get_or_parse(&path).expect("second parse after cache hit");
        assert!(
            Arc::ptr_eq(&first, &second),
            "second call must return the cached Arc"
        );

        // Invalidation: manual clear → re-parse yields a different Arc.
        clear_sourcemap_cache();
        let third = get_or_parse(&path).expect("third parse after clear");
        assert!(
            !Arc::ptr_eq(&first, &third),
            "after clear, the new Arc must be a fresh allocation"
        );
    }
}
