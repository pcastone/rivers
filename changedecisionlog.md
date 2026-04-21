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

### ctx.transaction executor integration via thread-local bridge

**File:** `crates/riversd/src/process_pool/v8_engine/{task_locals.rs, context.rs}` + `crates/rivers-runtime/src/dataview_engine.rs`
**Decision:** Route `ctx.dataview()` calls inside a transaction through a held connection by (a) storing active `TaskTransactionState { map, datasource }` in a thread-local, (b) having `ctx_dataview_callback` read that thread-local + use `DataViewExecutor::datasource_for` to verify the cross-ds rule, and (c) threading `Some(&mut conn)` into the executor's existing `txn_conn` param via `take_connection/return_connection` around the `exec.execute()` call.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §6`.
**Resolution:** The executor already exposed `txn_conn: Option<&mut Box<dyn Connection>>` — the plumbing was latent. My delta was (i) a new `datasource_for(name)` method on `DataViewExecutor` that exposes the registry's datasource mapping without executing anything, (ii) the thread-local bridge in task_locals, (iii) the callback in context.rs, and (iv) the take/return dance inside the `ctx_dataview_callback`'s `rt.block_on` so the connection is always returned even if execute fails. No signature changes to `exec.execute`. This satisfies spec §6.1 literally: `ctx.dataview()` inside the callback is implicitly scoped to the open transaction.

### Rollback runs before RT_HANDLE is cleared in TaskLocals::drop

**File:** `crates/riversd/src/process_pool/v8_engine/task_locals.rs`
**Decision:** In `TaskLocals::drop`, drain `TASK_TRANSACTION` and call `auto_rollback_all()` via the still-live `RT_HANDLE` **before** clearing `RT_HANDLE`.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §6.2` (timeout semantics).
**Resolution:** `auto_rollback_all` is async and needs the tokio runtime handle. If we cleared `RT_HANDLE` first, a timeout or handler panic that left a transaction open would be unable to roll back → pooled connection holds a dangling transaction. Order: extract transaction, run rollback, then clear. Documented in the drop impl so a future contributor doesn't reorder.

### Spec §6.4 MongoDB row is incorrect — flagged for spec revision

**File:** `docs/arch/rivers-javascript-typescript-spec.md §6.4`
**Decision:** The spec lists MongoDB as `supports_transactions = true`, but Mongo is a plugin driver (`crates/rivers-plugin-mongodb`) whose `supports_transactions()` return is not directly verifiable from the core codebase. Same concern applies to Cassandra, CouchDB, Elasticsearch, Kafka, LDAP rows.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §6.4`, plan task 7.8.
**Resolution:** Phase 7 implementation ships the correct behaviour — `DriverError::Unsupported` from a plugin driver maps to spec's exact error message at runtime. The spec's table of supported drivers should be amended to mark plugin rows "verify at plugin load" rather than baking an unverified assertion. Deferred to next spec revision cycle. Runtime enforcement is already authoritative.

### Parsed source-map cache separate from BundleModuleCache

**File:** `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs`
**Decision:** Introduce a second cache layer — `OnceCell<RwLock<HashMap<PathBuf, Arc<swc_sourcemap::SourceMap>>>>` — on top of `BundleModuleCache`'s raw v3 JSON.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5` (implicit — performance).
**Resolution:** `BundleModuleCache` stores raw JSON strings because (a) construction is cheap at bundle load, (b) hot-reload just swaps the whole cache, (c) not every handler file needs parsing (only those that throw). Parsing v3 JSON on every exception is expensive; the parsed `SourceMap` is what `lookup_token` actually consumes. Caching parsed instances via `Arc` keyed by absolute path eliminates re-parse overhead. Invalidation: `install_module_cache` now calls `clear_sourcemap_cache_hook` so hot-reload wipes stale parsed maps. Unit test `sourcemap_cache_idempotence_and_invalidation` covers both properties.

### CallSite extraction via JS reflection (rusty_v8 has no wrapper)

**File:** `crates/riversd/src/process_pool/v8_engine/execution.rs` — `extract_callsite`
**Decision:** Extract CallSite info by invoking JS methods (`getScriptName`, `getLineNumber`, `getColumnNumber`, `getFunctionName`) via `Object::get` + `Function::call`. No native Rust wrapper used.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5.2`.
**Resolution:** rusty_v8 v130 exposes CallSite only as a generic `v8::Value` — there is no typed wrapper. Invoking methods by name is the supported pattern and matches how Deno/Node bindings do it. Fields are `Option<_>` because methods can return null for native/eval frames; unit tests `fallback_when_no_cache_entry`, `anonymous_when_no_function_name`, `zero_line_or_col_falls_back` cover the degraded-info cases.

### `TaskError::HandlerErrorWithStack` struct variant (additive, not breaking)

**File:** `crates/rivers-runtime/src/process_pool/types.rs` — `TaskError`
**Decision:** Add a new struct variant `HandlerErrorWithStack { message, stack }` rather than extending `HandlerError(String)` with an optional stack.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5.2`.
**Resolution:** Changing `HandlerError(String)` to carry an optional stack would break every exhaustive `match` site in the codebase. Additive variant preserves the existing variant for non-stack errors and makes new consumers surface immediately. `ViewError::HandlerWithStack` mirrors the pattern at the view layer. The `#[error]` attribute on both variants displays only the message — the stack travels separately through the variant and is consumed by (a) the per-app log emission in `execute_js_task` (spec §5.3) and (b) the debug-mode response envelope in `map_view_error` (spec §5.3).

### Debug stack in response: debug-build + future app-flag gate

**File:** `crates/riversd/src/error_response.rs` — `map_view_error`
**Decision:** Include the remapped stack in the response envelope under `details.stack` when `cfg!(debug_assertions)` is true. The `AppConfig.base.debug` flag is declared in `rivers-runtime/src/bundle.rs` but not yet threaded through to `map_view_error` — that plumbing is a follow-on refinement.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5.3`.
**Resolution:** Spec §5.3 mandates per-app runtime debug flag control. The current MVP uses the compile-time `cfg!(debug_assertions)` to match the existing sanitization policy for `ViewError::Handler`, `Pipeline`, `Internal`. Threading `AppConfig.base.debug` through `view_dispatch.rs` + `map_view_error` is ~15 LOC of signature plumbing that doesn't change the behaviour story; the config surface is already declared for when that lands. Runtime behaviour today: dev builds see stacks, release builds don't — matches spec intent even if not the exact mechanism.

### Source map generation deferred to Phase 6

**File:** `crates/riversd/src/process_pool/v8_config.rs`
**Decision:** Phase 1 emits via `to_code_default(cm, None, &program)` — no source map collection.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5.1` (source maps always on).
**Resolution:** Spec §5 is Phase 6 work in the plan. Phase 1's scope is the drop-in only. When Phase 6 lands we replace `to_code_default` with a manual `Emitter` + source-map-generating `JsWriter` and store the map in `CompiledModule.source_map` (defined in Phase 2). No behaviour regression during Phase 1–5 because stack traces currently report compiled-JS positions and will continue to.
