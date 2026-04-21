# Phase 6 Completion — Source Map Stack-Trace Remapping

> **Branch:** `docs/guide-v0.54.0-updates` (continues from TS pipeline Phases 0–11)
> **Plan file:** `/Users/pcastone/.claude/plans/we-will-address-these-stateless-spark.md`
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md §5`
> **Closes:** `processpool-runtime-spec-v2` Open Question #5
> **Prior:** Phase 6.1 (generation) shipped in commit `a301b6b`. Full 11-phase history archived to `todo/gutter.md`.

**Goal:** handler authors see original `.ts:line:col` positions in stack traces — both in the per-app log (always) and in the error-response envelope (when `debug = true`). Close probe case `RT-TS-SOURCEMAP`.

**All prerequisites are in place** (verified in prior session):

| Piece | Location |
|---|---|
| v3 source maps stored per module | `CompiledModule.source_map` in `rivers-runtime/src/module_cache.rs` |
| Process-global cache reader | `riversd/src/process_pool/module_cache.rs:get_module_cache()` |
| `swc_sourcemap` dep | `crates/riversd/Cargo.toml` (v10) |
| Script origin = absolute `.ts` path | `execute_as_module` + `resolve_module_callback` in `execution.rs` |
| `AppLogRouter` wired at `TASK_APP_NAME` | `task_locals.rs` |
| `PrepareStackTraceCallback` type | `v8-130.0.7/src/isolate.rs:393-412` |
| `isolate.set_prepare_stack_trace_callback` | rusty_v8 method |

**Critical path:** 6A → 6C → 6D. 6B lands before 6D becomes testable. 6E/6F/6G/6H parallelise once 6D works.

---

## 6A — Register PrepareStackTraceCallback (~30 min, low risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [ ] **6A.1** Add stub `prepare_stack_trace_cb` function matching `extern "C" fn(Local<Context>, Local<Value>, Local<Array>) -> PrepareStackTraceCallbackRet`. Initial behaviour: return the error's existing `.stack` string unchanged (so shipping the stub is a no-op for semantics).
- [ ] **6A.2** In `execute_js_task` (execution.rs:~304) after `acquire_isolate(effective_heap)`, call `isolate.set_prepare_stack_trace_callback(prepare_stack_trace_cb)`.
- [ ] **6A.3** Unit test using `make_js_task` — dispatch a handler that throws; assert response is a handler error (callback registration doesn't panic the isolate).

**Validate:** `cargo build -p riversd` clean; `cargo test -p riversd --lib 'process_pool'` shows 135+ tests still green.

## 6B — Parsed source-map cache (~30 min, low risk)

**Files:** new `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs`; edit `v8_engine/mod.rs`, `process_pool/module_cache.rs`

- [ ] **6B.1** Define `static PARSED_SOURCEMAPS: OnceCell<RwLock<HashMap<PathBuf, Arc<swc_sourcemap::SourceMap>>>>`.
- [ ] **6B.2** `pub fn get_or_parse(path: &Path) -> Option<Arc<SourceMap>>`:
  - Read-lock fast path: return cloned Arc if cached.
  - Slow path: fetch JSON via `module_cache::get_module_cache()?.get(path)?.source_map`; parse via `SourceMap::from_reader(bytes.as_bytes())`; write-lock, insert, return Arc.
- [ ] **6B.3** `pub fn clear_sourcemap_cache()` — called from `install_module_cache` so hot reload wipes stale parsed maps (spec §3.4 atomic-swap).
- [ ] **6B.4** Register submodule in `v8_engine/mod.rs`.
- [ ] **6B.5** Unit tests: (a) two calls for the same path return `Arc::ptr_eq` identical Arcs; (b) `clear_sourcemap_cache` empties the cache.

**Validate:** 2/2 new tests green.

## 6C — CallSite extraction helper (~1.5 hours, medium risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

V8's CallSite is a JS object; no rusty_v8 wrapper. Extract via property-lookup + function-call.

- [ ] **6C.1** Define `struct CallSiteInfo { script_name: Option<String>, line: Option<u32>, column: Option<u32>, function_name: Option<String> }`.
- [ ] **6C.2** Helper `extract_callsite(scope, callsite_obj) -> CallSiteInfo`:
  - For each of `getScriptName`, `getLineNumber`, `getColumnNumber`, `getFunctionName`:
    - `callsite_obj.get(scope, method_name_v8_str.into())` → Value
    - Cast to `v8::Local<v8::Function>`
    - `fn.call(scope, callsite_obj.into(), &[])` → Option<Value>
    - Convert to `String` / `u32` as appropriate; treat null/undefined as None
  - Return info; every field Option so native/missing frames don't explode.
- [ ] **6C.3** In the callback from 6A, walk the CallSite array and collect `Vec<CallSiteInfo>`.
- [ ] **6C.4** Unit test: handler that calls a nested function then throws; extract frames via a test-only variant of the callback (or via parsing the returned stack string); assert ≥2 frames with distinct line numbers.

**Validate:** extractor returns correct line/col/name for a known fixture.

## 6D — Token remap + stack formatting (~1.5 hours, medium risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [ ] **6D.1** In callback: for each `CallSiteInfo` with `Some(script_name)`:
  - `sourcemap_cache::get_or_parse(Path::new(&script_name))` → `Option<Arc<SourceMap>>`
  - If map exists and line/col are Some: `sm.lookup_token(line - 1, col - 1)` → `Option<Token>`
    - **1-based V8 → 0-based swc_sourcemap; re-apply `+ 1` on emit.**
  - Pull `token.get_src()`, `token.get_src_line() + 1`, `token.get_src_col() + 1`
- [ ] **6D.2** Frame format:
  - Remapped: `"    at {fn_name or '<anonymous>'} ({src_file}:{src_line}:{src_col})"`
  - Fallback (null script_name, cache miss, lookup None): `"    at {fn_name} ({script_name or '<unknown>'}:{line}:{col})"`
- [ ] **6D.3** Prepend the error's `toString()` — V8 stack convention is `Error: msg\n    at …`.
- [ ] **6D.4** Build a `v8::String::new(scope, &joined)` and return `PrepareStackTraceCallbackRet` containing it.
- [ ] **6D.5** Integration test: write a `.ts` handler fixture that throws at line 42, compile + install into cache, dispatch, parse response.stack (or equivalent); assert `.ts` path and line `42` appear (not compiled line).

**Validate:** remap integration test green.

## 6E — Route remapped stacks to per-app log (~1 hour, low risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`, `crates/riversd/src/process_pool/types.rs`, AppLogRouter call site

