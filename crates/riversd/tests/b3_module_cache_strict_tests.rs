//! B3 / P1-8: module cache miss must hard-fail in production.
//!
//! These tests live in their own integration binary (one process per binary)
//! so installing into the process-global `MODULE_CACHE` slot does not bleed
//! into the broader v8_bridge_tests suite — those tests construct ad-hoc
//! handler files in /tmp and rely on the no-cache-installed disk-read path.
//!
//! See `crates/riversd/src/process_pool/module_cache.rs` for the
//! `ModuleCacheMode` policy and `RIVERS_DEV_MODULE_CACHE` opt-out.

use std::collections::HashMap;
use std::path::PathBuf;

use rivers_runtime::module_cache::{BundleModuleCache, CompiledModule};
use riversd::process_pool::{
    module_cache::{arm_production_strict, install_module_cache},
    Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskKind,
};

/// Dispatch a handler whose entry-point path is NOT registered in the
/// installed module cache. In production mode (the default) the dispatch
/// must surface a `MODULE_NOT_REGISTERED` error and never reach disk +
/// live-compile fallback.
#[tokio::test]
async fn b3_production_mode_cache_miss_hard_fails() {
    // Install a non-empty cache that does NOT contain the dispatch's
    // entry-point. A populated cache (vs the default empty one) tightens
    // the assertion: even with cache infrastructure live, a missing module
    // must error rather than silently fall back.
    let registered = PathBuf::from("/tmp/__rivers_b3_registered_handler__.js");
    let mut map = HashMap::new();
    map.insert(
        registered.clone(),
        CompiledModule {
            source_path: registered,
            compiled_js: "function handler(ctx) { return { ok: true }; }".into(),
            source_map: String::new(),
            imports: Vec::new(),
        },
    );
    install_module_cache(BundleModuleCache::from_map(map));
    // Arm the production-strict gate, mirroring what the bundle loader does
    // after a real bundle install. Tests in OTHER binaries that don't arm
    // keep the legacy disk-fallback behaviour.
    arm_production_strict();

    // Choose a path GUARANTEED not to be in the cache. Importantly, the file
    // CAN exist on disk — the test verifies the resolver does NOT consult
    // disk in production. Use a unique path under temp so an unrelated test
    // never collides.
    let unregistered = std::env::temp_dir()
        .join(format!("__rivers_b3_unregistered_{}.js", std::process::id()));
    std::fs::write(&unregistered, "function handler(ctx) { return { reached: true }; }").unwrap();

    let mgr = ProcessPoolManager::from_config(&HashMap::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: unregistered.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("b3-strict-test".into())
        .app_id("test-app-uuid".into())
        .node_id("test-node-1".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap();

    let result = mgr.dispatch("default", ctx).await;
    let _ = std::fs::remove_file(&unregistered);

    let err = result.expect_err("B3: cache miss in production must error, not silently fall back");
    let msg = err.to_string();
    assert!(
        msg.contains("MODULE_NOT_REGISTERED"),
        "B3: error must carry the MODULE_NOT_REGISTERED code, got: {msg}"
    );
    assert!(
        msg.contains("not in the validated bundle module cache"),
        "B3: error must explain the boundary, got: {msg}"
    );
    assert!(
        msg.contains("RIVERS_DEV_MODULE_CACHE=permissive"),
        "B3: error must point operators at the dev escape hatch, got: {msg}"
    );

    // The handler body's `reached: true` sentinel must NOT appear anywhere —
    // proves the disk + live-compile path was never taken.
    assert!(
        !msg.contains("reached"),
        "B3: production must not have read the file from disk, got: {msg}"
    );
}

/// `ctx.args["_source"]` always bypasses the cache (tests, dynamic dispatch).
/// This is intentional and documented in `resolve_module_source` — verify
/// production-strict mode does NOT regress that escape hatch.
#[tokio::test]
async fn b3_inline_source_bypasses_cache_in_production() {
    // Install a cache (intentionally empty) and arm production-strict so
    // we're in the same state as a deployed riversd after bundle load.
    install_module_cache(BundleModuleCache::default());
    arm_production_strict();

    let mgr = ProcessPoolManager::from_config(&HashMap::new());
    // Use a path that's not on disk; with `_source` injected the resolver
    // never touches the filesystem or the cache.
    let phantom = "/nonexistent/__rivers_b3_inline__.js";
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: phantom.into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({
            "_source": "function handler(ctx) { ctx.resdata = { inline: true }; }",
        }))
        .trace_id("b3-inline-test".into())
        .app_id("test-app-uuid".into())
        .node_id("test-node-1".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap();

    let result = mgr.dispatch("default", ctx).await.expect("inline _source path runs");
    assert_eq!(result.value["inline"], true, "inline source must execute");
}
