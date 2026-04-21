//! Bundle module cache population + global storage.
//!
//! Types (`CompiledModule`, `BundleModuleCache`) live in
//! `rivers-runtime::module_cache`. This module owns:
//!
//! - `populate_module_cache`: walks each app's `libraries/` subtree,
//!   compiles every `.ts` via `compile_typescript` and stores `.js` verbatim.
//! - `install_module_cache`: atomically swaps the process-global cache slot
//!   (supports spec §3.4 hot-reload semantics).
//! - `get_module_cache`: the V8 dispatch read path.
//!
//! Cross-crate layering: the spec plan originally envisioned this work inside
//! `rivers-runtime::loader::load_bundle`, but `compile_typescript` depends on
//! `swc_core` which `rivers-runtime` doesn't link against (it's a lower-level
//! facade crate). Keeping compilation in `riversd` avoids pulling swc into
//! the runtime crate's build surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use once_cell::sync::OnceCell;

use rivers_runtime::module_cache::{BundleModuleCache, CompiledModule};
use rivers_runtime::rivers_core_config::RiversError;
use rivers_runtime::LoadedBundle;

use super::v8_config::compile_typescript_with_imports;

/// Process-global cache slot. Installed after bundle load; swapped atomically
/// on hot reload.
static MODULE_CACHE: OnceCell<RwLock<Arc<BundleModuleCache>>> = OnceCell::new();

/// Install (or replace) the global module cache.
///
/// Spec §3.4: "Hot reload replaces the entire cache atomically."
pub fn install_module_cache(cache: BundleModuleCache) {
    let cell = MODULE_CACHE.get_or_init(|| RwLock::new(Arc::new(BundleModuleCache::default())));
    *cell.write().expect("module cache lock poisoned") = Arc::new(cache);
}

/// Read the current global module cache, if installed.
///
/// Returns `None` before any bundle has loaded — callers MUST handle that
/// case (e.g., via `resolve_module_source`'s inline `_source` fallback path
/// for unit tests that inject source directly).
pub fn get_module_cache() -> Option<Arc<BundleModuleCache>> {
    MODULE_CACHE
        .get()
        .map(|lock| lock.read().expect("module cache lock poisoned").clone())
}

/// Walk `{app_dir}/libraries/` and compile every `.ts` + store every `.js`.
///
/// Canonicalises each path to ensure the V8 resolve callback's lookups match.
/// Rejects `.tsx` unconditionally (spec §2.5) — the rejection message flows
/// out of `compile_typescript` itself.
///
/// Errors are fail-fast: the first compile failure aborts the walk and the
/// entire bundle load (spec §2.5).
pub fn compile_app_modules(
    app_name: &str,
    app_dir: &Path,
    acc: &mut HashMap<PathBuf, CompiledModule>,
) -> Result<(), RiversError> {
    let libraries = app_dir.join("libraries");
    if !libraries.is_dir() {
        // Apps without a libraries/ tree (pure TOML apps) are valid.
        return Ok(());
    }

    for entry in walk_dir(&libraries)? {
        let abs = entry.canonicalize().map_err(|e| {
            RiversError::Io(format!(
                "cannot canonicalise handler path {}: {}",
                entry.display(),
                e
            ))
        })?;

        let ext = abs
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());

        match ext.as_deref() {
            Some("ts") => {
                let source = std::fs::read_to_string(&abs).map_err(|e| {
                    RiversError::Io(format!(
                        "cannot read handler {}: {}",
                        abs.display(),
                        e
                    ))
                })?;
                let filename = abs.to_string_lossy().to_string();
                let (compiled_js, imports, source_map) =
                    compile_typescript_with_imports(&source, &filename).map_err(|e| {
                        RiversError::Config(format!(
                            "TypeScript compile error in app '{app_name}', file {filename}: {e:?}"
                        ))
                    })?;
                acc.insert(
                    abs.clone(),
                    CompiledModule {
                        source_path: abs,
                        compiled_js,
                        source_map,
                        imports,
                    },
                );
            }
            Some("tsx") => {
                // Spec §2.5: explicit rejection; `compile_typescript` also
                // rejects but catching it early produces a clearer error.
                return Err(RiversError::Config(format!(
                    "JSX/TSX is not supported in Rivers v1: {}",
                    abs.display()
                )));
            }
            Some("js") => {
                let source = std::fs::read_to_string(&abs).map_err(|e| {
                    RiversError::Io(format!(
                        "cannot read handler {}: {}",
                        abs.display(),
                        e
                    ))
                })?;
                acc.insert(
                    abs.clone(),
                    CompiledModule {
                        source_path: abs.clone(),
                        compiled_js: source,
                        source_map: String::new(),
                        // Plain .js imports would need a separate parser pass.
                        // Phase 3 cycle detection operates on the post-transform
                        // TS AST; .js handlers in the canary today don't use
                        // multi-module imports. Leave empty until needed.
                        imports: Vec::new(),
                    },
                );
            }
            _ => {
                // Non-source files (JSON, schemas, markdown, etc.) are ignored.
            }
        }
    }

    Ok(())
}

