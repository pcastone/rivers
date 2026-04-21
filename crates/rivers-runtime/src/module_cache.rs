//! Bundle module cache — pre-compiled handler source, keyed by absolute path.
//!
//! Per `docs/arch/rivers-javascript-typescript-spec.md` §2.6, §2.7, §3.4:
//! every `.ts` file under every app's `libraries/` subtree is compiled at
//! bundle load time via the swc full-transform pipeline. The compiled JS and
//! (Phase 6) source map are stored here, keyed by canonicalised absolute path
//! so the V8 resolve callback (spec §3.6) performs a lookup instead of a live
//! compilation.
//!
//! `.js` files are stored verbatim (no compilation). `.tsx` is rejected at
//! population time (spec §2.5).
//!
//! The cache is immutable for the lifetime of a loaded bundle. Hot reload
//! replaces the entire cache atomically (spec §3.4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One entry in the bundle module cache.
///
/// Fields map 1:1 to spec §3.4 `CompiledModule`. `source_map` is populated by
/// Phase 6; Phase 2 stores an empty string.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Absolute path on disk of the original source file.
    pub source_path: PathBuf,
    /// swc-compiled JavaScript (or verbatim source for `.js` files).
    pub compiled_js: String,
    /// Source map (JSON string). Empty until Phase 6 lands.
    pub source_map: String,
}

/// Map of absolute handler source paths to their compiled JS.
///
/// Internal storage is an `Arc<HashMap>` so clones are cheap — dispatch paths
/// may need to pass the cache across V8 callback boundaries.
#[derive(Debug, Default, Clone)]
pub struct BundleModuleCache {
    modules: Arc<HashMap<PathBuf, CompiledModule>>,
}

impl BundleModuleCache {
    /// Build a cache from a pre-populated map.
    pub fn from_map(modules: HashMap<PathBuf, CompiledModule>) -> Self {
        Self { modules: Arc::new(modules) }
    }

    /// Look up a compiled module by its absolute path.
    ///
    /// The caller is expected to canonicalise the path before lookup —
    /// `PathBuf::canonicalize` or an equivalent invariant-preserving step.
    pub fn get(&self, abs_path: &Path) -> Option<&CompiledModule> {
        self.modules.get(abs_path)
    }

    /// Iterate every cached module. Order is HashMap-arbitrary.
    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &CompiledModule)> {
        self.modules.iter()
    }

    /// Number of cached modules.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// True when no modules have been cached.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip_by_canonical_path() {
        let mut map = HashMap::new();
        let path = PathBuf::from("/app/libraries/handlers/orders.ts");
        map.insert(
            path.clone(),
            CompiledModule {
                source_path: path.clone(),
                compiled_js: "function h(ctx){}".into(),
                source_map: String::new(),
            },
        );
        let cache = BundleModuleCache::from_map(map);
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());
        let hit = cache.get(&path).expect("cache hit");
        assert_eq!(hit.compiled_js, "function h(ctx){}");
        assert!(hit.source_map.is_empty(), "Phase 2 stores no source map");
    }

    #[test]
    fn cache_miss_on_unknown_path() {
        let cache = BundleModuleCache::default();
        assert!(cache.get(Path::new("/nonexistent")).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_clone_is_cheap_shared_arc() {
        let cache = BundleModuleCache::default();
        let clone = cache.clone();
        // Same Arc backing → same address.
        assert!(Arc::ptr_eq(&cache.modules, &clone.modules));
    }
}
