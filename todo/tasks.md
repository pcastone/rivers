# JavaScript / TypeScript Pipeline — Implementation Plan

> **Branch:** TBD (new branch off `docs/guide-v0.54.0-updates` or fresh off `main`)
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md` (v1.0, 2026-04-21)
> **Defect report:** `dist/rivers-upstream/rivers-ts-pipeline-findings.md`
> **Probe:** `dist/rivers-upstream/cb-ts-repro-bundle/` (to be moved to `tests/fixtures/ts-pipeline-probe/` in Phase 0.2)
> **Supersedes:** `processpool-runtime-spec-v2 §5.3`
> **Target version:** 0.55.0 (breaking handler semantics)

**Goal:** Close 6 TS-pipeline defects CB filed. Ordinary TS idioms (typed params, generics, `type` imports, `export function handler`, multi-file bundles) dispatch cleanly end-to-end; transactional handlers gain an ACID primitive via `ctx.transaction()`; probe bundle passes 9/9; canary goes from 69/69 → 69+N/69+N with zero regressions.

**Grounding facts from exploration (verified against current source, not spec):**
1. TS compilation is **lazy at request time** today (`execution.rs:416-437`). Spec §2.6/2.7 move it to bundle-load time — a larger structural change than spec §10 implies.
2. `crates/riversd/src/transaction.rs` already defines a complete `TransactionMap`. `ctx.transaction()` is a wiring job, not a new implementation.
3. `swc_core` is not in any Cargo.toml anywhere in the workspace. Fresh integration.
4. `rivers.d.ts` does not exist anywhere in the repo. Fresh file.
5. `canary-bundle/canary-handlers/libraries/handlers/*.ts` are real TS files (not `.ts`-named JS), but contain no true TS syntax (ES5 subset only).

**Spec corrections to resolve during implementation:**
1. **§6.4 MongoDB row** claims `supports_transactions = true` — MongoDB is a plugin driver, not verified in this repo. Pick verify-or-amend in Task 7.8.
2. **§10 item 1** conflates swc drop-in (Phase 1, 2–3 days) with exhaustive-upfront compilation (Phase 2, ~1 week). Treat as separate phases.
3. **Validation pipeline caveat** — `validate_*` functions in `crates/rivers-runtime/src/` exist but are not invoked during `load_bundle`. Phase 2 code goes into `loader.rs:load_bundle()` directly, not the validation pipeline.

**Critical path:** 1 → 2 → 4 → 5 gates every handler-level unblock. Phases 3, 6, 7, 8–10 can parallelise after 2 lands. Phase 11 closes.

---

## Phase 0 — Preflight

- [x] **0.1** Archive filesystem-driver epic from `todo/tasks.md` to `todo/gutter.md`; write new task list. **Validate:** gutter ends with filesystem epic; tasks.md starts with Phase 1. (Done 2026-04-21.)
- [x] **0.2** Move probe bundle from gitignored `dist/rivers-upstream/cb-ts-repro-bundle/` to tracked `tests/fixtures/ts-pipeline-probe/`; findings.md also copied to `tests/fixtures/` so the bundle's `../rivers-ts-pipeline-findings.md` link resolves. (Done 2026-04-21.)
- [x] **0.3** Added `just probe-ts` recipe to `Justfile` (default base `http://localhost:8080/cb-ts-repro/probe`). No GitHub CI wiring — the probe, like the canary, runs against a real riversd + infra, not the CI sandbox. (Done 2026-04-21.)

## Phase 1 — swc drop-in (Defects 1, 2) — spec §2.1–2.5

- [x] **1.1** Add `swc_core` to `crates/riversd/Cargo.toml`. **Correction:** spec says `v0.90` but crates.io current is `v64` (swc uses major-per-release); used `v64` + features `ecma_ast`, `ecma_parser`, `ecma_parser_typescript`, `ecma_transforms_typescript`, `ecma_codegen`, `ecma_visit`, `common`, `common_sourcemap`. `cargo build -p riversd` green. (Done 2026-04-21.)
- [x] **1.2** Replaced body of `compile_typescript()` with swc full-transform pipeline (parse → resolver → `typescript()` → fixer → `to_code_default`). `TsSyntax { decorators: true }`, `EsVersion::Es2022`. (Done 2026-04-21.)
- [x] **1.3** Deleted `strip_type_annotations()` + line-based loop. Docstring rewritten to describe the swc pipeline. No dead-code warnings on the touched file. (Done 2026-04-21.)
- [x] **1.4** `.tsx` rejection at compile entry returns `TaskError::HandlerError("JSX/TSX is not supported in Rivers v1: <path>")`. Unit test `compile_typescript_rejects_tsx` green. (Done 2026-04-21.)
- [x] **1.5** Replaced the single `contains("const x")` assertion with 16 rigorous cases in `process_pool_tests.rs`: variable/parameter/return annotations, generics, type-only imports, `as`, `satisfies`, interface, type-alias, `enum`, `namespace`, `as const`, TC39 decorator, `.tsx` rejection, syntax-error reporting, JS passthrough. All 16 green. (Done 2026-04-21.)
- [x] **1.6** Verified the 3 pre-existing TS tests in `wasm_and_workers.rs` + `execute_typescript_handler` dispatch test still pass unchanged — swc is a superset of the old stripper's semantics for those inputs. (Done 2026-04-21.)
- [ ] **1.7** **Deferred to Phase 5 integration run.** At Phase 1 end the probe would only re-test cases A/B/C/D/E/H/I (already covered by 16 unit tests). Real signal comes at Phase 5 when 9/9 is achievable. Running it now requires full deploy + service registry + infra for no net-new coverage.
- [x] **1.8** Created `changedecisionlog.md` (first entry: swc full-transform + v0.90→v64 correction + decorator-lowering strategy + source-map deferral) and appended `todo/changelog.md` with Phase 1 summary. (Done 2026-04-21.)

## Phase 2 — Bundle-load-time compile + module cache — spec §2.6, §2.7, §3.4

- [x] **2.1** Defined `CompiledModule` + `BundleModuleCache` in new `crates/rivers-runtime/src/module_cache.rs` + registered in `lib.rs`. `Arc<HashMap<PathBuf, CompiledModule>>` backing for O(1) clone. 3 unit tests green. (Done 2026-04-21.)
- [x] **2.2** `BundleModuleCache::{from_map, get, iter, len, is_empty}` — same file. Canonicalised-path key contract documented. (Done 2026-04-21.)
- [x] **2.3** Walk + compile moved to `crates/riversd/src/process_pool/module_cache.rs` (not rivers-runtime — swc_core layering, see changedecisionlog.md). Recursive walker that skips non-source files. Unit test `walks_ts_and_js_skips_other` green. (Done 2026-04-21.)
- [x] **2.4** Same file. `.ts` → `compile_typescript`; `.js` → verbatim. `source_map` field left empty (Phase 6 populates). Unit test green. (Done 2026-04-21.)
- [x] **2.5** Fail-fast via `RiversError::Config("TypeScript compile error in app '<name>', file <path>: ...")`. Unit test `fails_fast_on_compile_error` green. (Done 2026-04-21.)
- [x] **2.6** `.tsx` rejected at walk time (before swc call) with "JSX/TSX is not supported in Rivers v1: <path>". Unit test `rejects_tsx_at_walk_time` green. (Done 2026-04-21.)
- [x] **2.7** Global `MODULE_CACHE: OnceCell<RwLock<Arc<BundleModuleCache>>>` with atomic-swap semantics. Installed from `bundle_loader/load.rs:load_and_wire_bundle` immediately after cross-ref validation. Hot-reload-ready per spec §3.4. Unit test `install_and_get_roundtrip` green. (Done 2026-04-21.)
- [x] **2.8** `resolve_module_source` rewritten: primary path = `get_module_cache().get(canonical_abs_path)`; fallback = disk read + live compile (with debug log). Defence-in-depth for modules outside `libraries/` until Phase 4 resolver lands. 124 pre-existing `process_pool` tests still green. (Done 2026-04-21.)
- [x] **2.9** Covered by unit test `fails_fast_on_compile_error` — a broken `.ts` in a fixture libraries tree produces the exact `ServerError::Config` surface the real load path exposes. No separate integration test needed. (Done 2026-04-21.)
- [x] **2.10** Covered by unit test `walks_ts_and_js_skips_other` — multi-file tree compiles, cache has every `.ts` + `.js`, non-source skipped. No separate integration test needed. (Done 2026-04-21.)
- [x] **2.11** Three decision entries in `changedecisionlog.md` (rivers-runtime/riversd split, OnceCell rationale, fallback on miss); Phase 2 summary in `todo/changelog.md`. (Done 2026-04-21.)

## Phase 3 — Circular import detection — spec §3.5

- [x] **3.1** Added `compile_typescript_with_imports` in `v8_config.rs` — same pipeline as `compile_typescript` but walks the post-transform Program for `ImportDecl`/`ExportAll`/`NamedExport` and returns `(String, Vec<String>)`. `imports` field added to `CompiledModule` in rivers-runtime. (Done 2026-04-21.)
- [x] **3.2** `check_cycles_for_app` in `riversd/.../module_cache.rs` resolves each module's raw specifiers against its referrer's directory, filters to same-app edges, and builds a `HashMap<PathBuf, Vec<PathBuf>>`. (Done 2026-04-21.)
- [x] **3.3** DFS with white/gray/black colouring; back-edge to gray yields the cycle path, formatted per spec §3.5. 5 unit tests green: two-module cycle, three-module cycle, self-import (side-effect form), acyclic-tree passthrough, type-only-imports-not-cycles. (Done 2026-04-21.)
- [ ] **3.4** Deferred to Phase 8.1 (tutorial covers `rivers.d.ts` + handler patterns + TS gotchas in one pass). Cycle-detection test names + error message format are the interim contract.

## Phase 4 — Module resolve callback with app-boundary enforcement (Defect 4) — spec §3.1–3.3, §3.6

- [x] **4.1** Replaced the stub callback in `execute_as_module` with `resolve_module_callback`. Checks: (a) `./` or `../` prefix required (bare specifiers throw), (b) `.ts` or `.js` extension required, (c) canonicalisation against referrer's parent directory, (d) lookup in `BundleModuleCache` (cache residency is the boundary check — files outside `{app}/libraries/` are not in the cache, so they naturally reject). Errors thrown via `v8::Exception::error` + `throw_exception`. (Done 2026-04-21.)
- [x] **4.2** Callback compiles a `v8::Module` from `CompiledModule.compiled_js` via `script_compiler::compile_module`. Registers the new module's `get_identity_hash()` → absolute path in `TASK_MODULE_REGISTRY` so nested resolves work. (Done 2026-04-21.)
- [x] **4.3** Referrer's path is looked up from `TASK_MODULE_REGISTRY` (thread-local, populated when each module is compiled). V8's resolve callback is `extern "C" fn` and cannot capture state through a Rust closure, so thread-local is the only practical bridge. (Decision note: plan said "not thread-local" — that's infeasible with V8's callback signature. Spec correction.) (Done 2026-04-21.)
- [x] **4.4** Rejection errors are thrown as V8 exceptions that propagate out of `module.instantiate_module()`; message format:
  - bare specifier: `module resolution failed: bare specifier "x" not supported — use "./" or "../" relative import`
  - missing ext: `module resolution failed: import specifier "./x" has no extension; hint: add ".ts" or ".js"`
  - canonicalise failure: `module resolution failed: cannot resolve "./x" from {referrer} — {io-error}`
  - not in cache: `module resolution failed: "./x" resolved to {abs} which is not in the bundle module cache (may be outside {app}/libraries/ or not pre-compiled)`
  Close to but not verbatim spec §3.2 shape; the information content matches. (Done 2026-04-21.)
- [ ] **4.5** Deferred to Phase 5 end-to-end probe run. Resolver build is clean; 129 process_pool tests still green. Case F requires module-namespace entrypoint lookup (Phase 5) to complete because the probe case uses `export function handler`. Probe run validates F + G together at Phase 5 end.

## Phase 5 — Module namespace entrypoint lookup (Defect 3) — spec §4

- [x] **5.1** `execute_as_module` captures `module.get_module_namespace()` as a `v8::Global<v8::Object>` and stashes it in `TASK_MODULE_NAMESPACE` thread-local. Cleared in `TaskLocals::drop`. Avoids lifetime plumbing across function-signature boundaries. (Done 2026-04-21.)
- [x] **5.2** Thread-local bridge means no signature change needed on `execute_js_task`; module handle is implicit via the thread-local. Cleaner than threading `Option<v8::Local<v8::Module>>` through three functions. (Done 2026-04-21.)
- [x] **5.3** `call_entrypoint` reads `TASK_MODULE_NAMESPACE` — Some → module namespace lookup, None → globalThis. `ctx` stays on global in both modes (inject_ctx_object injects it there). (Done 2026-04-21.)
- [x] **5.4** Removed the "V1: module must set on globalThis" comment at execution.rs:222-224; replaced with accurate spec §4 reference. (Done 2026-04-21.)
- [x] **5.5** New regression test `execute_classic_script_still_uses_global_scope` — plain `function onRequest(ctx)` dispatch passes. Existing 129 process_pool tests also still green. (Done 2026-04-21.)
- [x] **5.6** New dispatch test `execute_module_export_function_handler` — `export function handler(ctx)` returns via namespace lookup, confirming probe case G scenario works end-to-end without globalThis.handler workaround. Probe run against real riversd deferred to Phase 10. (Done 2026-04-21.)

## Phase 6 — Source maps + stack trace remapping — spec §5

- [ ] **6.1** Enable swc source-map emission in Phase 1's compile path; store string in `CompiledModule.source_map`. **Validate:** cache entries have non-empty maps.
- [ ] **6.2** Register `SetPrepareStackTraceCallback` at isolate acquisition in `execution.rs:acquire_isolate` path. **Validate:** callback fires when a handler throws.
- [ ] **6.3** Implement callback: for each `CallSite`, extract `scriptName/line/column`, look up source map in `BundleModuleCache`, run swc source-map consumer to remap. **Validate:** unit test with known `.ts → .js` remaps line 47 (compiled) → line 32 (source).
- [ ] **6.4** Wire remapped stacks into `execute_js_task` error path: write to per-app log via `AppLogRouter` with `trace_id`. **Validate:** integration test — handler throws; `log/apps/<app>.log` contains remapped trace.
- [ ] **6.5** Debug mode only (`debug = true` in app config): include remapped trace in error response envelope under `debug.stack` (spec §5.3 JSON shape). Non-debug omits stack. **Validate:** two integration tests, debug-on and debug-off.
- [ ] **6.6** Close `processpool-runtime-spec-v2 Open Question #5` — cross-ref note in both specs.

## Phase 7 — ctx.transaction() (Defect 5) — spec §6

- [ ] **7.1** Add `TASK_TRANSACTION_MAP: RefCell<Option<Arc<TransactionMap>>>` thread-local in `crates/riversd/src/process_pool/v8_engine/task_locals.rs`. **Validate:** build clean.
- [ ] **7.2** Set/clear thread-local in `TaskLocals::set()` and `Drop` impl. **Validate:** unit test — after task, thread-local is None.
- [ ] **7.3** Implement `ctx_transaction_callback` in `v8_engine/context.rs`: args `(datasource_name: string, fn: Function)`; resolve datasource → driver → check `supports_transactions()` → `begin_transaction()` → install TransactionMap entry → invoke JS callback → commit on Ok / rollback on throw / rollback-via-guard on panic → clear entry. **Validate:** unit tests for commit + rollback.
- [ ] **7.4** Inject callback at ctx construction (same section as existing callbacks near `context.rs:67-73`). **Validate:** `ctx.transaction` reachable from handler.
- [ ] **7.5** Modify `ctx_dataview_callback` at `context.rs:514` to check `TASK_TRANSACTION_MAP`: if txn active, route via held connection; if datasource mismatches, throw `TransactionError: dataview "{name}" uses datasource "{ds}" which differs from transaction datasource "{txn_ds}"`. **Validate:** integration test for mismatch path.
- [ ] **7.6** Throw `TransactionError: nested transactions not supported` if `ctx.transaction()` called while thread-local already holds an entry. **Validate:** unit test.
- [ ] **7.7** Throw `TransactionError: datasource "{name}" does not support transactions` when driver `supports_transactions() = false`. **Validate:** integration test with Faker driver.
- [ ] **7.8** Verify spec §6.4 table vs actual `supports_transactions()` returns: PG/MySQL/SQLite = true (confirmed); Faker/EventBus/Memcached/Redis = false (confirmed). MongoDB/Cassandra/CouchDB/Elasticsearch/Kafka/LDAP are plugin drivers — pick (a) verify by plugin load, or (b) amend spec §6.4 to mark "plugin — verify at plugin load." Decision-log entry. **Validate:** decision logged; spec updated if (b).
- [ ] **7.9** Integration tests on PG cluster (192.168.2.209): commit persists; rollback undoes; cross-ds throw; nested throw; unsupported throw. **Validate:** 5/5 green.
- [ ] **7.10** Log decisions in `changedecisionlog.md` for spec items #10 (ambient ctx), #11 (no nesting/XA), #12 (cross-ds throw).

## Phase 8 — MCP view documentation (Defect 6) — spec §7

- [x] **8.1** Updated `docs/guide/tutorials/tutorial-mcp.md` Step 1 with the `[api.views.mcp.handler] type = "none"` sentinel (previously missing — tutorial had drifted from the canary-verified form) and added the spec §7.2 Common Errors table. (Done 2026-04-21.)
- [x] **8.2** Added a cross-reference note at the top of `docs/arch/rivers-application-spec.md §13` pointing to `rivers-javascript-typescript-spec.md` as the authoritative source for the runtime TS/module behaviour. (Done 2026-04-21.)
- [x] **8.3** Verified `canary-bundle/canary-sql/app.toml` MCP block matches the documented form (has `[api.views.mcp.handler] type = "none"`, `view_type = "Mcp"`, `method = "POST"`). No drift. (Done 2026-04-21.)

## Phase 9 — rivers.d.ts — spec §8

- [ ] **9.1** Create `types/rivers.d.ts` at repo root; declare `Rivers` global (`Rivers.log`, `Rivers.crypto`, `Rivers.keystore`, `Rivers.env`). **Validate:** `tsc --noEmit` on sample handler tsconfig resolves types.
- [ ] **9.2** Declare ctx surface: `ctx.data`, `ctx.resdata`, `ctx.dataview(name, params?)`, `ctx.store.{get,set,del}`, `ctx.datasource(name)`, `ctx.transaction(ds, fn)`, `ctx.trace_id`, `ctx.node_id`, `ctx.app_id`, `ctx.session`. JSDoc each. **Validate:** IDE completion works on sample handler.
- [ ] **9.3** Declare `QueryResult`, `ExecuteResult`, `ParsedRequest`, `TransactionError`. **Validate:** types exported.
- [ ] **9.4** Do NOT declare `console`, `process`, `require`, `fetch` (spec §8.3 negative). **Validate:** sample handler using `fetch` gets a type error.
- [ ] **9.5** Add sample `tsconfig.json` + reference to `types/rivers.d.ts` in the getting-started tutorial. **Validate:** copy-paste into new handler project gives completion.
- [ ] **9.6** Wire `types/rivers.d.ts` into `cargo deploy` artifact set — deployed instance ships file at `types/rivers.d.ts`. **Validate:** deployed bundle has the file.

## Phase 10 — Canary Fleet TS + transaction coverage — spec §9

- [ ] **10.1** Add `.ts` handler files under `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/`: `param-strip.ts`, `var-strip.ts`, `import-type.ts` (+ helper), `generic.ts`, `multimod.ts` (+ helper), `export-fn.ts`, `enum.ts`, `decorator.ts`, `namespace.ts`, `sourcemap.ts`. (Circular case handled separately in 10.6.) **Validate:** each returns a `TestResult` shaped per `test-harness.ts`.
- [ ] **10.2** Add transaction handlers: `txn-commit.ts`, `txn-rollback.ts`, `txn-cross-ds.ts`, `txn-nested.ts`, `txn-unsupported.ts`. **Validate:** each returns a `TestResult`.
- [ ] **10.3** Register every new handler in `canary-bundle/canary-handlers/app.toml` under `[api.views.*]` + `[api.views.*.handler]` with `language = "typescript"` + correct `entrypoint`. **Validate:** `riverpackage validate canary-bundle/canary-handlers` green.
- [ ] **10.4** Add "TYPESCRIPT" profile to `canary-bundle/run-tests.sh` with `test_ep` lines for 10.1. **Validate:** script runs; each reports PASS.
- [ ] **10.5** Add "TRANSACTIONS-TS" profile with `test_ep` lines for 10.2 (reuse `PG_AVAIL` conditional). **Validate:** 5/5 PASS on PG cluster.
- [ ] **10.6** Circular-import test runs outside `run-tests.sh`: standalone shell test invokes `riverpackage validate` on a fixture with a cycle and asserts non-zero exit + expected error. **Validate:** test passes.
- [ ] **10.7** Source-map test asserts the per-app log contains a `.ts:line:col` reference matching source, not compiled `.js:line:col`. **Validate:** integration test green.
- [ ] **10.8** Canary fleet total goes from 69/69 to 69+N/69+N green. **Validate:** `run-tests.sh` summary — zero fails, zero errors.

## Phase 11 — Cleanup + docs + version bump

- [ ] **11.1** Delete remaining dead code from old TS pipeline. **Validate:** `cargo build --workspace` clean; no unused/dead warnings.
- [ ] **11.2** Mark `processpool-runtime-spec-v2 §5.3` superseded-by. **Validate:** cross-ref present.
- [ ] **11.3** Update `CLAUDE.md` "Key Crates" table if `rivers-runtime` gained responsibilities (module cache). **Validate:** table reflects reality.
- [ ] **11.4** Append per-phase `changelog.md` entries. **Validate:** 11 entries added.
- [ ] **11.5** Version bump 0.54.1 → 0.55.0. Update `VERSION`, workspace `Cargo.toml` version, CLAUDE.md rivers-dev skill mentions. **Validate:** `riversctl --version` reports 0.55.0.
- [ ] **11.6** `cargo deploy` fresh instance; canary green; probe 9/9. **Validate:** zero failures.
- [ ] **11.7** Git commit per phase (11 commits). **Validate:** `git log --oneline` reads as a clean story.

---

## Files touched (hot list)

- `crates/riversd/Cargo.toml` — swc_core dep
- `crates/riversd/src/process_pool/v8_config.rs` — swc body, stripper deleted
- `crates/riversd/src/process_pool/v8_engine/execution.rs` — resolver, namespace lookup, stack-trace callback, cache lookup
- `crates/riversd/src/process_pool/v8_engine/context.rs` — `ctx.transaction`, txn-aware `ctx.dataview`
- `crates/riversd/src/process_pool/v8_engine/task_locals.rs` — `TASK_TRANSACTION_MAP`
- `crates/riversd/src/transaction.rs` — reuse existing `TransactionMap`
- `crates/riversd/tests/process_pool_tests.rs` — strengthened regressions
- `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` — updated TS tests
- `crates/rivers-runtime/src/loader.rs` — cache population
- `crates/rivers-runtime/src/module_cache.rs` — new
- `canary-bundle/canary-handlers/app.toml` + `libraries/handlers/ts-compliance/*.ts`
- `canary-bundle/run-tests.sh` — new profiles
- `types/rivers.d.ts` — new
- `docs/guide/tutorials/tutorial-js-handlers.md` — MCP section
- `docs/arch/processpool-runtime-spec-v2.md` — supersede header
- `tests/fixtures/ts-pipeline-probe/` — moved from `dist/rivers-upstream/cb-ts-repro-bundle/`

## End-to-end verification

1. `cargo test --workspace` — all passing (new unit tests in Phases 1/2/3/4/5/7).
2. `cd tests/fixtures/ts-pipeline-probe && ./run-probe.sh` — 9/9 pass.
3. `cargo deploy /tmp/rivers-canary && cd canary-bundle && ./run-tests.sh` — zero fails, zero errors.
4. Sample handler with typed params, `import { helper } from "./helpers.ts"`, `export function handler(ctx)`, `ctx.transaction("pg", () => { ... })` dispatches successfully.
