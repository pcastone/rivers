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

### G0.1 — Debug-mode envelope: align spec to existing `ErrorResponse` shape

**File:** `docs/arch/rivers-javascript-typescript-spec.md §5.3` (to be edited in G8.4)
**Decision:** Spec §5.3 currently shows `{error, trace_id, debug: {stack}}`. Rivers' existing `ErrorResponse` convention (used across every error path in the codebase, pre-dating this spec) is `{code, message, trace_id, details: {stack}}`. Amend the spec to match the existing shape. No code changes.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §5.3`; `crates/riversd/src/error_response.rs:ErrorResponse`.
**Resolution:** Changing the envelope at the code layer would rename fields across every `ErrorResponse` site, break every API consumer that parses the current shape, and require a major version bump + migration doc. Zero information loss either way — `code+message` carries the same signal as `error`, `details.stack` carries the same signal as `debug.stack`. Spec edit is the low-risk path. Logged here because the choice locks the target for downstream tasks G5–G8.

### G0.2 — `Rivers.db / Rivers.view / Rivers.http` — drop from spec §8.3

**File:** `docs/arch/rivers-javascript-typescript-spec.md §8.3` (to be edited in G8.6)
**Decision:** Spec §8.3 requires `rivers.d.ts` declare `Rivers.db`, `Rivers.view`, `Rivers.http`. None of these exist at runtime — grep of `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` confirms only `Rivers.log`, `Rivers.crypto`, `Rivers.keystore`, `Rivers.env` are injected. Amend the spec to drop the three aspirational surfaces.
**Spec reference:** `docs/arch/rivers-javascript-typescript-spec.md §8.3`.
**Resolution:** Adding empty stub declarations would be aspirational clutter — a type checker would accept calls that fail at runtime. Adding real implementations is out of scope for the TS-pipeline work. Spec edit is the right lever. If `Rivers.db/view/http` ship as runtime surfaces in a future release, the `.d.ts` + spec can be updated together.

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

---

## Code-review remediation (P0-4 / P0-1)

### Broker consumer supervisor — nonblocking startup with bounded backoff

**Files:** `crates/riversd/src/broker_supervisor.rs` (new), `crates/riversd/src/bundle_loader/wire.rs`, `crates/riversd/src/server/context.rs`, `crates/riversd/src/health.rs`
**Decision:** Move `MessageBrokerDriver::create_consumer().await` out of `wire_streaming_and_events` and into a dedicated supervisor task spawned via `tokio::spawn`. Wiring returns immediately; HTTP listener bind is independent of broker reachability. State surfaced through a new `BrokerBridgeRegistry` on `AppContext`.
**Source reference:** `docs/code_review.md` finding P0-4.
**Resolution:** The Kafka driver's blocking work (rskafka client setup + partition discovery) is fully contained inside `create_consumer`; once the consumer exists, the bridge is already async-capable. Moving the await into the supervisor means no driver-side change is required, and any future broker driver inherits the same nonblocking guarantee. Backoff is exponential doubling capped at 60s (`SupervisorBackoff`), seeded by the existing `[data.datasources.<name>.consumer].reconnect_ms` config — operators have one knob, the cap protects against runaway delays under sustained outage. Health endpoint adds `broker_bridges: Vec<BrokerBridgeHealth>` so degraded brokers are visible separately from process readiness.

### Protected-view fail-closed gate (security_pipeline + bundle-load validation)

**Files:** `crates/riversd/src/security_pipeline.rs`, `crates/riversd/src/bundle_loader/load.rs`
**Decision:** (a) `run_security_pipeline` rejects with `500 Internal Server Error` when a non-public view is dispatched and `ctx.session_manager.is_none()`. (b) Bundle load (`load_and_wire_bundle`, AM1.2) refuses bundles that declare any non-public view when no session manager is available — strengthens the existing storage-engine check to name the actual security boundary.
**Source reference:** `docs/code_review.md` finding P0-1.
**Resolution:** Two-layer defense. The runtime check is the authoritative security boundary because it's evaluated for every dispatch, even when configs hot-reload mid-flight. The bundle-load check is defense-in-depth — it catches the misconfig at deploy time so operators don't discover it via a 500 in prod. The validation predicate was extracted into `check_protected_views_have_session(views, has_session_manager, has_storage_engine) -> Result<(), String>` so it can be unit-tested without staging a disk bundle; six unit tests cover the truth table. The error message names the offending view and explains the missing dependency (storage vs session manager) so operators get an actionable hint.

### Host path redaction is unconditional (B4 / P1-9)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs` (new helper `redact_to_app_relative`), `crates/riversd/src/process_pool/v8_engine/mod.rs` (re-export), `crates/riversd/src/process_pool/module_cache.rs` (`module_not_registered_message` uses redactor).
**Decision:** Path redaction in V8 script origins, resolve-callback errors, and `MODULE_NOT_REGISTERED` formatting is applied unconditionally — same in debug and release builds. Helper is `pub(crate)` so the future SQLite path policy (G_R8.2) can reuse it.
**Source reference:** `docs/code_review.md` finding P1-9; `todo/tasks.md` task B4 (controller-resolved decision).
**Resolution:** Two reasons not to gate on `cfg!(debug_assertions)`: (1) the redacted form (`{app}/libraries/handlers/foo.ts`) is more useful than absolute paths for log grep across hosts and deployments — even local devs benefit; (2) the security posture must not depend on build mode, otherwise a misconfigured staging build with debug assertions on becomes a leak vector. The existing debug-mode `details.stack` field in `error_response::map_view_error` is unaffected — debug builds CAN show stacks per spec, and B4 just guarantees those stacks are redacted at the source. Algorithm is the same `libraries`-anchor walk used by the older `shorten_app_path` in `v8_config.rs`, but operates on `&str` and returns `Cow` to avoid allocation when no redaction is needed (inline test sources, already-redacted strings, empty inputs). 8 unit tests pin the contract; 2 integration tests in `path_redaction_tests.rs` dispatch real handlers and assert no `/Users/`, `/var/folders/`, or workspace prefix appears in the response or stack.
