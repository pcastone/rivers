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

- [ ] **1.1** Add `swc_core = { version = "0.90", features = ["ecma_parser_typescript", "ecma_transforms_typescript", "ecma_codegen", "common"] }` to `crates/riversd/Cargo.toml`. **Validate:** `cargo build -p riversd` succeeds.
- [ ] **1.2** Replace body of `compile_typescript()` in `crates/riversd/src/process_pool/v8_config.rs:133-193` with swc full-transform pipeline (parse → `typescript::typescript()` pass → emit). ES2022 target, TC39 Stage 3 decorators enabled, `decorators: true` in parser. **Validate:** Task 1.5 suite passes.
- [ ] **1.3** Delete `strip_type_annotations()` (`v8_config.rs:196-226`) and the line-based stripping loop (`:134-192`). Update docstring to describe swc-backed full transform. **Validate:** `cargo build` clean, no dead-code warnings.
- [ ] **1.4** Add `.tsx` rejection at compile entry: `TaskError::HandlerError("JSX/TSX not supported in Rivers v1: <path>")`. **Validate:** unit test asserts the error.
- [ ] **1.5** Strengthen `crates/riversd/tests/process_pool_tests.rs:288`: replace `contains("const x")` with an equality assertion. Add cases for parameter annotation, variable annotation, `type`-only import, generic function, `as` assertion, `enum`, `namespace`, `satisfies`, TC39 decorator, `interface` block, `const` assertion. **Validate:** `cargo test compile_typescript` green.
- [ ] **1.6** Update the 3 TS tests in `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` to match new semantics (swc is a superset; should still pass unchanged, may need assertion tweaks). **Validate:** `cargo test wasm_and_workers` green.
- [ ] **1.7** Run probe bundle against a built riversd — expect A, B, C, D, E, H, I green; F and G still red (waiting on Phases 4/5). **Validate:** `./run-probe.sh` matches expected-at-Phase-1 matrix.
- [ ] **1.8** Log decision (full-transform vs strip-only) in `changedecisionlog.md`; append `changelog.md`. **Validate:** both files have new entries.

## Phase 2 — Bundle-load-time compile + module cache — spec §2.6, §2.7, §3.4

- [ ] **2.1** Define `CompiledModule { source_path: PathBuf, compiled_js: String, source_map: String }` in `crates/rivers-runtime/src/module_cache.rs` (new file). **Validate:** type compiles; documented.
- [ ] **2.2** Define `BundleModuleCache` wrapping `HashMap<PathBuf, CompiledModule>` keyed by canonicalised absolute path; methods `insert`, `get`, `iter`, `len`. **Validate:** unit test for insert/get round-trip.
- [ ] **2.3** Extend `crates/rivers-runtime/src/loader.rs:load_bundle()` to walk each app's `libraries/` subtree and enumerate `.ts` + `.js` files. **Validate:** unit test with a fixture bundle reports every file under `libraries/`.
- [ ] **2.4** For each `.ts` file in 2.3, invoke `compile_typescript()` (Phase 1) and populate `BundleModuleCache` with compiled JS + source map; for `.js`, store source verbatim. **Validate:** integration test with mixed `.ts`/`.js` handlers produces a full cache.
- [ ] **2.5** Fail-fast: any compile error aborts bundle load with path-and-line error; no partial cache. **Validate:** fixture with one broken `.ts` fails load; error includes file + line.
- [ ] **2.6** Reject `.tsx` at bundle load before swc invocation with spec §2.5 message. **Validate:** fixture with `.tsx` fails with the exact message.
- [ ] **2.7** Attach populated `BundleModuleCache` to `LoadedApp` / ProcessPool startup so V8 dispatch can read it. **Validate:** dispatch test confirms cache reachable from execution thread.
- [ ] **2.8** Rewrite `execution.rs:resolve_module_source()` (`:416-437`) to read pre-compiled JS from `BundleModuleCache`. Retain the `_source` inline-injection path for unit tests (call swc there). **Validate:** first request on a `.ts` handler no longer invokes swc (log/metric counter assertion).
- [ ] **2.9** Integration test: bundle with a syntax error in `libraries/handlers/orders.ts` fails `load_bundle`; riversd does not start. **Validate:** test passes.
- [ ] **2.10** Integration test: valid multi-file bundle loads; cache contains every file under `libraries/`. **Validate:** test passes.
- [ ] **2.11** Log decision + changelog entries.

