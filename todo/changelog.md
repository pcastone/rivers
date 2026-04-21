# Changelog

## 2026-04-21 — TS pipeline Phase 8: MCP view documentation

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `docs/guide/tutorials/tutorial-mcp.md` | Added the `[api.views.mcp.handler] type = "none"` sentinel to Step 1's example (was missing — doc drift) and added spec §7.2 Common Errors table | Spec §7.1 + §7.2 | Canary had the correct form but the tutorial didn't. Four error-cause-fix rows: invalid view_type, missing method, missing handler, invalid guard type |
| `docs/arch/rivers-application-spec.md` | Added cross-reference at top of §13 pointing to `rivers-javascript-typescript-spec.md` as the authoritative source for runtime TS/module behaviour | Spec boundary clarity | rivers-application-spec is about config surface; rivers-javascript-typescript-spec is about runtime — cross-ref disambiguates |

## 2026-04-21 — TS pipeline Phase 5: module namespace entrypoint lookup

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/riversd/src/process_pool/v8_engine/task_locals.rs` | Added `TASK_MODULE_NAMESPACE: RefCell<Option<v8::Global<v8::Object>>>` thread-local; cleared in drop | Spec §4 | Using a Global avoids lifetime plumbing through function signatures — the namespace object outlives the HandleScope boundary via V8's persistent handle system |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | `execute_as_module` now, after `module.evaluate()`, calls `module.get_module_namespace()`, casts to Object, wraps as `Global`, stashes in thread-local | Spec §4.1 | `get_module_namespace` requires instantiated + evaluated state, so this order is correct |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | `call_entrypoint` branches on `TASK_MODULE_NAMESPACE`: Some → lookup on namespace Object (spec §4.1), None → lookup on globalThis (classic script). `ctx` always on globalThis regardless of mode (inject_ctx_object puts it there) | Spec §4.3 backward compat | Function body reorganised; `global` local removed, replaced with an explicit `scope.get_current_context().global(scope)` call for `ctx` lookup |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | Removed the stale "V1: module must set on globalThis" comment at `:222-224` | Spec §4.3 | Comment was acknowledging the gap this phase closes |
| `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` | Added `execute_module_export_function_handler` (export fn reaches via namespace) + `execute_classic_script_still_uses_global_scope` (regression for non-module path) | Spec §4 regression coverage | Both green; existing 129 process_pool tests also green |

Probe case G (and F) end-to-end run deferred to Phase 10. Unit dispatch tests exercise both module-mode namespace lookup and classic-script global lookup, so the Phase 5 scope is fully covered by test.

## 2026-04-21 — TS pipeline Phase 4: V8 module resolve callback

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/riversd/src/process_pool/v8_engine/task_locals.rs` | Added `TASK_MODULE_REGISTRY: RefCell<HashMap<i32, PathBuf>>` thread-local; cleared in `TaskLocals::drop` | Spec §3.6 requires resolver to identify the referrer | V8 resolve callback is `extern "C" fn`; thread-local is the only state-propagation mechanism |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | Replaced the `None`-returning stub in `instantiate_module` with `resolve_module_callback`. Validates `./`/`../` prefix, `.ts`/`.js` extension, canonicalises against referrer's parent, looks up in `BundleModuleCache`, compiles a `v8::Module`, registers new module in the registry | Spec §3.1–3.6 | Spec §3.2 boundary check is implicit: cache residency means the file was under `{app}/libraries/` at bundle load. 4 distinct rejection error messages (bare, no-ext, canonicalise-fail, not-in-cache). Throws via `v8::Exception::error` |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | Root module also registers its `get_identity_hash() → path` in the registry before `instantiate_module` so the first layer of resolves can find its referrer | Spec §3.6 | Uses `canonicalize` with fallback to raw path (tests may pass synthetic paths) |

Deferred: probe case F end-to-end run waits on Phase 5 (namespace entrypoint lookup) since the probe uses `export function handler`. No dispatch-level unit test here — the resolver only executes inside V8's `instantiate_module` which needs a full cache+bundle fixture; Phase 5's end-to-end run covers it.

