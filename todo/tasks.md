# Code-Review Remediation Plan

> **Source:** `docs/code_review.md` (2026-04-24)
> **Prior plan:** TS pipeline gap closure (G0–G8) — fully complete, archived in `todo/gutter.md` history.
> **Goal:** close every P0/P1/P2 finding from the full-codebase review. Order follows the review's "Recommended Remediation Sequence" so boot/security come first and broad refactors last.

**Theme of the review:** the right primitives exist (storage traits, drivers, pool, event priorities, V8 task locals, security pipeline), but hot paths bypass them. One authoritative path for app identity, datasource access, storage, and host capabilities is the goal.

**Status legend:** `[ ]` open · `[~]` in progress · `[x]` done · `[!]` blocked
# RXE — `rivers-plugin-exec` Review

> **Branch:** current worktree
> **Source:** user request on 2026-04-24: focus only on `rivers-plugin-exec`; consolidation will happen in a separate session.
> **Goal:** produce a source-grounded per-crate review report at `docs/review/rivers-plugin-exec.md`.

**Grounding confirmed:**
- Crate path: `crates/rivers-plugin-exec`.
- Crate type: `cdylib` + `rlib`.
- Production Rust source: 13 files, 3,375 lines under `src/`.
- Key dependencies: `rivers-driver-sdk`, `tokio`, `serde`, `serde_json`, `sha2`, `hex`, `tracing`, `jsonschema`, `libc`.
- Review focus from `docs/review_inc/rivers-per-crate-focus-blocks.md`: command execution, SHA-256 hash pinning, integrity modes, stdin/args input modes, privilege drop, process lifecycle, dual semaphore concurrency, stdio bounds, and environment sanitization.

## Pending Tasks

- [x] **RXE0.1 — Read crate manifest and focus block.** Done 2026-04-25: read `Cargo.toml` (declares cdylib + rlib, depends on rivers-driver-sdk + tokio + sha2 + jsonschema + libc) and the section 1 focus block; report grounding section names crate role, source files, dependencies, and the 8 review axes.

## Phase A — Unblock boot & fail closed (P0-4, P0-1)

### A1 — Broker consumer startup must not block bundle load (P0-4)

**Files:** `crates/riversd/src/bundle_loader/wire.rs`, `crates/riversd/src/broker_supervisor.rs` (new), `crates/riversd/src/server/context.rs`, `crates/riversd/src/health.rs`, `crates/riversd/src/server/handlers.rs`

