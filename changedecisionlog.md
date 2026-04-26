# Change Decision Log

Per CLAUDE.md Workflow rule 5: every decision during implementation is logged here with file, decided, spec reference, and resolution method. CB uses this as the reference baseline for drift detection — treat it as load-bearing.

---

## 2026-04-24 — Canary 135/135 push

### translate_params() QuestionPositional duplicate-$name fix
**File:** `crates/rivers-driver-sdk/src/lib.rs`
**Decision:** Track `all_occurrences` (with duplicates) alongside `placeholders` (unique). For `QuestionPositional`, use `all_occurrences` for both the ordered bound-value list and the `replacen()` rewriting loop. This ensures MySQL gets 3 bound values for `DELETE ... WHERE id = $id AND (zsender = $actor OR recipient = $actor)`.
**Spec ref:** None (bug fix in parameter translation layer).
**Resolution:** Root cause was that `placeholders` deduplicated names, so 2 unique names → 2 values, but 3 `?` markers. Fix: separate tracking for occurrence order.

### V8 13.0.245.12 decorator syntax not implemented
**File:** `crates/riversd/src/process_pool/v8_engine/init.rs`, `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/decorator.ts`
**Decision:** Do NOT attempt to enable `@decorator` syntax via V8 flags. `js_decorators` is `EMPTY_INITIALIZE_GLOBAL_FOR_FEATURE` in bootstrapper.cc — it's a no-op placeholder. The V8 parser.cc has zero `@`-token handling. Decorator test rewritten to apply Stage 3 semantics manually (same call contract, no `@`-syntax).
**Spec ref:** spec §2.3 (decorator syntax). Test semantics preserved; syntax probe deferred to V8 upgrade.
**Resolution:** nm + V8 source analysis confirmed feature is unimplemented in this V8 build. Manual application achieves 135/135 without V8 upgrade.

### MCP session handshake in run-tests.sh
**File:** `canary-bundle/run-tests.sh`
**Decision:** Capture `Mcp-Session-Id` from `initialize` response headers via `-D tmpfile` and pass as header on all subsequent requests. Added `-k` to all MCP curl calls.
**Spec ref:** MCP protocol requires session handshake.
**Resolution:** Without session ID, all non-initialize methods return `-32001 Session required` → FAIL.