Plan correction: task 4.3 said "thread via closure capture (not thread-local)." V8's resolve callback signature is `extern "C" fn(Context, String, FixedArray, Module) -> Option<Module>` — no closure captures possible. Thread-local is the only option. Noted in tasks.md.

## 2026-04-21 — TS pipeline Phase 3: circular import detection

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/rivers-runtime/src/module_cache.rs` | Added `imports: Vec<String>` field to `CompiledModule` (raw specifiers, post-transform). Doc note that type-only imports are erased by the swc pass before extraction | Spec §3.5 | Construct sites updated with `imports: Vec::new()` where the real list comes from the compile step |
| `crates/riversd/src/process_pool/v8_config.rs` | Split `compile_typescript` into a thin wrapper over `compile_typescript_with_imports(&str, &str) -> Result<(String, Vec<String>), _>`. New `extract_imports(&Program)` walks ModuleItem::ModuleDecl for Import/ExportAll/NamedExport | Spec §3.5 | Keeps 21 existing callers on the String-returning API; only the populate path sees the `Vec<String>` |
| `crates/riversd/src/process_pool/module_cache.rs` | `check_cycles_for_app` builds per-app adjacency, DFS cycle detection, formats errors per spec §3.5. Runs after each app's compile inside `populate_module_cache`. Only relative specifiers (`./`, `../`) are cycle candidates — bare and absolute are deferred to Phase 4's resolver | Spec §3.5 | Graph is per-app; cross-app imports are prohibited so cross-app cycles are structurally impossible. 5 new unit tests cover two-module, three-module, self-import, acyclic-tree-OK, and type-only-not-cycle |

## 2026-04-21 — TS pipeline Phase 2: bundle-load-time compile + module cache

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/rivers-runtime/src/module_cache.rs` | New file. `CompiledModule { source_path, compiled_js, source_map }` + `BundleModuleCache` wrapping `Arc<HashMap<PathBuf, CompiledModule>>` | Spec §3.4 | Types in rivers-runtime so any crate can reference them; Arc-clone is O(1). 3 unit tests |
| `crates/rivers-runtime/src/lib.rs` | Registered new `module_cache` submodule | Module hygiene | One-line addition |
| `crates/riversd/src/process_pool/module_cache.rs` | New file. Population helpers (`compile_app_modules`, `populate_module_cache`) + process-global slot (`install_module_cache`, `get_module_cache`) | Spec §2.6–2.7 | Kept in riversd to avoid dragging swc_core into rivers-runtime's build surface. Recursive walker; fail-fast compile; `.tsx` rejected at walk time. 5 unit tests |
| `crates/riversd/src/process_pool/mod.rs` | Registered new `module_cache` submodule | Module hygiene | Feature-gated to `static-engines` alongside v8_config |
| `crates/riversd/Cargo.toml` | Added `once_cell = "1"` | Global cache slot | Standard choice for statics with lazy init |
| `crates/riversd/src/bundle_loader/load.rs` | After validation, call `populate_module_cache(&bundle)` + `install_module_cache(cache)` | Spec §2.6 bundle-load timing | Placed between cross-ref validation and DataViewRegistry setup; fail-fast via ServerError::Config |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | Rewrote `resolve_module_source` to consult the global cache first, fall back to disk read + live compile on miss | Spec §2.8 | Fallback path kept for handlers outside `libraries/`; logged at debug level. Pre-existing 124 process_pool tests still green |
| `changedecisionlog.md` | Added entries: rivers-runtime/riversd split, global OnceCell rationale, fallback-on-miss reasoning | CLAUDE.md Workflow rule 5 | Three new decisions, each naming file + spec ref + resolution |

