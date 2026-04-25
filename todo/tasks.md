# Code-Review Remediation Plan

> **Source:** `docs/code_review.md` (2026-04-24)
> **Prior plan:** TS pipeline gap closure (G0–G8) — fully complete, archived in `todo/gutter.md` history.
> **Goal:** close every P0/P1/P2 finding from the full-codebase review. Order follows the review's "Recommended Remediation Sequence" so boot/security come first and broad refactors last.

**Theme of the review:** the right primitives exist (storage traits, drivers, pool, event priorities, V8 task locals, security pipeline), but hot paths bypass them. One authoritative path for app identity, datasource access, storage, and host capabilities is the goal.

**Status legend:** `[ ]` open · `[~]` in progress · `[x]` done · `[!]` blocked

---

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

---

## Phase B — Lock down V8 host capabilities (P0-2, P1-5, P1-8, P1-9)

### B1 — Gate `ctx.ddl()` to ApplicationInit only (P0-2)

**Files:** `crates/riversd/src/process_pool/v8_engine/context.rs`, `crates/riversd/src/view_engine/pipeline.rs`, `crates/riversd/src/process_pool/v8_engine/task_locals.rs`

- [ ] **B1.1** Add `task_kind` field (or use existing) on `TaskLocals`/`SerializedTaskContext`; populate it explicitly per dispatch path: `ApplicationInit`, `Rest`, `MessageConsumer`, `SecurityHook`, `ValidationHook`.
- [ ] **B1.2** `ctx_ddl_callback`: read current task kind; throw JS error unless kind is `ApplicationInit` AND task carries an explicit `app_id` + `datasource_id`.
- [ ] **B1.3** Optionally restrict to a per-app DDL allowlist (manifest field). Defer if not needed for canary.
- [ ] **B1.4** Negative tests: REST handler, message consumer, validation hook, security hook each call `ctx.ddl(...)` → all four throw.
- [ ] **B1.5** Positive test: ApplicationInit handler can call `ctx.ddl("CREATE TABLE …")` against a configured datasource.

### B2 — `ctx.store` failures must not silently fall back (P1-5)

**Files:** `crates/riversd/src/process_pool/v8_engine/context.rs`

- [ ] **B2.1** Storage callbacks (`set`, `get`, `delete`): if a storage engine is configured, propagate backend errors as JS exceptions. No `TASK_STORE` fallback path.
- [ ] **B2.2** Restrict task-local fallback to an explicit `RIVERS_DEV_NO_STORAGE=1` mode (or equivalent config flag). Document in handler-tutorial.
- [ ] **B2.3** Tests: fault-inject backend (Redis down) → `ctx.store.set` throws; assert no silent success.

### B3 — Module cache miss must hard-fail in production (P1-8)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`, `crates/riversd/src/process_pool/module_cache.rs`

- [ ] **B3.1** Add a `mode` to module cache: `Production` (miss → error) vs `Development` (miss → disk + live compile, with warn log).
- [ ] **B3.2** Default deployed builds to Production; allow opt-in dev mode via env var or `[base.debug]`.
- [ ] **B3.3** Test: synthesize an import not present in the prepared cache; assert dispatch returns `MODULE_NOT_REGISTERED` and never compiles live in production mode.

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

- [ ] **C1.1** New `dispatch::build_task_context(app, view, handler_id, kind) -> SerializedTaskContext` — single helper that binds app id, view id, handler id, datasource map, storage engine, lockbox handle, driver factory, debug flag, task kind. Used by every dispatch site.
- [ ] **C1.2** Replace all empty-app-id sites: `message_consumer.rs`, `security_pipeline.rs`, `view_engine/validation.rs`, view dispatch error path.
- [ ] **C1.3** Remove the `"app:default"` fallback in `TaskLocals::set`. Empty app id is now a programmer error → panic in debug, error log + reject in release.
- [ ] **C1.4** Tests: `MessageConsumer.ctx.store.set("k","v")` is readable from the same app's REST handler; cross-app namespace isolation verified.
- [ ] **C1.5** Tests: SecurityHook + ValidationHook see the same `ctx.app_id` as the REST handler they wrap.

---

## Phase D — DataView ↔ ConnectionPool integration (P0-3, P1-1, P1-10)

### D1 — Fix pool internals before adoption (P1-1)

**Files:** `crates/riversd/src/pool.rs`

- [ ] **D1.1** `PoolGuard::drop`: preserve original `created_at` so `max_lifetime` actually expires the underlying connection. Don't reset on return.
- [ ] **D1.2** `acquire`: include `idle_return` queue depth in capacity accounting, OR collapse to a single mutex-protected pool state. No double-counting paths.
- [ ] **D1.3** `PoolManager`: replace `Vec<Arc<ConnectionPool>>` with `HashMap<String, Arc<ConnectionPool>>` keyed by datasource id. O(1) lookup, no duplicates possible.
- [ ] **D1.4** Tests: long-lived connection expires; burst load doesn't exceed `max_connections`; duplicate datasource id rejected at registration.