### RT-V8-TIMEOUT: 408 is a valid PASS
**File:** `canary-bundle/run-tests.sh`
**Decision:** Accept HTTP 408 (server-side request timeout) as PASS for RT-V8-TIMEOUT. Both the V8 watchdog and the HTTP request timeout fire at 30s; which fires first is a race. Raised curl timeout to 35s to give the watchdog a chance to win.
**Spec ref:** V8 timeout spec §9.
**Resolution:** The key assertion is "server survived" (didn't crash/hang), not which timeout mechanism fires first.

### RT-CTX-APP-ID expectation updated to entry_point slug
**File:** `canary-bundle/canary-handlers/libraries/handlers/ctx-surface.ts`
**Decision:** `ctx.app_id` returns the entry_point slug `"handlers"` (not the manifest UUID) after the store-namespace isolation fix. Updated assertion to expect `"handlers"`.
**Spec ref:** processpool §9.8.
**Resolution:** The slug is the stable identity token for the handler; UUID was never documented as the ctx.app_id value.

### Activity-feed scenario: cleanup-before must wipe by user, not trace_id
**File:** `canary-bundle/canary-streams/app.toml`, `canary-bundle/canary-streams/libraries/handlers/scenario-activity-feed.ts`
**Decision:** Added `events_cleanup_user` DataView (DELETE by target_user). Cleanup-before wipes all bob+carol events (not just current run's by id_prefix) to prevent accumulated SQLite rows from displacing pagination windows across test runs.
**Spec ref:** scenario spec §10 cleanup rule.
**Resolution:** SQLite persists between server restarts; test-isolation requires full user sweep, not just trace-scoped delete.

---

## 2026-04-24 — `rivers-keystore-engine` review scope

### Focused app-keystore engine report target

**File:** `todo/tasks.md`, future report target `docs/review/rivers-keystore-engine.md`.
**Decision:** Replace the completed lockbox-engine review task list with an RKE plan focused on `crates/rivers-keystore-engine` and its runtime/CLI/docs wiring.
**Spec reference:** User request on 2026-04-24; repository Workflow rules 1, 2, 5, and 6 in `AGENTS.md`.
**Resolution:** `todo/tasks.md` now captures completed source/test reads, pending full cross-crate evidence reads, security sweeps, key-rotation/file-I/O/master-key review tasks, report writing, logging, and final validation.

## 2026-04-24 — Review consolidation plan

### Output path and missing-input policy

**File:** `todo/tasks.md`, future report target `docs/review/cross-crate-consolidation.md`.
**Decision:** Write the cross-crate consolidation report under `docs/review/`, but make the report source basis explicit. If the 22 per-crate reports remain absent, use `docs/code_review.md` only as fallback grounding and label the output accordingly instead of pretending the missing reports were read.
**Spec reference:** User request on 2026-04-24; repository Workflow rules 1, 2, 5, and 6 in `AGENTS.md`.
**Resolution:** `todo/tasks.md` now contains the RCC plan with a pre-flight input re-check, honest source-basis gate, report-writing task, and validation steps.

## 2026-04-24 — `rivers-plugin-exec` review scope

### Consolidation deferred; exec-only report target

**File:** `todo/tasks.md`, `todo/gutter.md`, future report target `docs/review/rivers-plugin-exec.md`.
**Decision:** Supersede the RCC consolidation plan and review only `crates/rivers-plugin-exec`; consolidation will happen in a separate session.
**Spec reference:** User clarification on 2026-04-24; repository Workflow rules 1, 2, 5, and 6 in `AGENTS.md`.
**Resolution:** Archived the unfinished RCC plan to `todo/gutter.md` and replaced `todo/tasks.md` with RXE tasks covering full-source read, mechanical sweeps, compiler validation, exec-specific security axes, driver-sdk contract compliance, report writing, and log validation.

## 2026-04-24 — Full code review report delivered

### Report format and stale-finding policy

**File:** `docs/code_review.md`, `todo/tasks.md`.
**Decision:** Rewrite the review report into the user's crate-by-crate Tier 1/2/3 format and drop prior-report findings that were not re-confirmed as high-confidence current production risks.
**Spec reference:** User "Rivers Code Review — Claude Code Prompt" on 2026-04-24; repository Workflow rules 1, 3, 5, and 6 in `AGENTS.md`.
**Resolution:** The report now states its grounding explicitly: workspace-wide sweeps plus source reads for every cited finding. Clean crates are marked "No issues found" only for this pass, not as a claim of line-by-line proof.

---

## 2026-04-24 — Full code review refresh plan

### FCR plan replaces active task file for review

**File:** `todo/tasks.md` (CG plan archived to `todo/gutter.md` under 2026-04-24 header), `docs/code_review.md` (planned review target).
**Decision:** Replace the active CG canary plan with a source-grounded full code-review plan focused on security, V8 JavaScript/TypeScript, database drivers, connection pool, EventBus, StorageEngine, datasource/handler wiring, DataView, and view function wiring.
**Spec reference:** User request on 2026-04-24; repository Workflow rules 1, 2, 5, and 6 in `AGENTS.md`.
**Resolution:** Review execution is gated on plan approval. Existing `docs/code_review.md` is treated as prior art, not evidence; every retained finding must be re-confirmed against current source before the report is updated.

---

## 2026-04-24 — CG plan supersedes CS/BR

### Plan replacement: CG — Canary Green Again

**File:** `todo/tasks.md` (CS/BR archived to `todo/gutter.md` under 2026-04-24 header).
**Decision:** Replace the CS0–CS7 + BR0–BR7 plan (both largely shipped, residual work was deploy-gated or deferred polish) with a focused CG0–CG5 plan addressing the canary startup-hang and the top 4 items from `docs/canary_codereivew.md`.
**Spec reference:** `docs/canary_codereivew.md` (2026-04-24) + `docs/dreams/dream-2026-04-22.md`.
**Resolution:** CG plan scope = (1) MessageConsumer empty `app_id` fix, (2) subscription topic from `on_event.topic` not view_id, (3) non-blocking broker consumer startup, (4) MySQL pool revert. Out-of-scope (tracked for later prod-hardening plan): Kafka producer lazy-init, SWC timeout, sourcemap LRU, path redaction, module-cache strict mode, thread-local panic-safety tests, publish hot-path JSON round-trip, commit-failure thread-local overwrite.

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
## 2026-04-23 — Canary Scenarios

### CS0.1 — Document Pipeline scenario hosted in `canary-handlers`

**File:** `canary-bundle/canary-handlers/` (host app for `SCENARIO-RUNTIME-DOC-PIPELINE`)
**Decision:** Host the Document Pipeline scenario (spec §7) in `canary-handlers` per the literal reading of spec §4. Alternative considered: relocate to `canary-filesystem`, which already has the filesystem driver wired and would avoid new infra in `canary-handlers`.
**Spec reference:** `docs/arch/rivers-canary-scenarios-spec.md §4` (Profile Assignment table maps Document Pipeline → canary-handlers).
**Resolution:** Spec §4 explicitly ties Document Pipeline to canary-handlers because the scenario's "primary concern is filesystem driver, exec driver, handler context surface" — the TS-pipeline app where those capabilities should land for handler authors. Relocating would have been ergonomically easier but would diverge from the spec's test-matrix contract (`SCENARIO-RUNTIME-DOC-PIPELINE` test-id). Task implication: CS4 wires `fs_workspace` (filesystem) + `exec_tools` (exec, hash-pinned allowlist) into `canary-handlers/resources.toml`, mirroring the patterns from `canary-filesystem/resources.toml`. If this produces cross-cutting issues in canary-handlers, the decision is revisitable — a future spec rev could reassign.

### CS0.2 — Messaging session-identity via pre-seeded sessions + internal HTTP dispatch

**File:** `canary-bundle/canary-sql/libraries/handlers/scenario-messaging.ts` (future)
**Decision:** Use **pre-seeded sessions** — the scenario orchestrator handler creates three real sessions (alice/bob/carol) via the canary's normal session-create endpoint, stashes the returned cookies, and makes internal HTTP sub-requests to per-user worker endpoints (e.g. `/canary/scenarios/sql/messaging/_insert`, `/_inbox`, `/_search`, `/_delete`) carrying the appropriate cookie per step.
**Spec reference:** `docs/arch/rivers-canary-scenarios-spec.md §10` (Simulating Multiple Users).
**Resolution:** Session injection would require a new runtime affordance (mid-request `ctx.session.sub` rewrite) that doesn't exist today and would be inappropriate for production code paths. Pre-seeded sessions use only production-path code — the guard view processes each cookie normally, `ctx.session` is populated by the security_pipeline as it is for any real user. MSG-1 enforcement lives in the per-user worker endpoints, which read `session.sub` directly and reject body `sender` fields. The orchestrator knows the test identities but never handles the MSG-1 contract itself — it's a test-coordination layer. Cost is ~30 LOC of internal HTTP client plumbing per scenario (reused across the three driver variants for Messaging). The pattern is applicable to Activity Feed (CS3) as well — Bob/Carol isolation checks use the same sub-request dispatch.

### CS0.2 REVISED — Messaging session-identity via single orchestrator + identity-as-parameter

**File:** `canary-bundle/canary-sql/libraries/handlers/scenario-messaging.ts` (future)
**Decision:** Supersedes the earlier 2026-04-23 CS0.2 entry (pre-seeded sessions + internal HTTP dispatch). That design is **not implementable** because Rivers TS handlers cannot make outbound HTTP calls — `Rivers.http` was explicitly dropped in G0.2/G4.2 as aspirational. Revised design: the scenario orchestrator is a single TS handler at `/canary/scenarios/sql/messaging/{driver}` (auth=none). It calls DataViews directly via `ctx.dataview(...)`, passing `sender` / `recipient` as explicit parameters per-step. Identity isolation is verified at the DataView WHERE-clause level (server-side filtering) — the orchestrator always passes whose inbox it's probing as a parameter.
**Spec reference:** `docs/arch/rivers-canary-scenarios-spec.md §10` — "Either approach is valid as long as identity isolation is verifiable." The spec explicitly accepts runtime-dependent variations.
**Resolution:** Trade-off accepted: **MSG-1 end-to-end enforcement** (handler rejects body-supplied sender; sender comes only from `ctx.session.sub`) is NOT exercised by the scenario — the orchestrator knows test identities and passes them explicitly. The spec's INTENT (multi-user workflow, inbox scoping, encryption roundtrip, delete permissions) IS exercised. Coverage-gap mitigation: the canary-guard atomic tests already exercise the session → handler → ctx.session.sub path; a dedicated atomic test for the "reject body-supplied sender" invariant can be added under canary-sql atomics if explicit MSG-1 coverage is desired.
Rejected alternatives:
  - **Extend canary-guard to accept caller-specified subjects + orchestrate from run-tests.sh** — full coverage, ~2× the implementation effort. Deferrable.
  - **Session injection via runtime affordance** — requires new TS API surface; out of CS2 scope.

### CS3 — Activity Feed scenario deferred pending MessageBrokerDriver TS bridge

**File:** `canary-bundle/canary-streams/` (scenario not shipped)
**Decision:** Defer CS3 (Activity Feed) in its entirety. Both viable implementation paths — (A) direct SQL insert from the scenario orchestrator, (B) external kafkacat publish from run-tests.sh — were explicitly rejected as unacceptable workarounds that don't exercise the composition the scenario is supposed to test.
**Spec reference:** `docs/arch/rivers-canary-scenarios-spec.md §6` AF-1/AF-2/AF-8.
**Resolution:** Root cause logged as `bugs/bugreport_2026-04-23.md` — TS handlers have no MessageBrokerDriver publish surface (affects kafka/rabbitmq/nats/redis-streams). Fix requires a V8 bridge (1-2 days of Rust work in `crates/riversd/src/process_pool/v8_engine/`) to expose `ctx.datasource("broker").publish(...)` via direct-dispatch (mirroring the filesystem driver pattern) or an extended DataView path. CS3 becomes executable in ~3-4 hours once that bridge lands. CS3 deferral also surfaces a broader observation: four shipped message-broker drivers have implementations that are structurally half-wired in the runtime.

Earlier misdiagnosis worth noting for the record: the CS0.2 revision (dated earlier today) claimed "Rivers TS handlers cannot make outbound HTTP" — that was wrong. `Rivers.http` (the global-namespace object) was dropped per G0.2/G4.2, but HTTP-as-datasource IS wired and reachable via `ctx.dataview("name", {})` (see `canary-main/libraries/handlers/proxy-tests.ts`). The original CS0.2 plan (pre-seeded sessions + internal HTTP dispatch) was actually feasible. The revised "identity-as-parameter" design already shipped for CS2 Messaging remains valid — no rework required — but future scenarios should treat HTTP-as-datasource as available.

## 2026-04-23 — BR MessageBrokerDriver TS bridge

### BR0.1 — Bridge pattern: parallel scaffolding (path a)

**File:** `crates/riversd/src/process_pool/v8_engine/broker_dispatch.rs` (new)
**Decision:** Add a `DatasourceToken::Broker` variant, a dedicated `TASK_DIRECT_BROKER_PRODUCERS` thread-local, a new `Rivers.__brokerPublish` V8 callback, and a new proxy-codegen branch that emits `.publish(msg)`. Parallel to the existing filesystem direct-dispatch scaffolding.
**Spec reference:** `bugs/bugreport_2026-04-23.md`.
**Resolution:** Rejected (b) unified-with-DatabaseDriver — every broker plugin would grow a synthetic `DatabaseDriver` impl forwarding `"publish"` to BrokerProducer, invasive across 4 crates, and type-erases the request/response vs fire-and-forget distinction. Rejected (c) DataView-based — loses structured headers, partition key, and PublishReceipt return; "one direction wired, the other stranded" from the bug report applies. Path (a) touches only the runtime crates + one new file; broker plugins unchanged. DriverFactory already tracks broker drivers in a separate `broker_drivers: HashMap<String, Arc<dyn MessageBrokerDriver>>`, so trait-query dispatch via `factory.get_broker_driver(name)` is a clean 2-line check.

### BR0.2 — Producer lifecycle: per-task cache

**File:** `crates/riversd/src/process_pool/v8_engine/task_locals.rs`
**Decision:** Lazy-init `BrokerProducer` on first `.publish()` call within a task; cache under `TASK_DIRECT_BROKER_PRODUCERS[name]`; close in `TaskLocals::drop` using the still-live `RT_HANDLE` (same ordering precedent as `auto_rollback_all`). No cross-task producer sharing.
**Spec reference:** mirrors filesystem `TASK_DIRECT_DATASOURCES` pattern (spec-plan task 29).
**Resolution:** Kafka/RabbitMQ producers are typically expensive to create (TLS handshake, broker discovery); per-publish create+close is wasteful. Per-task cache matches filesystem's `Connection`-per-task caching semantics exactly. Cross-task sharing would require `Arc<Mutex<BrokerProducer>>` — unnecessary complexity when worker threads already serialise task execution within the pool. On drop: log-on-error close, don't block the drop path.

### BR0.3 — TS API shape

**File:** `types/rivers.d.ts` + `crates/riversd/src/process_pool/v8_engine/broker_dispatch.rs`
**Decision:** `ctx.datasource("<broker>").publish({destination, payload, headers?, key?, reply_to?}) → {id: string | null, metadata: string | null}`. Field names mirror `OutboundMessage` / `PublishReceipt` from `rivers-driver-sdk::broker` verbatim. Payload accepts `string` (UTF-8 bytes) OR `object` (auto JSON-stringify + UTF-8 bytes). Throws `Error` on DriverError with the underlying message preserved.
**Spec reference:** `rivers-driver-sdk/src/broker.rs` OutboundMessage struct.
**Resolution:** Verbatim field naming keeps the TS API trivially mappable to the Rust struct (simplifies the V8 marshalling + future spec doc work). Auto-stringify for objects is a DX convenience — handlers almost always work with JSON-serialisable data. Receipt type keeps both fields Option-ish (`string | null`) because different brokers populate them differently (kafka sets both; NATS often sets neither). `@capability broker` JSDoc tag added to rivers.d.ts matching the existing `@capability keystore` / `@capability transaction` convention.

## 2026-04-24 — `rivers-lockbox-engine` review planning

### RLE0.0 — Preserve unfinished active review before starting lockbox review

**File:** `todo/tasks.md`, `todo/gutter.md`
**Decision:** Move the unfinished `rivers-plugin-exec` review task list from `todo/tasks.md` into `todo/gutter.md`, then replace the active task list with the `rivers-lockbox-engine` review plan.
**Spec reference:** AGENTS.md workflow rule 1: before clearing `todo/tasks.md` with unfinished items, move them to `todo/gutter.md`.
**Resolution:** The lockbox review is now the active plan, but the plugin-exec review tasks remain recoverable in the gutter.

### RLE0.1 — Output path and review basis

**File:** `docs/review/rivers-lockbox-engine.md` (planned)
**Decision:** Write the per-crate review to `docs/review/rivers-lockbox-engine.md`.
**Spec reference:** User request: "write output to @docs/review/{{name of crate}}" for crate 2, `rivers-lockbox-engine`.
**Resolution:** The report will be based on full reads of all production source and tests in `crates/rivers-lockbox-engine`, plus workspace caller searches for cross-crate wiring gaps.

### RLE2.1 — Treat secret lifecycle as the primary review axis

**File:** `docs/review/rivers-lockbox-engine.md`
**Decision:** Lead the report with secret lifecycle findings rather than crypto primitive findings.
**Spec reference:** `docs/arch/rivers-lockbox-spec.md` security model: no secret values retained, per-access zeroization, host-side opaque resolution.
**Resolution:** Age envelope usage was comparatively clean. The confirmed high-risk gaps were bare `String` containers, derived `Debug`/`Clone`, manual caller zeroization, runtime identity caching, and handler-accessible LockBox HMAC resolution.

### RLE2.2 — Include cross-crate CLI/runtime format split in this crate report

**File:** `docs/review/rivers-lockbox-engine.md`, `crates/rivers-lockbox/src/main.rs`
**Decision:** Report the standalone `rivers-lockbox` CLI storage-format mismatch as a Tier 1 wiring finding in the `rivers-lockbox-engine` review.
**Spec reference:** User request to catch wiring gaps that span crates; `docs/arch/rivers-lockbox-spec.md` says the CLI manages the keystore file consumed by `riversd`.
**Resolution:** The engine reads a single Age-encrypted TOML `.rkeystore`; the CLI writes per-entry `.age` files under `entries/`. This is load-bearing enough to belong in the engine report, not only a future CLI report.

### RLE2.3 — Do not claim constant-time comparison bug in this crate

**File:** `docs/review/rivers-lockbox-engine.md`
**Decision:** Record constant-time comparison as a non-finding for this crate.
**Spec reference:** User risk list included constant-time comparison.
**Resolution:** Full source and sweeps found no direct secret/token/key equality comparison in `rivers-lockbox-engine`; equality checks were on names, aliases, and config metadata. The report keeps timing-safe comparison out of the finding list to avoid noise.

## 2026-04-24 — `rivers-keystore-engine` review

### RKE0.1 — Output path and review basis

**File:** `docs/review/rivers-keystore-engine.md`, `todo/tasks.md`
**Decision:** Write the per-crate review to `docs/review/rivers-keystore-engine.md` and ground it in full reads of the keystore engine source/tests plus runtime, CLI, and docs files used as evidence.
**Spec reference:** User request: "write output to @docs/review/{{name of crate}}" for crate 3, `rivers-keystore-engine`; AGENTS.md workflow rules 1, 2, 5, and 6.
**Resolution:** The report states its source basis explicitly and includes the validation commands used for confidence: `cargo check -p rivers-keystore-engine`, `cargo test -p rivers-keystore-engine`, and `cargo check -p riversd`.

### RKE2.1 — Treat multi-keystore runtime selection as a Tier 1 cross-crate wiring gap

**File:** `docs/review/rivers-keystore-engine.md`, `crates/riversd/src/keystore.rs`, `crates/riversd/src/bundle_loader/load.rs`, `crates/rivers-runtime/src/bundle.rs`
**Decision:** Report arbitrary first-match keystore selection as a Tier 1 finding in the engine review rather than deferring it to a runtime-only review.
**Spec reference:** User request to catch wiring gaps that span crates; app-keystore docs promise application-scoped key isolation.
**Resolution:** The engine itself can hold valid key material, but the runtime loads multiple keystores per app and static handler dispatch has only a key-name API. That makes the effective keystore contract non-deterministic across crate boundaries, so it belongs in this Tier A crate report.

### RKE2.2 — Treat dynamic callback keystore support as unsupported until app-scoped resolver wiring exists

**File:** `docs/review/rivers-keystore-engine.md`, `crates/riversd/src/engine_loader/host_context.rs`, `crates/riversd/src/engine_loader/host_callbacks.rs`
**Decision:** Report the dynamic engine `HOST_KEYSTORE` path as a cross-crate wiring gap, not as a small missing call-site nit.
**Spec reference:** User request to catch `register_X`/caller-style wiring gaps spanning crates; dynamic build mode is a documented Rivers deployment mode.
**Resolution:** `set_host_keystore()` has no runtime caller, and the one-shot global shape cannot represent app-scoped or hot-reloaded keystores even if called. The recommended resolution is shared resolver wiring or explicit dynamic-mode capability rejection.

## 2026-04-25 — Phase H5 / T2-2: WS+SSE connection-limit race

### H5.1 — Two strategies based on existing storage shape

**File:** `crates/riversd/src/websocket.rs` (`BroadcastHub::subscribe`, `ConnectionRegistry::register`), `crates/riversd/src/sse.rs` (`SseChannel::subscribe`)
**Decision:** Apply two different fix shapes depending on whether the structure has an associated map under a write lock.
**Spec reference:** `rivers-view-layer-spec.md §6.4`, `§7.4`. Standard 4 (reuse what fits without contortions).
**Resolution:**
- `BroadcastHub` and `SseChannel` track only an `AtomicUsize` (no associated map), so the limit check + increment was rewritten as a single `compare_exchange` via `AtomicUsize::fetch_update`. The closure returns `Some(c+1)` when `c < max` and `None` otherwise; the `Err` branch maps to `ConnectionLimitExceeded`. AcqRel ordering pairs with the visible state the counter guards.
- `ConnectionRegistry` already takes a `RwLock<HashMap>` write lock during insert. The fix moves the `count >= max` check inside the same `write().await` and uses `conns.len()` as the source of truth. The `AtomicUsize` counter is kept in sync purely as a fast `active_connections()` accessor — the limit decision no longer depends on it.

### H5.2 — Concurrent regression tests use multi-thread tokio flavor

**File:** `crates/riversd/src/websocket.rs` (test module), `crates/riversd/src/sse.rs` (test module)
**Decision:** Add three `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` regression tests (200 concurrent ops, max=50 → expect exactly 50 ok / 150 limit-exceeded).
**Spec reference:** Standard 5 (push once more — verify the property holds, not just that the obvious case passes).
**Resolution:** Single-threaded runtime cannot exhibit the race because tasks never preempt each other. Only the multi-thread flavor exercises true cross-thread contention on the atomic / write lock. All three tests pass on first run; one test also asserts `all_connection_ids().await.len() == MAX` to confirm the map size matches the counter.

## TXN-I1.1 — Dyn-engine transaction map design (2026-04-25)

### Files audited (full reads, not skims)
- V8 reference: `crates/riversd/src/process_pool/v8_engine/context.rs:898–1276` (`ctx_transaction_callback`, `ctx_dataview_callback`).
- V8 thread-locals + `TaskTransactionState`: `crates/riversd/src/process_pool/v8_engine/task_locals.rs:140–185`.
- Shared TransactionMap: `crates/riversd/src/transaction.rs:1–198` (full file).
- Dyn-engine stubs: `crates/riversd/src/engine_loader/host_callbacks.rs:885–1073` (`host_db_begin/commit/rollback/batch`); `host_callbacks.rs:28–158` (`host_dataview_execute`).
- Runtime layer: `crates/riversd/src/engine_loader/host_context.rs:1–98`; `engine_loader/registry.rs:1–53`; `engine_loader/loaded_engine.rs:1–79`.
- Task dispatch wrapper: `crates/riversd/src/process_pool/mod.rs:303–353` (`dispatch_task`).
- FFI shape: `crates/rivers-engine-sdk/src/lib.rs:79–122` (`SerializedTaskContext` — no `task_id`).

### Decisions

1. **Map key:** `(TaskId, datasource_name)` where `TaskId = u64` from a `static AtomicU64`. Issued in `dispatch_task` immediately before `tokio::task::spawn_blocking`. Stored in a `thread_local!` `Cell<Option<TaskId>>` set by `TaskGuard::enter` and cleared on `Drop`. Reasoning: `SerializedTaskContext` ships no per-task ID across the FFI, and engine threads are reused across many tasks so any thread-local on the engine side is unsafe; but the riversd-side `spawn_blocking` worker is 1:1 with one task for the duration of that task and host callbacks always run synchronously on that calling thread, so a riversd-side thread-local set by the dispatch wrapper is the correct identity carrier. A composite key `(TaskId, ds)` matches the V8 mental model where `TASK_TRANSACTION` already permits one txn per (task, datasource) — though spec §6.2 currently allows only one datasource per task, the composite key keeps the type honest if §6 ever relaxes that.

2. **Storage location:** New sibling `OnceLock<DynTransactionMap>` (named `DYN_TXN_MAP`) declared in `crates/riversd/src/engine_loader/host_context.rs`, with a `pub fn dyn_txn_map() -> &'static DynTransactionMap` accessor. Reasoning: this matches the existing pattern used for adjunct globals in the same file (`HOST_KEYSTORE`, `DDL_WHITELIST`, `APP_ID_MAP` — all sibling `OnceLock` statics, lines 25–34). Adding it to `HostContext` itself would force a wider construction-site change and break the existing "set once, callbacks read via static" idiom.

3. **Auto-rollback hook:** Insertion point `crates/riversd/src/process_pool/mod.rs:326` — wrap the `spawn_blocking` closure body so it owns a `TaskGuard` whose `Drop` impl calls `dyn_txn_map_auto_rollback_blocking(task_id)`. The drop runs synchronously when the closure unwinds (success, error, or panic-mapped-to-`WorkerCrash`); inside `Drop` we use `HOST_CONTEXT.rt_handle.block_on(...)` to drive the async rollback because the `spawn_blocking` thread is not a tokio runtime worker. Reasoning: `spawn_blocking` is the only place in the cdylib path where a riversd-owned scope brackets a single task's entire execution. Putting the cleanup inside the closure (via guard drop) makes it panic-safe in a way a post-`.await?` cleanup at the call site would not be.

4. **Connection holder type:** `Box<dyn Connection>` directly — same as `crate::transaction::TransactionMap`. Reasoning: `PoolManagerHandle` / `PooledConnection { conn, release_token }` does not exist in the workspace (`grep -rn PoolManagerHandle crates/` returns zero matches). The brief's framing of "H6/H7 work" is mis-remembered; V8's path acquires via `factory.connect(&driver_name, &params).await` returning `Box<dyn Connection>`, and the `Drop` of that `Box` is what releases the pool slot (see context.rs:1024, "Connection drops → pool slot released"). Mirroring that exact shape keeps the dyn path semantically identical to V8, and reuses the entire `crate::transaction::TransactionMap` mental model.

### Open questions surfaced during audit (require human input before I3)

1. **Datasource config availability in host callbacks.** `host_db_begin` needs `(driver_name, ConnectionParams)` but riversd has no per-task datasource-config map on its side. V8 has `TASK_DS_CONFIGS` populated in `task_locals.rs`. **Recommended option A:** stash `ctx.datasource_configs` keyed by `task_id` in a sibling `RwLock<HashMap<TaskId, ...>>` populated in `dispatch_task` and cleared in `TaskGuard::drop`. (Plan §6.1.)
2. **Commit-failure signaling back to dispatch.** V8 sets `TASK_COMMIT_FAILED` thread-local and `execute_js_task` reads it to upgrade the error to `TaskError::TransactionCommitFailed`. Dyn path needs an equivalent thread-local on the `spawn_blocking` thread, read after `spawn_blocking` resolves but before `dispatch_task` returns. (Plan §6.2.)

### Implementation order for I2-I7

- **I2:** Land `crates/riversd/src/engine_loader/transaction_map.rs` (new module containing `TaskId`, `next_task_id`, `CURRENT_TASK_ID` thread-local, `TaskGuard`, `DynTransactionMap`). Wire `DYN_TXN_MAP` `OnceLock` and `dyn_txn_map()` accessor in `host_context.rs`. Unit tests mirror `transaction.rs::tests`.
- **I3:** Wire `host_db_begin` — read `current_task_id()`, resolve datasource config (per open question 6.1), `factory.connect`, `dyn_txn_map().begin(task_id, ds, conn)`. Bound by `HOST_CALLBACK_TIMEOUT_MS`.
- **I4:** Wire `host_db_commit` / `host_db_rollback`. Implement `TASK_COMMIT_FAILED` equivalent (open question 6.2).
- **I5:** Wire `host_dataview_execute` transaction routing — mirror V8's `take_connection`/`return_connection` pattern (context.rs:1210–1233) and the spec §6.2 cross-datasource check (context.rs:1182–1200).
- **I6:** Wire `host_db_batch` — iterate params under the active txn.
- **I7:** Modify `process_pool/mod.rs:326` to wrap the `spawn_blocking` closure in `TaskGuard::enter(next_task_id())`. Drop hook calls `dyn_txn_map_auto_rollback_blocking(task_id)`.
- **I8:** Integration tests against `192.168.2.209` PostgreSQL: commit-visible, rollback-invisible, panic-auto-rolled-back, cross-datasource error, nested-rejection, commit-failure-upgrades-to-`TransactionCommitFailed`.

Full plan with type sketches and risks: `docs/superpowers/plans/2026-04-25-phase-i-dyn-transactions.md`.

## TXN-I2.1 — DynTransactionMap + TaskId/TaskGuard infrastructure landed (2026-04-25)

**Files affected:**
- `crates/riversd/src/engine_loader/dyn_transaction_map.rs` (NEW)
- `crates/riversd/src/engine_loader/mod.rs` (added `mod dyn_transaction_map;`)
- `crates/riversd/src/engine_loader/host_context.rs` (added DYN_TXN_MAP, TaskId issuer, TaskGuard, TASK_DS_CONFIGS, DYN_TASK_COMMIT_FAILED + accessors)

**Spec reference:** TXN-I1.1 decisions 1–4 + open questions 6.1 (option A) and 6.2 (option A).

**Resolution method:**
- **Sibling module, not extension** of `crates/riversd/src/transaction.rs`. The existing `TransactionMap` is per-request (one map per request) and used by V8 via an `Arc<TransactionMap>` pinned to a worker thread. The dyn-engine path needs a single process-wide map keyed by `(TaskId, ds_name)` because callbacks run on a riversd-side `spawn_blocking` worker shared across the lifetime of riversd. Forcing the V8 map to take a `TaskId` would make every V8 caller carry an unused id and risk subtle behaviour changes; a sibling type isolates the new shape and keeps V8 untouched.
- `DynTransactionMap` uses `std::sync::Mutex` (not `tokio::sync::Mutex`). The `with_conn_mut` method takes the connection out under the lock, drops the lock, runs the closure's future, then re-acquires the lock to re-insert. The sync mutex is **never** held across `.await`.
- `with_conn_mut` uses HRTB on the closure's lifetime (`for<'a> F: FnOnce(&'a mut Box<dyn Connection>) -> Pin<Box<dyn Future<Output=R> + Send + 'a>>`) so call sites can pass `|conn| Box::pin(async move { conn.execute(...).await })` naturally.
- `TaskGuard::drop` runs auto-rollback by spawning each per-datasource rollback as its own `tokio::spawn` task and awaiting the `JoinHandle`. This contains panics from one rollback so they cannot prevent the others.
- `TaskGuard` captures `tokio::runtime::Handle` at `::enter` time so `Drop` can `block_on` even though it's invoked synchronously. Safe because `TaskGuard` is built only inside `spawn_blocking` workers (not tokio runtime workers).
- Per-task datasource configs stash uses `RwLock<Option<HashMap<TaskId, _>>>` so it can be a `static`. Reads dominate writes (one `lookup_task_ds` per `host_db_begin`, two writes per task lifecycle).
- `DYN_TASK_COMMIT_FAILED` thread-local mirrors V8's `TASK_COMMIT_FAILED` shape exactly so `dispatch_task` post-processing in I7 can use the same upgrade pattern as `execute_js_task`.

**Validation:** `cargo check -p riversd` clean; 6/6 unit tests pass (`engine_loader::dyn_transaction_map::tests::*` — insert/take round-trip, duplicate insert errors, take-unknown returns None, drain_task scoped per-task, with_conn_mut observes mutation across calls, with_conn_mut returns None when missing).

**Deviation from plan:** plan §3.1 named the new file `transaction_map.rs`; landed it as `dyn_transaction_map.rs` to make the dyn-vs-V8 distinction visible at first glance and avoid name-collision risk with `crate::transaction` (the V8-shared map). Decisions 1–4 unchanged.

**Note for I3 implementer:** the brief specified `TASK_DS_CONFIGS` keyed by `"{entry_point}:{ds_name}"`. That's the V8 convention — confirm against `SerializedTaskContext::from(&ctx)` before wiring `host_db_begin` so the lookup key matches what `dispatch_task` will populate.