## Phase 3 — Circular import detection — spec §3.5

- [ ] **3.1** During 2.4's compile pass, extract `import` specifiers from each compiled module's swc AST; store as `Vec<PathBuf>` per module. **Validate:** unit test on a 3-import file returns all 3 paths.
- [ ] **3.2** Build per-app dependency graph after all modules compile. **Validate:** unit test: 5-module graph has correct edges.
- [ ] **3.3** DFS cycle detection; on cycle, fail bundle load with spec §3.5 formatted error (full path chain). **Validate:** fixtures for 2-module cycle, 3-module cycle, self-import all reject.
- [ ] **3.4** Document circular-import rejection in tutorial + `rivers.d.ts` preamble. **Validate:** tutorial has a "gotcha" subsection.

## Phase 4 — Module resolve callback with app-boundary enforcement (Defect 4) — spec §3.1–3.3, §3.6

- [ ] **4.1** Replace stub callback at `execution.rs:65-72` with real impl: require explicit extension, reject bare specifiers, canonicalise relative paths, enforce chroot inside `{app}/libraries/`. **Validate:** unit tests — `./sibling.ts` resolves; `./sibling` (missing ext), `"lodash"` (bare), `/etc/passwd` (abs), `../../../other-app/foo.ts` (escape) all reject.
- [ ] **4.2** Callback looks up resolved absolute path in `BundleModuleCache` (Phase 2) and compiles a `v8::Module` from cached JS. **Validate:** dispatch test where handler imports a sibling succeeds.
- [ ] **4.3** Thread app-libraries-root into callback via closure capture (not thread-local). **Validate:** code review.
- [ ] **4.4** Rejection errors include referrer file + specifier + resolved path + boundary (spec §3.2 format). **Validate:** unit test inspects error string.
- [ ] **4.5** Run probe — case F passes. **Validate:** `./run-probe.sh` F green.

## Phase 5 — Module namespace entrypoint lookup (Defect 3) — spec §4

- [ ] **5.1** Modify `execute_as_module()` (`execution.rs:29-97`) to return the `v8::Local<v8::Module>` handle after `module.evaluate()`. **Validate:** compiler happy; no lifetime regressions.
- [ ] **5.2** In `execute_js_task()` (`:205-291`), branch entrypoint lookup: pass module handle into `call_entrypoint` on module mode; classic path unchanged. **Validate:** unit test.
- [ ] **5.3** Extend `call_entrypoint()` (`:352-410`) to accept optional module handle; when present, look up on `module.get_module_namespace()`. **Validate:** unit test — `export function handler(ctx)` resolves without global write.
- [ ] **5.4** Remove "V1: module must set on globalThis" comment at `execution.rs:222-224`. **Validate:** grep confirms removal.
- [ ] **5.5** Regression: classic script handlers still look up on global. **Validate:** existing canary handler tests remain green.
- [ ] **5.6** Run probe — case G passes; A + H still pass. **Validate:** `./run-probe.sh` 9/9 (except source-map tests land in Phase 6).

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

- [ ] **8.1** Add MCP section to `docs/guide/tutorials/tutorial-js-handlers.md` with spec §7.1 full example + §7.2 Common Errors table. **Validate:** tutorial renders; example passes `riverpackage validate`.
- [ ] **8.2** Cross-ref `docs/arch/rivers-application-spec.md §13` to spec §7. **Validate:** link present.
- [ ] **8.3** Verify `canary-bundle/canary-sql/app.toml` MCP block matches documented form; fix any drift. **Validate:** canary MCP tests still pass.

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