### D2 — Route DataView execution through the pool (P0-3)

**Files:** `crates/rivers-runtime/src/dataview_engine.rs`, `crates/riversd/src/server/handlers.rs`, application context wiring

- [ ] **D2.1** Move `Arc<PoolManager>` into the application runtime context (or a layer reachable from `DataViewExecutor`).
- [ ] **D2.2** `DataViewExecutor::execute`: replace `factory.connect(driver, params).await` with `pool_manager.acquire(datasource_id).await?` returning a `PoolGuard`. Direct `factory.connect` only on the cold pool-fill path.
- [ ] **D2.3** `/health/verbose`: report from pool state (active/idle/max/last-error) instead of opening fresh connections per probe.
- [ ] **D2.4** Tests: 100 sequential DataView calls reuse ≤ N connections (where N = pool max); pool snapshot is non-empty after first call.

### D3 — Enforce DataView timeouts (P1-10)

**Files:** `crates/rivers-runtime/src/dataview_engine.rs`

- [ ] **D3.1** Wrap connect+execute in `tokio::time::timeout(request.timeout)`; map elapsed to `DataViewError::Timeout` with datasource id.
- [ ] **D3.2** Health verbose probe: bounded, parallel (`join_all` with per-DS timeout), result cached for short TTL.
- [ ] **D3.3** Tests: slow datasource (faker with sleep, or fault-injected Postgres) → timeout fires within budget; request worker freed.

---

## Phase E — Kafka producer & EventBus correctness (P1-3, P1-4)

### E1 — Kafka producer routes by destination (P1-3)

**Files:** `crates/rivers-runtime/src/dataview_engine.rs`, `crates/rivers-plugin-kafka/src/lib.rs`

- [ ] **E1.1** Producer: lazy initialization (no metadata call at create time). Cache `PartitionClient` per topic with bounded TTL + exponential backoff on failure.
- [ ] **E1.2** `publish(message)` honors `message.destination` for topic routing — not the producer-creation topic.
- [ ] **E1.3** Tests: one producer publishes to two distinct destinations; metadata fetch failure on topic A doesn't block topic B.

### E2 — EventBus global priority across exact + wildcard (P1-4)

**Files:** `crates/rivers-core/src/eventbus.rs`

- [ ] **E2.1** At dispatch time, merge exact + wildcard subscribers into a single list, then sort by priority. `Expect` < `Handle` < `Emit` < `Observe` (or current order — keep the spec).
- [ ] **E2.2** Optionally: enforce at subscribe time that wildcard subscribers may only register at `Observe` priority. Decision in `changedecisionlog.md`.
- [ ] **E2.3** Test: wildcard `Expect` runs before exact `Emit`; wildcard `Observe` runs after.

---

## Phase F — Hardening (P1-6, P1-7, P1-11, P1-12)

### F1 — Static files: canonicalize after symlink resolution (P1-6)

**Files:** `crates/riversd/src/static_files.rs`

- [ ] **F1.1** Canonicalize both root and resolved file path before serving. Compare canonicalized prefix.
- [ ] **F1.2** Reject symlinks outright in production mode (config flag `static.allow_symlinks`, default false).
- [ ] **F1.3** Tests: `../` traversal rejected; symlink-out-of-root rejected; legitimate file inside root served.

### F2 — Bound SWC compile time (P1-7)

**Files:** `crates/riversd/src/process_pool/v8_config.rs`, `crates/riversd/src/process_pool/module_cache.rs`