## 2026-04-21 — TS pipeline Phase 1: swc full-transform drop-in

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/riversd/Cargo.toml` | Added `swc_core = "64"` with features `ecma_ast`, `ecma_parser`, `ecma_parser_typescript`, `ecma_transforms_typescript`, `ecma_codegen`, `ecma_visit`, `common`, `common_sourcemap` | Spec §2.1 | Spec says v0.90 but crates.io current is v64; v0.90 builds fail due to `serde::__private` regression. Decision logged in `changedecisionlog.md` |
| `crates/riversd/src/process_pool/v8_config.rs` | Replaced hand-rolled `compile_typescript` + `strip_type_annotations` with swc full-transform pipeline (parse → resolver → typescript → fixer → to_code_default) | Spec §2.1–2.5 | ES2022 target, `TsSyntax { decorators: true }`, `.tsx` rejected at entry with spec §2.5 error message |
| `crates/riversd/tests/process_pool_tests.rs` | Replaced single `contains("const x")` regression test with 16 cases covering every spec §2.2 feature | Spec §9.2 regression coverage | Cases: parameter/variable/return annotations, generics, type-only imports, `as`, `satisfies`, interface, type alias, enum, namespace, `as const`, TC39 decorator, `.tsx` rejection, syntax error reporting, JS passthrough. All 16 green |
| `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` | 3 pre-existing TS tests + `execute_typescript_handler` dispatch test verified green unchanged | Spec §10 item 1 | swc is a superset of the old stripper for those inputs; no assertion tweaks needed |
| `changedecisionlog.md` | New file; captures swc full-transform vs strip-only, v0.90→v64 correction, decorator lowering strategy, source-map deferral to Phase 6 | CLAUDE.md Workflow rule 5 | CB drift-detection baseline starts here |

## 2026-04-21 — TS pipeline Phase 0: preflight for `rivers-javascript-typescript-spec.md`

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `todo/gutter.md` | Archived filesystem-driver epic (3339 lines) under dated header | CLAUDE.md workflow rule 1 | 157 unchecked checkboxes preserved verbatim; epic is complete per commits 09c4025/20febbe, only bookkeeping was skipped |
| `todo/tasks.md` | Replaced with 11-phase TS pipeline plan | `docs/arch/rivers-javascript-typescript-spec.md` + `dist/rivers-upstream/rivers-ts-pipeline-findings.md` | Plan matches spec §10 plus an explicit Phase 2 for bundle-load-time compilation which spec §10 conflates with Phase 1 |
| `tests/fixtures/ts-pipeline-probe/` | Moved from gitignored `dist/rivers-upstream/cb-ts-repro-bundle/` to tracked fixture tree | Spec §9.1 "Probe Bundle Adoption" | Delete the dist/ copy; keep `dist/rivers-upstream/rivers-ts-pipeline-findings.md` as the upstream snapshot |
| `tests/fixtures/rivers-ts-pipeline-findings.md` | Copied from dist/ into tracked tree | Probe README links to `../rivers-ts-pipeline-findings.md` | Keeping both the upstream snapshot (dist/) and the tracked copy (tests/fixtures/) |
| `Justfile` | Added `just probe-ts [base]` recipe | Spec §9.1 regression-suite wiring | No GitHub CI addition — probe/canary both require a real riversd + infra, they run locally like canary |
| `docs/arch/rivers-javascript-typescript-spec.md` | Tracked the spec itself in this commit | Anchor for all subsequent phase work | First commit that binds spec + plan + probe together |

## 2026-04-03 — Configure canary-bundle for 192.168.2.x test infrastructure

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `canary-bundle/canary-sql/app.toml` | Added host/port/database/username for PG (209), MySQL (215); changed SQLite from `:memory:` to file path `canary-sql/data/canary.db` | sec/test-infrastructure.md | Direct connection config, nopassword=true |
| `canary-bundle/canary-nosql/app.toml` | Added host/port/database/username for Mongo (212), ES (218), CouchDB (221), Cassandra (224), LDAP (227), Redis (206) | sec/test-infrastructure.md | Direct connection config, nopassword=true |
| `canary-bundle/canary-streams/app.toml` | Uncommented Kafka datasource (203:9092), added Redis datasource (206:6379) | sec/test-infrastructure.md | Enabled for test infra |
| `canary-bundle/canary-streams/resources.toml` | Uncommented Kafka and Redis datasource declarations, removed lockbox references | sec/test-infrastructure.md | nopassword=true replaces lockbox |
| `canary-bundle/canary-nosql/resources.toml` | Removed all lockbox references and x-type fields | sec/test-infrastructure.md | nopassword=true replaces lockbox |
| `canary-bundle/canary-sql/resources.toml` | Removed lockbox references and x-type fields | sec/test-infrastructure.md | nopassword=true replaces lockbox |
| `canary-bundle/riversd.toml` | Created new server config for canary with memory storage engine, no TLS | Test environment config | Separate from riversd-canary.toml (which has security/session/CSRF config) |
| `canary-bundle/canary-sql/data/` | Created empty directory for SQLite file-based database | SQLite file path config | Directory must exist before runtime creates the .db file |

## 2026-04-03 — Canary fleet spec updated to v1.1 (v0.53.0 conformance)

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `docs/arch/rivers-canary-fleet-spec.md` | Bumped to v1.1, added canary-ops profile (port 9105, 24 tests), 3 per-app logging tests in canary-handlers, 4 SQLite path fallback tests in canary-sql, metrics/logging config sections | v0.53.0 features: AppLogRouter, config discovery, riversctl PID/stop/status, doctor, metrics, TLS, SQLite path, riverpackage, engine loader | Absorbed into source spec. Total tests: 75 → 107 across 7 profiles |
| `docs/arch/rivers-canary-fleet-amd2.md` | Created AMD-2 documenting all v0.53.0 additions | Amendment convention from AMD-1 | Historical reference, changes already in source spec |
| `docs/bugs/rivers-canary-fleet-spec.md` | Synced duplicate copy with updated spec | Duplicate exists in docs/bugs/ | Copied from docs/arch/ |

## 2026-04-03 — Prometheus metrics endpoint

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `Cargo.toml` (workspace) | Add `metrics 0.24` and `metrics-exporter-prometheus 0.16` to workspace deps | Build philosophy: reusable infrastructure | Added after `neo4rs` entry |
| `crates/riversd/Cargo.toml` | Add `metrics` (required) and `metrics-exporter-prometheus` (optional) deps; new `metrics` feature gating the exporter, added to default features | Feature-gated optional infrastructure | `metrics` feature enables `dep:metrics-exporter-prometheus` |
| `crates/rivers-core-config/src/config/runtime.rs` | Add `MetricsConfig` struct with `enabled` (bool) and `port` (Option<u16>, default 9091) | New config section for `[metrics]` in riversd.conf | Placed before `RuntimeConfig`; derives Default (enabled=false) |
| `crates/rivers-core-config/src/config/server.rs` | Add `metrics: Option<MetricsConfig>` field to `ServerConfig` | Top-level config section | Optional field, defaults to None (metrics disabled) |
| `crates/riversd/src/server/metrics.rs` | Created metrics helper module: `record_request`, `set_active_connections`, `record_engine_execution`, `set_loaded_apps` | Infrastructure only; not wired into request pipeline yet | Uses `metrics` crate global recorder macros |
| `crates/riversd/src/server/mod.rs` | Export `metrics` module behind `#[cfg(feature = "metrics")]` | Feature-gated module | Conditional compilation |
| `crates/riversd/src/server/lifecycle.rs` | Initialize PrometheusBuilder in both `run_server_no_ssl` and `run_server_with_listener_and_log`, after runtime init, before StorageEngine | Start exporter on port 9091 (configurable) | `#[cfg(feature = "metrics")]` gated; logs info on success, warn on failure |

