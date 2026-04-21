# TS Pipeline Spec-Compliance Gap Closure

> **Branch:** `docs/guide-v0.54.0-updates` (continues TS pipeline work)
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md`
> **Gap analysis source:** this session's §-by-§ walkthrough. All 6 CB defects closed; remaining gaps are spec-compliance format/plumbing issues and canary coverage shortfall.
> **Prior:** TS pipeline Phases 0–11 + Phase 6 completion archived in `todo/gutter.md`.

**Goal:** close every observable gap between the implementation and the spec, split into four priority tiers. P0 is high-impact compliance (canary validation + runtime debug flag); P1 is format drift that changes observable surface; P2 is nice-to-haves; P3 is spec-doc corrections.

**Critical path:** G1 (canary TS coverage) is the biggest remaining gap — spec §9.2 lists 16 required test IDs; 1 is shipped at canary level today.

---

## G0 — Foundation decisions (blocking P0+)

Before executing G1–G8, two small calls clear ambiguity for the rest of the plan:

- [x] **G0.1** Decision: **option (a)** — amend spec §5.3 envelope shape to match existing `ErrorResponse` convention (`{code, message, trace_id, details.stack}`). Zero code change; spec edit in G8.4. Logged in `changedecisionlog.md`. (Done 2026-04-21.)
- [x] **G0.2** Decision: **option (a)** — drop `Rivers.db / Rivers.view / Rivers.http` from spec §8.3. None of these exist at runtime; aspirational stubs would create broken type-check signals. Spec edit in G8.6. Logged in `changedecisionlog.md`. (Done 2026-04-21.)

---

## P0 — High-impact spec compliance

### G1 — Canary TS-syntax coverage (spec §9.2)

**Scope:** 10 TS-syntax handler endpoints + 1 circular-import shell test + run-tests.sh profile. Each handler returns a `TestResult` per `test-harness.ts`; test harness asserts `passed=true`.

**Rationale:** spec §9.2 mandates canary-level exercise of every TS compiler feature. Unit tests prove `compile_typescript` works; canary proves the full dispatch invokes it correctly on a running riversd. Biggest single compliance gap.

**Files:**
- new: `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/*.ts` (one per case)
- edit: `canary-bundle/canary-handlers/app.toml` (register 10 views under `[api.views.ts_*]`)
- edit: `canary-bundle/run-tests.sh` (TYPESCRIPT profile expansion)
- new: `canary-bundle/tests/circular-import-rejection.sh` (standalone; not part of run-tests.sh)

Tasks:

- [ ] **G1.1** `param-strip.ts` — handler with `function h(ctx: any)` typed params; return `{ passed: true }` if it loads. Exercises probe case B.
- [ ] **G1.2** `var-strip.ts` — `const x: number = 42` body; validates variable annotation stripping (probe case C).
- [ ] **G1.3** `import-type.ts` + `import-type-helpers.ts` — `import { type Something, foo } from "./import-type-helpers.ts"`; uses `foo` at runtime (probe case D).
- [ ] **G1.4** `generic.ts` — `function identity<T>(x: T): T { return x; }` + call site (probe case E).
- [ ] **G1.5** `multimod.ts` + `multimod-helpers.ts` — relative import across two files (probe case F). Both must end up in the module cache at bundle load.
- [ ] **G1.6** `export-fn.ts` — `export function handler(ctx) { ... }`; verifies module namespace entrypoint lookup (probe case G).
- [ ] **G1.7** `enum.ts` — `enum Status { Active, Inactive }` with runtime use; verifies swc enum lowering is wired.
- [ ] **G1.8** `decorator.ts` — TC39 Stage 3 decorator on a class method; verifies parser accepts syntax + V8 executes.
- [ ] **G1.9** `namespace.ts` — `namespace util { export const VERSION = "1.0"; }` with runtime read; verifies swc namespace lowering.
- [ ] **G1.10** Circular-import shell test: fixture bundle with an a.ts ↔ b.ts cycle; `riverpackage validate` must fail with the spec §3.5 error format. Lives at `canary-bundle/tests/circular-import-rejection.sh`.
- [ ] **G1.11** Register all 9 runtime handlers in `canary-handlers/app.toml` under `[api.views.ts_*]` (10 views — 9 runtime + 1 path decision helper if needed).
- [ ] **G1.12** Extend `run-tests.sh` TYPESCRIPT profile with 9 `test_ep` lines; `ts-sourcemap` already present from Phase 6H.

**Validate:** `./run-tests.sh` shows PASS for all 9 new IDs + the sourcemap probe; canary total goes from 69+N/69+N to 69+N+9/69+N+9 green.

**Effort:** ~3 hours (mostly mechanical handler wrapper code).

### G2 — Canary transaction handlers (spec §9.2)

**Scope:** 5 transaction test endpoints. Requires a live PG datasource configured for the canary app.

**Files:**
- new: `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/txn-commit.ts`, `txn-rollback.ts`, `txn-cross-ds.ts`, `txn-nested.ts`, `txn-unsupported.ts`
- edit: `canary-bundle/canary-handlers/app.toml` (register under TRANSACTIONS-TS profile)
- edit: `canary-bundle/canary-handlers/resources.toml` (add `pg` datasource if not present)
- edit: `canary-bundle/run-tests.sh` (TRANSACTIONS-TS profile extension with PG_AVAIL gate)

Tasks:

- [ ] **G2.1** `txn-commit.ts` — `ctx.transaction("pg", () => { ctx.dataview("insert"); return {ok:true}; })`; verify row committed via a follow-up ctx.dataview select. **Validate:** assertion passes against live PG.
- [ ] **G2.2** `txn-rollback.ts` — `ctx.transaction` callback throws; verify no row persisted via follow-up select. **Validate:** row count unchanged.
- [ ] **G2.3** `txn-cross-ds.ts` — `ctx.transaction("pg", () => ctx.dataview("mysql_view"))`; asserts the spec §6.2 cross-ds TransactionError shape. **Validate:** error message matches verbatim.
- [ ] **G2.4** `txn-nested.ts` — `ctx.transaction("pg", () => ctx.transaction("pg", ...))`; asserts `TransactionError: nested transactions not supported`.
- [ ] **G2.5** `txn-unsupported.ts` — `ctx.transaction("faker", ...)`; asserts `TransactionError: datasource "faker" does not support transactions`. Requires faker datasource declared in canary resources.
- [ ] **G2.6** `resources.toml` has PG datasource pointing at 192.168.2.209; re-use existing canary-sql pattern.
- [ ] **G2.7** `run-tests.sh` TRANSACTIONS-TS profile — existing block is placeholder (spec surface tests from Phase 7); extend with PG_AVAIL gate for G2.1–G2.5. Tests skip cleanly on a non-infra deploy.

**Validate:** 5/5 PASS on PG cluster; SKIP cleanly without PG.

**Effort:** ~2 hours + infra access for the roundtrip run.

### G3 — Per-app debug flag runtime plumbing (spec §5.3)

**Scope:** replace `cfg!(debug_assertions)` gate in `error_response::map_view_error` with a runtime read of `AppConfig.base.debug` for the matched app.

**Files:**
- edit: `crates/riversd/src/error_response.rs` — `map_view_error` signature adds `debug_enabled: bool`
- edit: `crates/riversd/src/server/view_dispatch.rs` — look up matched view's app's `AppConfig.base.debug`; pass to `map_view_error`

Tasks:

- [x] **G3.1** `map_view_error` signature extended with `debug_enabled: bool`; replaces `cfg!(debug_assertions)` checks for Handler, HandlerWithStack, Pipeline, Internal. `cfg!(debug_assertions)` retained as an OR fallback for dev-build convenience. (Done 2026-04-21.)
- [x] **G3.2** `view_dispatch.rs` error branch looks up `ctx.loaded_bundle.apps[].manifest.app_id == manifest_app_id` and reads `.config.base.debug`. Falls back to `false` on lookup miss. Passed into `map_view_error`. (Done 2026-04-21.)
- [x] **G3.3** Updated existing 6 `map_view_error(...)` test calls to pass `false`. Added 2 new G3 tests: `g3_handler_with_stack_surfaces_when_debug_enabled` (always passes) and `g3_handler_with_stack_debug_disabled_in_release_hides` (asserts OR semantics — hides in release, surfaces in cargo-test debug). 24/24 `error_response_tests` green. (Done 2026-04-21.)
- [x] **G3.4** Decision-log entry captured in G0.1 / G8.4 rationale — runtime flag IS the mechanism; OR with `cfg!(debug_assertions)` for dev convenience documented in the function docstring. (Done 2026-04-21.)

**Validate:** tests green; integration run shows debug=true app produces `details.stack` + debug=false app omits it, in the SAME build.

**Effort:** ~1 hour.

### G4 — `rivers.d.ts` spec alignment (spec §8.3)

**Scope:** rename `Ctx` → `ViewContext` with type alias, reconcile `Rivers.db/view/http` per G0.2, add capability-gated JSDoc markers.

**Files:** edit `types/rivers.d.ts`, edit `docs/guide/tutorials/tutorial-ts-handlers.md` if naming changes propagate.

Tasks:

- [x] **G4.1** Renamed primary interface `Ctx` → `ViewContext` (with JSDoc note). Added `type Ctx = ViewContext` alias at end-of-file for backcompat. Updated `HandlerFn`'s parameter type. (Done 2026-04-21.)
- [x] **G4.2** Per G0.2: `Rivers.db/view/http` dropped from spec §8.3 (G8.6). `rivers.d.ts` declares only the runtime-injected surface. No stubs added. (Done 2026-04-21.)
- [x] **G4.3** Capability markers added: `Rivers.keystore` + `Rivers.crypto.encrypt/decrypt` (`@capability keystore`), `ctx.transaction` (`@capability transaction`). Informational comment block describing the capability-tag convention added at the bottom of the file. `allow_outbound_http` marker deferred — no typed surface to annotate until `Rivers.http` ships. (Done 2026-04-21.)
- [x] **G4.4** `tutorial-ts-handlers.md` updated: `Ctx` → `ViewContext` in the "Using the Rivers-shipped rivers.d.ts" section. (Done 2026-04-21.)

**Validate:** `tsc --noEmit` on a sample handler using the new `ViewContext` name resolves; `Ctx` alias works for backcompat.

**Effort:** ~30 min.

---

## P1 — Format / cosmetic drift

### G5 — Error message format alignment

Spec uses specific multi-line error formats in §2.5, §3.1, §3.2. Implementation condenses to single lines with equivalent information.

**Files:** `crates/riversd/src/process_pool/v8_config.rs`, `v8_engine/execution.rs` (resolve_module_callback)

Tasks:

- [ ] **G5.1** §2.5 `.tsx` rejection — change from `"JSX/TSX is not supported in Rivers v1: {filename}"` to `"JSX/TSX is not supported in Rivers v1: {app}/{path}"`. Requires passing app-name through the compile path (currently only filename is known at compile time). Needs context plumbing or path parsing heuristic.
- [ ] **G5.2** §3.1 missing-extension error — expand to multi-line with referrer:
  ```
  module resolution failed: import specifier "{spec}" has no extension
    in {referrer_path}
    hint: use "{spec}.ts" or "{spec}.js"
  ```
- [ ] **G5.3** §3.2 boundary-violation error — rephrase from "not in the bundle module cache" to spec format:
  ```
  module resolution failed: "{spec}" resolves outside app boundary
    in {referrer_path}
    resolved to: {abs_path}
    boundary: {app}/libraries/
  ```
  Note: this requires knowing the `{app}/libraries/` root at callback time — add to `TASK_MODULE_REGISTRY` alongside the path map.
- [ ] **G5.4** Update the 5 corresponding unit tests (`compile_typescript_rejects_tsx` + resolve-callback tests if any) to match new format strings.

**Validate:** existing unit tests pass with updated assertions; no behaviour change, only message change.

**Effort:** ~1 hour.

### G6 — Debug envelope field names (resolved by G0.1)

Per G0.1 decision. If option (a) — spec changes to match Rivers' `ErrorResponse` convention — this work is a spec edit only, covered by G8.5. If option (b) — response envelope changes — it's a bigger migration:

- [ ] **G6.1** Only applicable if G0.1 = option (b): rename response fields `message` → `error`, `details` → `debug`. Changes every error-producing site in Rivers; requires version-bump + API-change migration doc + client compatibility check.

**Validate:** all error responses migrate; existing clients documented.

**Effort:** option (a) = 0; option (b) = ~1 day + migration plan.

---

## P2 — Nice-to-have tightening

### G7 — ES2022 codegen target (spec §2.4)

Currently: parser target = ES2022, codegen target = default (ESNext). Spec intent is that ES2023+ syntax gets lowered to ES2022. In practice V8 v130 supports most ES2023; gap is theoretical.

**Files:** `crates/riversd/src/process_pool/v8_config.rs`

Tasks:

- [ ] **G7.1** Set `Config::target(EsVersion::Es2022)` on the Emitter's `cfg` field. Check rusty/swc API exact name — `CodegenConfig::target` or similar.
- [ ] **G7.2** Add a unit test emitting an ES2023+ feature (e.g., `Array.prototype.findLast`) and assert the output uses ES2022-compatible syntax.

**Validate:** test green; existing tests unaffected (current TS corpus is ES2022 or below).

**Effort:** ~30 min.

---

## P3 — Spec document corrections

### G8 — Spec self-corrections

Not code changes; they're edits to `docs/arch/rivers-javascript-typescript-spec.md` to reflect implementation reality.

Tasks:

- [x] **G8.1** §2.1 updated: `swc_core = "64"` with full feature list + note that swc uses major-per-release; `swc_sourcemap = "10"` direct dep added. (Done 2026-04-21.)
- [x] **G8.2** §2.2 bullet list: removed "TC39 Stage 3 decorator lowering" (that pass doesn't live in `typescript::typescript()`). Added a clarifying note pointing at §2.3 for decorator handling. (Done 2026-04-21.)
- [x] **G8.3** §2.3 rewritten: removed the invalid `DecoratorVersion::V202203` snippet; documented the actual parse-and-pass-through model with V8 executing Stage 3 decorators natively. Points out `swc_ecma_transforms_proposal::decorators` is not applied. (Done 2026-04-21.)
- [x] **G8.4** §5.3 envelope example aligned to `ErrorResponse` shape — `{code, message, trace_id, details.stack}`. Non-debug responses omit `details` entirely. (Done 2026-04-21.)
- [x] **G8.5** §6.4 driver table qualified: built-in rows cite source file; plugin rows marked "verify at plugin load" with a note that runtime enforcement is authoritative. (Done 2026-04-21.)
- [x] **G8.6** §8.3 required-declarations list corrected: removes `Rivers.db/view/http` with a rationale note; explicit cross-ref to `rivers_global.rs` as the authoritative injection surface. (Done 2026-04-21.)

**Validate:** spec reads consistently with implementation; every MUST/SHOULD has a satisfied counterpart.

**Effort:** ~1 hour (all editing, no code).

---

## Files touched (hot list)

- **new:** 10 files under `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/`
- **new:** `canary-bundle/tests/circular-import-rejection.sh`
- **edit:** `canary-bundle/canary-handlers/app.toml` (14 new `[api.views.ts_*]` and `[api.views.txn_*]` blocks)
- **edit:** `canary-bundle/canary-handlers/resources.toml` (if PG datasource needed)
- **edit:** `canary-bundle/run-tests.sh` (profile expansions)
- **edit:** `crates/riversd/src/error_response.rs` (signature + tests)
- **edit:** `crates/riversd/src/server/view_dispatch.rs` (debug flag lookup)
- **edit:** `crates/riversd/src/process_pool/v8_config.rs` (error messages, ES2022 codegen)
- **edit:** `crates/riversd/src/process_pool/v8_engine/execution.rs` (resolve_module_callback error messages)
- **edit:** `types/rivers.d.ts` (ViewContext rename, capability markers)
- **edit:** `docs/guide/tutorials/tutorial-ts-handlers.md` (type name propagation)
- **edit:** `docs/arch/rivers-javascript-typescript-spec.md` (G8 self-corrections)
- **edit:** `changedecisionlog.md`, `todo/changelog.md`

## Verification — end to end

1. `cargo test -p riversd --lib` — 310/310 prior tests still green; ~6 new tests from G3, G5, G7.
2. `cargo deploy /tmp/rivers-gap-closure` — deploy succeeds with all updates.
3. `just probe-ts` against deployed instance — all 9 probe cases green.
4. `canary-bundle/run-tests.sh` — TYPESCRIPT profile shows 10/10 PASS; TRANSACTIONS-TS shows 5/5 PASS on PG cluster.
5. `canary-bundle/tests/circular-import-rejection.sh` — non-zero exit with expected spec §3.5 error.
6. Spec re-read: every MUST/SHOULD in `rivers-javascript-typescript-spec.md` maps to an implementation element or an explicit deferral with cross-ref.

## Effort summary

| Tier | Items | Effort | Risk |
|------|-------|--------|------|
| G0 | 2 decisions | 30 min | low |
| G1 canary TS-syntax | 12 tasks | ~3 hours | low |
| G2 canary transaction | 7 tasks | ~2 hours + infra | medium (PG access) |
| G3 debug flag plumbing | 4 tasks | ~1 hour | low |
| G4 rivers.d.ts | 4 tasks | ~30 min | low |
| G5 error formats | 4 tasks | ~1 hour | low |
| G6 envelope fields | 1 task (option b only) | 0 or ~1 day | medium if (b) |
| G7 ES2022 codegen | 2 tasks | ~30 min | low |
| G8 spec corrections | 6 tasks | ~1 hour | low |
| **Total P0** | G0+G1+G2+G3+G4 | **~7 hours** | |
| **Total P1+P2+P3** | G5+G7+G8 | **~2.5 hours** | |
| **Grand total** | | **~9.5 hours** (excluding G6-b if chosen) | |

## Execution order

1. **G0.1, G0.2** — decisions first (clears ambiguity)
2. **G8.1–G8.6** — spec corrections (quick wins; locks the target for code changes)
3. **G3** — debug flag plumbing (unblocks canary G2 tests that need debug=true)
4. **G4** — rivers.d.ts cleanup (independent; quick)
5. **G1** — canary TS-syntax handlers (biggest chunk; mechanical)
6. **G5** — error message alignment (can run parallel to G1)
7. **G7** — ES2022 codegen (independent; quick)
8. **G2** — canary transaction handlers (last; needs live infra)
9. **G6** — only if G0.1 = option (b)

## Design decisions to log (changedecisionlog.md)

1. **G0.1 decision** — spec vs envelope alignment
2. **G0.2 decision** — Rivers.db/view/http aspirational vs declared
3. **G3 approach** — runtime AppConfig lookup vs compile-time cfg
4. **G5.3 plumbing** — how `{app}/libraries/` root reaches the resolve callback (extend `TASK_MODULE_REGISTRY`?)

## Non-goals (explicit out-of-scope)

- Implementing `Rivers.db`, `Rivers.view`, `Rivers.http` runtime surfaces (if G0.2 picks option (a)).
- Full esbuild-style bundler (spec §1.2 out-of-scope).
- Node-style `node_modules` resolution.
- JSX/TSX support.
- Chained source maps (`.js` files with `//# sourceMappingURL`).
- Cross-app code sharing.
