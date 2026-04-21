# Changelog

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