- [x] **A1.1** New module `broker_supervisor` owns the connect+retry+run lifecycle. `wire_streaming_and_events` now calls `spawn_broker_supervisor(...)` which returns immediately — HTTP listener bind no longer waits on `create_consumer().await`. (Done 2026-04-24.)
- [x] **A1.2** Added `BrokerBridgeRegistry` (Arc<RwLock<HashMap>>) to `AppContext`. Per-bridge state (`pending` / `connecting` / `connected` / `disconnected` / `stopped`) plus last error and failure-count surfaced via new `broker_bridges: Vec<BrokerBridgeHealth>` field on `VerboseHealthResponse`. Process readiness is independent — `/health` still 200 even when brokers are degraded. (Done 2026-04-24.)
- [x] **A1.3** Bounded exponential backoff lives in `SupervisorBackoff` (`base_ms` from configured `reconnect_ms`, doubling, capped at `max_ms = 60_000`). Supervisor catches every `create_consumer` failure, increments `failed_attempts`, and retries with `tokio::select!` against the shutdown receiver. Driver-side blocking (rskafka's `partition_client`) is now contained — no driver code change needed. (Done 2026-04-24.)
- [x] **A1.4** New test file `crates/riversd/tests/broker_supervisor_tests.rs` — 3 tests: `spawn_returns_immediately_when_broker_unreachable` (asserts spawn elapsed < 50ms vs unreachable TEST-NET-1 host + supervisor retries), `supervisor_reaches_connected_after_transient_failures` (mock driver fails twice then succeeds; registry transitions to Connected, counter resets), `empty_registry_is_healthy`. Plus `verbose_health_serializes_broker_bridges` in `health_tests.rs`. All green. (Done 2026-04-24.)

**Validate:** ✅ 3/3 supervisor tests + 12/12 broker_bridge tests + 12/12 health tests all green. `cargo build -p riversd` clean.

### A2 — Protected views fail closed when session manager is absent (P0-1)

**Files:** `crates/riversd/src/security_pipeline.rs`, `crates/riversd/src/bundle_loader/load.rs`

- [x] **A2.1** `run_security_pipeline` now checks `ctx.session_manager.is_none()` BEFORE the `if let Some(ref mgr)` branch and returns `500 Internal Server Error` with a sanitized message ("session manager not configured; protected view cannot be served") via `error_response::internal_error`. Logged at ERROR with trace_id, view_type, method. (Done 2026-04-24.)
- [x] **A2.2** Strengthened existing AM1.2 check in `load_and_wire_bundle`: was "protected view + no storage_engine"; now "protected view + no session_manager" with diagnostic explaining which dependency is missing (storage vs session manager). Extracted into testable helper `check_protected_views_have_session(views, has_session_manager, has_storage_engine) -> Result<(), String>`. (Done 2026-04-24.)
- [x] **A2.3** New file `crates/riversd/tests/security_pipeline_tests.rs` — 2 tests: `protected_view_without_session_manager_fails_closed` asserts 500, `public_view_without_session_manager_still_works` asserts auth=none passes through. (Done 2026-04-24.)
- [x] **A2.4** 6 unit tests on `check_protected_views_have_session` in `bundle_loader::load`: rejects with no storage; rejects with storage but no session manager (forward-looking); allows when session manager present; allows public-only bundles; allows empty view set; rejects mixed bundles where one view is protected. All green. (Done 2026-04-24.)

**Validate:** ✅ 6 unit tests + 2 integration tests green. Full `cargo test -p riversd --lib` = 345/345 + 1 ignored. Pre-existing failure in `cli_tests::version_string_contains_version` (hardcodes 0.50.1 vs current 0.55.0) flagged for separate cleanup; unrelated to Phase A.
- [x] **RXE0.2 — Run mechanical sweeps.** Done 2026-04-25: panics (~140 hits, all in test code; production has no `unwrap`/`expect`/`panic!`), unsafe/FFI (3 production unsafe blocks: `geteuid`, `getpwnam` in validator + executor, `kill -PGID`), no `let _ =` discards, no production `Mutex::`/`RwLock::` (concurrency uses `tokio::sync::Semaphore` Arc-shared), one cast `pid as i32` for the `kill` syscall, ~50 `format!` hits (all error messages, no shell construction), `Command::new` once via `tokio::process::Command`, plugin entry `_rivers_abi_version` + `_rivers_register_driver` gated on `plugin-exports`. No `dead_code` allows. Findings drafted only after full reads.

- [x] **RXE0.3 — Run compiler validation.** Done 2026-04-25: `cargo check -p rivers-plugin-exec` clean, no warnings in this crate. `cargo test -p rivers-plugin-exec --lib` green: 93 passed / 0 failed / 2 ignored. The 2 ignored tests are `non_zero_exit_returns_error` and `empty_output_returns_error` (broken-pipe-on-Linux-CI per a tracked issue, unrelated to review).

- [x] **RXE1.1 — Read all production source files in full.** Done 2026-04-25: read `lib.rs` (73), `schema.rs` (232), `template.rs` (209), `integrity.rs` (292), `executor.rs` (699), `config/{mod.rs,parser.rs,types.rs,validator.rs}` (11+354+199+401), and `connection/{mod.rs,driver.rs,exec_connection.rs,pipeline.rs}` (554+109+53+189). Every finding cites file:line.

- [x] **RXE1.2 — Check hash authorization and integrity modes.** Done 2026-04-25: traced `sha256` config field from `parser.rs:124` through validator (length/hex check at `validator.rs:108`) through `verify_at_startup` to `CommandIntegrity::verify`. Findings RXE-T1-1 (TOCTOU + symlink follow-through), RXE-T1-2 (`every:N` first-call gap, with the existing `every:3` test confirming the gap), RXE-T2-5 (concurrent verify race) document all integrity-mode implications.

## Phase B — Lock down V8 host capabilities (P0-2, P1-5, P1-8, P1-9)

### B1 — Gate `ctx.ddl()` to ApplicationInit only (P0-2)

**Files:** `crates/riversd/src/process_pool/v8_engine/context.rs`, `crates/riversd/src/view_engine/pipeline.rs`, `crates/riversd/src/process_pool/v8_engine/task_locals.rs`

- [x] **B1.1** Add `task_kind` field (or use existing) on `TaskLocals`/`SerializedTaskContext`; populate it explicitly per dispatch path: `ApplicationInit`, `Rest`, `MessageConsumer`, `SecurityHook`, `ValidationHook`.
- [x] **B1.2** `ctx_ddl_callback`: read current task kind; throw JS error unless kind is `ApplicationInit` AND task carries an explicit `app_id` + `datasource_id`.
- [x] **B1.3** Optionally restrict to a per-app DDL allowlist (manifest field). Defer if not needed for canary.
- [x] **B1.4** Negative tests: REST handler, message consumer, validation hook, security hook each call `ctx.ddl(...)` → all four throw.
- [x] **B1.5** Positive test: ApplicationInit handler can call `ctx.ddl("CREATE TABLE …")` against a configured datasource.

### B2 — `ctx.store` failures must not silently fall back (P1-5)

**Files:** `crates/riversd/src/process_pool/v8_engine/context.rs`

- [x] **B2.1** Storage callbacks (`set`, `get`, `delete`): if a storage engine is configured, propagate backend errors as JS exceptions. No `TASK_STORE` fallback path.
- [x] **B2.2** Restrict task-local fallback to an explicit `RIVERS_DEV_NO_STORAGE=1` mode (or equivalent config flag). Document in handler-tutorial.
- [x] **B2.3** Tests: fault-inject backend (Redis down) → `ctx.store.set` throws; assert no silent success.

### B3 — Module cache miss must hard-fail in production (P1-8)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`, `crates/riversd/src/process_pool/module_cache.rs`

- [x] **B3.1** Add a `mode` to module cache: `Production` (miss → error) vs `Development` (miss → disk + live compile, with warn log).
- [x] **B3.2** Default deployed builds to Production; allow opt-in dev mode via env var or `[base.debug]`.
- [x] **B3.3** Test: synthesize an import not present in the prepared cache; assert dispatch returns `MODULE_NOT_REGISTERED` and never compiles live in production mode.

### B4 — Redact host paths in errors (P1-9)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`, `crates/riversd/src/process_pool/module_cache.rs`, `crates/riversd/src/error_response.rs`

- [x] **B4.1** New helper `redact_to_app_relative(path) -> Cow<str>` lives next to `boundary_from_referrer` in `v8_engine/execution.rs`. Reuses the same `libraries`-anchor algorithm as `shorten_app_path` but operates on `&str` and is `pub(crate)` so other crate modules (G_R8.2 SQLite policy) can call it. Returns input unchanged when no `libraries/` segment is present (inline test sources, already-redacted strings, empty inputs).
- [x] **B4.2** V8 script origins (both root module in `execute_as_module` and resolved modules in `resolve_module_callback`) now register the redacted form as the script resource name. V8 stack traces report `{app}/libraries/handlers/foo.ts`, never `/Users/...`.
- [x] **B4.3** Resolve-callback messages (`in {referrer}`, `resolved to:`) and `MODULE_NOT_REGISTERED` formatting in `module_cache::module_not_registered_message` now redact through `redact_to_app_relative`. Boundary line in resolve callback still uses `boundary_from_referrer` (path-only) but its display also runs through redaction.
- [x] **B4.4** New integration test `path_redaction_tests.rs` — dispatches a handler that throws, asserts neither response nor stack contains `/Users/` or workspace prefix. Plus 7 unit tests on `redact_to_app_relative` (multiple input shapes incl. trailing-slash, no-libraries, empty string, already-relative, deep nesting). All green.

---

## Phase C — Restore datasource & app identity on every dispatch path (P1-2)

### C1 — Centralize task-context enrichment

**Files:** `crates/riversd/src/message_consumer.rs`, `crates/riversd/src/security_pipeline.rs`, `crates/riversd/src/view_engine/validation.rs`, `crates/riversd/src/process_pool/v8_engine/task_locals.rs`, new helper module under `crates/riversd/src/dispatch/`

- [x] **C1.1** New `dispatch::build_task_context(app, view, handler_id, kind) -> SerializedTaskContext` — single helper that binds app id, view id, handler id, datasource map, storage engine, lockbox handle, driver factory, debug flag, task kind. Used by every dispatch site.
- [x] **C1.2** Replace all empty-app-id sites: `message_consumer.rs`, `security_pipeline.rs`, `view_engine/validation.rs`, view dispatch error path.
- [x] **C1.3** Remove the `"app:default"` fallback in `TaskLocals::set`. Empty app id is now a programmer error → panic in debug, error log + reject in release.
- [x] **C1.4** Tests: `MessageConsumer.ctx.store.set("k","v")` is readable from the same app's REST handler; cross-app namespace isolation verified.
- [x] **C1.5** Tests: SecurityHook + ValidationHook see the same `ctx.app_id` as the REST handler they wrap.

---

## Phase D — DataView ↔ ConnectionPool integration (P0-3, P1-1, P1-10)

### D1 — Fix pool internals before adoption (P1-1)

**Files:** `crates/riversd/src/pool.rs`

- [x] **D1.1** `PoolGuard::drop`: preserve original `created_at` so `max_lifetime` actually expires the underlying connection. Don't reset on return.
- [x] **D1.2** `acquire`: include `idle_return` queue depth in capacity accounting, OR collapse to a single mutex-protected pool state. No double-counting paths.
- [x] **D1.3** `PoolManager`: replace `Vec<Arc<ConnectionPool>>` with `HashMap<String, Arc<ConnectionPool>>` keyed by datasource id. O(1) lookup, no duplicates possible.
- [x] **D1.4** Tests: long-lived connection expires; burst load doesn't exceed `max_connections`; duplicate datasource id rejected at registration.

### D2 — Route DataView execution through the pool (P0-3)

**Files:** `crates/rivers-runtime/src/dataview_engine.rs`, `crates/rivers-runtime/src/lib.rs`, `crates/riversd/src/pool.rs`, `crates/riversd/src/bundle_loader/load.rs`, `crates/riversd/src/bundle_loader/reload.rs`, `crates/riversd/src/server/context.rs`, `crates/riversd/src/server/handlers.rs`, `crates/riversd/tests/pool_tests.rs`

- [x] **D2.1** New `Arc<PoolManager>` field on `AppContext` (always present, initialized empty in `AppContext::new`). Bundle loader registers one `ConnectionPool` per datasource (default `PoolConfig`, `entry_point:ds_name` keying that mirrors the existing `ds_params` scheme). New `ConnectionAcquirer` trait + `PooledConnection`/`AcquireError` types live in `rivers-runtime` so `DataViewExecutor` can hold an `Arc<dyn ConnectionAcquirer>` without circular dep on the binary crate; `PoolManager` impls the trait via a small `PoolGuardAdapter`. (Done 2026-04-25.)
- [x] **D2.2** `DataViewExecutor::execute`: when `acquirer` is installed and `has_pool(datasource_id)` is true, acquire a `PoolGuard` for the duration of one call (single checkout, multiple `conn.execute/prepare/execute_prepared` calls, RAII drop returns to idle). Pre-existing broker-produce fallback preserved via new `connect_and_execute_or_broker` helper. When `acquirer` is `None` we keep the legacy `factory.connect` per call (warn-logged) so test fixtures without a pool still pass. (Done 2026-04-25.)
- [x] **D2.3** `/health/verbose`: derives `pool_snapshots` and per-datasource probe status from `PoolManager::snapshots()` + per-pool circuit state (no fresh handshake). Datasources without a registered pool (brokers) fall back to the legacy direct-probe path so operators still see them. New `circuit_state` field on `PoolSnapshot` exposes the breaker. (Done 2026-04-25.)
- [x] **D2.4** Three new tests in `crates/riversd/tests/pool_tests.rs::d2`: `d2_4_executor_reuses_pool_connections_for_100_calls` (100 sequential calls → 1 driver handshake, well below `max_size=4`); `d2_4_pool_snapshot_non_empty_after_first_call` (snapshot.idle=1 after first call returns); `d2_4_direct_connect_fallback_still_works_without_acquirer` (3 calls → 3 handshakes when no acquirer wired). All 33 pool tests + 357 lib tests + 38 test binaries green. (Done 2026-04-25.)

**Validate:** ✅ `cargo build -p riversd` clean. `cargo test -p riversd --tests` all binaries pass except pre-existing `cli_tests::version_string_contains_version`. `cargo test -p rivers-runtime` clean (the cache_bench / executor_invalidates_cache_after_write failures pre-date D2 — both DDL-gating issues unrelated to pool routing).

### D3 — Enforce DataView timeouts (P1-10)

**Files:** `crates/rivers-runtime/src/dataview_engine.rs`

- [x] **D3.1** Wrap connect+execute in `tokio::time::timeout(request.timeout)`; map elapsed to `DataViewError::Timeout` with datasource id.
- [x] **D3.2** Health verbose probe: bounded, parallel (`join_all` with per-DS timeout), result cached for short TTL.
- [x] **D3.3** Tests: slow datasource (faker with sleep, or fault-injected Postgres) → timeout fires within budget; request worker freed.

---

## Phase E — Kafka producer & EventBus correctness (P1-3, P1-4)

### E1 — Kafka producer routes by destination (P1-3)

**Files:** `crates/rivers-runtime/src/dataview_engine.rs`, `crates/rivers-plugin-kafka/src/lib.rs`

- [x] **E1.1** Producer: lazy initialization (no metadata call at create time). Cache `PartitionClient` per topic with bounded TTL + exponential backoff on failure.
- [x] **E1.2** `publish(message)` honors `message.destination` for topic routing — not the producer-creation topic.
- [x] **E1.3** Tests: one producer publishes to two distinct destinations; metadata fetch failure on topic A doesn't block topic B.

### E2 — EventBus global priority across exact + wildcard (P1-4)

**Files:** `crates/rivers-core/src/eventbus.rs`

- [x] **E2.1** At dispatch time, merge exact + wildcard subscribers into a single list, then sort by priority. `Expect` < `Handle` < `Emit` < `Observe` (or current order — keep the spec).
- [x] **E2.2** Optionally: enforce at subscribe time that wildcard subscribers may only register at `Observe` priority. Decision in `changedecisionlog.md`.
- [x] **E2.3** Test: wildcard `Expect` runs before exact `Emit`; wildcard `Observe` runs after.

---

## Phase F — Hardening (P1-6, P1-7, P1-11, P1-12)

### F1 — Static files: canonicalize after symlink resolution (P1-6)

**Files:** `crates/riversd/src/static_files.rs`

- [x] **F1.1** Canonicalize both root and resolved file path before serving. Compare canonicalized prefix.
- [x] **F1.2** Reject symlinks outright in production mode (config flag `static.allow_symlinks`, default false).
- [x] **F1.3** Tests: `../` traversal rejected; symlink-out-of-root rejected; legitimate file inside root served.

### F2 — Bound SWC compile time (P1-7)

**Files:** `crates/riversd/src/process_pool/v8_config.rs`, `crates/riversd/src/process_pool/module_cache.rs`

- [x] **F2.1** Run `compile_typescript` in a supervised worker (existing swc supervisor from prior P0 work — extend, don't duplicate). Hard timeout (default 5s, configurable).
- [x] **F2.2** Timeout → `ValidateError::CompileTimeout` with sanitized error and module id.
- [x] **F2.3** Add a small fuzz/property corpus of pathological TS inputs (deep generics, exponential type instantiation). Run under timeout in CI.

### F3 — PostgreSQL config builder, not interpolation (P1-11)

**Files:** `crates/rivers-drivers-builtin/src/postgres.rs`

- [x] **F3.1** Replace string-interpolated connection string with `tokio_postgres::Config` builder calls.
- [x] **F3.2** Tests: passwords with spaces, quotes, `=`, and `&` connect successfully; database names with special chars connect successfully.

### F4 — Validate handler-supplied status & headers (P1-12)

**Files:** `crates/riversd/src/view_engine/validation.rs`

- [x] **F4.1** `parse_handler_view_result`: status must be in `100..=599` else error response.
- [x] **F4.2** Reject header names violating RFC 7230 token grammar; reject header values with CR/LF/NUL.
- [x] **F4.3** Decision: do we block handler-set security headers (CSP, HSTS, etc.) absent explicit policy opt-in? Log in `changedecisionlog.md`, then enforce.
- [x] **F4.4** Tests: status 999 rejected; header `X-Bad: foo\r\nInjection: yes` rejected.

---

## Phase G — P2 nice-to-haves

### G_R1 — Redis cluster: SCAN, not KEYS (P2-1)

**Files:** `crates/rivers-storage-backends/src/redis_backend.rs`, `crates/rivers-core/src/storage.rs`

- [x] **G_R1.1** Cluster path: iterate primaries, run `SCAN` per node, merge cursors. Mirror the single-node implementation.
- [x] **G_R1.2** Replace any hot-path key listing with explicit ownership records or index sets where feasible.
- [x] **G_R1.3** Test against the 3-node cluster (192.168.2.206-208).

### G_R2 — EventBus subscription lifecycle (P2-2)

**Files:** `crates/rivers-core/src/eventbus.rs`

- [x] **G_R2.1** `subscribe` returns a `SubscriptionHandle` that unregisters on `Drop`.
- [x] **G_R2.2** Bound broadcast subscribers; tie to request/session lifetime where applicable.
- [x] **G_R2.3** Add metrics: `rivers_eventbus_subscribers{kind}`, `rivers_eventbus_dispatch_seconds{event}`.

### G_R3 — Single source of truth for reserved storage prefixes (P2-3)

**Files:** `crates/rivers-core-config/src/storage.rs`, `crates/riversd/src/process_pool/v8_engine/context.rs`

- [x] **G_R3.1** Move reserved-prefix list to one shared `const RESERVED_PREFIXES: &[&str]` module. Both core storage and V8 context import it.
- [x] **G_R3.2** Test that every public storage entry point enforces the same set (reflection-style test or shared helper).

### G_R4 — Lifecycle observer hooks: contract or timeout (P2-4)

**Files:** `crates/riversd/src/view_engine/pipeline.rs`

- [x] **G_R4.1** Decision: truly fire-and-forget (spawn into bounded queue) vs awaited-with-timeout. Log in `changedecisionlog.md`.
- [x] **G_R4.2** Implement chosen path; remove misleading "fire-and-forget" comment if awaited.
- [x] **G_R4.3** Test: slow observer does not extend request latency past contract bound.

### G_R5 — Module detection by metadata, not string match (P2-5)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [x] **G_R5.1** Use bundle metadata (file is registered as a module) or extension. Drop `contains("export ")` heuristic.
- [x] **G_R5.2** Tests: comment containing `export ` does not flip the path; string literal containing `import ` does not flip the path.

### G_R6 — Promise resolution tied to task timeout (P2-6)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [x] **G_R6.1** Promise-pump loop honors the configured task timeout; pending-promise error includes timeout value and handler id.
- [x] **G_R6.2** Tests: handler `await new Promise(r => setTimeout(r, 100))` resolves under a 1s timeout; same handler with 10ms timeout returns a clear timeout error.

### G_R7 — MySQL pool ownership & DriverFactory runtime strategy (P2-7)

**Files:** `crates/rivers-drivers-builtin/src/mysql.rs`, `crates/rivers-core/src/driver_factory.rs`

- [x] **G_R7.1** After D2 lands: MySQL `mysql_async::Pool` is datasource-scoped (one per datasource), not per-connection.
- [x] **G_R7.2** `DriverFactory::connect`: keep `spawn_blocking` + isolated runtime ONLY for plugin drivers that require it. Built-in async drivers run on the active runtime.
- [x] **G_R7.3** Document the policy in `crates/rivers-core/src/driver_factory.rs` doc comment.

### G_R8 — SQLite path policy (P2-8)

**Files:** `crates/rivers-drivers-builtin/src/sqlite.rs`, `riversd.toml` schema

- [x] **G_R8.1** Restrict SQLite paths to an approved data dir (config: `sqlite.allowed_root`). Reject paths outside on bundle load.
- [x] **G_R8.2** Redact absolute paths in production logs (uses B4.1 helper).
- [x] **G_R8.3** Don't auto-`mkdir -p` parent dirs unless `sqlite.create_parent_dirs = true`.

---

## Cross-cutting test recommendations (review §Test Recommendations)

These are not separate phases — they are the verification bar for the work above. Each appears in the relevant phase's task list. Repeated here as a single checklist for canary integration:

- [x] Non-public view + missing session manager → fail closed (A2.3)
- [x] REST handler cannot call `ctx.ddl` (B1.4)
- [x] ApplicationInit can call allowed DDL; cannot call disallowed DDL (B1.5)
- [x] MessageConsumer `ctx.store.set` readable from same-app HTTP handler (C1.4)
- [x] DataView calls reuse pool state and obey max connections (D2.4)
- [x] DataView connect+query timeout fires (D3.3)
- [x] Broker startup completes when Kafka unreachable (A1.4)
- [x] Kafka producer routes per `OutboundMessage.destination` (E1.3)
- [x] Wildcard `Expect` runs before exact `Emit`/`Observe` (E2.3)
- [x] Static file symlink escape rejected (F1.3)
- [x] Module cache miss fails in production (B3.3)
- [x] Public errors redact absolute paths (B4.4)
- [x] Redis cluster list uses SCAN (G_R1.3)

---

## Files touched (hot list)

- `crates/riversd/src/security_pipeline.rs`
- `crates/riversd/src/process_pool/v8_engine/context.rs`
- `crates/riversd/src/process_pool/v8_engine/execution.rs`
- `crates/riversd/src/process_pool/v8_engine/task_locals.rs`
- `crates/riversd/src/process_pool/v8_config.rs`
- `crates/riversd/src/process_pool/module_cache.rs`
- `crates/riversd/src/bundle_loader/wire.rs`
- `crates/riversd/src/message_consumer.rs`
- `crates/riversd/src/view_engine/pipeline.rs`
- `crates/riversd/src/view_engine/validation.rs`
- `crates/riversd/src/server/handlers.rs`
- `crates/riversd/src/static_files.rs`
- `crates/riversd/src/error_response.rs`
- `crates/riversd/src/pool.rs`
- new: `crates/riversd/src/dispatch/` (centralized task-context builder)
- `crates/rivers-runtime/src/dataview_engine.rs`
- `crates/rivers-core/src/eventbus.rs`
- `crates/rivers-core/src/driver_factory.rs`
- `crates/rivers-core-config/src/storage.rs`
- `crates/rivers-storage-backends/src/redis_backend.rs`
- `crates/rivers-drivers-builtin/src/postgres.rs`
- `crates/rivers-drivers-builtin/src/mysql.rs`
- `crates/rivers-drivers-builtin/src/sqlite.rs`
- `crates/rivers-plugin-kafka/src/lib.rs`
- `changedecisionlog.md`, `todo/changelog.md`, `bugs/<per-finding>.md`

---

## Execution order (review §Recommended Remediation Sequence)

1. **A1** — broker consumer nonblocking startup (boot blocker)
2. **A2** — protected views fail closed (security)
3. **B1, B2, B3, B4** — V8 host capability lockdown (security + observability)
4. **C1** — centralize task-context enrichment (foundation for D)
5. **D1 → D2 → D3** — pool fix → DataView integration → timeouts (in this strict order; D2 depends on D1)
6. **E1, E2** — Kafka destination semantics + EventBus priority
7. **F1–F4** — hardening (parallelizable with E)
8. **G_R1–G_R8** — P2 cleanup; schedule per quarter

**Dependency notes:**
- C1 is a foundation for B1 (task-kind needs to be set somewhere) — sequence them in the same effort.
- D1 must land before D2 to avoid amplifying pool bugs.
- B4 (path redaction) provides the helper used by G_R8.2 — do B4 first.
- G_R7 depends on D2 — schedule after.

---

## Verification — end to end

1. `cargo test --workspace` — all crate suites green; net-new tests for each `[ ]` task above.
2. `cargo deploy /tmp/rivers-review-fix` — deploy succeeds.
3. `canary-bundle/run-tests.sh` — ALL profiles green on full infra (192.168.2.x).
4. Boot-with-broker-down test: `iptables -A OUTPUT -p tcp --dport 9092 -j DROP` → `riversd` boots; `/health/verbose` reports broker degraded; remove rule → consumer recovers.
5. Misconfig test: deploy a bundle with a protected view + no session config → boot refuses with clear error.
6. Negative-capability tests: REST handler attempting `ctx.ddl` returns clear JS error; `ctx.store` failure with backend down throws (not silently succeeds).
7. Pool reuse test: 1000 sequential DataView calls show pool active+idle bounded by configured max.
8. Path redaction test: trigger handler error, grep response for absolute workspace path → 0 matches.

---

## Effort summary

| Phase | Findings | Effort | Risk |
|-------|----------|--------|------|
| A boot/security | P0-1, P0-4 | ~1 day | high (hot path) |
| B V8 lockdown | P0-2, P1-5, P1-8, P1-9 | ~2 days | high (handler-visible behavior) |
| C task identity | P1-2 | ~1 day | medium (touches all dispatch sites) |
| D pool integration | P0-3, P1-1, P1-10 | ~2-3 days | high (perf + correctness) |
| E Kafka + EventBus | P1-3, P1-4 | ~1 day | medium |
| F hardening | P1-6, P1-7, P1-11, P1-12 | ~1 day | low |
| G P2 cleanup | P2-1..P2-8 | ~2 days | low |
| **Total** | 24 findings | **~10-11 days** | |

---

## Open decisions (log in `changedecisionlog.md` as work proceeds)

1. **B1.3** — Per-app DDL allowlist or just task-kind gate? (default: task-kind gate only)
2. **B3.2** — Production module-cache mode flag name & default. (default: production-strict, opt-in dev)
3. **C1.3** — Empty-app-id fallback removal: panic-in-debug + reject-in-release vs reject everywhere?
4. **D2.1** — Pool ownership: app runtime context vs `DataViewExecutor` member?
5. **E2.2** — Wildcard subscribers restricted to `Observe` priority?
6. **F4.3** — Block handler-set security headers (CSP, HSTS) absent explicit policy?
7. **G_R4.1** — Lifecycle observers: fire-and-forget queue vs awaited-with-timeout?

---

## Non-goals

- Driver feature additions (no new datasource types).
- Spec rewrites (G8 already shipped).
- Performance benchmarking suite (separate sprint).
- Plugin ABI changes (engine-sdk and driver-sdk stay v-current).

---

## Final Status (2026-04-25)

**ALL 22 task groups across Phases A–G complete.** 24 commits on `claude/wizardly-bassi-bf1b67`.

### Verification

- `cargo test --workspace --lib` — all crate suites green
- `cargo test -p riversd --tests` — 39/40 integration test files green; only failure is the **pre-existing** `cli_tests::version_string_contains_version` (hardcodes "0.50.1" vs current 0.55.0; spawned for separate cleanup, unrelated to this branch's work)
- New tests added across the branch: ~60 new tests across pool, security_pipeline, dataview timeout, Kafka destination, EventBus priority, static-file canonicalization, SWC timeout, postgres config, handler header validation, Redis SCAN, reserved-prefix sharing, V8 module detection, SQLite path policy, EventBus subscription lifecycle, observer hook timeout, V8 promise timeout, MySQL pool ownership, runtime policy.

### Open follow-ups (non-blocking)

1. **`cli_tests` version assertion** — pre-existing, spawned as separate task.
2. **NIT I-1 (C1+B1 review)** — `SerializedTaskContext` doc says receivers MUST treat `task_kind: None` as `Rest`; dynamic engine stores `None` directly. Functionally equivalent for B1 (gate rejects None correctly), but a small clarification commit could either honor the literal contract or drop the MUST language. Defer.
3. **`MysqlConnection.options` field** — F3 / G_R7 didn't wire ConnectionParams.options into the typed builder. Pre-existing scope, separate task.
4. **MySQL pool eviction on credential rotation** — G_R7 noted that the shared `mysql_async::Pool` registry has no eviction path on hot reload. Worth a follow-up.
5. **End-to-end canary run** — none of these P0/P1/P2 fixes have been validated on the live 192.168.2.x cluster yet. The unit + integration test layer is green; canary-bundle/run-tests.sh against PG/Kafka/Redis/Mongo/MySQL clusters is the next gate.

### Decisions logged

7 controller decisions resolved against the open-decision list (B1.3, B3.2, C1.3, D2.1, E2.2, F4.3, G_R4.1). Rationale captured in `changedecisionlog.md`.

---

## Phase H — Residual code-review gaps (post-2026-04-25 audit)

> **Source:** Fresh gap analysis 2026-04-25 (PM) against `docs/code_review.md` (current 725-line review with Tier-based finding IDs — T1=blocker, T2=correctness, T3=hardening) on `origin/main` at `42103fc`. Phases A–G closed the 24 P0/P1/P2 findings from the prior review pass; the following items are either **still open** in the current review document or were partially addressed and have a verified residual gap.
>
> **Verified directly by reading source on 2026-04-25:** H4 (`mysql.rs:44–49` — password still excluded from pool key with broken rationale), H11 (`eventbus.rs:458–471` — `Observe` handlers still `tokio::spawn` unbounded). Other items inherited verdicts from the gap report; verify before starting.

### Tier 1 — production blockers (4)

- [x] **H1 — riversd T1-4: V8 `ctx.ddl()` bypasses the DDL whitelist path.** DONE 2026-04-27: Whitelist check added in `ctx_ddl_callback` (context.rs:721–777) before `factory.connect()` is called. Consults `engine_loader::ddl_whitelist()` and `engine_loader::app_id_for_entry_point()` — same data structures as the dynamic-engine path. Error message matches `host_ddl_execute` verbatim. Integration tests in `crates/riversd/tests/v8_ddl_whitelist_tests.rs`: `h1_whitelisted_ddl_succeeds_for_application_init` and `h1_unwhitelisted_ddl_rejected_for_application_init` both pass (SQLite-backed, verify table creation/absence post-check).
  **File:** `crates/riversd/src/process_pool/v8_engine/context.rs:614–722` (`ctx_ddl_callback`).
  Phase B1 gated `ctx.ddl()` to `ApplicationInit` (good), but the callback then calls `factory.connect()` and `conn.ddl_execute()` directly, never consulting `DDL_WHITELIST` the way the dynamic-engine path (`engine_loader/host_callbacks.rs`) does. An init handler can run any DDL the connecting user has permission for, regardless of the per-app/per-database allowlist.
  Validation:
  - Init handler calling `ctx.ddl("DROP TABLE …")` against a database **not** in `app.manifest.init.ddl_whitelist` rejects with the same `DDL operation not permitted` error the dynamic-engine path produces.
  - Whitelisted DDL still succeeds.
  - Unit test alongside the existing B1 negative tests.

- [x] **H2 — riversd T1-6: Synchronous V8 host bridge has no timeout.** DONE 2026-04-28: All blocking `recv()` sites in `engine_loader/host_callbacks.rs` replaced with `recv_timeout(HOST_CALLBACK_TIMEOUT_MS)` + JoinHandle abort on timeout. Covered: `host_dataview_execute`, `host_store_get`, `host_store_set`, `host_store_del`, `host_datasource_build`, `host_ddl_execute`. V8-path (`context.rs`) was already fixed in prior round. Two new unit tests added: `dyn_engine_recv_timeout_returns_timeout_when_task_hangs` and `dyn_engine_host_callback_budget_is_bounded_and_nonzero`. Error code -13 used for timeout (distinct from -10 driver-error and -12 panic). 428 tests pass.
  **File:** `crates/riversd/src/process_pool/v8_engine/context.rs:708–722` (and analogous `recv()` sites in adjacent host callbacks).
  The pattern is `tokio::spawn(async move { … tx.send(...) }); rx.recv()` (blocking). If the spawned task stalls (driver hang, pool starvation), `recv()` waits forever and pins the V8 worker.
  Validation:
  - Wrap each blocking `recv()` in a deadline derived from the configured task timeout (use `recv_timeout` on a `std::sync::mpsc` or convert to a `tokio::sync::oneshot` with `tokio::time::timeout`).
  - On timeout: throw a JS error with the budget value and the host-callback name. Cancel the spawned task if possible.
  - Test: handler invokes a host callback that intentionally never replies; assert worker reclaims within `task_timeout_ms + 100ms`.

- [x] **H3 — rivers-core T1-1: Plugin ABI version probe not panic-contained.** DONE 2026-04-27: `call_ffi_with_panic_containment` helper (lines 298–303) wraps all FFI probes including the ABI version call at line 347. `AssertUnwindSafe` is sound for raw fn-pointer closures. Returns `PluginLoadResult::Failed` with "_rivers_abi_version panicked" on panic. All 33 rivers-core drivers_tests pass.
  **File:** `crates/rivers-core/src/driver_factory.rs:305–312` (call to `_rivers_abi_version`).

- [x] **H4 — rivers-drivers-builtin T1-1: MySQL pool cache key omits password.** DONE 2026-04-27: SHA-256 password fingerprint (8 bytes hex) included in pool key; evict_pool + is_auth_error + retry on auth failure in connect(). 2 cluster-gated conformance tests. Unit test `is_auth_error_boundary_codes` covers codes 1043/1044/1045/1046.
  **File:** `crates/rivers-drivers-builtin/src/mysql.rs:39–49` (`pool_key`).
  Two datasources with same `(host, port, database, username)` but different passwords end up sharing whichever pool got created first. The doc-comment rationale ("auth will reject and we'll re-create next time") is wrong — `get_or_create_pool` returns the cached pool unconditionally; nothing evicts on auth failure. Result: rotated/separate-tenant credentials silently fail or, worse, route to the wrong account.
  Validation:
  - Hash the password (e.g. `sha256` truncated to 8 bytes hex) and append to the key. Never include raw password bytes.
  - Add an eviction path on first auth-failure: if the cached pool's first checkout returns an auth error, evict and rebuild.
  - Test: register two datasources with same host/db/user, different passwords; both connect successfully and route to their own credentials.
  - Test: rotate password on a datasource (re-register `ConnectionParams`); old pool is evicted on next checkout failure.

### Tier 2 — correctness / contract (10)

- [x] **H5 — riversd T2-2: Connection-limit race in WebSocket and SSE registries.** DONE 2026-04-27: WebSocket registry (`websocket.rs`): limit check and insert now happen under the same `write().await` lock — `conns.len() >= max` is evaluated inside the held `RwLock` write guard, so no concurrent registration can pass the check and then race past the insert. SSE channel (`sse.rs`): uses `AtomicUsize::fetch_update` (compare-exchange loop) — atomically checks `current < max` and increments in one CAS; returns `ConnectionLimitExceeded` on failure. Both paths have concurrent-stress tests (38 riversd unit tests pass).
  **Files:** `crates/riversd/src/websocket.rs`, `crates/riversd/src/sse.rs`.

- [x] **H6 — riversd T2-6: V8 outbound HTTP host callback has no timeout.**
  Done 2026-04-27. New `crates/riversd/src/http_client.rs` module provides `outbound_client()` — a process-wide `reqwest::Client` built with `.timeout(30_000ms)` and `.connect_timeout(5s)`. V8 path (`http.rs:134`) now calls `crate::http_client::outbound_client()` instead of `reqwest::Client::new()`. Two tests: `outbound_client_is_shared` (proves OnceLock identity) and `outbound_http_times_out_on_unreachable_endpoint` (TEST-NET-3, fires within 35s). All 428 lib tests green.

- [x] **H7 — riversd T2-7: Dynamic engine HTTP host callback also lacks timeout.**
  Done 2026-04-27. Dynamic-engine path (`host_context.rs:342`) sets `http_client: crate::http_client::outbound_client().clone()` — same shared client as H6. Both paths now share identical 30s timeout / 5s connect-timeout policy. Validated with same test suite.

- [x] **H8 — riversd T2-8: Transaction host callbacks are stubs (dynamic-engine path).**
  Done 2026-04-25 — Phase I (I1-I9 + I-X.1-3) closed it end-to-end. See Phase I commits on `feature/phase-i-dyn-transactions` and `changedecisionlog.md` TXN-I1.1 / TXN-I2.1 / TXN-I6+I7.1 / TXN-I8.1.

ORIGINAL ENTRY:
  **File:** `crates/riversd/src/engine_loader/host_callbacks.rs:887-1020` (`host_db_begin`, `host_db_commit`, `host_db_rollback`).
  **Scope clarified 2026-04-25:** the V8 path is **already fully implemented** (`process_pool/v8_engine/context.rs::ctx_transaction_callback` ~line 898 with `TASK_TRANSACTION` map, real begin/commit/rollback semantics, timeout handling per H2, and a `TASK_COMMIT_FAILED` financial-correctness upgrade). The stubs are limited to the dynamic-engine cdylib host callbacks — comments explicitly say `TODO: Wire to TransactionMap in Task 8`.
  **Decision (2026-04-25):** implement properly — mirror the V8 semantics on the cdylib side, re-using `Connection::begin_transaction/commit_transaction/rollback_transaction` (which are already on the trait at `crates/rivers-driver-sdk/src/traits.rs:517-535`) and `DataViewExecutor::execute(..., txn_conn: Some(...))` (already wired at `crates/rivers-runtime/src/dataview_engine.rs:759-783`).

  Sub-tasks **I1–I9** below.

### Phase I — Dynamic-engine transactions (H8 implementation)

> **Source:** Decision under H8 (2026-04-25) to implement the dyn-engine transaction path properly rather than throw `not implemented`. Mirrors the V8 implementation at `process_pool/v8_engine/context.rs::ctx_transaction_callback`. **Goal:** every WASM/cdylib task can call `Rivers.db.begin/commit/rollback` and `Rivers.db.execute` inside a transaction with the same correctness guarantees as the V8 path.
>
> **Key existing scaffolding** (verified 2026-04-25):
> - `Connection::begin_transaction/commit_transaction/rollback_transaction` exist on the driver trait (`rivers-driver-sdk/src/traits.rs:517-535`) with default-error impls.
> - `Connection::execute_batch("BEGIN" | "COMMIT" | "ROLLBACK")` already implemented for postgres, mysql, sqlite (`rivers-drivers-builtin/src/{postgres,mysql,sqlite}.rs`).
> - `DataViewExecutor::execute(..., txn_conn: Option<&mut Box<dyn Connection>>)` already supports the transactional path with cross-datasource rejection.
> - `HOST_CALLBACK_TIMEOUT_MS = 30_000` constant from H2 — apply the same budget to dyn-engine commit/rollback.
> - `TaskError::TransactionCommitFailed` already exists for the financial-correctness upgrade.

- [x] **I1 — Audit + design.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I1.1 and `docs/superpowers/plans/2026-04-25-phase-i-dyn-transactions.md`. Decisions: map keyed `(TaskId, datasource)`, sibling `OnceLock<DynTransactionMap>` in `engine_loader::host_context`, auto-rollback hook on `dispatch_task` exit via `TaskGuard::drop`.
  Read these in full and decide three things before any code:
  - V8 path: `crates/riversd/src/process_pool/v8_engine/context.rs:895-1100` (the entire `ctx_transaction_callback` plus the `TASK_TRANSACTION` thread-local definition + `TxnMap` type wherever it lives).
  - Dyn-engine stubs: `crates/riversd/src/engine_loader/host_callbacks.rs:887-1020` (`host_db_begin`, `host_db_commit`, `host_db_rollback`).
  - `host_db_execute` (DataView dispatch on the cdylib path) — find it, understand how it currently looks up `DataViewExecutor` and whether/how it could pass a `txn_conn`.
  Decisions:
  1. **Scope key:** what identifies "the current task" on the cdylib side? V8 uses a thread-local because tasks run on the V8 worker thread end-to-end. Cdylib host callbacks are invoked from engine threads that may not be 1:1 with task identity. Likely answer: include `task_id` in the input JSON for every `host_db_*` call and key the map by `(task_id, datasource)`. Confirm by reading what fields `host_db_*` already accept (most callbacks already take `task_id` via `read_input`).
  2. **Map storage:** parallel `OnceLock<Mutex<HashMap<(TaskId, String), Box<dyn Connection>>>>` next to `HOST_CONTEXT`, OR re-use V8's `TASK_TRANSACTION` (no — it's a thread-local on V8's worker; cdylib threads can't see it).
     Pick parallel map; name it `DYN_TXN_MAP`.
  3. **Auto-rollback hook:** how does the cdylib task lifecycle signal "task done — clean up any leftover txn"? Likely the engine wrapper that dispatches a wasm/dylib task already has a finally-style block. Find it and plan to call `dyn_txn_map.rollback_all_for_task(task_id)` there.
  Output: a 1-page decision note appended to `changedecisionlog.md` as `### TXN-I1.1 — Dyn-engine transaction map design`.

- [x] **I2 — Define `DynTransactionMap` type + module.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I2.1. Module `crates/riversd/src/engine_loader/dyn_transaction_map.rs` with `(TaskId, datasource)`-keyed inner `tokio::Mutex<HashMap>`; full begin/has/take/with_conn_mut/commit/rollback/auto_rollback_all_for_task surface + 6 unit tests passing.
  **Files:** new `crates/riversd/src/engine_loader/transaction_map.rs`; modify `crates/riversd/src/engine_loader/mod.rs` to declare/re-export.
  Type sketch (adapt to actual types and async-trait import):
  ```rust
  pub(crate) struct DynTransactionMap {
      inner: tokio::sync::Mutex<HashMap<(TaskId, String), Box<dyn Connection>>>,
  }
  impl DynTransactionMap {
      pub fn new() -> Self { ... }
      pub async fn begin(
          &self, task_id: TaskId, datasource: &str, conn: Box<dyn Connection>,
      ) -> Result<(), DriverError>;          // errors if (task_id, ds) already exists
      pub async fn take(
          &self, task_id: TaskId, datasource: &str,
      ) -> Option<Box<dyn Connection>>;     // remove + return
      pub async fn with_conn_mut<F, R>(
          &self, task_id: TaskId, datasource: &str, f: F,
      ) -> Option<R>
      where
          F: FnOnce(&mut Box<dyn Connection>) -> R;
      pub async fn rollback_all_for_task(&self, task_id: TaskId);
  }
  ```
  Plus a single `OnceLock<DynTransactionMap>` accessor in `engine_loader::host_context` (or wherever `HOST_CONTEXT` lives). Pattern after the existing `OnceLock` accessors added in H1.
  Tests: unit-test that `begin` rejects duplicate `(task_id, ds)`, `take` is one-shot, `rollback_all_for_task` drains exactly that task's entries.

- [x] **I3 — Implement `host_db_begin`.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I6+I7.1 (covers I3+I4+I5 landing). `host_db_begin_inner` (host_callbacks.rs:1094) reads task_id, looks up `(driver_name, ConnectionParams)` via `lookup_task_ds(task_id, ds)` against `TASK_DS_CONFIGS`, runs `factory.connect → conn.begin_transaction` under `block_on` (safe on spawn_blocking worker), inserts into `dyn_txn_map`. Returns `{"ok": true, "datasource": ...}` on success.
  **File:** `crates/riversd/src/engine_loader/host_callbacks.rs` (replace the stub at ~line 902-928).
  Steps the implementation should perform, in order:
  1. Read input JSON; require `task_id` and `datasource` fields. Return `-3` with `{"error": "missing field"}` on missing.
  2. Look up the `PoolManagerHandle` from `HOST_CONTEXT`. Acquire a connection: `let conn = pool.acquire(&datasource).await?` — use the same trait the V8 path uses; expose via a getter on `HOST_CONTEXT` if not already exposed. If `acquire` returns `Ok(None)` (broker datasource), return error JSON.
  3. Call `conn.begin_transaction().await`. If it errors, drop the conn (PoolGuard returns to pool naturally) and return error JSON.
  4. Insert into `DYN_TXN_MAP::begin(task_id, &datasource, conn)`. If insert errors (already exists), call `conn.rollback_transaction().await` and return error JSON — never silently overwrite.
  5. Return `{"ok": true, "datasource": datasource}` on success.
  Test: integration test that begins, then asserts `DYN_TXN_MAP` contains the entry; teardown via `rollback`.

- [x] **I4 — Implement `host_db_commit`.** Done 2026-04-25 — `host_db_commit_inner` (host_callbacks.rs:1273) takes the conn from `dyn_txn_map`, wraps `conn.commit_transaction()` in `tokio::time::timeout(HOST_CALLBACK_TIMEOUT_MS)`, and on driver error or timeout calls `signal_commit_failed(ds, msg)` (financial-correctness gate). Dispatch upgrades the resulting handler error to `TaskError::TransactionCommitFailed { datasource, message }`.
  **File:** same.
  Mirror the V8 commit semantics:
  1. Read `task_id` + `datasource`; resolve to map key.
  2. `let conn = DYN_TXN_MAP::take(task_id, &datasource).await` — if `None`, return error JSON `{"error": "no active transaction for datasource"}`.
  3. `tokio::time::timeout(HOST_CALLBACK_TIMEOUT_MS, conn.commit_transaction()).await` — three outcomes:
     - `Ok(Ok(()))`: success → `{"ok": true}`. Conn drops back to pool via PoolGuard.
     - `Ok(Err(e))`: commit failure. Set `TASK_COMMIT_FAILED` (or its dyn-engine equivalent — find or add one). Return error JSON `{"error": "TransactionError: commit failed: <msg>", "fatal": true}`.
     - `Err(_)` (timeout): same financial-correctness upgrade. Return `{"error": "TransactionError: commit timed out after 30000ms", "fatal": true}`. Connection abandoned (no rollback attempted — same conservative policy as V8).
  Test: integration that commits, verifies persistence on a real backend (postgres if available).

- [x] **I5 — Implement `host_db_rollback`.** Done 2026-04-25 — `host_db_rollback_inner` (host_callbacks.rs:1418) takes the conn from `dyn_txn_map`, wraps `conn.rollback_transaction()` in `tokio::time::timeout(HOST_CALLBACK_TIMEOUT_MS)`. Idempotent (no active txn → `{"ok": true}` with no work). Driver error or timeout returns `{"ok": true, "warning": ...}` since rollback failures don't trip `TASK_COMMIT_FAILED` — the writes were never committed.
  **File:** same.
  1. Read `task_id` + `datasource`.
  2. `let conn = DYN_TXN_MAP::take(task_id, &datasource).await` — if `None`, return success (idempotent: rolling back nothing is a no-op).
  3. `tokio::time::timeout(HOST_CALLBACK_TIMEOUT_MS, conn.rollback_transaction()).await` with timeout/error logged at `warn` (rollback failures don't trip `TASK_COMMIT_FAILED` — the writes were never committed). Return `{"ok": true}` even on rollback errors (so the caller's retry/cleanup logic isn't blocked) but include `"warning"` field with the message.

- [x] **I6 — Wire `host_db_execute` (DataView) to thread `txn_conn`.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I6+I7.1. New `execute_dataview_with_optional_txn(executor: Arc<DataViewExecutor>, ...)` helper (host_callbacks.rs:218) checks `task_active_datasources`, enforces spec §6.2 cross-DS rejection, threads the held conn via `DynTransactionMap::with_conn_mut` (lock dropped during the executor await). Falls through to the normal pool path when no txn is active for the task.
  **File:** the cdylib DataView host callback (find via `grep -n "host_db_execute\|host_dataview\|fn host_db_query" crates/riversd/src/engine_loader/host_callbacks.rs`).
  After resolving the dataview's datasource, check `DYN_TXN_MAP` for an active `(task_id, datasource)` entry:
  ```rust
  let result = DYN_TXN_MAP.with_conn_mut(task_id, &datasource, |conn| {
      executor.execute(name, params, method, trace_id, Some(conn)).await
  }).await
  .unwrap_or_else(|| {
      // No txn for this datasource — normal pool-acquire path
      executor.execute(name, params, method, trace_id, None).await
  });
  ```
  (The `with_conn_mut` async closure may need a small dance because Rust async closures aren't first-class — use a manual pattern: `take`, run, re-insert, OR add an `apply_async` method to `DynTransactionMap` that holds the mutex during the async call. Mind that holding a `tokio::Mutex` across an .await on a different conn is fine; just don't let the `txn_conn` operation block forever.)
  Cross-datasource enforcement: `DataViewExecutor::execute` already rejects when the dataview's datasource differs from the open transaction's datasource (via `datasource_for`) — verify this still triggers.
  Test: integration test that issues a `dataview("write_x")` on datasource A inside a transaction on A → write executes on the txn conn (verify with a second non-txn dataview that doesn't see the write until commit).

- [x] **I7 — Auto-rollback on cdylib task end.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I6+I7.1. `dispatch_task` extracted its dyn-engine branch into `dispatch_dyn_engine_task(ctx, serialized, engine_runner)` (process_pool/mod.rs:316); the closure body wraps engine execution in a `TaskGuard::enter` whose Drop calls `dyn_txn_map().auto_rollback_all_for_task(task_id)` and clears `TASK_DS_CONFIGS`. Fires whether the engine returns Ok, Err, or panics (panic gets mapped to WorkerCrash but the guard's drop still runs since it lives on the closure stack).
  **File:** the dispatch wrapper that runs a cdylib/wasm task end-to-end. Find via `grep -n "spawn.*engine_run\|dispatch_task\|engine_loader::run_task" crates/riversd/src --include="*.rs"`.
  After the task entry-point returns (success OR failure), call `DYN_TXN_MAP.rollback_all_for_task(task_id).await`. This guarantees no leaked transactions if a handler panics, returns an error, or calls `begin` without `commit`.
  Test: cdylib task that calls `begin` then panics → next `acquire` on the same datasource succeeds (no leaked checkout).

- [x] **I8 — End-to-end tests.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I8.1. New `mod dyn_e2e_tests` in `crates/riversd/src/process_pool/mod.rs` (5 tests, all green): commit persists, rollback discards, auto-rollback on engine error, cross-datasource rejection in txn, two-task isolation by TaskId. Uses real built-in SQLite driver against tempfile-backed DBs; durability oracle uses a fresh `rusqlite::Connection::open(...)` outside the dispatch. Postgres parallel cases not added — 192.168.2.209 reachability not assured from worktree; can land later under `#[ignore]`.
  **File:** new `crates/riversd/tests/dyn_engine_transaction_tests.rs` (or extend an existing wasm/cdylib test file).
  Required cases:
  1. **Commit persists:** wasm task begins, writes via `Rivers.db.execute`, commits. Outside the task, a fresh dataview call sees the row.
  2. **Rollback discards:** wasm task begins, writes, rolls back. Outside, the row is absent.
  3. **Auto-rollback on task error:** wasm task begins, writes, returns error. Outside, the row is absent (auto-rollback fired).
  4. **Cross-datasource rejection:** transaction open on `ds-a`, dataview call on `ds-b` → JS error.
  5. **Concurrent transactions don't share state:** two tasks each open a transaction on the same datasource; their writes are isolated until commit.
  6. **Commit timeout upgrades the error:** mock driver whose `commit_transaction` sleeps past 30s → caller sees `TransactionCommitFailed`-style error.
  Use the postgres test cluster at `192.168.2.209` for cases 1-5 if available; otherwise skip those gated on infra (per the canary-bundle pattern).

- [x] **I9 — Update spec + remove all `TODO: Wire to TransactionMap in Task 8` comments.** Done 2026-04-25 — see `changedecisionlog.md` TXN-I8.1. Three TODO comments removed in I3-I5 (db_begin/commit/rollback wired to real impls); db_batch's stale TODO replaced with a fn-doc note clarifying that `Rivers.db.batch` is a DataView batch-execute primitive (NOT a transaction wrapper) and that wiring lands separately. New §6.8 "Transactions" subsection in `docs/arch/rivers-data-layer-spec.md` covering both engines. `docs/arch/rivers-driver-spec.md` §2 updated with the dyn-engine note. `docs/code_review.md` T2-8 annotated with this PR's resolution. tasks.md flipped per this section.
  **Files:** `docs/arch/rivers-data-layer-spec.md` (add a §"Dynamic-engine transactions" subsection mirroring the V8 description), `docs/arch/rivers-driver-spec.md` (note that `begin/commit/rollback_transaction` are now exercised by both engines), and the three host callbacks (delete the TODO comments now that they're implemented).
  Update `docs/code_review.md` T2-8 with `Resolved YYYY-MM-DD by <commit-sha>` per H-X.1.

### Sequencing for Phase I

1. **I1** (audit + decision) — must come first; outputs the design note.
2. **I2** (DynTransactionMap) — pure infrastructure, no behavior change.
3. **I3 → I4 → I5** — implement begin/commit/rollback in order; each is testable in isolation.
4. **I6** — wire DataView through the txn map (depends on I2-I5).
5. **I7** — auto-rollback hook (depends on I2).
6. **I8** — end-to-end tests (depends on everything above).
7. **I9** — docs cleanup at the end.

### Cross-cutting

- [x] **I-X.1** — annotate `docs/code_review.md` T2-8 with resolution sha after I8. Done 2026-04-25 — annotation added with cross-references to the specific files/line-ranges that close the finding (dyn_transaction_map.rs, host_callbacks.rs:1062-1473 for begin/commit/rollback, host_callbacks.rs:218-298 for execute_dataview_with_optional_txn, process_pool/mod.rs:316-384 for dispatch_dyn_engine_task + TaskGuard). H1-H15 broader annotation pass DEFERRED — per the brief's mechanical-only decision rule, mapping each H finding to its specific PR #83 commit was not ≤5min; tracked as a follow-up TODO below.
- [x] **I-X.2** — log a decision-log entry for every non-obvious choice (auto-rollback semantics, timeout-on-rollback policy, map-key shape). Done 2026-04-25 — see `changedecisionlog.md` entries TXN-I1.1 (audit + design), TXN-I2.1 (DynTransactionMap landing), TXN-I6+I7.1 (DataView txn wiring + dispatch TaskGuard), TXN-I8.1 (e2e + close-out, the present commit).
- [x] **I-X.3** — re-run the H Tier 1 + Tier 2 regression suites after I lands; make sure the V8 transaction path is still untouched. Done 2026-04-25 — see TXN-I8.1 validation block. `cargo test -p riversd --lib` 421/421 passed (was 416 + 5 new e2e tests). engine_loader 12/12, process_pool 213/213, V8 44/44 unchanged. Integration suites: pool_tests 33/33, task_kind_dispatch 47/47, ddl_pipeline 10/10, v8_ddl_whitelist 2/2, process_pool_tests 10/10, full `cargo test -p riversd` green across every binary.

#### Follow-up TODOs from Phase I close-out

- [x] **I-FU1 — Backfill H1-H15 annotations in `docs/code_review.md`.** Phase H closed 14 of 15 Tier-1/Tier-2 findings via PR #83 (squash sha `6ee5036`) but the corresponding T-findings in `docs/code_review.md` are not annotated. Mapping each finding to its specific squash-commit hunk is non-mechanical; needs a dedicated pass. Suggested approach: walk `docs/code_review.md` top-to-bottom, for each Tier-1/Tier-2 finding lacking a "Resolved" line check the H-task in this file (e.g. T1-1 ↔ H2, T1-2 ↔ H1) and stamp the annotation referencing PR #83 + the H-task id.
- [x] **I-FU2 — Postgres parallel e2e tests under `#[ignore]`.** Shipped: `process_pool::pg_e2e_tests` mirrors all 5 SQLite e2e cases (commit/rollback/auto-rollback/cross-DS/concurrent) against the live Postgres test cluster at 192.168.2.209. Double-gated on `#[ignore]` AND a runtime `RIVERS_TEST_CLUSTER=1`+TCP-probe check (`cluster_available()`). Reuses `txn_test_fixtures` with a new `build_postgres_executor` helper and an additional PostgresDriver registration in `ensure_host_context`. PostgresDriver is stateless so registration is unconditional — only the per-test `connect()` calls touch the network, and those are gated. Each test uses a unique table name (pid + atomic counter prefix) and a Drop-based best-effort cleanup so concurrent runs and aborted runs don't leak schema in the shared `rivers` database. Default `cargo test` produces 5 ignored / 0 run; cluster CI uses `RIVERS_TEST_CLUSTER=1 cargo test -- --include-ignored`. Live verification could not be performed from this Bash-tool sandbox (compiled Rust binaries get "No route to host" to 192.168.2.209 even though `nc`/`ping`/`curl` work — appears to be a macOS app-firewall restriction); CI on a cluster-host runner is the canonical green-light.

---

## Phase H follow-up — missed during 2026-04-25 batch

> **Source:** Post-Phase H gap re-scan (after PR #83 was opened) found one Tier-2 finding from `docs/code_review.md` that was not on the original Phase H list. Tracked here so it doesn't get lost; can land independently of Phase I.

- [x] **H18 — rivers-drivers-builtin T2-1: MySQL unsigned integers wrap into negative on `i64` cast.**
  **File:** `crates/rivers-drivers-builtin/src/mysql.rs:559` (`mysql_async::Value::UInt(u)` matched and emitted as `QueryValue::Integer(*u as i64)`).
  Values above `i64::MAX` (~9.2×10¹⁸) wrap to negative numbers — silently corrupts results from `BIGINT UNSIGNED` columns at scale (snowflake ids, large counters, monotonic timestamps).

  **Resolved approach (2026-04-25):** match the de-facto industry standard for large 64-bit integers in JSON APIs (Twitter snowflakes, Stripe IDs, GitHub IDs, Discord, Mastodon, MongoDB Extended JSON). **Two-layer fix:**

  1. **Add `QueryValue::UInt(u64)` variant** in `crates/rivers-driver-sdk/src/types.rs:12`. Preserves the type at the driver→runtime boundary; mirrors `mysql_async::Value::UInt(u64)`, `sqlx`'s separate `I64`/`U64`, and `diesel`'s `Bigint`/`Unsigned<Bigint>`. Touch every match arm on `QueryValue` — minimum: `crates/rivers-drivers-builtin/src/{mysql,postgres,sqlite}.rs`, the four `fn query_value_to_json` helpers (`crates/rivers-plugin-elasticsearch/src/lib.rs:387`, `crates/rivers-plugin-couchdb/src/lib.rs:562`, `crates/rivers-plugin-neo4j/src/lib.rs:318`, `crates/riversd/src/process_pool/v8_engine/direct_dispatch.rs:150`), `crates/rivers-runtime/src/dataview_engine.rs` (parameter validation + result marshalling), and any schema-validation match arms.

  2. **JSON serialization: stringify above `Number.MAX_SAFE_INTEGER` (2⁵³−1 = 9_007_199_254_740_991).** Below the threshold emit as a JSON number; at-or-above emit as a JSON string. Apply to **both** `Integer(i64)` (when `|v| > 2⁵³−1`) and `UInt(u64)` (when `v > 2⁵³−1`). Replace `QueryValue`'s current `#[derive(Serialize)] #[serde(untagged)]` with a custom `Serialize` impl in `types.rs`. Keep `Deserialize` derived (untagged) — handlers send numbers; the precision-loss issue is on the *outbound* path.

  This **per-value** policy (Twitter / Stripe / Discord pattern) keeps small IDs and counters as natural JSON numbers and only stringifies when JS clients would silently lose precision via IEEE-754 double rounding. The alternative **per-column always-string** policy can be layered on later as a schema attribute (e.g. `"jsonNumberMode": "string"`) without breaking the per-value default.

  ### Sub-tasks

  - [x] **H18.1 — Add the variant + custom Serialize.**
    `crates/rivers-driver-sdk/src/types.rs`: add `UInt(u64)`. Replace `#[derive(Serialize)]` with a manual `impl Serialize for QueryValue` that emits a JSON string for `Integer` when `|v| > 2⁵³−1` and for `UInt` when `v > 2⁵³−1`; otherwise emits a JSON number. Constants: `const SAFE_INT_MAX: i64 = 9_007_199_254_740_991;` and `const SAFE_UINT_MAX: u64 = 9_007_199_254_740_991;`. Document the threshold + rationale in the doc comment on the enum.
    Validation: round-trip unit tests in `types.rs` cover `Integer(0)`, `Integer(2⁵³−2)` → number, `Integer(2⁵³)` → string, `Integer(-2⁵³)` → string, `UInt(0)`, `UInt(2⁵³−1)` → number, `UInt(2⁵³)` → string, `UInt(u64::MAX)` → string `"18446744073709551615"`.

  - [x] **H18.2 — Switch MySQL driver to emit `UInt`.**
    `crates/rivers-drivers-builtin/src/mysql.rs:559`: change `QueryValue::Integer(*u as i64)` → `QueryValue::UInt(*u)`. Remove the lossy cast.
    Validation: integration test against MySQL cluster (192.168.2.215-217) on a `BIGINT UNSIGNED PRIMARY KEY` table with rows `0`, `42`, `9_007_199_254_740_991`, `9_007_199_254_740_992`, `18_446_744_073_709_551_610`. Dataview returns: first three as JSON numbers, last two as JSON strings.

  - [x] **H18.3 — Update remaining `QueryValue` match-arm sites.**
    Each of: `crates/rivers-drivers-builtin/src/{postgres,sqlite}.rs` (no native u64 source — the new variant is just one more arm that's never produced); the four `query_value_to_json` helpers (elasticsearch, couchdb, neo4j, direct_dispatch); `crates/rivers-runtime/src/dataview_engine.rs` (param validation + result marshalling); schema-validation match arms (find via `grep -rn "match.*QueryValue\b" crates --include='*.rs'`).
    For helpers that produce JSON, delete any local stringify logic — the custom `Serialize` is the single source of truth. (Helpers that produce non-JSON wire formats — e.g. neo4j Cypher params — should still match the new variant explicitly.)
    Validation: `cargo check --workspace` clean; per-driver integration tests still pass.

  - [x] **H18.4 — Schema-spec note.**
    Add a paragraph to `docs/arch/rivers-schema-spec-v2.md` (or wherever JSON marshalling is documented) describing the >2⁵³−1 stringification rule. Reference Twitter / Stripe as prior art. Note that the threshold is `Number.MAX_SAFE_INTEGER`, not `i64::MAX` (the JS-precision boundary, not the Rust-type boundary).

  - [x] **H18.5 — Decision log entry.**
    Append `MYSQL-H18.1` to `changedecisionlog.md` covering: per-value vs per-column choice; threshold = 2⁵³−1; custom Serialize over `#[serde(untagged)]`; deserializer left untagged because the issue is outbound-only.

  - [x] **H18.6 — Cross-finding annotation.**
    When H18 lands, annotate `docs/code_review.md` rivers-drivers-builtin T2-1 with `Resolved YYYY-MM-DD by <commit-sha>` (mirrors I-X.1 / I-FU1 pattern).

- [x] **H9 — riversd T2-9: Engine log callback uses `std::str::from_utf8_unchecked`.** DONE 2026-04-27: Already fixed — `host_callbacks.rs` uses `String::from_utf8_lossy` at lines 762 and 932, with no `from_utf8_unchecked` anywhere in the file. All 38 riversd tests pass.
  **File:** `crates/riversd/src/engine_loader/host_callbacks.rs:497`.

- [x] **H10 — rivers-runtime T2-2: Result schema validation silently disables itself.** DONE 2026-04-27: `validate_query_result` now hard-fails on missing (`DataViewError::SchemaFileNotFound`) or malformed (`DataViewError::SchemaFileParseError`) schema files instead of logging a warning and returning `Ok(())`. Two new error variants added to `DataViewError`. Bundle-load pipeline (`validate_existence::validate_schema_files`) already catches missing files at load time; runtime check is defense-in-depth for on-disk corruption. Four unit tests cover: missing file errors, malformed JSON errors, valid schema passes, missing required field errors. All 197 lib unit tests pass.
  **File:** `crates/rivers-runtime/src/dataview_engine.rs:1337–1343` (`validate_query_result`).

- [x] **H11 — rivers-core T2-1: `Observe`-tier EventBus handlers spawn unbounded.** DONE 2026-04-27: Per-bus `tokio::sync::Semaphore` bounds concurrent Observe-tier spawns. `try_acquire_owned()` is used — saturated semaphore drops the dispatch (never blocks the publish loop) and increments `observe_dropped` (`AtomicU64`). Metrics counter `rivers_eventbus_observe_dropped_total` emitted under `#[cfg(feature = "metrics")]`. `[base.eventbus] observe_concurrency` (default 64) wired from `ServerConfig` through `BaseConfig::EventBusConfig` to `AppContext::new()` via `EventBus::with_caps()`. Two new unit tests: `observe_concurrency_cap_drops_excess_spawns` (1000 events, cap=8, asserts dropped > 0) and `observe_concurrency_no_drop_when_cap_sufficient` (50 events, cap=200, asserts zero drops). All 33 rivers-core unit tests pass.
  **Files:** `crates/rivers-core/src/eventbus.rs`, `crates/rivers-core-config/src/config/server.rs`, `crates/riversd/src/server/context.rs`.

- [x] **H12 — rivers-storage-backends T2-2: SQLite TTL arithmetic overflow.** DONE 2026-04-27: `compute_expiry(now: u64, ttl: u64) -> u64` helper uses `now.saturating_add(ttl)` — caps at `u64::MAX` instead of wrapping. Used at every TTL-bearing `set`/`set_if_absent` call site. Unit tests: `ttl_overflow_saturates_at_u64_max` and `ttl_normal_addition_unaffected` — both pass. All 21 sqlite unit tests pass.
  **File:** `crates/rivers-storage-backends/src/sqlite_backend.rs`.

- [x] **H13 — rivers-engine-v8 T2-1: `HostCallbacks` copied via `ptr::read` without `Copy`/`Clone`.** DONE 2026-04-27: `HostCallbacks` in `rivers-engine-sdk` already has `#[derive(Copy, Clone)]` at line 207. `rivers-engine-v8/src/lib.rs:51` uses `*callbacks` (deref, not `ptr::read`), with SAFETY comment documenting Copy soundness. All 16 rivers-engine-v8 tests pass.
  **File:** `crates/rivers-engine-v8/src/lib.rs:46`.

- [x] **H14 — rivers-engine-wasm T2-1: signed-to-unsigned offset cast in WASM memory bridge.** DONE 2026-04-27: `checked_offset(i32) -> Option<usize>` helper at line 312 uses `usize::try_from(offset).ok()`. `wasm_log_helper` at line 327 uses `checked_offset` for both ptr and len, dropping the log line with a warning on negative values. Unit tests `checked_offset_rejects_negative` and `checked_offset_accepts_non_negative` confirm behavior. All 10 rivers-engine-wasm tests pass.
  **File:** `crates/rivers-engine-wasm/src/lib.rs:257, 267, 277`.

### Tier 3 — hardening (1)

- [x] **H15 — riversd T3-1: Manual JSON log construction in `rivers_global.rs`.** DONE 2026-04-27: `build_app_log_line` now uses `serde_json::json!({...}).to_string()` for the outer object; `fields` (V8 JSON.stringify output) is parsed back to `serde_json::Value` and embedded as a nested value rather than concatenated text. Fallback to a string-embedded form on parse failure preserves log lines even if V8 produces malformed JSON. All 38 riversd unit tests pass.
  **File:** `crates/riversd/src/process_pool/v8_engine/rivers_global.rs`.

### Verification deferred to Phase H follow-ups

Two T2 items the gap audit could not resolve from grep alone — verify before claiming done or open:

- [x] **H16 — riversd T2-4: Pool capacity accounting may ignore the return queue.**
  Verified 2026-04-25 against `crates/riversd/src/pool.rs` (post-Phase D, commit `1f01873`): closed by Phase D commit `2dfbb7b` (D1). The pool now has a single `state: Arc<StdMutex<PoolState>>` (line 502) holding both the `idle: VecDeque<PooledConnection>` and a unified `total: usize` counter that "includes idle connections, checked-out (active) connections, and any in-flight create reservations" (line 95-97 doc comment). All mutators take the same lock: `acquire` reserves a slot via `state.total += 1` under the lock before the create `.await` (line 598), `PoolGuard::drop` decrements via the same lock (line 179), `PoolGuard::take` decrements (line 157), `health_check` decrements by failure count (line 755), `drain` decrements by dropped idle count (line 792). There is no separate atomic, no async-mutex idle queue, and no sync return queue — the dual-counter shape the original T2-4 cited has been removed. Capacity check at line 596 (`state.total < self.config.max_size`) reads the same field every release path writes. CLOSED — no source change required.

- [x] **H17 — riversd T2-5: Pool health check holds idle mutex across `.await`.**
  Verified 2026-04-25 against `crates/riversd/src/pool.rs::ConnectionPool::health_check` (lines 717-768): the function drains the idle queue into a local `VecDeque` under the state lock at lines 720-723 (`std::mem::take(&mut state.idle)`), drops the lock when the closure ends, then iterates `to_check.pop_front()` calling `pooled.conn.ping().await` with NO lock held (lines 729-744), and finally re-acquires the lock at line 749 to re-insert healthy entries and decrement `total`. The lock type is `std::sync::Mutex` (not `tokio::Mutex`), so holding it across `.await` would not even compile — the structural guarantee is enforced by the type system. The pattern matches the recommended fix exactly. CLOSED — no source change required.

### Cross-cutting

- [x] **H-X.1 — Update `docs/code_review.md` after each H-task lands** — Done 2026-04-28: 14 "Resolved 2026-04-27" annotations added to docs/code_review.md by commit `b6df4d5` covering H1–H15 (H8 already annotated by Phase I).
- [x] **H-X.2 — Canary regression run** — Done 2026-04-28: riversd 428 passed / 0 failed; rivers-core 33/33; rivers-drivers-builtin 22/22; rivers-runtime 199/199; rivers-storage-backends 21/21; rivers-engine-v8 16/16; rivers-engine-wasm 10/10. workspace `cargo check` clean.

### Sequencing

1. **H4** first — MySQL tenant isolation is a security defect masquerading as a perf optimization. Small change, high impact.
2. **H1+H2** as a pair — both touch `v8_engine/context.rs` and the dynamic-engine path. H1 closes the whitelist bypass; H2 prevents host-bridge stalls from pinning workers. Bundle.
3. **H6+H7** as a pair — both add HTTP timeouts on outbound calls; share the client-builder helper.
4. **H10** before **H8** — schema validation hard-fail is straightforward; transaction stubs need a design decision first.
5. **H3, H9, H13, H14** — all small unsafe/FFI hardening; can land in one PR.
6. **H11** — concurrency cap on Observe dispatch; needs the new config knob.
7. **H5, H12, H15** — schedule per quarter as hardening. (H16, H17 verified closed 2026-04-25 — both resolved by Phase D commit `2dfbb7b`; no source change required.)


- [x] **RXE1.3 — Check command invocation safety.** Done 2026-04-25: traced parameters into `stdin`/`args`/`env`/`cwd`/spawn. No shell invocation (verified: `Command::new` plus `cmd.args()`, no `sh -c`). Each placeholder produces exactly one argv slot via `template.rs`. `env_clear=true` default; warning when false. Stdin written as JSON bytes. `cwd = working_directory`. Stdout chunked-read with cap; **stderr single-read into 64 KB buffer (RXE-T2-1)**, **UTF-8 boundary slice can panic (RXE-T1-4)**, **stdout overflow check after extend (RXE-T2-2)**. Timeout fires SIGKILL at the process group. Schema-error formatting leaks the offending value (RXE-T2-4). working_directory parser default `/tmp` (RXE-T3-2) and validator does not check writability or symlink (RXE-T2-6).

- [x] **RXE1.4 — Check privilege drop and child lifecycle.** Done 2026-04-25: `pre_exec` calls `setsid` only, then std's `Command::uid/gid` apply uid/gid drop after. **`setgroups` is never called (RXE-T1-3)** — supplementary groups inherit. **No `umask`, no `RLIMIT_*`, no `sigprocmask` reset (RXE-T2-7)**. Process group: `setsid` makes child the PGID leader; SIGKILL via `kill(-pid)` reaches all descendants (verified by `timeout_kills_process` and `output_overflow_kills_process` tests). Zombie reaping handled by tokio. Shutdown/orphan: `kill_on_drop` set; if `riversd` SIGTERMs, tokio task drop fires SIGKILL — best-effort, recorded in coverage gaps. `nix_is_root()` called per-spawn (RXE-T3-3).

- [x] **RXE1.5 — Check concurrency and resource bounds.** Done 2026-04-25: global `try_acquire` first (`pipeline.rs:91`), per-command second (`pipeline.rs:106`) — consistent order, no deadlock since both are non-blocking. RAII permits release on all paths including panic. Concurrency tests pass. Bounds: stdout has chunked-loop cap with off-by-up-to-8 KB overshoot (RXE-T2-2), stderr fixed 64 KB single-read (RXE-T2-1), stdin unbounded by params object size (acceptable since params come from validated handlers).

- [x] **RXE1.6 — Check driver-sdk contract compliance.** Done 2026-04-25: read `crates/rivers-driver-sdk/src/traits.rs` in full. `Connection::execute` correctly calls `check_admin_guard` (`exec_connection.rs:33`) and rejects everything but `query`. `ddl_execute` left at SDK default (Unsupported) — correct for exec. `admin_operations` returns `&[]` via SDK default — correct, exec uses one operation name. Transactions / `prepare` / `has_prepared` / `execute_prepared` all use SDK defaults — correct. ABI: `_rivers_abi_version` + `_rivers_register_driver` exported under `plugin-exports`. **Static-build registration helper missing (RXE-T2-3)** — this is the only contract-adjacent gap.

- [x] **RXE1.7 — Read integration tests for coverage context.** Done 2026-04-25: read `tests/integration_test.rs` (379 lines, 8 tests). Coverage: stdin round-trip, args interpolation, integrity correct/tampered, timeout, non-zero exit, unknown command, concurrency. **Not covered**: symlink swap, `every:N` first call, `setgroups`, RLIMIT/umask/sigmask, multi-byte stderr panic, stderr deadlock, concurrent verify race, working_dir hardening, shutdown/orphan with in-flight children. Documented in the report's "Coverage Notes" section.

- [x] **RXE2.1 — Write per-crate review report.** Done 2026-04-25: `docs/review/rivers-plugin-exec.md` written in the established format (matches `rivers-keystore-engine.md` and `rivers-lockbox-engine.md`). 4 Tier 1, 7 Tier 2, 5 Tier 3, plus non-findings, repeated-pattern note, coverage notes, bug density assessment, and ordered recommended-fix list. Every finding cites file:line; every non-finding explains what was investigated.

- [x] **RXE2.2 — Update logs.** Done 2026-04-25: appended `RXE-1.1` block to `changedecisionlog.md` covering single-crate scope, severity-tier definitions, T1-vs-T2 borderline calls (RXE-T1-4 and RXE-T1-2), `getpwnam` reentrancy non-finding rationale, and the combined-fix rationale. Appended row to `todo/changelog.md` with file basis (3375 LOC source + 379-line integration test + 645-line SDK trait file) and validation results.

- [x] **RXE2.3 — Mark tasks complete and verify whitespace.** Done 2026-04-25: all 14 RXE sub-tasks flipped to `[x]` with one-line completion notes. `git diff --check` clean.

# RW — Rivers-Wide Code Review Remediation

> **Source:** `docs/review/rivers-wide-code-review-2026-04-27.md` (validated 2026-04-27)
> **Scope:** 22 crates reviewed; 95 findings (24 Tier 1, 67 Tier 2, 4 Tier 3)
> **Goal:** close every Tier 1 in Phase 1–2; close all Tier 1/Tier 2 by end of Phase 5.
> **Sequencing rationale:** the review's bottom line — "looks wired, returns success, does the wrong thing" — means silent-security failures (Phase 1) outrank everything; broker correctness (Phase 2) is the next-largest risk class; unwired features (Phase 3) and shared guardrails (Phase 4) can be batched; tooling honesty (Phase 5) is last because it doesn't degrade running services.

## Phase RW1 — Stop Silent Security Failures

### RW1.1 — `rivers-driver-sdk` DDL guard + error sanitization (4 findings: 1×T1, 3×T2)

**Files:** `crates/rivers-driver-sdk/src/{traits.rs,retry.rs}` (verify exact paths)

- [x] **RW1.1.a** — Replace `is_ddl_statement()`'s naive whitespace trim with a comment-aware leading-token parser that strips `--` line comments and `/* */` block comments before classifying. Add the same parser into operation inference so both paths agree. Test: `SELECT 1` with leading `-- DROP TABLE\n` must classify as query, not DDL. (`lib.rs`: `first_sql_token()` + `is_ddl_statement()` rewritten)
- [x] **RW1.1.b** — Sanitize forbidden-DDL rejection errors so they never echo raw statement prefixes (which can contain credential material from connection-string-style payloads). Return generic message + redacted classification, log full statement at DEBUG only. (`lib.rs`: `check_admin_guard()` sanitized; existing integration test updated to match)
- [x] **RW1.1.c** — Rewrite `$N` positional parameter substitution from parsed spans, not global string replacement. Test: parameter named `$1` in a string literal where another parameter `$10` exists must not get clobbered. (`lib.rs`: `translate_params()` DollarPositional/QuestionPositional/ColonNamed all rewritten span-based)
- [x] **RW1.1.d** — Use `saturating_mul` / checked arithmetic in exponential retry backoff so it cannot overflow before max-delay capping. Test: 64 retries with base 1s and 2× factor must converge to max_delay, not panic. (`http_executor/connection.rs` + `oauth2.rs`: `saturating_pow` + `saturating_mul`)
- [x] **RW1.1.validate** — `cargo test -p rivers-driver-sdk` green; new tests for each subtask above. (203 tests pass; 13 new RW1.1 tests added)

### RW1.2 — `rivers-plugin-exec` lifecycle/TOCTOU/privilege-drop hardening (8 findings: 3×T1, 4×T2, 1×T3)

> Many of these overlap with the prior RXE findings already documented. Verify which are still open before duplicating work.

- [x] **RW1.2.a** (RXE-T1-? cross-ref) — Wrap stdin write, stdout/stderr drain, and child-wait under one lifecycle controller so the configured timeout governs all child I/O, not just `wait()`.
- [x] **RW1.2.b** — Replace path-based exec after hash verify with file-handle execution (`fexecve` or open-then-fork-then-exec on the verified `OwnedFd`) to close the TOCTOU window between hash check and spawn.
- [x] **RW1.2.c** — Call `setgroups(0, NULL)` before drop in `pre_exec` so supplementary groups don't survive the uid/gid change.
- [x] **RW1.2.d** — Drain stdout and stderr concurrently with byte caps. Stderr currently single-read into 64 KB; make it chunked-read with an explicit cap and concurrent with stdout.
- [x] **RW1.2.e** — Move `every:N` integrity counter increment to *after* successful semaphore acquisition, so rejected attempts don't burn scheduled checks.
- [x] **RW1.2.f** — Fail closed on invalid `env_clear` config values (anything other than `true`/`false`); current code only matches exact `"true"`, silently inheriting env on typos.
- [x] **RW1.2.g** — Stop ignoring process-group setup and kill-syscall errors; log + propagate via the executor result.
- [x] **RW1.2.h** — Fix UTF-8 boundary slice in lossy stderr truncation that can panic on multi-byte sequences.
- [x] **RW1.2.validate** — `cargo test -p rivers-plugin-exec` green; integration test exercising the timeout/lifecycle controller on a child that ignores stdin.

### RW1.3 — `riversctl` shutdown fallback + stop-signal correctness (7 findings: 2×T1, 5×T2)

**Files:** `crates/riversctl/src/{commands/stop.rs,commands/shutdown.rs,admin_client.rs,commands/log.rs,commands/tls.rs}` (verify)

- [ ] **RW1.3.a** — Distinguish network-unreachable from HTTP-status/auth/RBAC failures in admin shutdown. Auth failure must NOT silently fall back to local OS signals — that bypasses the admin authorization model.
- [x] **RW1.3.b** — In local stop, check `kill()` return value and verify the process actually exited before removing the PID file. Currently any kill failure still removes the PID file. Done: `send_term`/`send_kill` return errors on non-zero rc; cleanup_pid_file only called after `!is_process_alive(pid)` confirms exit.
- [x] **RW1.3.c** — Build one typed admin HTTP client with explicit connect/request timeouts, auth, and schema-tested request bodies. Replace ad-hoc `reqwest::Client::new()` call sites. Done: `build_admin_client()` in `admin.rs:46` with `connect_timeout(5s)` and `timeout(30s)`; distinguishes `AdminError::Network` from `AdminError::Http`.
- [x] **RW1.3.d** — Wire `[base.admin_api].private_key` config field through to the CLI admin signing path. Currently parsed and ignored. Reject malformed env keys loudly instead of silent fallback. Done: `admin.rs:58` documents and uses `key_path` arg sourced from config; `ADMIN_PRIVATE_KEY` OnceLock wired at startup.
- [x] **RW1.3.e** — Fix `log set` to send the field name `target` the server expects, not `event`. Add a contract test against the admin schema. Done: `admin.rs:456` sends `{"target": ..., "level": ...}`; unit test `log_set_body_uses_target_key` at line 513.
- [x] **RW1.3.f** — TLS import must `chmod 0600` imported private-key files atomically (write to temp with mode then rename), not after. Done: `tls_cmd.rs:221` writes to `.tmp` with mode 0600 from creation, then atomic rename; `write_private_key_atomic()` helper at line 233.
- [x] **RW1.3.g** — Decide `deploy` semantics: either expose the staged-deploy lifecycle explicitly (status flags, `promote` subcommand) or drive the full deploy/test/approve/promote flow. Currently it leaves a pending deployment with no follow-through. Done: `admin.rs:180` surfaces staged/pending status explicitly with `"riversctl deploy promote <id>"` instructions.
- [ ] **RW1.3.validate** — `cargo test -p riversctl` green; integration test asserts auth failure on admin shutdown does NOT trigger local signal fallback.

### RW1.4 — Secret wrapper rollout: LockBox + keystore zeroization/Debug/Clone (multiple findings across 6 crates)

**Files:** new `crates/rivers-core/src/secret.rs` (or co-located with existing secret types); refactor sites in `rivers-lockbox-engine`, `rivers-keystore-engine`, `rivers-lockbox`, `rivers-keystore`, `cargo-deploy`, `riversctl`.

- [x] **RW1.4.a — Define the secret wrapper.** One small type `Secret<T: Zeroize>` with: redacted `Debug` (`"<redacted>"`), no `Clone` impl (force explicit `.clone_secret()`), `Drop` calls `zeroize`, and an explicit `expose(&self) -> &T` API. Add unit tests for redaction and drop-time zeroization (use a sentinel allocator or `zeroize`'s test hooks). Done: `crates/rivers-core/src/secret.rs` exists with full `Secret<T>` implementation.
- [ ] **RW1.4.b — `rivers-lockbox-engine`.** Replace `ResolvedEntry`'s public `String` plaintext with `Secret<String>`. Strip `Debug` and `Clone` derives on secret-bearing types. Zeroize plaintext buffers on error paths (currently only on success).
- [ ] **RW1.4.c — `rivers-lockbox-engine` resolver.** Resolve secrets by stable name/alias during per-access fetch instead of metadata index; current path returns the wrong secret after rotation/reorder.
- [ ] **RW1.4.d — `rivers-lockbox-engine` permissions.** Move keystore permission checks into the actual decrypt/read call path so runtime reads recheck, not just startup.
- [ ] **RW1.4.e — `rivers-keystore-engine` durable save.** Atomic save with file + parent-directory fsync. Lock + version-guard against concurrent saves losing rotations.
- [ ] **RW1.4.f — `rivers-keystore-engine` types.** Make `key_material` private; remove `Debug` derives from `AppKeystore`, `AppKeystoreKey`, `KeyVersion`. Use `Secret<>` wrapper.
- [ ] **RW1.4.g — `rivers-keystore-engine` rotation overflow.** Use checked arithmetic on key version increment.
- [ ] **RW1.4.h — `rivers-lockbox` CLI.** Route storage through `rivers-lockbox-engine` (kill the bespoke per-entry directory store). Remove `--value` argv input. Use hidden TTY input (`rpassword` or equivalent). Atomic writes everywhere. Validate user-provided names as paths. Make `rekey` transactional (write all entries with new identity to a staging dir, fsync, atomic swap).
- [ ] **RW1.4.i — `rivers-lockbox` alias safety.** Stop overwriting alias file with `{}` on read/parse failure — fail loudly.
- [ ] **RW1.4.j — `rivers-keystore` CLI.** Fail `init` if target keystore exists unless `--force` (with confirmation). Use `Secret<>` for age identity. Lock keystore across read-modify-write.
- [ ] **RW1.4.k — `cargo-deploy` TLS key.** Create private-key file with `0600` from the start (open with restrictive mode), not chmod-after.
- [ ] **RW1.4.validate** — Each crate's `cargo test -p <crate>` green; new unit test on `Secret<String>` confirming redacted debug and drop-zeroization; sweep `rg 'derive\(.*Debug.*\)' crates/rivers-lockbox* crates/rivers-keystore*` returns no secret-bearing matches.

## Phase RW2 — Make Broker & Transaction Contracts Real

### RW2.1 — Define broker ack/nack/group contract in SDK

**Files:** `crates/rivers-driver-sdk/src/broker.rs` (new or extend), shared test fixtures in `crates/rivers-driver-sdk/tests/broker_contract.rs`

- [x] **RW2.1.a** — Specify a typed `BrokerSemantics` enum: `AtLeastOnce`, `AtMostOnce`, `FireAndForget`. Each driver's `MessageBrokerDriver` must declare which it supports. Done: enum defined in `rivers-driver-sdk/src/broker.rs`.
- [x] **RW2.1.b** — Define explicit `Result<AckOutcome, BrokerError>` for `ack()`/`nack()`. Drivers that cannot honor `nack` must return `BrokerError::Unsupported`, not `Ok(())`. Done: `AckOutcome` enum and typed ack/nack in broker trait.
- [x] **RW2.1.c** — Write SDK contract test fixtures: `receive → nack → expect redelivery`, `receive → ack → expect no redelivery`, `multi-consumer-same-group → expect single delivery`, `multi-subscription → expect all subjects active`. Done: `crates/rivers-driver-sdk/tests/broker_contract.rs` with in-memory mock driver covering all three semantics modes.
- [ ] **RW2.1.validate** — Fixtures compile and run against an in-memory mock driver implementing all three semantics modes.

### RW2.2 — Fix NATS driver against contract (5 findings: 2×T1, 3×T2)

**Files:** `crates/rivers-plugin-nats/src/lib.rs`

- [x] **RW2.2.a** — Replace plain `subscribe()` with NATS queue subscription or JetStream durable consumer so the constructed consumer-group identity is actually used. Done: queue_subscribe implemented in NATS plugin.
- [x] **RW2.2.b** — Implement real ack/nack via JetStream message disposition, OR return `Unsupported` on core-NATS. Done: nack returns `BrokerError::Unsupported` on core-NATS.
- [x] **RW2.2.c** — Activate every configured subscription, not just the first. Done: all configured subjects subscribed.
- [x] **RW2.2.d** — Implement `OutboundMessage.key` as subject suffix, OR return error on key set. Done: `lib.rs:173` appends key as `<base>/<key>` subject suffix when key is set.
- [x] **RW2.2.e** — Wire schema checker into deploy validation, or remove it. Done: `check_nats_schema` called from `check_schema_syntax` at `lib.rs:41`.
- [ ] **RW2.2.validate** — Run new SDK contract fixtures (RW2.1.c) against `rivers-plugin-nats`.

### RW2.3 — Fix Kafka driver against contract (1 finding: 1×T1)

**Files:** `crates/rivers-plugin-kafka/src/lib.rs`

- [x] **RW2.3.a** — Track `delivered-but-unacked` offset separately from `committed/acknowledged` offset. `receive()` must NOT advance the committed offset before `ack()`. Done: at-least-once semantics with pre-ack offset tracking documented at `lib.rs:36-46`.
- [x] **RW2.3.b** — Make `nack()` reset the consumer position so the message redelivers, OR return `Unsupported` if Rivers-managed group coordination cannot guarantee it. Done: nack rewinds offset at `lib.rs:390-396`.
- [x] **RW2.3.c** — Document Rivers-managed consumer-group semantics (since `rskafka` lacks broker-side group coordination); cover with the SDK contract fixtures. Done: `/// # Rivers-managed consumer-group semantics (RW2.3.c)` doc block at `lib.rs:30`.

### RW2.4 — Fix Redis Streams driver against contract (3 findings: 1×T1, 2×T2)

**Files:** `crates/rivers-plugin-redis-streams/src/lib.rs`

- [x] **RW2.4.a** — Implement PEL reclaim/redelivery via `XAUTOCLAIM` (or `XPENDING` + `XCLAIM`); change consumer read from pure `>` to a reclaim+new mix. Alternative: return `Unsupported` for `nack`. Done: ack/nack with PEL reclaim implemented.
- [x] **RW2.4.b** — Add `MAXLEN`/`MINID` trimming on `XADD` based on configured stream cap; default to a finite cap. Done: MAXLEN applied on XADD.
- [x] **RW2.4.c** — Persist `OutboundMessage.headers` as additional stream fields and restore them on `receive()`. Done: headers persisted as stream fields.

### RW2.5 — Fix RabbitMQ driver against contract (3 findings: 1×T1, 2×T2)

**Files:** `crates/rivers-plugin-rabbitmq/src/lib.rs`

- [x] **RW2.5.a** — Call `basic_qos` (prefetch limit) before `basic_consume`. Default prefetch to a finite value; expose as config. Done: basic_qos called before basic_consume.
- [x] **RW2.5.b** — Add a configurable timeout around publish + confirm wait so a dead broker can't pin a producer indefinitely. Done: configurable confirm timeout implemented.
- [x] **RW2.5.c** — Wire schema checker into deploy validation, or remove it. Done: check_rabbitmq_schema wired into check_schema_syntax.

### RW2.6 — Fix MongoDB transaction execution (3 findings: 1×T1, 2×T2)

**Files:** `crates/rivers-plugin-mongodb/src/lib.rs`

- [x] **RW2.6.a** — All CRUD methods must attach the active `ClientSession` when `self.session` is set, so work runs inside the transaction. Done: session routing for all CRUD ops.
- [x] **RW2.6.b** — Bound `find()` cursor materialization with a configured row cap; default to a finite limit. Done: max_rows cap enforced.
- [x] **RW2.6.c** — Require an explicit `_filter` for multi-document update/delete, or make the broad `{}` filter opt-in via an explicit flag. Done: `lib.rs:323,373` require non-empty `_filter`; `allow_full_scan=true` opt-in for broad ops.

### RW2.7 — Fix Neo4j transaction execution (5 findings: 2×T1, 3×T2)

**Files:** `crates/rivers-plugin-neo4j/src/lib.rs`

- [x] **RW2.7.a** — Route query execution through the active `Txn` when one is open. Currently queries bypass the transaction. Done: txn routing via `if let Some(ref mut txn) = self.txn`.
- [x] **RW2.7.b** — Propagate row-stream errors out of `ping()` instead of swallowing them. Done: `ping()` at `lib.rs:216` uses `map_err`; `execute_query()` at line 268 propagates mid-stream errors instead of swallowing.
- [x] **RW2.7.c** — Bind native Bolt parameter values for `Null`, `Array`, `Json` instead of stringifying. Fail loudly on result types the converter can't represent (currently silently drops temporals etc.). Done: native BoltType binding.
- [x] **RW2.7.d** — Either register Neo4j in the static plugin inventory or drop the default static feature so it's not built dead. Done: Neo4j registered in `server/drivers.rs:44`.

## Phase RW3 — Kill Unwired Features

### RW3.1 — Schema checker / DDL implementation gaps

- [ ] **RW3.1.a** — `rivers-plugin-elasticsearch`: implement `ddl_execute()` for the declared admin operations, OR remove `admin_operations()` returns so they're not advertised.
- [ ] **RW3.1.b** — Cross-reference `rg 'pub fn check_.*schema' crates/rivers-plugin-*` and `rg 'fn admin_operations' crates/rivers-plugin-*` against production callers; close every gap (NATS, RabbitMQ already covered in RW2.2.e and RW2.5.c).

### RW3.2 — Static plugin registration inventory

- [ ] **RW3.2.a** — Add `crates/riversd/tests/static_plugin_registry.rs` that fails if a `rivers-plugin-*` crate is built with the static feature but isn't in the `riversd` static driver inventory. Catches the Neo4j-class drift.
- [ ] **RW3.2.b** — Audit current static-feature wiring and either register or drop each plugin (Neo4j is the documented case).

### RW3.3 — Config field consumption tests

- [~] **RW3.3.a — `rivers-core-config`** — Centralize full `ServerConfig` validation in the loader; add recursive unknown-key validation for nested sections (currently stops after `[base]`). Fix the `init_timeout_seconds` allowlist entry to match the real field name `init_timeout_s`. Bind `SessionCookieConfig::validate()` to every load path including hot reload. Partial: `init_timeout_seconds` → `init_timeout_s` fixed in `validate_config.rs:17`. Recursive validation and SessionCookieConfig binding still open.
- [ ] **RW3.3.b — Storage policy fields** — Add tests that set retention/cache policy fields and assert runtime behavior changes; fail or warn loudly if a parsed field is ignored.
- [ ] **RW3.3.c — `riverpackage --config`** — Either wire `--config` into engine config loading or remove/reject the flag so it can't silently no-op.

## Phase RW4 — Add Shared Driver Guardrails

### RW4.1 — Shared timeout policy

- [ ] **RW4.1.a** — Add a `driver_timeouts` helper module in `rivers-driver-sdk` exposing typed connect/request/response-body/broker-confirm timeouts with sensible defaults.
- [ ] **RW4.1.b** — Apply to `rivers-plugin-elasticsearch` (`Client::new()` → builder with timeouts), `rivers-plugin-influxdb` (same), `rivers-plugin-ldap` (wrap connect/bind/search/add/modify/delete), `rivers-plugin-rabbitmq` (publish-confirm), `riversctl` admin client (covered by RW1.3.c).
- [ ] **RW4.1.c** — Add CI lint: `rg 'reqwest::Client::new\(\)' crates/rivers-plugin-* crates/riversctl` must point to a justification or use the helper.

### RW4.2 — Shared response/row caps

- [ ] **RW4.2.a** — Define `max_rows`, `max_response_bytes`, `max_prefetch` defaults in driver SDK config helpers.
- [ ] **RW4.2.b** — Enforce in: `rivers-plugin-ldap` (paged search), `rivers-plugin-cassandra` (paged execution), `rivers-plugin-mongodb` (cursor cap, RW2.6.b cross-ref), `rivers-plugin-elasticsearch` (response cap), `rivers-plugin-couchdb` (`_find`/views), `rivers-plugin-influxdb` (CSV response), `rivers-plugin-rabbitmq` (covered by RW2.5.a prefetch).
- [ ] **RW4.2.c** — CI lint: `rg 'resp\.text\(\)|resp\.json\(\)' crates/rivers-plugin-*` must justify or wrap with a capped reader.

### RW4.3 — Shared URL path-segment encoder

- [ ] **RW4.3.a** — Add `crates/rivers-driver-sdk/src/url.rs` with `percent_encode_path_segment()` helper.
- [ ] **RW4.3.b** — Apply in `rivers-plugin-elasticsearch` (document IDs in URL paths) and `rivers-plugin-couchdb` (doc IDs, design doc names, view names, revision query values).

### RW4.4 — Driver-specific structured-construction fixes

- [ ] **RW4.4.a — CouchDB Mango selectors** — Build selectors structurally (serde_json::Value) instead of string-replacement of placeholders into JSON source. Add round-trip tests with values containing `"`, `\`, and bare placeholders.
- [ ] **RW4.4.b — CouchDB insert** — Check HTTP status before parsing response body and returning success.
- [x] **RW4.4.c — InfluxDB batching durability** — Only clear the buffered-writes vector after a successful flush; on failure, retain or surface for retry. Done: `batching.rs::flush_buffer` now clears buffer only after HTTP success; on failure the buffer retains all lines. `last_flush` timestamp also updated only on success.
- [ ] **RW4.4.d — InfluxDB batching URL** — Carry the bucket per buffered line, OR reject batching when target bucket varies; currently the batched URL omits bucket.
- [ ] **RW4.4.e — InfluxDB line-protocol escaping** — Escape measurement names; escape backslashes in field strings; full line-protocol conformance test.
- [ ] **RW4.4.f — Elasticsearch auth ping** — Use auth-aware request path on initial ping so authenticated clusters don't fail at connect.
- [ ] **RW4.4.g — Elasticsearch default index** — Read and prefer the configured default index; currently silently ignored.
- [ ] **RW4.4.h — Cassandra write affected_rows** — Report `0`/unknown for writes unless the driver returns a real count; current always-`1` is misleading.
- [ ] **RW4.4.i — LDAP TLS** — Support LDAPS/StartTLS with cert verification on by default before bind; do not transmit credentials over plain LDAP.

## Phase RW5 — Make Tooling Honest

### RW5.1 — `cargo-deploy` (5 findings: 1×T1, 4×T2)

**Files:** `crates/cargo-deploy/src/main.rs`

- [x] **RW5.1.a** — Make missing engine dylibs in dynamic mode a fatal error (currently silent success). Done: "absence is fatal" enforced at `main.rs:202`.
- [x] **RW5.1.b** — Assemble deployments in a versioned staging directory and atomically switch the live target via symlink rename (no in-place writes against the live tree). Done: atomic staging with rename implemented.
- [ ] **RW5.1.c** — Generate TLS certs only on bootstrap; require an explicit `--renew-tls` to replace on redeploy.
- [ ] **RW5.1.d** — Open private-key files with `0600` from creation (covered by RW1.4.k cross-ref).
- [ ] **RW5.1.e** — Resolve actual cargo target directory honoring `CARGO_TARGET_DIR`; stop hard-coding `target/release`.

### RW5.2 — `riverpackage` scaffolding + packaging (3 findings)

**Files:** `crates/riverpackage/src/main.rs` and template assets

- [ ] **RW5.2.a** — Update `init` scaffold templates so generated bundles pass current-validator-schema `riverpackage validate` cleanly.
- [ ] **RW5.2.b** — Implement real zip output for `pack`, OR change the documented contract to tar.gz only and update help text.
- [ ] **RW5.2.c** — `--config` wiring (cross-ref RW3.3.c).

### RW5.3 — CLI golden tests

- [ ] **RW5.3.a** — Add golden tests for `cargo deploy <staging>` happy path + each fatal error case (missing engine, missing TLS material, target-dir override).
- [ ] **RW5.3.b** — Add golden tests for `riverpackage init → validate → pack` round-trip.
- [ ] **RW5.3.c** — Add golden tests for `riversctl status`, `riversctl stop`, `riversctl admin shutdown` covering auth-failure-no-fallback (cross-ref RW1.3.a).

## Phase RW-CI — Review heuristics as CI checks

- [x] **RW-CI.1** — Add `scripts/review-lints.sh` running the seven `rg` heuristics from §"Review Heuristics To Add To CI" of the report; wire into a non-blocking advisory CI job first, then promote to required. Done: implemented as `scripts/lint-heuristics.sh` (different filename from what tasks.md specified).
- [ ] **RW-CI.2** — Broker plugin tests must source ack/nack/group fixtures from RW2.1.c (one shared contract test set).
- [x] **RW-CI.3** — `rg '#\[derive\(.*Debug.*\)\]' crates/rivers-lockbox* crates/rivers-keystore*` must return zero matches on secret-bearing types. Done: removed `#[derive(Debug)]` from `Keystore` in `lockbox-engine/types.rs`; added manual `impl Debug` that shows version + entry count with values redacted. Keystore-engine's `AppKeystore`, `AppKeystoreKey`, `KeyVersion` already had no derive-Debug; `KeyInfo` and `EncryptResult` contain no secret material so their derives are safe.

## RW Cross-Cutting

- [ ] **RW-X.1 — Annotate the source review.** After each phase lands, add "Resolved YYYY-MM-DD by `<commit-sha>`" annotations to `docs/review/rivers-wide-code-review-2026-04-27.md` under the relevant findings, mirroring the H-task convention.
- [ ] **RW-X.2 — Canary regression run** after Phase RW1 lands and again after Phase RW2 lands. 135/135 must remain green.
- [ ] **RW-X.3 — De-duplicate vs. existing H-tasks and RXE follow-ups.** Several RW1.2.x items overlap with the prior `rivers-plugin-exec` review; before starting RW1.2, walk the existing RXE Tier 1 findings list and mark RW1.2 sub-items as "duplicate of RXE-Tx-y" where appropriate.

# CB P1 Batch 2 — P1.5, P1.6, P1.7

> **Source:** `docs/superpowers/specs/2026-04-29-cb-p1-batch2-design.md`
> **Goal:** close P1.5 (per-DataView `skip_introspect`), P1.6 (OTLP protobuf→JSON transcoder), P1.7 (auto-OTel spans via OTLP exporter).
> **Implementation order:** P1.5 → P1.7 (deps + config + exporter) → P1.6 (transcoder, aligns to P1.7 dep versions).
> **Version bump:** all three together → `bump-patch`.

## P1.5 — Per-view introspection skip

- [ ] **P1.5.a** — Add `skip_introspect: bool` field (with `#[serde(default)]` and doc comment) to `DataViewConfig` in `crates/rivers-runtime/src/dataview.rs`.
- [ ] **P1.5.b** — In `crates/riversd/src/bundle_loader/load.rs`, in the inner DataView introspection loop after the datasource-level `introspect` check, skip introspection when `dv_config.skip_introspect` is true and emit `tracing::debug!` with dataview name.
- [ ] **P1.5.c** — Add structural validation rule `S-DV-1` in `crates/rivers-runtime/src/validate_structural.rs`: warn (non-fatal) when `skip_introspect = true` and the DataView has a non-empty GET query.
- [ ] **P1.5.d** — Validation: build a minimal mutation DataView (`INSERT INTO ...`) on an introspect-enabled datasource with `skip_introspect = true` and confirm bundle loads without the previous LIMIT-0 wrap failure.

## P1.7 — Auto-OTel spans via OTLP exporter (deps before P1.6)

- [ ] **P1.7.a** — Add OTel deps to `crates/riversd/Cargo.toml`: `opentelemetry` (feat `trace`), `opentelemetry-otlp` (feat `http-proto`, `reqwest-client`), `opentelemetry_sdk` (feat `rt-tokio`), `tracing-opentelemetry`. Pin versions consistent with `opentelemetry-proto` so P1.6 can align.
- [ ] **P1.7.b** — Create `crates/rivers-core-config/src/config/telemetry.rs` with `TelemetryConfig { otlp_endpoint: String, service_name: String (default fn) }`.
- [ ] **P1.7.c** — Export `TelemetryConfig` from `crates/rivers-core-config/src/config/mod.rs` and add `pub telemetry: Option<TelemetryConfig>` to `ServerConfig` in `crates/rivers-core-config/src/config/runtime.rs`.
- [ ] **P1.7.d** — In `crates/riversd/src/server/lifecycle.rs`, when `[telemetry]` is present at startup: init `opentelemetry-otlp` HTTP exporter at `otlp_endpoint`, build `opentelemetry_sdk::trace::TracerProvider`, install `tracing_opentelemetry::layer()` into the global `tracing` subscriber. Auto-fill `service_version` from the binary version. When absent, behavior unchanged.
- [ ] **P1.7.e** — In `crates/riversd/src/server/view_dispatch.rs`, wrap handler dispatch in a `tracing::info_span!` capturing `handler`, `app`, `method`, `req_bytes`, `status` (post), `duration_ms` (post).
- [ ] **P1.7.f** — Add a span around DataView execution in the DataView executor capturing `dataview`, `datasource`, `method`, `duration_ms`.
- [ ] **P1.7.g** — Validation: with `[telemetry]` configured at a local OTLP collector, hit a view and confirm a handler span and a downstream DataView span arrive with expected attributes; with `[telemetry]` removed, confirm no exporter is initialized and behavior is unchanged.

## P1.6 — OTLP protobuf → JSON transcoder

- [ ] **P1.6.a** — Add `opentelemetry-proto` (features `gen-tonic-messages`, `with-serde`) and `prost` to `crates/riversd/Cargo.toml`. Versions must match the OTel stack pinned in P1.7.a.
- [ ] **P1.6.b** — Create `crates/riversd/src/otlp_transcoder.rs` exposing `TranscodeError { UnknownSignal(String), DecodeFailed(String) }` and `pub fn transcode_otlp_protobuf(path: &str, body: &[u8]) -> Result<Vec<u8>, TranscodeError>`. Map `/v1/traces`, `/v1/metrics`, `/v1/logs` to the corresponding `opentelemetry_proto::tonic::collector::*::v1::Export*ServiceRequest`; decode via `prost::Message::decode`, serialize via `serde_json::to_vec`.
- [ ] **P1.6.c** — Register module in `crates/riversd/src/lib.rs` (`pub mod otlp_transcoder`).
- [ ] **P1.6.d** — In `crates/riversd/src/server/view_dispatch.rs`, in the body extraction path before the codecomponent executor: when `Content-Type` starts with `application/x-protobuf`, call the transcoder. On success replace body bytes with JSON and treat as `application/json` downstream. On `UnknownSignal` pass through unchanged. On `DecodeFailed` return HTTP 415 with the error message.
- [ ] **P1.6.e** — Validation: POST a real OTLP-protobuf trace payload to `/v1/traces` and confirm the handler receives JSON; POST garbage protobuf and confirm 415; POST `application/x-protobuf` to a non-OTLP path and confirm pass-through.

## CB-Batch2 Cross-Cutting

- [ ] **CB-B2.X.1** — Run `just bump-patch` once all three items are merged together (single PR or coordinated patch series).
- [ ] **CB-B2.X.2** — Update `changelog.md` and `changedecisionlog.md` entries per Standards 6 & 7 referencing `docs/superpowers/specs/2026-04-29-cb-p1-batch2-design.md`.
- [ ] **CB-B2.X.3** — Confirm canary remains green (135/135) after the batch lands.

# CB P1.1 — MCP Resource Subscriptions / Push Notifications

> **Source:** `docs/superpowers/specs/2026-04-29-cb-p1-1-mcp-subscriptions-design.md`
> **Goal:** implement MCP `resources/subscribe` + `notifications/resources/updated` over a Streamable HTTP (SSE) transport. v1 uses polling for change detection.
> **Implementation order:** Layer 4 (config) → Layer 2 (registry) → Layer 1 (SSE transport) → Layer 5 (handlers) → Layer 3 (poller).
> **Version bump:** `bump-minor` — new transport + change-detection subsystem.

## Layer 4 — Config surface

- [ ] **P1.1.4.a** — Add `subscribable: bool` (default false) and `poll_interval_seconds: u64` (default 5) to `McpResourceConfig` in `crates/rivers-runtime/src/view.rs`.
- [ ] **P1.1.4.b** — Create `crates/rivers-core-config/src/config/mcp.rs` with `McpConfig { max_subscriptions_per_session: u64 (default 100), min_poll_interval_seconds: u64 (default 1) }`.
- [ ] **P1.1.4.c** — Export `McpConfig` from `crates/rivers-core-config/src/config/mod.rs`; add `pub mcp: Option<McpConfig>` to `ServerConfig` in `runtime.rs`.
- [ ] **P1.1.4.d** — Add validation rule `S-MCP-2` in `crates/rivers-runtime/src/validate_structural.rs`: warn when `subscribable = true` and the bound DataView has no GET method.

## Layer 2 — Subscription registry

- [ ] **P1.1.2.a** — Create `crates/riversd/src/mcp/subscriptions.rs` with `SubscriptionRegistry`, `SessionChannel { sender: mpsc::Sender<sse::Event>, subscribed_uris: HashSet<String>, app_id: String }`. Bounded mpsc capacity 64.
- [ ] **P1.1.2.b** — Implement `attach_sse`, `detach`, `subscribe` (enforce `max_subscriptions_per_session`, return `SubscribeError::TooMany`), `unsubscribe`, `notify_changed` (URI-dedupe before send; drop + WARN on full channel), `snapshot_subscriptions`.
- [ ] **P1.1.2.c** — Unit tests: subscribe/unsubscribe round-trip, max-subscriptions enforcement, notification delivery, slow-consumer drop, dedupe.
- [ ] **P1.1.2.d** — Wire `Arc<SubscriptionRegistry>` onto `AppContext` and construct in `crates/riversd/src/server/lifecycle.rs` at startup.

## Layer 1 — Streamable HTTP (SSE) transport

- [ ] **P1.1.1.a** — In `crates/riversd/src/server/view_dispatch.rs::execute_mcp_view`, add a branch for `GET` + `Accept: text/event-stream` + valid `Mcp-Session-Id`: build `axum::response::sse::Sse`, register with registry via `attach_sse`, on disconnect call `detach`.
- [ ] **P1.1.1.b** — Add 30-second SSE keepalive (comment frames) using `Sse::keep_alive`.
- [ ] **P1.1.1.c** — In `handle_initialize` (`dispatch.rs`), advertise `capabilities.resources.subscribe = true` only when ≥1 resource has `subscribable = true`.
- [ ] **P1.1.1.d** — Integration test: open SSE stream against an MCP endpoint with a valid session-id; observe keepalive frames; close cleanly.

## Layer 5 — Subscribe / unsubscribe handlers

- [ ] **P1.1.5.a** — Thread `session_id: &str` parameter through `crate::mcp::dispatch::dispatch` (currently extracted at `view_dispatch.rs:514` but not passed into `dispatch`).
- [ ] **P1.1.5.b** — Add `"resources/subscribe"` and `"resources/unsubscribe"` arms in `dispatch.rs:35-46`. Implement `handle_resources_subscribe` (validate URI matches a `subscribable = true` resource, call `registry.subscribe`, ensure poller running) and `handle_resources_unsubscribe`.
- [ ] **P1.1.5.c** — Define notification frame format: `{"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"..."}}` — emitted by registry on `notify_changed`.
- [ ] **P1.1.5.d** — Integration test: subscribe over POST → JSON ack; mutate underlying DataView → SSE delivers notification; unsubscribe → no further notifications.

## Layer 3 — Change poller

- [ ] **P1.1.3.a** — Create `crates/riversd/src/mcp/poller.rs` with `ChangePoller { registry, dataview_executor, handles: Mutex<HashMap<(app_id, uri), JoinHandle>> }`.
- [ ] **P1.1.3.b** — Implement `ensure_running((app_id, uri))`: spawn task that resolves URI → DataView (re-using logic from `handle_resources_read`), executes, BLAKE3-hashes `query_result.rows`, sleeps `poll_interval_seconds.max(min_poll_interval_seconds)`, re-executes, calls `notify_changed` on hash diff.
- [ ] **P1.1.3.c** — Refcount cleanup: poller exits when `registry.snapshot_subscriptions()` reports zero subscribers for its `(app_id, uri)`.
- [ ] **P1.1.3.d** — Construct `ChangePoller` in `lifecycle.rs`, place on `AppContext`.
- [ ] **P1.1.3.e** — Integration test: two sessions subscribe to the same URI → only one poller runs (verify via debug log or poller-count metric); both receive notifications; first session disconnects → poller continues; second disconnects → poller exits within one cycle.

## P1.1 Cross-cutting

- [ ] **P1.1.X.1** — Document the `read-then-subscribe` pattern (subscribers receive deltas only, not initial snapshot) in `docs/guide/tutorials/` (new file or extend MCP tutorial).
- [ ] **P1.1.X.2** — Document the deterministic-ORDER-BY requirement for subscribable DataViews (hash includes row order).
- [ ] **P1.1.X.3** — Run `just bump-minor` once feature is merged.
- [ ] **P1.1.X.4** — Update `changelog.md` and `changedecisionlog.md` referencing the design spec.
- [ ] **P1.1.X.5** — Confirm canary stays green; add a P1.1-specific canary covering subscribe → mutate → notification round-trip.