- [ ] **6E.1** In `call_entrypoint`'s error branch (execution.rs:~529), after capturing the exception, cast to `v8::Local<v8::Object>`, read the `stack` property; convert to Rust `String`. This is already the remapped trace (the callback fires on `.stack` property access).
- [ ] **6E.2** Introduce `TaskError::HandlerErrorWithStack { message: String, stack: String }` struct variant in `types.rs`. Additive — exhaustive matches elsewhere will surface in the build.
- [ ] **6E.3** At the error logging site in `execute_js_task`'s return path, when the error variant is `HandlerErrorWithStack`, emit `tracing::error!(target: "rivers.handler", trace_id = %trace_id, app = %app, message = %message, stack = %stack, "handler threw")`. AppLogRouter routes via `TASK_APP_NAME` thread-local into `log/apps/<app>.log`.
- [ ] **6E.4** Integration test: trigger a handler throw; read `log/apps/<app>.log`; assert it contains the `.ts:line:col` string.

**Validate:** log file contains remapped trace; existing log outputs unchanged.

## 6F — Debug-mode error envelope (~1 hour, low risk)

**Files:** `crates/rivers-runtime/src/bundle.rs`, `crates/riversd/src/server/view_dispatch.rs` (or the `TaskError` → HTTP response conversion site)

- [ ] **6F.1** Check `AppConfig` for existing `debug: bool`. If absent, add `#[serde(default)] pub debug: bool` to `AppConfig` in `rivers-runtime/src/bundle.rs`. Sourced from `[base] debug = true` in `app.toml`.
- [ ] **6F.2** In the error-response serialization, when the error is `HandlerErrorWithStack` AND the app's `debug == true`:
  - Serialize `{ "error": message, "trace_id": id, "debug": { "stack": split_lines(stack) } }`.
  - Otherwise: `{ "error": message, "trace_id": id }` — no `debug` key at all.
- [ ] **6F.3** Two integration tests: app with `debug = true` returns `debug.stack`; app with default `debug = false` omits it.

**Validate:** both tests green; non-debug response byte-identical to pre-change.

## 6G — Spec cross-refs + tutorial + changelogs (~30 min)

**Files:** `docs/arch/rivers-processpool-runtime-spec-v2.md`, `docs/arch/rivers-javascript-typescript-spec.md`, `docs/guide/tutorials/tutorial-ts-handlers.md`, `changedecisionlog.md`, `todo/changelog.md`

- [ ] **6G.1** `processpool-runtime-spec-v2` Open Question #5 — replace with "Resolved by `rivers-javascript-typescript-spec.md §5` — see Phase 6 completion commits (TBD)."
- [ ] **6G.2** `rivers-javascript-typescript-spec.md §5.4` — tighten wording to note the implementation landed.
- [ ] **6G.3** `tutorial-ts-handlers.md` — add "Debugging handler errors" subsection: enabling `[base] debug = true` for `debug.stack` in dev; per-app log location `log/apps/<app>.log` is always remapped.
- [ ] **6G.4** `changedecisionlog.md` — four new entries:
  1. Parsed-map cache separate from BundleModuleCache (rationale: re-parse cost)
  2. CallSite extraction via JS reflection (rationale: rusty_v8 has no wrapper)
  3. `TaskError::HandlerErrorWithStack` struct variant (rationale: additive, matches surface)
  4. App-level debug flag not view-level (rationale: spec §5.3 says app config)