/// Compile every handler module across a loaded bundle.
///
/// Spec §2.7: exhaustive upfront compilation — every `.ts` under every
/// app's `libraries/` is compiled regardless of whether any view references
/// it. Any single compile failure aborts the whole bundle load.
///
/// Spec §3.5: after all modules compile, per-app circular-import detection
/// runs; any cycle aborts bundle load with a formatted path chain.
pub fn populate_module_cache(bundle: &LoadedBundle) -> Result<BundleModuleCache, RiversError> {
    let mut map = HashMap::new();
    for app in &bundle.apps {
        let app_name = app.manifest.entry_point.as_deref().unwrap_or("unknown");
        let before = map.len();
        compile_app_modules(app_name, &app.app_dir, &mut map)?;
        let app_paths: Vec<PathBuf> = map
            .keys()
            .skip(before)
            .cloned()
            .collect();
        check_cycles_for_app(app_name, &app.app_dir, &app_paths, &map)?;
    }
    Ok(BundleModuleCache::from_map(map))
}

/// Detect circular imports within a single app.
///
/// Builds a directed graph from every compiled module's `imports` field
/// (raw specifiers resolved against the referrer's directory) and runs DFS
/// cycle detection. On a cycle, returns a `RiversError::Config` whose
/// message matches spec §3.5:
///
/// ```text
/// circular import detected in {app}:
///   libraries/handlers/a.ts
///     → libraries/shared/b.ts
///     → libraries/helpers/c.ts
///     → libraries/handlers/a.ts
/// ```
fn check_cycles_for_app(
    app_name: &str,
    app_dir: &Path,
    app_paths: &[PathBuf],
    cache: &HashMap<PathBuf, CompiledModule>,
) -> Result<(), RiversError> {
    // Build adjacency list: module → resolved import paths (only those that
    // resolve inside the same app's cache).
    let mut graph: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for module_path in app_paths {
        let module = match cache.get(module_path) {
            Some(m) => m,
            None => continue,
        };
        let parent = module_path.parent().unwrap_or_else(|| Path::new("/"));
        let mut edges = Vec::new();
        for spec in &module.imports {
            // Only relative imports are cycle candidates. Bare and absolute
            // specifiers are handled (rejected) by the Phase 4 resolver.
            if !(spec.starts_with("./") || spec.starts_with("../")) {
                continue;
            }
            let joined = parent.join(spec);
            // Canonicalise lazily — if the file doesn't exist on disk, skip:
            // the Phase 4 resolver will surface the missing-import error at
            // dispatch time. Here we only care about resolvable edges.
            if let Ok(abs) = joined.canonicalize() {
                if cache.contains_key(&abs) {
                    edges.push(abs);
                }
            }
        }
        graph.insert(module_path.clone(), edges);
    }

    // DFS: colour 0 = unvisited, 1 = on current path (gray), 2 = fully
    // explored (black). A back-edge to a gray node is a cycle.
    let mut color: HashMap<&PathBuf, u8> = graph.keys().map(|k| (k, 0u8)).collect();
    let mut path: Vec<&PathBuf> = Vec::new();

    for start in graph.keys() {
        if color.get(start).copied() != Some(0) {
            continue;
        }
        if let Some(cycle) = dfs_find_cycle(start, &graph, &mut color, &mut path) {
            let rel = |p: &Path| -> String {
                p.strip_prefix(app_dir)
                    .map(|r| r.to_string_lossy().to_string())
                    .unwrap_or_else(|_| p.to_string_lossy().to_string())
            };
            let mut msg = format!("circular import detected in {app_name}:\n");
            for (i, p) in cycle.iter().enumerate() {
                if i == 0 {
                    msg.push_str(&format!("  {}\n", rel(p)));
                } else {
                    msg.push_str(&format!("    → {}\n", rel(p)));
                }
            }
            return Err(RiversError::Config(msg.trim_end().to_string()));
        }
    }

    Ok(())
}

