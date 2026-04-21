# Change Decision Log

Per CLAUDE.md Workflow rule 5: every decision during implementation is logged here with file, decision, spec reference, and resolution method. CB uses this as the reference baseline for drift detection — treat it as load-bearing.

---

## 2026-04-21 — TS pipeline Phase 1

### swc full-transform, not strip-only

**File:** `crates/riversd/src/process_pool/v8_config.rs`
**Decision:** Use `swc_core::ecma::transforms::typescript::typescript()` full transform, not `typescript::strip` (strip-only).
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §2.2` + Decision Log #1.
**Resolution:** Strip-only passes `enum`, `namespace`, and TC39 decorators through unchanged to V8, producing parse errors. Full transform lowers them to runtime JS. Unit tests `compile_typescript_lowers_enum` and `compile_typescript_lowers_namespace` verify the keywords do not survive into output.

### swc_core version correction: v0.90 → v64

**File:** `crates/riversd/Cargo.toml`
**Decision:** Pin `swc_core = "64"` instead of the spec-mandated `"0.90"`.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §2.1` (spec says `version = "0.90"`).
**Resolution:** The spec was authored against a stale version view. crates.io current is `swc_core` v64.0.0 at 2026-04-21; swc uses major-per-release versioning. v0.90 dependencies transitively import `swc_common-0.33` which calls `serde::__private`, a private module removed from modern `serde` and unavailable in this workspace — the v0.90 build fails with `unresolved import `serde::__private``. v64 is API-compatible with the spec's pseudocode (`parse_file_as_program`, `typescript(Config, Mark, Mark) -> impl Pass`, `to_code_default`). Spec §2.1 should be amended to `version = "64"` or expressed as `version = "*"`-with-tested-lower-bound during spec revision.

### Decorator lowering: parser-accepts, V8-executes (no swc lowering pass)

**File:** `crates/riversd/src/process_pool/v8_config.rs`
**Decision:** Parser accepts TC39 Stage 3 decorators (`TsSyntax { decorators: true }`) but no decorator-lowering pass runs. Decorators reach V8 as-is.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §2.3` ("TC39 Stage 3 decorators only").
**Resolution:** swc v64's `typescript::typescript()` pass does not include decorator lowering — decorator lowering lives in `swc_ecma_transforms_proposal::decorators`. The pinned V8 (v130) supports Stage 3 decorators natively under `--harmony-decorators`. Passing decorators through is both simpler and matches spec §2.3's "supports" wording. If a future runtime drops native Stage 3 decorator support, we re-add the `decorators(Config { legacy: false, .. })` pass between `typescript()` and `fixer()`. Test `compile_typescript_accepts_tc39_decorator_syntax` exercises parse-through.

### Bundle module cache lives in `riversd`, not `rivers-runtime`

**File:** `crates/riversd/src/process_pool/module_cache.rs` (population + global slot); `crates/rivers-runtime/src/module_cache.rs` (types only).
**Decision:** `CompiledModule` + `BundleModuleCache` types go in rivers-runtime (so they can be referenced by lower-level types later), but the population helpers (`compile_app_modules`, `populate_module_cache`) and the process-global slot (`install_module_cache`, `get_module_cache`) live in riversd.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §2.6–2.7`. The spec's Phase 10 plan says "Extend `crates/rivers-runtime/src/loader.rs:load_bundle()` to walk each app's `libraries/` subtree."
**Resolution:** `compile_typescript` depends on `swc_core` which is a riversd-only dependency; pulling swc into rivers-runtime would inflate every downstream crate's build surface (rivers-runtime is re-exported as a dylib in dynamic mode). Splitting types → rivers-runtime vs population → riversd keeps swc contained. The compile happens during `load_and_wire_bundle` in riversd, not inside `rivers_runtime::load_bundle`. Spec Phase 2 task 2.3 wording updated in the plan to reflect this — same effect, different layering.

### Module cache is a process-global `OnceCell<RwLock<Arc<_>>>`, not threaded through dispatch

**File:** `crates/riversd/src/process_pool/module_cache.rs`
**Decision:** Single static `MODULE_CACHE` rather than threading a cache reference through `execute_js_task` or `TaskContext`.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §3.4` (atomic hot reload).
**Resolution:** The cache is server-wide and immutable after load (swapped atomically on hot reload). Threading it through dispatch would mean changing `execute_js_task`'s signature and every caller — 10+ files. A global slot with a read-through `get_module_cache() -> Option<Arc<_>>` API covers the same semantics with a ~20-line module. RwLock inside OnceCell supports the atomic-replacement requirement; the Arc wrap keeps reads lock-free after the initial get.

### resolve_module_source falls back to disk read on cache miss

**File:** `crates/riversd/src/process_pool/v8_engine/execution.rs`
**Decision:** On cache miss in `resolve_module_source`, fall back to disk read + live compile with a `tracing::debug!` log instead of erroring.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §2.7` ("exhaustive upfront compilation").
**Resolution:** Strict spec compliance would error on cache miss. But during Phase 2 there are handlers outside `libraries/` (legacy, MCP-internal, etc.) whose modules are resolved by explicit paths set up in `resolve_handler_module`. A hard error would break these before Phases 4/5 land. The fallback is a defence-in-depth path with a debug log so operators can spot modules that should be moved into `libraries/`. Once Phase 4's module resolver lands and all handler modules are bundle-resident, the fallback can be promoted to `tracing::warn!` or an error.

### Source map generation deferred to Phase 6

**File:** `crates/riversd/src/process_pool/v8_config.rs`
**Decision:** Phase 1 emits via `to_code_default(cm, None, &program)` — no source map collection.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5.1` (source maps always on).
**Resolution:** Spec §5 is Phase 6 work in the plan. Phase 1's scope is the drop-in only. When Phase 6 lands we replace `to_code_default` with a manual `Emitter` + source-map-generating `JsWriter` and store the map in `CompiledModule.source_map` (defined in Phase 2). No behaviour regression during Phase 1–5 because stack traces currently report compiled-JS positions and will continue to.