- [ ] **6G.5** `todo/changelog.md` — Phase 6 completion entry.

**Validate:** doc cross-refs resolve; changelog entries present.

## 6H — Canary sourcemap coverage (~1 hour, low risk)

**Files:** new `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/sourcemap.ts`; edit `canary-handlers/app.toml`, `canary-bundle/run-tests.sh`

- [ ] **6H.1** Create `sourcemap.ts` handler: top-of-file throw at a distinctive line (e.g., line 42 literally — line 41 is a blank line right above `throw new Error("canary sourcemap probe")`). Export as `sourcemapProbe`.
- [ ] **6H.2** Register in `canary-handlers/app.toml`:
  ```toml
  [api.views.sourcemap_test]
  path      = "/canary/rt/ts/sourcemap"
  method    = "POST"
  view_type = "Rest"
  auth      = "none"
  debug     = true

  [api.views.sourcemap_test.handler]
  type       = "codecomponent"
  language   = "typescript"
  module     = "libraries/handlers/ts-compliance/sourcemap.ts"
  entrypoint = "sourcemapProbe"
  ```
  (Move `debug` to the app-level `[base]` section if 6F.1 places it there rather than per-view.)
- [ ] **6H.3** `run-tests.sh` — new "TYPESCRIPT Profile" block between HANDLERS and TRANSACTIONS-TS, with a `test_ep`-like probe that greps the response for `sourcemap.ts:42`.

**Validate:** canary endpoint returns an error envelope; `debug.stack` array contains `sourcemap.ts:42`.

---

## Files touched (hot list)

- **new:** `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs`
- **edit:** `crates/riversd/src/process_pool/v8_engine/execution.rs` — callback register + body
- **edit:** `crates/riversd/src/process_pool/v8_engine/mod.rs` — submodule register
- **edit:** `crates/riversd/src/process_pool/module_cache.rs` — `clear_sourcemap_cache` from `install_module_cache`
- **edit:** `crates/riversd/src/process_pool/types.rs` — `HandlerErrorWithStack` variant
- **edit:** `crates/rivers-runtime/src/bundle.rs` — `AppConfig.debug`
- **edit:** `crates/riversd/src/server/view_dispatch.rs` — error envelope
- **edit:** `docs/arch/rivers-processpool-runtime-spec-v2.md`, `rivers-javascript-typescript-spec.md`, `tutorial-ts-handlers.md`
- **new:** `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/sourcemap.ts`
- **edit:** `canary-bundle/canary-handlers/app.toml`, `run-tests.sh`

## Verification (end to end)

1. `cargo test -p riversd --lib 'process_pool'` — 135+ prior tests green + new tests from 6A/6B/6C/6D/6E/6F.
2. `cargo deploy /tmp/rivers-sourcemap` — succeeds; `types/rivers.d.ts` present.
3. `riversd` running; POST `/canary-fleet/handlers/canary/rt/ts/sourcemap` — response body includes `debug.stack` with `sourcemap.ts:42:*`.
4. `tail log/apps/canary-handlers.log` — remapped trace present, correlated by `trace_id`.
5. Toggle `[base] debug = false`; redeploy; same request returns no `debug` key; log still has the remapped trace.

## Design decisions locked (will mirror into changedecisionlog.md during 6G)

1. **Parsed-map cache separate from BundleModuleCache.** Raw JSON stays in the module cache (cheap to hot-reload); parsed `Arc<SourceMap>` lives in its own `OnceCell<RwLock<HashMap>>`. `install_module_cache` invalidates both.
2. **CallSite via JS reflection.** rusty_v8 v130 has no CallSite wrapper. Invoke methods by name through `Object::get` + `Function::call`. Matches Deno's approach.
3. **`HandlerErrorWithStack` struct variant, not an `Option<String>` on `HandlerError`.** Additive; exhaustive matches surface everywhere that needs updating.
4. **App-level `debug` flag.** Spec §5.3 says `debug = true in app config`. Matches existing app-wide flags; avoids per-view proliferation.

## Non-goals

- `_source` inline handlers (tests only) — no on-disk path, no cache entry.
- Minification remapping (we don't minify).
- Chained source maps / `//# sourceMappingURL` directives in `.js` files.
- Remote source-map fetching.

## Effort estimate

| Task | Hours | Risk |
|---|---|---|
| 6A | 0.5 | low |
| 6B | 0.5 | low |
| 6C | 1.5 | medium |
| 6D | 1.5 | medium |
| 6E | 1 | low |
| 6F | 1 | low |
| 6G | 0.5 | low |
| 6H | 1 | low |
| **Total** | **~7.5** | |