## 2026-04-03 — EventBus LogHandler routes app events to per-app log files

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/rivers-core/src/logging.rs` | Route events with app context to per-app log files via AppLogRouter | `rivers-logging-spec.md` — per-app log isolation | After stdout/file write in `handle()`, resolve effective `app_id` (payload `app_id` > `self.app_id`), skip if empty or `"default"`, write to `global_router()` |

## 2026-04-03 — Per-app logging fixes (AppLogRouter)

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/riversd/src/bundle_loader/load.rs` | Use `entry_point` (not `app_name`) when registering with AppLogRouter | V8 callbacks use `TASK_APP_NAME` from `ctx.app_id` which comes from `entry_point` | Changed line 224 from `&app.manifest.app_name` to `entry_point` |
| `crates/rivers-core/src/app_log_router.rs` | Flush existing BufWriter before replacing on hot reload | Prevents data loss when `register()` is called again for an already-registered app | Added `flush()` call on old writer in `register()` |
| `crates/rivers-core/src/app_log_router.rs` | Add `Drop` impl that calls `flush_all()` | Ensures buffered data is written when AppLogRouter is dropped | Added `impl Drop for AppLogRouter` |
| `crates/rivers-core/src/app_log_router.rs` | Remove per-write `flush()` from `write()` | BufWriter flushes at 8KB buffer full and on Drop; per-write flush defeats the purpose of buffering | Removed `let _ = writer.flush();` from `write()` |
| `crates/riversd/src/server/lifecycle.rs` | Add explicit `flush_all()` in graceful shutdown sequence | Belt-and-suspenders with Drop impl; ensures flush before process exit | Added after `wait_for_drain()`, before aborting admin/redirect servers |
| `crates/rivers-core/src/app_log_router.rs` (test) | Add `flush_all()` before reading files in test | Required after removing per-write flush | Added `router.flush_all()` in `write_appends_to_correct_file` test |