- [ ] **F2.1** Run `compile_typescript` in a supervised worker (existing swc supervisor from prior P0 work — extend, don't duplicate). Hard timeout (default 5s, configurable).
- [ ] **F2.2** Timeout → `ValidateError::CompileTimeout` with sanitized error and module id.
- [ ] **F2.3** Add a small fuzz/property corpus of pathological TS inputs (deep generics, exponential type instantiation). Run under timeout in CI.

### F3 — PostgreSQL config builder, not interpolation (P1-11)

**Files:** `crates/rivers-drivers-builtin/src/postgres.rs`

- [ ] **F3.1** Replace string-interpolated connection string with `tokio_postgres::Config` builder calls.
- [ ] **F3.2** Tests: passwords with spaces, quotes, `=`, and `&` connect successfully; database names with special chars connect successfully.

### F4 — Validate handler-supplied status & headers (P1-12)

**Files:** `crates/riversd/src/view_engine/validation.rs`

- [ ] **F4.1** `parse_handler_view_result`: status must be in `100..=599` else error response.
- [ ] **F4.2** Reject header names violating RFC 7230 token grammar; reject header values with CR/LF/NUL.
- [ ] **F4.3** Decision: do we block handler-set security headers (CSP, HSTS, etc.) absent explicit policy opt-in? Log in `changedecisionlog.md`, then enforce.
- [ ] **F4.4** Tests: status 999 rejected; header `X-Bad: foo\r\nInjection: yes` rejected.

---

## Phase G — P2 nice-to-haves

### G_R1 — Redis cluster: SCAN, not KEYS (P2-1)

**Files:** `crates/rivers-storage-backends/src/redis_backend.rs`, `crates/rivers-core/src/storage.rs`

- [ ] **G_R1.1** Cluster path: iterate primaries, run `SCAN` per node, merge cursors. Mirror the single-node implementation.
- [ ] **G_R1.2** Replace any hot-path key listing with explicit ownership records or index sets where feasible.
- [ ] **G_R1.3** Test against the 3-node cluster (192.168.2.206-208).

### G_R2 — EventBus subscription lifecycle (P2-2)

**Files:** `crates/rivers-core/src/eventbus.rs`

- [ ] **G_R2.1** `subscribe` returns a `SubscriptionHandle` that unregisters on `Drop`.
- [ ] **G_R2.2** Bound broadcast subscribers; tie to request/session lifetime where applicable.
- [ ] **G_R2.3** Add metrics: `rivers_eventbus_subscribers{kind}`, `rivers_eventbus_dispatch_seconds{event}`.

### G_R3 — Single source of truth for reserved storage prefixes (P2-3)

**Files:** `crates/rivers-core-config/src/storage.rs`, `crates/riversd/src/process_pool/v8_engine/context.rs`

- [ ] **G_R3.1** Move reserved-prefix list to one shared `const RESERVED_PREFIXES: &[&str]` module. Both core storage and V8 context import it.
- [ ] **G_R3.2** Test that every public storage entry point enforces the same set (reflection-style test or shared helper).

### G_R4 — Lifecycle observer hooks: contract or timeout (P2-4)

**Files:** `crates/riversd/src/view_engine/pipeline.rs`

- [ ] **G_R4.1** Decision: truly fire-and-forget (spawn into bounded queue) vs awaited-with-timeout. Log in `changedecisionlog.md`.
- [ ] **G_R4.2** Implement chosen path; remove misleading "fire-and-forget" comment if awaited.
- [ ] **G_R4.3** Test: slow observer does not extend request latency past contract bound.

### G_R5 — Module detection by metadata, not string match (P2-5)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [ ] **G_R5.1** Use bundle metadata (file is registered as a module) or extension. Drop `contains("export ")` heuristic.
- [ ] **G_R5.2** Tests: comment containing `export ` does not flip the path; string literal containing `import ` does not flip the path.

### G_R6 — Promise resolution tied to task timeout (P2-6)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [ ] **G_R6.1** Promise-pump loop honors the configured task timeout; pending-promise error includes timeout value and handler id.
- [ ] **G_R6.2** Tests: handler `await new Promise(r => setTimeout(r, 100))` resolves under a 1s timeout; same handler with 10ms timeout returns a clear timeout error.

### G_R7 — MySQL pool ownership & DriverFactory runtime strategy (P2-7)

**Files:** `crates/rivers-drivers-builtin/src/mysql.rs`, `crates/rivers-core/src/driver_factory.rs`

- [ ] **G_R7.1** After D2 lands: MySQL `mysql_async::Pool` is datasource-scoped (one per datasource), not per-connection.
- [ ] **G_R7.2** `DriverFactory::connect`: keep `spawn_blocking` + isolated runtime ONLY for plugin drivers that require it. Built-in async drivers run on the active runtime.
- [ ] **G_R7.3** Document the policy in `crates/rivers-core/src/driver_factory.rs` doc comment.

### G_R8 — SQLite path policy (P2-8)

**Files:** `crates/rivers-drivers-builtin/src/sqlite.rs`, `riversd.toml` schema

- [ ] **G_R8.1** Restrict SQLite paths to an approved data dir (config: `sqlite.allowed_root`). Reject paths outside on bundle load.
- [ ] **G_R8.2** Redact absolute paths in production logs (uses B4.1 helper).
- [ ] **G_R8.3** Don't auto-`mkdir -p` parent dirs unless `sqlite.create_parent_dirs = true`.

---

## Cross-cutting test recommendations (review §Test Recommendations)

These are not separate phases — they are the verification bar for the work above. Each appears in the relevant phase's task list. Repeated here as a single checklist for canary integration:

- [ ] Non-public view + missing session manager → fail closed (A2.3)
- [ ] REST handler cannot call `ctx.ddl` (B1.4)
- [ ] ApplicationInit can call allowed DDL; cannot call disallowed DDL (B1.5)
- [ ] MessageConsumer `ctx.store.set` readable from same-app HTTP handler (C1.4)
- [ ] DataView calls reuse pool state and obey max connections (D2.4)
- [ ] DataView connect+query timeout fires (D3.3)
- [ ] Broker startup completes when Kafka unreachable (A1.4)
- [ ] Kafka producer routes per `OutboundMessage.destination` (E1.3)
- [ ] Wildcard `Expect` runs before exact `Emit`/`Observe` (E2.3)
- [ ] Static file symlink escape rejected (F1.3)
- [ ] Module cache miss fails in production (B3.3)
- [ ] Public errors redact absolute paths (B4.4)
- [ ] Redis cluster list uses SCAN (G_R1.3)

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