fn dfs_find_cycle<'a>(
    node: &'a PathBuf,
    graph: &'a HashMap<PathBuf, Vec<PathBuf>>,
    color: &mut HashMap<&'a PathBuf, u8>,
    path: &mut Vec<&'a PathBuf>,
) -> Option<Vec<PathBuf>> {
    color.insert(node, 1); // gray
    path.push(node);
    if let Some(edges) = graph.get(node) {
        for next in edges {
            let c = color.get(next).copied().unwrap_or(0);
            if c == 0 {
                if let Some(cycle) = dfs_find_cycle(next, graph, color, path) {
                    return Some(cycle);
                }
            } else if c == 1 {
                // Back-edge → cycle. Slice the current path from `next` back
                // to the top, then append `next` again to show the loop close.
                let start_idx = path.iter().position(|p| *p == next).unwrap_or(0);
                let mut cycle: Vec<PathBuf> =
                    path[start_idx..].iter().map(|p| (*p).clone()).collect();
                cycle.push(next.clone());
                return Some(cycle);
            }
        }
    }
    path.pop();
    color.insert(node, 2); // black
    None
}

/// Recursive directory walk yielding regular files.
fn walk_dir(root: &Path) -> Result<Vec<PathBuf>, RiversError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| {
            RiversError::Io(format!("cannot read directory {}: {}", dir.display(), e))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                RiversError::Io(format!("read_dir entry error in {}: {}", dir.display(), e))
            })?;
            let path = entry.path();
            let ft = entry
                .file_type()
                .map_err(|e| RiversError::Io(format!("file_type {}: {}", path.display(), e)))?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, contents: &str) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, contents).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn walks_ts_and_js_skips_other() {
        let dir = TempDir::new().unwrap();
        let app = dir.path();
        let ts = write(
            app,
            "libraries/handlers/a.ts",
            "function handler(ctx: any) { return { ok: true }; }",
        );
        let js = write(
            app,
            "libraries/handlers/b.js",
            "function handler(ctx) { return { ok: true }; }",
        );
        write(app, "libraries/schemas/x.json", "{}");

        let mut map = HashMap::new();
        compile_app_modules("test-app", app, &mut map).unwrap();
        assert_eq!(map.len(), 2, "only .ts and .js cached: {map:?}");
        assert!(map.contains_key(&ts), "ts module present");
        assert!(map.contains_key(&js), "js module present");

        let ts_entry = map.get(&ts).unwrap();
        assert!(
            !ts_entry.compiled_js.contains(": any"),
            "types stripped: {}",
            ts_entry.compiled_js
        );

        let js_entry = map.get(&js).unwrap();
        assert!(
            js_entry.compiled_js.contains("function handler(ctx)"),
            "js preserved verbatim"
        );
    }

    #[test]
    fn rejects_tsx_at_walk_time() {
        let dir = TempDir::new().unwrap();
        let app = dir.path();
        write(app, "libraries/handlers/Component.tsx", "const x = 1;");

        let mut map = HashMap::new();
        let err = compile_app_modules("test-app", app, &mut map).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("JSX/TSX is not supported"), "got: {msg}");
        assert!(msg.contains("Component.tsx"), "path in error: {msg}");
    }

    #[test]
    fn missing_libraries_dir_is_ok() {
        let dir = TempDir::new().unwrap();
        let mut map = HashMap::new();
        compile_app_modules("test-app", dir.path(), &mut map).unwrap();
        assert!(map.is_empty(), "no libraries/ → empty cache");
    }

    #[test]
    fn fails_fast_on_compile_error() {
        let dir = TempDir::new().unwrap();
        let app = dir.path();
        write(app, "libraries/handlers/good.ts", "const x = 1;");
        write(app, "libraries/handlers/broken.ts", "function ((((");

        let mut map = HashMap::new();
        let err = compile_app_modules("test-app", app, &mut map).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("TypeScript compile error"), "got: {msg}");
        assert!(msg.contains("broken.ts"), "broken file named: {msg}");
    }

    fn make_bundle_with_app(app_dir: PathBuf) -> LoadedBundle {
        use rivers_runtime::bundle::{AppManifest, BundleManifest, ResourcesConfig};
        use rivers_runtime::LoadedApp;

        LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "probe-bundle".into(),
                bundle_version: "0.0.0".into(),
                source: None,
                apps: vec!["probe".into()],
            },
            apps: vec![LoadedApp {
                manifest: AppManifest {
                    app_name: "probe".into(),
                    description: None,
                    version: None,
                    app_type: "app-service".into(),
                    app_id: "00000000-0000-0000-0000-000000000000".into(),
                    entry_point: Some("probe".into()),
                    app_entry_point: None,
                    source: None,
                    spa: None,
                    init: None,
                },
                resources: ResourcesConfig::default(),
                config: Default::default(),
                app_dir,
            }],
        }
    }

    #[test]
    fn cycle_detection_two_module_loop() {
        let dir = TempDir::new().unwrap();
        let app = dir.path().to_path_buf();
        write(
            &app,
            "libraries/handlers/a.ts",
            "import { b } from \"./b.ts\";\nexport function a() { return b(); }",
        );
        write(
            &app,
            "libraries/handlers/b.ts",
            "import { a } from \"./a.ts\";\nexport function b() { return a(); }",
        );
        let bundle = make_bundle_with_app(app);
        let err = populate_module_cache(&bundle).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("circular import detected"), "got: {msg}");
        assert!(msg.contains("a.ts"), "a.ts in cycle msg: {msg}");
        assert!(msg.contains("b.ts"), "b.ts in cycle msg: {msg}");
    }

    #[test]
    fn cycle_detection_three_module_loop() {
        let dir = TempDir::new().unwrap();
        let app = dir.path().to_path_buf();
        write(
            &app,
            "libraries/handlers/a.ts",
            "import \"./b.ts\";\nexport const a = 1;",
        );
        write(
            &app,
            "libraries/handlers/b.ts",
            "import \"./c.ts\";\nexport const b = 1;",
        );
        write(
            &app,
            "libraries/handlers/c.ts",
            "import \"./a.ts\";\nexport const c = 1;",
        );
        let bundle = make_bundle_with_app(app);
        let err = populate_module_cache(&bundle).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("circular import detected"), "got: {msg}");
        for f in &["a.ts", "b.ts", "c.ts"] {
            assert!(msg.contains(f), "{f} in cycle msg: {msg}");
        }
    }

    #[test]
    fn cycle_detection_self_import() {
        let dir = TempDir::new().unwrap();
        let app = dir.path().to_path_buf();
        // Side-effect import (no named binding) so swc cannot tree-shake it
        // away — this genuinely reaches the runtime import graph.
        write(
            &app,
            "libraries/handlers/loop.ts",
            "import \"./loop.ts\";\nexport const z = 1;",
        );
        let bundle = make_bundle_with_app(app);
        let err = populate_module_cache(&bundle).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("circular import detected"), "got: {msg}");
        assert!(msg.contains("loop.ts"), "self-loop filename: {msg}");
    }

    #[test]
    fn cycle_detection_acyclic_tree_ok() {
        let dir = TempDir::new().unwrap();
        let app = dir.path().to_path_buf();
        write(
            &app,
            "libraries/handlers/main.ts",
            "import { helper } from \"./util.ts\";\nexport function main() { return helper(); }",
        );
        write(
            &app,
            "libraries/handlers/util.ts",
            "export function helper() { return 42; }",
        );
        let bundle = make_bundle_with_app(app);
        let cache = populate_module_cache(&bundle).expect("acyclic bundle loads");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn cycle_detection_type_only_imports_are_not_cycles() {
        // Spec §3.5 operates on runtime imports; type-only imports are
        // erased by the swc typescript transform before extract_imports runs.
        let dir = TempDir::new().unwrap();
        let app = dir.path().to_path_buf();
        write(
            &app,
            "libraries/handlers/a.ts",
            "import type { X } from \"./b.ts\";\nexport const a: X = 1 as X;",
        );
        write(
            &app,
            "libraries/handlers/b.ts",
            "import type { A } from \"./a.ts\";\nexport type X = A;",
        );
        let bundle = make_bundle_with_app(app);
        // No runtime imports → no cycle. swc erased both `import type` lines.
        let cache = populate_module_cache(&bundle).expect("type-only: no cycle");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn install_and_get_roundtrip() {
        // This test mutates the process-global slot; keep assertions scoped
        // so failures give a clear signal.
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("/tmp/x.ts"),
            CompiledModule {
                source_path: PathBuf::from("/tmp/x.ts"),
                compiled_js: "const x = 1;".into(),
                source_map: String::new(),
                imports: Vec::new(),
            },
        );
        let cache = BundleModuleCache::from_map(map);
        install_module_cache(cache);

        let got = get_module_cache().expect("installed");
        assert!(
            got.get(Path::new("/tmp/x.ts")).is_some(),
            "cache reinstall reachable via get"
        );

        // Swap with an empty cache — verify atomic replacement (not append).
        install_module_cache(BundleModuleCache::default());
        let got = get_module_cache().unwrap();
        assert!(got.is_empty(), "hot reload replaces, doesn't merge");
    }
}