## 2026-04-20 — Task 8: FILESYSTEM profile — 7/7 passing

### Canary test results before this session
- Pass: 52 / Fail: 50 / Error: 1 (FS-CHROOT-ESCAPE 500) / Total: 103

### Changes made

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `rivers-engine-v8/src/execution.rs` | Added `inject_datasource_method()` — injects `ctx.datasource(name)` into the V8 cdylib handler context; builds typed JS proxy for filesystem ops | filesystem driver spec §3.3 | Parses `datasource_tokens` for `direct://` entries, injects `__rivers_build_fs_proxy` and `__rivers_ds_dispatch` globals, wires `ctx.datasource` to lookup function |
| `rivers-engine-v8/src/execution.rs` | Fixed `inject_datasource_method` bugs: (1) register `ds_dispatch_callback` as `__rivers_ds_dispatch` global, (2) fixed `global()` object access pattern (removed invalid `.into()` Option match) | N/A | Two-line fix: add `dispatch_fn` registration; use `let global = scope.get_current_context().global(scope)` directly |
| `rivers-engine-v8/src/execution.rs` | Fixed proxy response reshaping: JS proxy `dispatch()` now reshapes `{rows, affected_rows}` response from host into per-op types (readFile→string, exists→bool, stat→object, readDir→array, find/grep→{results,truncated}) | filesystem driver spec §4 | Added reshape logic inside `dispatch()` function in JS proxy |
| `rivers-engine-v8/src/execution.rs` | Fixed rename/copy param names: proxy sent `{from,to}` but driver expects `{oldPath,newPath}` (rename) and `{src,dest}` (copy) | filesystem driver implementation | Updated proxy to send correct parameter names |
| `riversd/src/engine_loader/host_callbacks.rs` | Fixed `host_datasource_build`: params were inserted as `QueryValue::Json(v)` but driver `get_string()` only matches `QueryValue::String(s)` | `QueryValue::String` pattern matching | Changed to proper type-dispatch (same logic as `host_dataview_execute`) |
| `riversd/src/engine_loader/host_callbacks.rs` | Fixed `host_datasource_build`: `Query::new("", op)` lowercased operation via `infer_operation()`, turning `"writeFile"` into `"writefile"` | `infer_operation()` implementation | Changed to `Query::with_operation(op, "", op)` to preserve case |
| `rivers-runtime/src/validate.rs` | Added `"Mcp"` to `VALID_VIEW_TYPES` | canary-sql MCP view | Added in previous session, kept here |
| `riversd/src/view_engine/pipeline.rs` | Wire direct datasources into codecomponent task context | filesystem driver spec §7 | Scan executor params for `driver=filesystem`, add `DatasourceToken::direct` per datasource |

### Canary test results after this session
- Pass: 58 / Fail: 45 / Error: 0 / Total: 103
- FILESYSTEM profile: 7/7 (FS-CRUD-ROUNDTRIP, FS-CHROOT-ESCAPE, FS-EXISTS-AND-STAT, FS-FIND-AND-GREP, FS-ARG-VALIDATION, FS-READ-DIR, FS-CONCURRENT-WRITES)
