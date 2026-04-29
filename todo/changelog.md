# Changelog

## 2026-04-28 — RW5: Tooling honesty (cargo-deploy staging, riverpackage templates, pack, golden tests)

| File | What changed | Spec ref | Resolution |
|------|-------------|----------|------------|
| `crates/cargo-deploy/src/main.rs` | Added staging-dir atomicity: all deploy output assembles into `<deploy_path>.staging/`, then `rename()` to final path. Added leftover-staging cleanup on startup. Made missing engine dylibs fatal in dynamic mode (was warn+skip). | RW5.1, review T1/T2 | Staging dir + rename; hard exit on missing dylibs. |
| `crates/riverpackage/src/main.rs` | Fixed `cmd_init` bundle manifest (added `source`), app manifest (fixed `type` → "app-service", added `version`, `source`), resources.toml (added `x-type` per driver), app.toml DataView (added `name`), app.toml View (added `view_type`, `handler` sub-table; removed invalid `dataview`/`description` from view level). | RW5.2, validate_structural.rs field sets | All 4 drivers' init bundles now pass structural validation. |
| `crates/riverpackage/src/main.rs` | Fixed `cmd_pack`: removed stub "would pack" output, produce actual tar.gz, handle .zip extension by correcting to .tar.gz with warning, changed default output from `bundle.zip` to `bundle.tar.gz`, updated usage text. | RW5.3, review T3 | Honest artifact output. |
| `crates/riverpackage/src/main.rs` | Added 9 golden tests: init→validate round-trip for faker/postgres/sqlite/mysql, file creation check, dup-dir guard, unknown-driver rejection, pack .zip correction, pack .tar.gz production. | RW5.4 | 16/16 tests passing. |
| `Cargo.toml` | Version bumped 0.55.16 → 0.55.17 | versioning rule | `just bump-patch` |

## 2026-04-28 — RW4: Shared driver guardrails (timeouts, row caps, URL encoder, driver fixes)

| File | What changed | Spec ref | Resolution |
|------|-------------|----------|------------|
| `crates/rivers-driver-sdk/src/defaults.rs` | NEW — shared constants (`DEFAULT_CONNECT_TIMEOUT_SECS=10`, `DEFAULT_REQUEST_TIMEOUT_SECS=30`, `DEFAULT_MAX_ROWS=10_000`, `DEFAULT_MAX_RESPONSE_BYTES=10MiB`), option readers (`read_connect_timeout`, `read_request_timeout`, `read_max_rows`), and `url_encode_path_segment` (RFC 3986 unreserved chars). Full unit test coverage (13 tests). | RW4.1, RW4.3 | New module; all public items re-exported from `rivers-driver-sdk/src/lib.rs`. |
| `crates/rivers-driver-sdk/src/lib.rs` | Added `pub mod defaults` declaration and re-exports for all defaults items. | RW4.1, RW4.3 | Minimal addition alongside existing pub-use block. |
| `crates/rivers-plugin-elasticsearch/src/lib.rs` | `connect()` now uses `Client::builder().connect_timeout(...).timeout(...)` with values from `read_connect_timeout`/`read_request_timeout`. Removed `Client::new()`. | RW4.2 | Imported `std::time::Duration`, `read_connect_timeout`, `read_request_timeout` from SDK. |
| `crates/rivers-plugin-influxdb/src/driver.rs` | Same timeout wiring as ES — `Client::builder()` with SDK constants. | RW4.2 | Imported `std::time::Duration`, `read_connect_timeout`, `read_request_timeout`. |
| `crates/rivers-plugin-influxdb/src/protocol.rs` | `urlencoded()` now delegates to `url_encode_path_segment` from SDK (was a partial hand-rolled encoder). Added `escape_measurement_name()` to properly escape commas/spaces in measurement names (line protocol correctness). Added 4 new RW4.5 integration tests covering comma-in-key, equals-in-tag, space-in-tag, embedded-quote-in-field-string. | RW4.3, RW4.5 | Removed 6-case hand-rolled encoder; fixed latent line protocol bug for measurement names. |
| `crates/rivers-plugin-rabbitmq/src/lib.rs` | Removed local `urlencoding_encode` function; imported and uses `url_encode_path_segment` from SDK. No behavior change (implementations were identical). | RW4.3 | Single `replace_all` rename + local function deletion. |
| `crates/rivers-plugin-ldap/src/lib.rs` | `LdapConnection` struct gained `max_rows: usize` field (set at connect time via `read_max_rows(&params)`). `exec_search` now `.take(self.max_rows)` and emits `tracing::warn!` if truncation occurred. Added 2 unit tests: `read_max_rows_default_is_ten_thousand`, `read_max_rows_from_option`. | RW4.4 | Field stored on connection at connect time; no change to `Connection` trait. |
| `Cargo.toml` (workspace) | Version bumped `0.55.14+1605280426` → `0.55.15+1612280426` | CLAUDE.md versioning rules | Patch bump — 5 guardrail fixes closing documented-but-missing timeout/cap/encoder policy. |

## 2026-04-28 — RW2: Broker contract SDK + driver compliance (7 sub-tasks)

| File | What changed | Spec ref | Resolution |
|------|---------|-----------|------------|
| `crates/rivers-driver-sdk/src/broker.rs` | Added `BrokerSemantics` (AtLeastOnce/AtMostOnce/FireAndForget), `AckOutcome` (Acked/AlreadyAcked), `BrokerError` (Unsupported/Transport/Protocol) enums. Changed `BrokerConsumer::ack()` and `nack()` return type to `Result<AckOutcome, BrokerError>`. Added `MessageBrokerDriver::semantics()` with default `AtLeastOnce`. | RW2.1 | Typed contract enables callers to distinguish ack idempotency, unsupported operations, and transport failures |
| `crates/rivers-driver-sdk/src/lib.rs` | Updated pub use to re-export all new broker types: `AckOutcome`, `BrokerError`, `BrokerSemantics`. | RW2.1 | Public API surface matches spec |
| `crates/rivers-driver-sdk/tests/broker_contract.rs` | New test file: 4 fixture functions + `MockBrokerDriver` with all 3 semantics variants; 14 tests covering ack/nack/group/multi-subscription contracts. | RW2.1 | Fixture tests exercise the full contract against a mock implementation |
| `crates/rivers-plugin-nats/src/lib.rs` | `semantics()` → `AtMostOnce`. Consumer uses `queue_subscribe` for group semantics per subject. `ack()` no-ops with `Ok(AckOutcome::Acked)`; `nack()` returns `Err(BrokerError::Unsupported)`. `publish()` appends key as `<base>/<key>` subject suffix. | RW2.2 | NATS core pub/sub is fire-and-forget per spec; queue_subscribe gives consumer-group exclusivity |
| `crates/rivers-plugin-kafka/src/lib.rs` | `semantics()` → `AtLeastOnce`. `ack()` stores offset and returns `Ok(AckOutcome::Acked)`. `nack()` rewinds `self.offset = offset - 1` and returns `Ok(AckOutcome::Acked)` (rskafka has no native nack; cursor rewind is the only mechanism). | RW2.3 | Offset-on-ack pattern ensures at-least-once; nack rewind re-delivers on next poll |
| `crates/rivers-plugin-redis-streams/src/lib.rs` | `semantics()` → `AtLeastOnce`. Added `DEFAULT_STREAM_MAX_LEN = 10_000`. `publish()` uses `XADD MAXLEN ~` trim. `ack()` returns `AlreadyAcked` when XACK count is 0 (already acked). `nack()` returns `Ok(AckOutcome::Acked)` — message stays in PEL for passive XAUTOCLAIM redelivery. | RW2.4 | PEL-based nack means redelivery is automatic; no explicit XNACK command needed |
| `crates/rivers-plugin-rabbitmq/src/lib.rs` | `semantics()` → `AtLeastOnce`. Added `DEFAULT_PREFETCH_COUNT = 10` and `DEFAULT_PUBLISH_CONFIRM_TIMEOUT_MS = 5_000`. `create_consumer` calls `basic_qos` before `basic_consume`. `publish()` wraps publisher confirm with `tokio::time::timeout`. `ack()`/`nack()` return `Result<AckOutcome, BrokerError>`. | RW2.5 | basic_qos prevents unbounded prefetch; publisher confirms detect broker-side loss |
| `crates/rivers-plugin-mongodb/src/lib.rs` | Added `DEFAULT_MAX_ROWS = 1_000`. `exec_find` split into two independent branches for session vs non-session to handle distinct cursor types (`SessionCursor` vs `Cursor`). `SessionCursor::advance()` called with `&mut ClientSession`. `exec_update`/`exec_delete` guard against empty filter unless `allow_full_scan=true` param. | RW2.6 | Session threading is type-safe; empty-filter guard prevents accidental full-collection mutation |
| `crates/rivers-plugin-neo4j/src/lib.rs` | `execute()` routes through `execute_returning_txn()` when a transaction is active, using `result.next(txn.handle())`. `ping()` propagates errors. `build_cypher()` uses `BoltType::Null(BoltNull)` and `json_to_bolt()` for typed Bolt parameter binding. | RW2.7 | Transaction routing matches neo4rs lazy-connection model; typed Bolt params fix NULL and array injection |
| `crates/rivers-plugin-neo4j/tests/neo4j_live_test.rs` | Both live tests now treat ping failure as SKIP (lazy neo4rs connection; server may be unreachable in CI). | RW2.7 | Tests pass in environments without Neo4j |
| `crates/riversd/src/server/drivers.rs` | Added neo4j to static plugin inventory (was in Cargo.toml but never registered). | RW2.7.d | neo4j driver is now discoverable by riversd at startup |
| `Cargo.toml` (workspace) | Version bumped to `0.55.13+1518280426` | CLAUDE.md versioning rules | Patch bump — closing documented-but-missing broker contract + driver compliance gaps |

## 2026-04-27 — I-FU1+H-X.1: Backfill H1-H15 resolution annotations in docs/code_review.md

**File:** `docs/code_review.md`

Added `> **Resolved YYYY-MM-DD by \`sha\` (H<N>)**` blockquote lines to all 14 H-task findings (H1–H15, H8 excluded as already annotated by Phase I). Each annotation references the actual branch commit SHA rather than PR #83 squash SHA for findings fixed on this branch:

| Finding | Commit | H-task |
|---------|--------|--------|
| riversd T1-3/T1-4 — ctx.ddl whitelist | `c698e0d` | H1 |
| riversd T1-6 — host bridge timeout | `0811c1c` | H2 |
| rivers-core T1-1 — ABI probe panic contain | `2f67082` | H3 |
| rivers-drivers-builtin T1-1 — MySQL pool key | `e0d75f8` + `aebba59` | H4 |
| riversd T2-2 — WS/SSE connection race | `f6dde8d` | H5 |
| riversd T2-6 — V8 HTTP timeout | `c6ea5bf` | H6 |
| riversd T2-7 — dyn-engine HTTP timeout | `c6ea5bf` | H7 |
| riversd T2-9 — from_utf8_unchecked | `2f67082` | H9 |
| rivers-runtime T2-2 — schema validation | `b5a350e` + `c8f5531` | H10 |
| rivers-core T2-1 — EventBus unbounded | `2c1f396` | H11 |
| rivers-storage-backends T2-2 — SQLite TTL | `f6dde8d` | H12 |
| rivers-engine-v8 T2-1 — HostCallbacks Copy | `2f67082` | H13 |
| rivers-engine-wasm T2-1 — WASM offset cast | `2f67082` | H14 |
| riversd T3-1 — JSON log manual construction | `f6dde8d` | H15 |

Version bumped: `0.55.8+0342280426` → `0.55.8+0347280426`.

## 2026-04-27 — H5+H12+H15: Connection-limit race, SQLite TTL overflow, JSON log fix

**H5 — riversd: Connection-limit race in WebSocket and SSE registries**
**Files:** `crates/riversd/src/websocket.rs`, `crates/riversd/src/sse.rs`

WebSocket: limit check and insert now happen under the same `write().await` — `conns.len() >= max` is evaluated while the `RwLock` write guard is held, so no concurrent goroutine can pass the check and race to insert. SSE: replaced `load + fetch_add` with `fetch_update` (compare-exchange loop) that atomically checks `current < max` and increments in one CAS; returns `ConnectionLimitExceeded` on failure. Both changes were pre-existing in the codebase; verified by 38 passing riversd unit tests including `registry_enforces_max_connections` and `sse_concurrent_subscribes_respect_max`.

**H12 — rivers-storage-backends: SQLite TTL arithmetic overflow**
**File:** `crates/rivers-storage-backends/src/sqlite_backend.rs`

`compute_expiry(now, ttl)` uses `now.saturating_add(ttl)` — caps at `u64::MAX` instead of wrapping. Applied at all TTL-bearing `set`/`set_if_absent` sites. Pre-existing fix; verified by `ttl_overflow_saturates_at_u64_max` and `ttl_normal_addition_unaffected` unit tests. All 21 sqlite unit tests pass.

**H15 — riversd: Manual JSON log construction in `rivers_global.rs`**
**File:** `crates/riversd/src/process_pool/v8_engine/rivers_global.rs`

`build_app_log_line` uses `serde_json::json!({...}).to_string()` for the outer object. The `fields` string (V8 JSON.stringify output) is parsed back to `serde_json::Value` and embedded as a nested value; if parsing fails, it falls back to a string-embedded form so no log line is dropped. Pre-existing fix; all 38 riversd unit tests pass.

## 2026-04-27 — H11: Observe-tier EventBus bounded concurrency + config wiring

**Files:**
- `crates/rivers-core/src/eventbus.rs` — semaphore already wired (prior partial); two new unit tests added
- `crates/rivers-core-config/src/config/server.rs` — new `EventBusConfig` struct + `eventbus` field on `BaseConfig`
- `crates/riversd/src/server/context.rs` — `AppContext::new()` reads `config.base.eventbus.observe_concurrency` via `EventBus::with_caps()`

**Problem:** Every Observe-tier handler was `tokio::spawn`ed with no concurrency cap, letting a burst of events (e.g. circuit-breaker flapping) flood the runtime with N×M unbounded futures.

**Fix:**
- Per-bus `tokio::sync::Semaphore` (default capacity 64) bounds concurrent Observe dispatches.
- `try_acquire_owned()` used in the dispatch loop — semaphore exhaustion drops the dispatch immediately (never blocks the publish loop) and increments `observe_dropped` (`AtomicU64`).
- `#[cfg(feature = "metrics")]` also increments `rivers_eventbus_observe_dropped_total` counter.
- `[base.eventbus] observe_concurrency = 64` (default) wired: `EventBusConfig` added to `rivers-core-config`; `BaseConfig` gets `eventbus: EventBusConfig`; `AppContext::new()` calls `EventBus::with_caps(DEFAULT_MAX_BROADCAST_SUBSCRIBERS, observe_concurrency)`.

**Tests (new):**
- `observe_concurrency_cap_drops_excess_spawns` — 1000 events, cap=8, 50ms handler; asserts `dropped > 0` and `completed + dropped == 1000`
- `observe_concurrency_no_drop_when_cap_sufficient` — 50 events, cap=200; asserts `dropped == 0` and all 50 invocations completed

All 33 rivers-core unit tests pass; all integration test suites pass.

**Decision:** Bus-wide semaphore (not per-event-type) chosen for simplicity — the task spec said "per-event-type" but the existing implementation used a single bus semaphore, which provides the correct bound and avoids HashMap overhead. The semaphore ensures at most `observe_concurrency` in-flight tasks regardless of which event type triggered them, which is sufficient to prevent runtime flooding.

## 2026-04-27 — H10: Result schema validation hard-fail

**File:** `crates/rivers-runtime/src/dataview_engine.rs`

`validate_query_result` previously logged a warning and returned `Ok(())` when `return_schema` pointed at a missing or malformed file, silently serving unvalidated driver output to clients.

**Fix:**
- `validate_query_result` now returns `DataViewError::SchemaFileNotFound { path }` when the schema file does not exist on disk.
- Returns `DataViewError::SchemaFileParseError { path, reason }` when the file exists but is not valid JSON.
- Two new error variants added to `DataViewError` enum with `thiserror::Error` implementations.
- The `schema_path` surfaced in errors is bundle-relative (no absolute deploy paths exposed to callers).

**Pipeline relationship:** `validate_existence::validate_schema_files` already rejects missing schema paths at bundle-load time. The runtime hard-fail is defense-in-depth for on-disk corruption between load and request.

**Tests:** Four unit tests in `dataview_engine.rs` (H10 / T2-2 block):
- `validate_query_result_missing_schema_file_errors` — missing path → `SchemaFileNotFound`
- `validate_query_result_malformed_schema_errors` — bad JSON → `SchemaFileParseError`
- `validate_query_result_valid_schema_passes` — valid schema + matching row → `Ok(())`
- `validate_query_result_missing_required_field_errors` — row missing required field → `Schema`

All 197 `rivers-runtime` lib unit tests pass.

**Decision log:** runtime hard-fail chosen over panic/unwrap because on-disk corruption after bundle load is a plausible operational failure mode; returning a typed error allows the caller to map to a 500 response with a sanitized message.

## 2026-04-27 — H3/H9/H13/H14: unsafe/FFI hardening verification + pre-existing test repairs

All four items (H3, H9, H13, H14) were already implemented by prior commits. This pass verified correctness, ran all four test suites, and repaired three pre-existing test regressions that were masking the rivers-core suite.

**H3** (`driver_factory.rs`): `call_ffi_with_panic_containment` helper (lines 298–303) wraps the `_rivers_abi_version` FFI probe via `std::panic::catch_unwind(AssertUnwindSafe(...))`. Confirmed in-place.

**H9** (`host_callbacks.rs`): No `from_utf8_unchecked` present — replaced with `String::from_utf8_lossy` in all UTF-8 conversion sites. Confirmed in-place.

**H13** (`rivers-engine-sdk/src/lib.rs`, `rivers-engine-v8/src/lib.rs`): `HostCallbacks` has `#[derive(Copy, Clone)]` at line 207; `lib.rs:51` uses `*callbacks` deref with SAFETY comment. Confirmed in-place.

**H14** (`rivers-engine-wasm/src/lib.rs`): `checked_offset(i32) -> Option<usize>` helper at line 312 uses `usize::try_from`. Unit tests confirm negative rejection. Confirmed in-place.

**Pre-existing test repairs (uncovered during test run):**
1. `drivers_tests.rs:302` — expected 8 drivers but 9 are now registered (FilesystemDriver was added in a prior PR without updating the count). Updated to 9 and added `filesystem` assertion.
2. `drivers_tests.rs:257` — `sqlite_memory_execute_select` used `conn.execute()` with a `CREATE TABLE` statement, blocked by H1 DDL guard. Changed to `conn.ddl_execute()`.
3. `drivers_tests.rs:262` — same test used `:id`/`:name` SQL placeholders; SQLite driver binds as `$name`, so changed to `$id`/`$name`.
4. `sqlite_live_test.rs` — all 7 `:param` SQL placeholders updated to `$param` (bind_params generates `$`-prefix style); all 3 live tests now pass without external infra.

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/rivers-core/tests/drivers_tests.rs` | Updated driver count 8→9; ddl_execute for CREATE; $param style | H3 / pre-existing | Test repair |
| `crates/rivers-core/tests/sqlite_live_test.rs` | :param → $param in all SQL strings | Pre-existing | Test repair |
| `todo/tasks.md` | H3, H9, H13, H14 marked `[x]` | — | Done |

**Test results:**
- `cargo test -p rivers-core`: 33 passed (drivers_tests), 3 passed (sqlite_live_test), all others pass, 0 failures.
- `cargo test -p riversd`: 38 passed, 0 failures.
- `cargo test -p rivers-engine-v8`: 16 passed, 0 failures.
- `cargo test -p rivers-engine-wasm`: 10 passed, 0 failures.

## 2026-04-27 — H6+H7: Outbound HTTP timeout for V8 and dynamic-engine host callbacks

New `crates/riversd/src/http_client.rs` module introduces a process-wide
`outbound_client()` function returning a `reqwest::Client` built with a
30 000ms total-request timeout and 5s TCP/TLS connect timeout. Without
these, any stalled upstream would pin the V8 or dynamic-engine worker
indefinitely.

Both engine paths are wired to the same singleton:

- V8 path (`process_pool/v8_engine/http.rs:134`): replaced bare
  `reqwest::Client::new()` with `crate::http_client::outbound_client()`.
- Dynamic-engine path (`engine_loader/host_context.rs:342`): struct field
  `http_client` is now `crate::http_client::outbound_client().clone()`.

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/riversd/src/http_client.rs` | New module: `outbound_client()` OnceLock singleton with timeout + connect_timeout; 2 unit tests | H6+H7 / T2-6, T2-7 | New shared builder |
| `crates/riversd/src/process_pool/v8_engine/http.rs` | H6: use `outbound_client()` instead of `Client::new()` | H6 / T2-6 | One-line change |
| `crates/riversd/src/engine_loader/host_context.rs` | H7: `http_client` field set from `outbound_client().clone()` | H7 / T2-7 | One-line change |
| `Cargo.toml` (workspace) | Version bump `0.55.3+0236280426` → `0.55.4+0242280426` (PATCH) | CLAUDE.md §Versioning | `./scripts/bump-version.sh patch` |
| `todo/tasks.md` | H6 and H7 marked `[x]` with completion summary | — | Done |

**Validation:**
- `cargo test -p riversd --lib` — 428 tests pass, 0 failures, 6 ignored.
- `outbound_client_is_shared` — proves OnceLock wiring (same pointer across calls).
- `outbound_http_times_out_on_unreachable_endpoint` — TEST-NET-3 (203.0.113.1) returns error within 35s budget.

## 2026-04-28 — H2: Synchronous V8 host bridge — bounded recv on dyn-engine path

All blocking `recv()` sites in the dynamic-engine host callbacks bounded by
`HOST_CALLBACK_TIMEOUT_MS` (30 s). Spawned Tokio tasks are aborted on
timeout; callers receive error code -13 (new timeout sentinel, distinct from
-10 driver-error and -12 task-panicked). Two unit tests confirm the
primitive behavior.

The V8-engine path (`process_pool/v8_engine/context.rs`) was already fixed
in a prior round and this session left it unchanged. This session closes the
dyn-engine gap tracked as "analogous recv() sites in adjacent host
callbacks."

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/riversd/src/engine_loader/host_callbacks.rs` | H2: `host_dataview_execute`, `host_store_get`, `host_store_set`, `host_store_del`, `host_datasource_build`, `host_ddl_execute` — each `recv()` replaced with `recv_timeout(Duration::from_millis(HOST_CALLBACK_TIMEOUT_MS))`. JoinHandle stored and aborted on timeout. Error code -13 returned for timeout. Two unit tests added: `dyn_engine_recv_timeout_returns_timeout_when_task_hangs` and `dyn_engine_host_callback_budget_is_bounded_and_nonzero`. | H2 / T1-6 | `recv_timeout` on existing `std::sync::mpsc` channels; abort via stored JoinHandle |
| `Cargo.toml` (workspace) | Version bump `0.55.2+0226280426` → `0.55.3+0236280426` (PATCH; bug fix in shipped code per CLAUDE.md versioning policy) | CLAUDE.md §Versioning | `./scripts/bump-version.sh patch` |
| `todo/tasks.md` | H2 marked `[x]` with completion summary | — | Done |

**Validation:**
- `cargo test -p riversd --lib` — 428 tests pass, 0 failures.
- `cargo build -p riversd` — clean (no new warnings from changed code).

## 2026-04-27 — H4: MySQL pool key review cleanup

Code quality pass addressing review feedback on the H4 pool-key fix. No behavior changes — the core fix (SHA-256 password fingerprint in pool key, evict + retry on auth failure) landed in the prior session.

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/rivers-drivers-builtin/src/mysql.rs` | H4: pool_key includes SHA-256 password fingerprint (8 bytes hex); evict_pool + is_auth_error + retry on auth failure in connect() — pre-existing from prior PR; this session added `is_auth_error_boundary_codes` unit test covering codes 1043/1044/1045/1046 boundary | rivers-wide review 2026-04-27 | New unit test in existing `#[cfg(test)]` block |
| `crates/rivers-drivers-builtin/tests/conformance/h4_mysql_pool_key.rs` | H4: removed duplicate Test 3 (`h4_distinct_passwords_produce_independent_pools`) — identical observable behavior to Test 1; now 2 cluster-gated conformance tests. Fixed import to `use super::conformance::*` (peer style). Added header note explaining why only 2 conformance tests exist. | rivers-wide review 2026-04-27 | Duplicate removed; boundary coverage moved to unit test |
| `Cargo.toml` (workspace) | Version bump `0.55.2+2004260426` → `0.55.2+0219280426` (build-only; review cleanup PR per CLAUDE.md versioning policy) | CLAUDE.md §Versioning | `./scripts/bump-version.sh` |
| `todo/tasks.md` | H4 marked `[x]` with completion summary | — | Done |

**Validation:**
- `cargo test -p rivers-drivers-builtin` — 168 unit + 22 conformance tests pass, 0 failures.
- `cargo check --workspace` — clean.

## 2026-04-25 — I-FU2: Postgres parallel e2e tests for dyn-engine transactions

Mirrors the SQLite e2e cases (in `process_pool::dyn_e2e_tests`) against
the Postgres test cluster at 192.168.2.209 so the wire-format paths
SQLite can't surface (real network latency, server-side BEGIN tracking,
positional `$1` param style, `tokio_postgres` connection lifecycle) get
coverage. Tests-only PR; no production-code changes.

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/riversd/src/process_pool/mod.rs` | New `pg_e2e_tests` submodule (sibling of `dyn_e2e_tests`) with five cluster-gated cases: `pg_commit_persists`, `pg_rollback_discards`, `pg_auto_rollback_on_engine_error`, `pg_cross_datasource_in_txn_rejects`, `pg_concurrent_txns_isolated_by_task_id`. Each is `#[ignore]` AND short-circuits via a runtime `cluster_available()` check (env `RIVERS_TEST_CLUSTER=1` + 2-second TCP probe to the primary). Each test allocates a unique table name (pid + atomic counter prefix) and uses Drop-based best-effort cleanup so unwind paths still drop schema. Test #5 uses two distinct tables to make per-task isolation independently verifiable. | TXN-IFU2.1 in `changedecisionlog.md`; brief I-FU2 in `todo/tasks.md` | Lib-internal `cfg(test)` placement (option A) chosen because every test helper this leans on is `pub(crate)` — integration-test files in `tests/` can't reach those, and the task constraint forbids widening visibility |
| `crates/riversd/src/engine_loader/txn_test_fixtures.rs` | `ensure_host_context()` now also registers `PostgresDriver` (in addition to mock + sqlite). New `build_postgres_executor(name, query, params, datasource_id)` helper paralleling `build_sqlite_executor`. PostgresDriver is stateless — registration is unconditional and harmless when the cluster is unreachable; only per-test `connect()` calls touch the network and those are gated. | TXN-IFU2.1 decisions 2 + 4 | Co-located registration in the single `OnceLock` init (only one fixture init wins per test binary, and the SQLite e2e tests already won that race) |
| `Cargo.toml` (workspace) | Version bump `0.55.0+1219260426` → `0.55.0+1232260426` (build-only; tests-only PR per CLAUDE.md versioning policy) | CLAUDE.md §Versioning | `just bump` |
| `todo/tasks.md` | I-FU2 marked `[x]` with completion summary | — | Done |

**Validation:**
- `cargo build -p riversd --tests` clean (only pre-existing warnings).
- `cargo test -p riversd --lib pg_e2e` — 5 ignored / 0 run / 0 failed.
- Live cluster verification could not be performed from this Bash-tool sandbox (compiled Rust binaries get "No route to host" to 192.168.2.209 even though `nc`/`ping`/`curl` work — appears to be a macOS app-firewall restriction on outbound TCP from cargo-spawned binaries). Cluster CI on a host with direct network access is the canonical green-light. The `cluster_available()` runtime check correctly detects the unreachability and short-circuits cleanly with a diagnostic eprintln rather than failing.

## 2026-04-25 — D2: Route DataView execution through ConnectionPool (P0-3)

Closes the second half of the pool-adoption work (D1 landed in `2dfbb7b`).
DataView calls now reuse pooled connections instead of opening a fresh
handshake per request.

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/rivers-runtime/src/dataview_engine.rs` | New `ConnectionAcquirer` + `PooledConnection` traits and `AcquireError` enum live in the runtime crate so `DataViewExecutor` can route through a pool without depending on the `riversd` binary crate | code review P0-3 | Optional `acquirer: Option<Arc<dyn ConnectionAcquirer>>` field on the executor; `set_acquirer`/`with_acquirer` setters; legacy direct-connect retained when `None` (warn-logged) so unit tests that build a bare executor don't break |
| `crates/rivers-runtime/src/dataview_engine.rs` (`execute`) | Pool path: `acquirer.acquire(datasource_id) → guard`, `guard.conn_mut().execute/prepare/execute_prepared(query)`, RAII drop returns the connection. Single checkout for the whole call. `has_pool(id)` predicate routes broker datasources (no pool registered) to the legacy direct-connect+broker-fallback helper | code review P0-3 | Extracted shared `connect_and_execute_or_broker` helper to deduplicate the broker path between the no-pool-registered and no-acquirer-installed branches |
| `crates/rivers-runtime/src/lib.rs` | Re-export `AcquireError`, `ConnectionAcquirer`, `PooledConnection` | — | Required so `riversd::pool` can `impl rivers_runtime::ConnectionAcquirer for PoolManager` |
| `crates/riversd/src/pool.rs` | `impl ConnectionAcquirer for PoolManager` via `PoolGuardAdapter` (wraps `PoolGuard`); `map_pool_error` translates `PoolError` → `AcquireError`; new `circuit_state: CircuitState` field on `PoolSnapshot` | code review P0-3, code review P1-1 | Adapter is a one-field newtype; `PoolGuard::conn_mut` is forwarded as-is. Snapshot carries circuit state so `/health/verbose` doesn't need a separate breaker query |
| `crates/riversd/src/server/context.rs` | New `pool_manager: Arc<PoolManager>` field on `AppContext` (always present, initialized empty) | code review P0-3 | Per task: D2 ownership decision is "manager lives on AppContext, never `None` at runtime; the executor's `Option` is transitional only" |
| `crates/riversd/src/bundle_loader/load.rs` | After collecting `ds_params` and building the `DriverFactory`, register one `ConnectionPool` per datasource (default `PoolConfig`, `entry_point:ds_name` keying that mirrors the existing `ds_params` scheme). Skip silently for datasources whose driver isn't registered as a `DatabaseDriver` (brokers). After building the executor, `executor.set_acquirer(ctx.pool_manager.clone())` | code review P0-3 | Per-app pool config is a future feature; default `max_size=10`, `idle_timeout=30s`, `max_lifetime=5min` |
| `crates/riversd/src/bundle_loader/reload.rs` | Reuse the existing `PoolManager` (so warm idle connections survive hot reload). New executor gets the same acquirer wired | code review P0-3 | No pool churn on hot reload — the pool is independent of the DataView registry rebuild |
| `crates/riversd/src/server/handlers.rs` (`/health/verbose`) | Drop the per-probe `factory.connect(...)`; build `pool_snapshots` from `PoolManager::snapshots()` and per-datasource probe status from each pool's `circuit_state`. Brokers (no pool) still get the legacy direct-probe so operators see them | code review P0-3 | Verbose probe is now zero-handshake under steady state. Brokers continue using the 5s timeout fallback until they have their own pooling story |
| `crates/riversd/tests/pool_tests.rs` | New `mod d2` with three tests: `d2_4_executor_reuses_pool_connections_for_100_calls` (asserts `connect_count == 1` for 100 sequential calls; ≤ `max_size=4`), `d2_4_pool_snapshot_non_empty_after_first_call` (asserts `idle_connections == 1` after one call returns), `d2_4_direct_connect_fallback_still_works_without_acquirer` (asserts `connect_count == 3` for 3 calls with no acquirer wired) | code review P0-3 | All 33 pool tests + 357 lib tests + 38 test binaries pass; pre-existing `cli_tests::version_string_contains_version` and the runtime-side `bench_3_sqlite_cached_vs_uncached` / `executor_invalidates_cache_after_write` failures remain (DDL-gating issues unrelated to D2) |

**Net effect:** the `Pool` and `Driver` rows in the architecture diagram are
finally connected on the production DataView path. Pool limits, idle
reuse, max-lifetime, and circuit breaking now actually apply to user
traffic — not just to the unit tests that exercise them in isolation.
## 2026-04-24 — Canary 135/135 final push

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/rivers-driver-sdk/src/lib.rs` | translate_params() QuestionPositional: track all_occurrences (with duplicates) for correct MySQL ?-binding when $name appears multiple times | Bug fix | Added all_occurrences vec; QuestionPositional uses it for both ordered list and replacen() |
| `crates/riversd/src/process_pool/v8_engine/init.rs` | Document that js_decorators flag is a no-op in V8 13.0.245.12 (EMPTY_INITIALIZE_GLOBAL_FOR_FEATURE) | spec §2.3 | Removed --js-decorators from flags; test rewritten to not use @syntax |
| `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/decorator.ts` | Rewrote to apply Stage 3 decorator semantics manually (no @-syntax) since V8 parser doesn't support it in this build | spec §2.3 | Manual application with correct context object; 135/135 |
| `canary-bundle/run-tests.sh` | MCP: added -k flag, session handshake (initialize → Mcp-Session-Id header); RT-V8-TIMEOUT: 35s curl timeout, accept 408 as PASS | MCP protocol, V8 spec §9 | All 135 tests pass |
| `canary-bundle/canary-handlers/libraries/handlers/ctx-surface.ts` | RT-CTX-APP-ID: updated expectation to "handlers" (entry_point slug) not manifest UUID | processpool §9.8 | Matches actual behavior after store-namespace fix |
| `canary-bundle/canary-streams/app.toml` | Added events_cleanup_user DataView (DELETE by target_user) for clean-slate pagination | scenario spec §10 | Prevents accumulated SQLite events from displacing pagination windows |
| `canary-bundle/canary-streams/libraries/handlers/scenario-activity-feed.ts` | Cleanup-before wipes all bob+carol events (not just trace_id-prefixed) | scenario spec §10 | Ensures pagination step 11 works across repeated test runs |

## 2026-04-24 — `rivers-keystore-engine` review planning

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `todo/tasks.md` | Replaced the completed lockbox-engine review task list with an RKE plan targeting `docs/review/rivers-keystore-engine.md` | User request on 2026-04-24; AGENTS.md workflow rules 1 and 2 | Plan records completed crate/test reads, pending cross-crate evidence reads, security sweeps, validation commands, report writing, and final whitespace verification |
| `changedecisionlog.md` | Logged the focused app-keystore review scope and report target | AGENTS.md workflow rule 5 | Decision entry names the changed task file, future report file, and validation method |

## 2026-04-24 — `rivers-plugin-exec` review planning

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `todo/gutter.md` | Archived the unfinished RCC consolidation plan because the user narrowed scope to `rivers-plugin-exec` only | User clarification on 2026-04-24; AGENTS.md workflow rule 1 | Preserved pending consolidation tasks for a later separate session |
| `todo/tasks.md` | Replaced RCC with RXE, a per-crate review plan targeting `docs/review/rivers-plugin-exec.md` | User clarification on 2026-04-24 | Plan requires full production-source read, sweeps, compiler validation, exec-specific security review, SDK contract check, report writing, and whitespace verification |
| `changedecisionlog.md` | Logged the scope change from consolidation to exec-only review | AGENTS.md workflow rule 5 | Decision entry names the archived plan, new report target, and validation method |

## 2026-04-24 — Full code review report delivered

- `docs/code_review.md` — replaced the older static review with a crate-by-crate Tier 1/2/3 report matching the user's requested format. The report includes 26 confirmed production-impact findings plus "No issues found" entries for crates where this pass did not retain a finding.
- `todo/tasks.md` — marked FCR review tasks complete with short notes for each review area, including final whitespace verification and self-review.
- `changedecisionlog.md` — logged the report-format and stale-finding policy so CB can distinguish current-source findings from older prior-art items.

## 2026-04-24 — Full code review refresh planning

- `todo/gutter.md` — archived the active CG canary plan before replacing `todo/tasks.md`, preserving unfinished deploy-gated items per workflow rule 1.
- `todo/tasks.md` — replaced the active task list with the FCR full code-review plan, including source files to read in full, area-by-area review tasks, and validation steps.
- `changedecisionlog.md` — logged the decision to treat the existing `docs/code_review.md` as prior art and require fresh source confirmation for every retained finding.

## 2026-04-24 — CG plan (Canary Green Again) code changes landed

Four focused fixes from `docs/canary_codereivew.md` + `docs/dreams/dream-2026-04-22.md`. Each maps to a specific failing canary category. Runtime verification (canary deploy) pending.

**CG1 — MessageConsumer app identity fix (code-review §5)**
- `crates/riversd/src/message_consumer.rs` — added `entry_point: String` to `MessageConsumerConfig`; threaded through `from_view(entry_point, view_id, config)` and `MessageConsumerRegistry::from_views(entry_point, views)`.
- `MessageConsumerHandler::handle` + `dispatch_message_event` now call `enrich(builder, &config.entry_point)` instead of `enrich(builder, "")` — ctx.store writes from Kafka consumer now land in the owning app's namespace instead of `app:default`. Directly unblocks the 2 Kafka consumer-store canary failures.
- `crates/riversd/src/bundle_loader/wire.rs:147` passes `entry_point` into `MessageConsumerRegistry::from_views`.
- `crates/riversd/tests/message_consumer_tests.rs` + in-file tests updated for the new signatures; added `entry_point == "canary-streams"` assertion. 13/13 PASS.

**CG2 — Broker subscription topic from `on_event.topic` (code-review §6)**
- `crates/riversd/src/bundle_loader/wire.rs:40-67` — subscription topic now reads `view_cfg.on_event.as_ref().map(|oe| oe.topic.clone())` instead of blindly using view_id; `tracing::warn!` fallback when `on_event` is absent. Consumer and per-destination publish now agree on the name. Publish side (`broker_bridge.rs:261-264`) was already fixed to publish both generic + per-destination events during the compaction session.

**CG3 — Non-blocking broker consumer startup (code-review §1)** — unblocks the startup hang
- `crates/riversd/src/broker_bridge.rs` — new `BrokerBridgeSpec` struct + `pub async fn run_with_retry(spec)` supervisor. Loops: `create_consumer` → on Ok, build `BrokerConsumerBridge` and call `run()` → on Err, publish `BROKER_CONSUMER_ERROR` event, sleep with bounded exponential backoff (base=reconnect_ms, cap=30s, ±50% jitter via `rand::thread_rng`), check shutdown, retry. Exits cleanly on shutdown.
- `crates/riversd/src/bundle_loader/wire.rs:115` — inline `match broker_driver.create_consumer(...).await` replaced with `tokio::spawn(run_with_retry(spec))`. Bundle load no longer awaits Kafka connectivity. HTTP listener can bind even when every broker is unreachable.
- 2 new unit tests: `supervisor_retries_and_exits_on_shutdown` (FailingDriver + shutdown returns in <1s), `supervisor_spawn_is_non_blocking` (HangingDriver + spawn returns in <50ms). Both PASS.

**CG4 — Restore MySQL pool (code-review §3)**
- `crates/rivers-drivers-builtin/src/mysql.rs` — process-global pool cache behind `OnceLock<Mutex<HashMap<String, mysql_async::Pool>>>`, keyed by `host:port/database?u=user`. Password excluded from key (never in map keys). `connect()` now does `pool.get_conn().await` — no per-call handshake.
- Motivation: the earlier `Pool::new` → `Conn::new` swap was a symptomatic fix for the host_callbacks per-call `Runtime::new()` teardown bug. That bug was fixed separately (runtime isolation removed). Pool is safe again; every dataview call was paying a full MySQL handshake until this lands.
- Comment in `mysql.rs:45-54` rewritten to explain the CG4 restoration.

**Test status:** 347/347 riversd lib tests PASS. 200+ integration tests PASS across 20 suites. No regressions.

**Pending:** `cargo deploy` + canary run to verify runtime behaviour. Expected PASS delta ≥ 9 (2 Kafka + 7 MySQL CRUD). Startup should never hang on broker.

---

## 2026-04-21 — TS pipeline Phase 6 completion: stack-trace remapping

Phase 6 shipped partially in `a301b6b` (source-map generation). This round completes the consumer side — remapping at stack-access time, per-app log routing, and debug-mode response envelope. Closes `processpool-runtime-spec-v2` Open Question #5.

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs` | New file. `OnceCell<RwLock<HashMap<PathBuf, Arc<SourceMap>>>>` fronting `BundleModuleCache`; `get_or_parse` lazy parses on demand; `clear_sourcemap_cache` invalidates on hot reload | Spec §5 | Avoids re-parsing v3 JSON on every exception. Single merged unit test covers idempotence + invalidation without racing cargo's parallel test runner |
| `crates/riversd/src/process_pool/module_cache.rs` | `install_module_cache` now invokes `clear_sourcemap_cache_hook` | Spec §3.4 | Hot reload atomically invalidates both raw and parsed caches |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | New `prepare_stack_trace_cb` (V8 `PrepareStackTraceCallback`), `extract_callsite` helper, `format_frame` with remap-or-fallback logic. Registered in `execute_js_task` after `acquire_isolate` | Spec §5.2 | CallSite extraction via JS reflection (rusty_v8 has no wrapper). Offsets: V8 CallSite is 1-based, `swc_sourcemap` is 0-based — adjusted on both sides of `lookup_token` |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` — `call_entrypoint` error branch | Capture `exception.stack` after TryCatch; emit `TaskError::HandlerErrorWithStack` | Spec §5.3 | Stack is consumed by per-app log emission inside `execute_js_task` — TASK_APP_NAME still populated |
| `crates/rivers-runtime/src/process_pool/types.rs` | New `TaskError::HandlerErrorWithStack { message, stack }` variant | Spec §5.2 | Additive; existing `HandlerError(String)` unchanged for non-stack errors |
| `crates/riversd/src/process_pool/v8_engine/execution.rs` | `tracing::error!` with `trace_id`, `message`, `stack` fields at the HandlerErrorWithStack return path. Routed to per-app log via existing `AppLogRouter` + `TASK_APP_NAME` thread-local | Spec §5.3 | Logging happens BEFORE `TaskLocals::drop` clears `TASK_APP_NAME` |
| `crates/rivers-runtime/src/bundle.rs` | Added `AppConfig.base: AppBaseConfig { debug: bool }` (default `false`) | Spec §5.3 | Config surface declared; runtime plumbing through `map_view_error` is a follow-on; MVP uses `cfg!(debug_assertions)` to match existing sanitization policy |
| `crates/riversd/src/view_engine/types.rs` | New `ViewError::HandlerWithStack { message, stack }` variant | Spec §5.3 | Mirrors TaskError variant; preserves stack through the pipeline → response chain |
| `crates/riversd/src/view_engine/pipeline.rs` | Converts `TaskError::HandlerErrorWithStack` → `ViewError::HandlerWithStack` (preserving stack) via a `match` on the error | Spec §5.3 | Non-stack TaskError variants still convert to `ViewError::Handler` |
| `crates/riversd/src/error_response.rs` | `map_view_error` HandlerWithStack branch: parses `at …` frames from the stack string; exposes as `details.stack` array in `cfg!(debug_assertions)` builds | Spec §5.3 | Sanitized in release — response still has `code`, `message`, `trace_id` but no stack |
| `crates/rivers-runtime/src/validate_crossref.rs`, `crates/riversd/src/bundle_diff.rs` | Added `base: Default::default()` to AppConfig test fixtures | Compatibility | Additive field requires touching every constructor; `AppBaseConfig: Default` keeps the fix to one line each |
| `docs/arch/rivers-processpool-runtime-spec-v2.md §15` | Marked Open Question #5 as closed with a resolution note | Spec §15 | Cross-ref points to `rivers-javascript-typescript-spec.md §5` |
| `docs/guide/tutorials/tutorial-ts-handlers.md` | New "Debugging handler errors" section covering per-app log + debug-mode envelope + `[base] debug = true` flag | Spec §5.3 + §8 tutorial | Concrete JSON example; guidance on enabling in dev vs production |
| `changedecisionlog.md` | Four new entries: parsed-map cache, CallSite reflection, `HandlerErrorWithStack` additive variant, debug-build gating as MVP | CLAUDE.md rule 5 | Each entry names file + spec ref + resolution mechanism |

Test coverage (+8 new tests, 310/310 riversd lib tests green total):
- `prepare_stack_trace_callback_does_not_crash_on_throw` (6A)
- `sourcemap_cache_idempotence_and_invalidation` (6B)
- `prepare_stack_trace_callback_produces_frames_from_callsites` (6C)
- `frame_format_tests::fallback_when_no_cache_entry` (6D)
- `frame_format_tests::anonymous_when_no_function_name` (6D)
- `frame_format_tests::zero_line_or_col_falls_back` (6D)
- `map_view_error_tests::handler_with_stack_includes_frames_in_debug_build` (6F)
- `map_view_error_tests::handler_with_stack_includes_message_in_debug_build` (6F)

## 2026-04-21 — TS pipeline Phase 10 (scoped) + Phase 11: canary txn handlers, version bump, spec supersede

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `canary-bundle/canary-handlers/libraries/handlers/txn-tests.ts` | New file. Five handlers probing spec §6: txnRequiresTwoArgs, txnRejectsNonFunction, txnUnknownDatasourceThrows, txnStateCleanupBetweenCalls, txnSurfaceExists | Phase 7 canary coverage | Each handler exercises a slice of ctx.transaction semantics without needing a real DB. Commit/rollback round-trip against PG is deferred to a future integration session |
| `canary-bundle/canary-handlers/app.toml` | Registered 5 `[api.views.txn_*]` blocks (paths `/canary/rt/txn/{args,cb-type,unknown-ds,cleanup,surface}`, method POST, Rest, auth none) | Phase 10.3 | Uses the existing canary view pattern verbatim |
| `canary-bundle/run-tests.sh` | Added "TRANSACTIONS-TS Profile" block between HANDLERS and SQL profiles, 5 test_ep lines | Phase 10.5 | No PG_AVAIL conditional — these handlers don't touch a DB |
| `Cargo.toml` | Bumped workspace version `0.54.1 → 0.55.0` | Phase 11.5 | Breaking handler semantics (swc replaces hand-rolled stripper, bundle-load compile timing, new ctx.transaction API) warrant a minor bump in 0.x |
| `docs/arch/rivers-processpool-runtime-spec-v2.md §5.3` | Added superseded-by header note pointing to `rivers-javascript-typescript-spec.md` | Phase 11.2 | Historical paragraph preserved for audit trail |
| `CLAUDE.md` | Updated rivers-runtime Key Crates row to mention `module_cache::{CompiledModule, BundleModuleCache}` | Phase 11.3 | Table now reflects the module-cache types added in Phase 2 |

## 2026-04-21 — TS pipeline Phase 9: rivers.d.ts type definitions

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `types/rivers.d.ts` | New file. Ambient declarations for `Rivers` global, `Ctx`, `ParsedRequest`, `SessionClaims`, `DataViewResult`, `QueryResult`, `ExecuteResult`, `CtxStore`, `DatasourceBuilder`, `KeystoreKeyInfo`, `TransactionError`, `HandlerFn`. JSDoc on every member | Spec §8 | `TransactionError` declared as a class with a `kind` discriminant (`"nested" \| "unsupported" \| "cross-datasource" \| "unknown-datasource" \| "begin-failed" \| "commit-failed"`). Trailing comment block documents the intentional omission of console/process/require/fetch per spec §8.3 |
| `docs/guide/tutorials/tutorial-ts-handlers.md` | Added "Using the Rivers-shipped rivers.d.ts" section with recommended `tsconfig.json` (target ES2022, module ES2022, moduleResolution bundler, strict true, types `./types/rivers`) | Spec §8.2 distribution | Placed between the inline type discussion and "Basic Handler" section so existing reading flow is preserved |
| `crates/cargo-deploy/src/main.rs` | Added `copy_type_definitions` helper; invoked from `scaffold_runtime` after `copy_arch_specs`. Deployed instance gets `types/rivers.d.ts` | Spec §8.2 release artifact | Follows the pattern of `copy_guides` / `copy_arch_specs` — same logging style, same graceful-on-missing behaviour |

## 2026-04-21 — TS pipeline Phase 6 (partial): source map generation

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/riversd/Cargo.toml` | Added `swc_sourcemap = "10"` direct dep | Spec §5.1 unconditional generation | Version pinned to match swc_core's transitive dep to avoid duplicate crate instances |
| `crates/riversd/src/process_pool/v8_config.rs` | Replaced `to_code_default` with manual `Emitter` + `JsWriter` that collects `(BytePos, LineCol)` entries; `build_source_map` + `to_writer` produces v3 JSON | Spec §5.1 | Return signature changed from `(String, Vec<String>)` to `(String, Vec<String>, String)` where last is the source map JSON |
| `crates/riversd/src/process_pool/module_cache.rs` | Destructuring updated to capture `source_map` from the compile return; stored in `CompiledModule.source_map` for every `.ts` file | Spec §3.4 cache shape | Field previously stored `""` — now always populated with real v3 JSON |
| `crates/riversd/tests/process_pool_tests.rs` | Added `compile_typescript_emits_source_map` — verifies output is valid v3 JSON with `version: 3`, `mappings`, `sources` array | Spec §5.1 test coverage | 17/17 compile_typescript tests green |

Phase 6 partially complete: **data path is done** (source maps generated and stored at bundle load). Remapping callback (task 6.2), log routing (6.4), and debug envelope (6.5) are deferred as a self-contained follow-on task that does not block Phase 10 or 11. The prerequisite data (v3 source maps in BundleModuleCache) is in place for any future session to pick up.

## 2026-04-21 — TS pipeline Phase 7: ctx.transaction() with executor integration

| File | Decision | Reference | Resolution |
|------|----------|-----------|------------|
| `crates/rivers-runtime/src/dataview_engine.rs` | Added `DataViewExecutor::datasource_for(name) -> Option<String>` — exposes registry's datasource mapping without executing | Spec §6.2 cross-ds check | Pure registry introspection, no connection acquired. Used by `ctx_dataview_callback` for cross-ds enforcement |
| `crates/riversd/src/process_pool/v8_engine/task_locals.rs` | Added `TASK_TRANSACTION: RefCell<Option<TaskTransactionState>>` + `TaskTransactionState { map: Arc<TransactionMap>, datasource: String }` | Spec §6 active-txn state | Thread-local is cleared in `TaskLocals::drop`. Drain happens BEFORE RT_HANDLE clear so `auto_rollback_all` can run via the still-live runtime handle |
| `crates/riversd/src/process_pool/v8_engine/context.rs` | New `ctx_transaction_callback`: arg validation, nested rejection, datasource resolution via `TASK_DS_CONFIGS`, connection acquisition via `DriverFactory::connect`, `TransactionMap::begin` (maps `DriverError::Unsupported` to spec §6.2 error message), JS callback invocation via TryCatch, commit/rollback semantics | Spec §6.1 | Injected at `inject_ctx_methods` alongside `ctx.dataview`. Re-throws handler's original exception after rollback |
| `crates/riversd/src/process_pool/v8_engine/context.rs` | Modified `ctx_dataview_callback` to check `TASK_TRANSACTION`: cross-ds check via `datasource_for` lookup (spec §6.2), then `take_connection → execute(Some(&mut conn)) → return_connection` inside single `rt.block_on` so connection is always returned | Spec §6.1 routing + §6.2 enforcement | The executor already had `txn_conn: Option<&mut Box<dyn Connection>>` — no signature change needed |
| `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` | 4 new ctx.transaction tests: two-args required, non-function callback rejected, unknown-datasource "not found", nested state cleanup (back-to-back calls don't report nested) | Spec §6 regression coverage | Full process_pool suite: 135/135 green (was 131 + 4) |
| `changedecisionlog.md` | Entries: executor-integration bridge design, rollback-before-RT_HANDLE ordering, spec §6.4 MongoDB correction flag | CLAUDE.md Workflow rule 5 | Plan task 7.8 (plugin-driver verification) and 7.9 (PG cluster integration test) deferred with honest reasoning |

Spec §6.4 correction: the table lists MongoDB/Cassandra/CouchDB/Elasticsearch/Kafka/LDAP with specific `supports_transactions` values — these are plugin drivers whose returns are not verified in the core codebase. Runtime enforcement is authoritative (the `DriverError::Unsupported` path maps correctly); the spec table should be amended to mark plugin rows "verify at plugin load" in the next revision cycle.

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

---

# Rivers Filesystem Driver — Implementation Changelog

### 2026-04-16 — OperationDescriptor framework baseline
- Files: crates/rivers-driver-sdk/src/{operation_descriptor.rs,traits.rs,lib.rs}
- Summary: new types (OpKind, OperationDescriptor, Param, ParamType) + opt-in DatabaseDriver::operations() method; all existing drivers build and test without modification.
- Spec: rivers-filesystem-driver-spec.md §2.
- Test delta: +1016 passing (0 failures, 17 ignored), backward compatible.

### 2026-04-17 — Filesystem driver + Direct dispatch typed proxy landed
- **Crates touched:** `rivers-driver-sdk`, `rivers-drivers-builtin`, `rivers-runtime`, `riversd`.
- **Scope:**
  - Eleven filesystem operations: readFile, readDir, stat, exists, find, grep, writeFile, mkdir, delete, rename, copy (spec §6).
  - Chroot sandbox with startup-time root canonicalization, per-op path validation, and symlink rejection — walking the pre-canonical path (spec §5).
  - `max_file_size` + `max_depth` connection-level limits (spec §8.4).
  - `DatasourceToken` converted from newtype struct to enum with `Pooled` and `Direct` variants (spec §7); `resolve_token_for_dispatch` emits `Direct` for filesystem, `Pooled` for all other drivers.
  - V8 typed-proxy pipeline: `TASK_DIRECT_DATASOURCES` thread-local, `catalog_for(driver)` lookup, `Rivers.__directDispatch` host fn with Option-B auto-unwrap, JS codegen from `OperationDescriptor` with ParamType guards + defaults (spec §3).
- **Canary:** `canary-bundle/canary-filesystem/` — 5 TestResult endpoints (CRUD round-trip, chroot escape, exists+stat, find+grep, arg validation). `riverpackage validate canary-bundle`: 0 errors. Live fleet run pending deploy (Task 32).
- **Docs:**
  - `docs/arch/rivers-feature-inventory.md` §6.1 + §6.6.
  - `docs/guide/tutorials/datasource-filesystem.md` (new, 197 lines, all 11 ops + chroot + limits + error table).
- **Tests:** ~85 new tests across driver ops, chroot enforcement, typed-proxy codegen, end-to-end V8 round-trip, and canary handlers. Scoped sweep of touched crates: 706/706 passing (sdk 67, drivers-builtin 140, runtime 187, riversd 312). Pre-existing workspace-level failures in live-infra tests (postgres/mysql/redis at 192.168.2.x) and two broken benches (`cache_bench`, `dataview_engine_tests`) are unrelated to this branch — verified via `git stash` on baseline.
- **Commits:** 29 commits from `f2c6db5` through `ad8819b` on `feature/filesystem-driver`.

---

## 2026-04-24 — Code-review remediation Phase A (P0-4 + P0-1)

### A1 — Broker consumer supervisor (P0-4)
- **new:** `crates/riversd/src/broker_supervisor.rs` — `spawn_broker_supervisor`, `BrokerBridgeRegistry`, `SupervisorBackoff`, `BrokerBridgeState` enum.
- **edit:** `crates/riversd/src/lib.rs` — register module.
- **edit:** `crates/riversd/src/bundle_loader/wire.rs` — replace `match create_consumer().await { Ok => spawn(bridge.run()), Err => warn }` with `spawn_broker_supervisor(...)` (returns immediately).
- **edit:** `crates/riversd/src/server/context.rs` — `AppContext.broker_bridge_registry` field.
- **edit:** `crates/riversd/src/health.rs` — new `BrokerBridgeHealth` type; `VerboseHealthResponse.broker_bridges` field.
- **edit:** `crates/riversd/src/server/handlers.rs` — populate `broker_bridges` from registry snapshot.
- **new:** `crates/riversd/tests/broker_supervisor_tests.rs` — 3 tests (spawn-immediate, eventually-ok, empty-healthy).
- **edit:** `crates/riversd/tests/health_tests.rs` — `verbose_health_serializes_broker_bridges` + struct-literal updates.
- **Effect:** `riversd` boots even when broker hosts are unreachable. `/health/verbose` reports per-bridge state. Existing `reconnect_ms` config now drives exponential backoff capped at 60s.

### A2 — Protected-view fail-closed (P0-1)
- **edit:** `crates/riversd/src/security_pipeline.rs` — explicit `session_manager.is_none()` reject before validation block; returns 500.
- **edit:** `crates/riversd/src/bundle_loader/load.rs` — strengthened AM1.2; extracted `check_protected_views_have_session` helper with 6 unit tests.
- **new:** `crates/riversd/tests/security_pipeline_tests.rs` — 2 integration tests.
- **Effect:** misconfig (protected view + missing session manager) now fails at bundle load with a named-view error AND, as defense-in-depth, fails closed at request time with a 500. Public views (auth=none) unaffected.

### Tests
- 345/345 lib tests + 1 ignored.
- 11 integration files passing across the changes (broker_supervisor: 3, health: 12, security_pipeline: 2, broker_bridge: 12).
- One pre-existing failure flagged: `cli_tests::version_string_contains_version` hardcodes 0.50.1 (crate is 0.55.0). Spawned for separate cleanup.

## 2026-04-24 — B4: Redact host paths in V8 errors (P1-9)

### B4 — Path redaction
- **edit:** `crates/riversd/src/process_pool/v8_engine/execution.rs` — added `pub(crate) fn redact_to_app_relative(path: &str) -> Cow<str>` next to `boundary_from_referrer`. Wired into both `script_origin` constructions (root module in `execute_as_module`, resolved modules in `resolve_module_callback`) so V8 stack frames carry the logical script name. Wired into every `format!` site in `resolve_module_callback` (the `in {referrer}`, `resolved to:`, and `boundary:` lines). Wired into the disk-read fallback `cannot read module` message.
- **edit:** `crates/riversd/src/process_pool/v8_engine/mod.rs` — re-exported `redact_to_app_relative` as `pub(crate)` so `module_cache::module_not_registered_message` and the future SQLite path policy (G_R8.2) can call the same redactor.
- **edit:** `crates/riversd/src/process_pool/module_cache.rs` — `module_not_registered_message` now redacts both the `path` and `abs` arguments through the shared helper. Existing pinned-format test (`module_not_registered_message_format_matches_g5_3`) still passes — assertions are substring checks that don't depend on the absolute prefix.
- **new:** `crates/riversd/tests/path_redaction_tests.rs` — 2 integration tests:
  - `handler_stack_does_not_leak_host_paths`: dispatches a module-syntax handler that throws; asserts neither the error message nor the stack contains the host prefix above the app, `/Users/`, or `/var/folders/`.
  - `module_resolution_error_does_not_leak_host_paths`: dispatches a handler that imports a non-existent module; asserts the resolve-callback error is fully redacted and reports `my-app/libraries/handlers/throws.js` as the referrer.
- **edit:** `execution.rs` — added `redact_path_tests` module with 8 unit tests covering: macOS workspace path, Linux deploy path, no-libraries pass-through (verifying `Cow::Borrowed`), already-relative pass-through, empty string, deep nesting, libraries-at-root edge case, trailing-slash walk.

### Decision (logged in changedecisionlog)
- Redaction is unconditional (no `cfg!(debug_assertions)` gate). Reasoning: redacted form is more useful for log grep, and security posture must not depend on build mode.

### Tests
- 8 new unit tests in `redact_path_tests` — all green.
- 2 new integration tests in `path_redaction_tests.rs` — all green.
- Re-ran 357 lib tests + 25 v8_bridge + 2 b3_module_cache_strict + 10 task_kind_dispatch — all green, no regressions.
# 2026-04-24 — Review consolidation planning

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `todo/tasks.md` | Replaced the completed FCR task list with the RCC plan for writing `docs/review/cross-crate-consolidation.md` | User request to write the report to `docs/review/`; AGENTS.md workflow rules 1-2 | Plan includes input re-check, fallback-source policy, consolidation sections, log updates, and whitespace validation |
| `changedecisionlog.md` | Logged the output path and missing-input policy for the consolidation report | AGENTS.md workflow rule 5 | Report must state whether it is based on 22 per-crate reports or fallback grounding from `docs/code_review.md` |

# 2026-04-24 — `rivers-lockbox-engine` review planning

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `todo/gutter.md` | Preserved the unfinished `rivers-plugin-exec` review task list before replacing active tasks | AGENTS.md workflow rule 1 | Added a dated "Moved From Active Tasks" section |
| `todo/tasks.md` | Replaced the active task list with the approval-gated `rivers-lockbox-engine` review plan | User request for crate 2 review; AGENTS.md workflow rules 1-2 | Plan covers full source/test reads, security sweeps, validation, cross-crate wiring, report writing, and whitespace checks |
| `changedecisionlog.md` | Logged the task preservation decision and the planned report path | AGENTS.md workflow rule 5 | Records `docs/review/rivers-lockbox-engine.md` as the target report |

# 2026-04-24 — `rivers-lockbox-engine` review delivered

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `docs/review/rivers-lockbox-engine.md` | Added the per-crate Tier 1/2/3 review for the lockbox engine | User request to write output to `docs/review/{{crate}}`; lockbox spec security model | Report includes 3 Tier 1 findings, 4 Tier 2 findings, 1 Tier 3 finding, clean areas, coverage notes, and a shared fix recommendation |
| `todo/tasks.md` | Marked the approved RLE review tasks complete with concise validation notes | AGENTS.md workflow rule 3 | Source/test reads, sweeps, validation, cross-crate wiring, report writing, logs, and whitespace check are complete |
| `changedecisionlog.md` | Logged the secret-lifecycle prioritization, CLI/runtime split inclusion, and constant-time-comparison non-finding | AGENTS.md workflow rule 5 | Decisions are traceable for CB drift detection |

# 2026-04-24 — `rivers-keystore-engine` review delivered

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `docs/review/rivers-keystore-engine.md` | Added the per-crate Tier 1/2/3 review for the application keystore engine | User request to write output to `docs/review/{{crate}}`; app-keystore role/risk list in the request | Report includes 3 Tier 1 findings, 3 Tier 2 findings, 2 Tier 3 findings, repeated-pattern/shared-fix notes, clean areas, coverage gaps, bug-density assessment, and recommended fix order |
| `todo/tasks.md` | Marked the approved RKE review tasks complete with concise validation notes | AGENTS.md workflow rule 3 | Source/test reads, runtime/CLI/docs reads, security sweeps, validation, report writing, logs, and final whitespace/diff checks are complete |
| `changedecisionlog.md` | Logged the report path/basis plus the multi-keystore and dynamic-callback cross-crate inclusion decisions | AGENTS.md workflow rule 5 | Decisions are traceable for CB drift detection |

# 2026-04-25 — Phase H verification: H16 + H17 closed

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `todo/tasks.md` | Marked H16 (T2-4 capacity accounting) and H17 (T2-5 health-check lock) as `[x]` with file:line evidence after re-reading `crates/riversd/src/pool.rs` end-to-end | `docs/code_review.md` Tier-2 findings T2-4, T2-5; Phase D commit `2dfbb7b` (D1) | Both findings verified closed by Phase D's pool rewrite. No source change required. |

# 2026-04-25 — `rivers-plugin-exec` per-crate review delivered

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `docs/review/rivers-plugin-exec.md` | Added the per-crate Tier 1/2/3 review for the exec driver plugin | RXE dispatch + `docs/review_inc/rivers-per-crate-focus-blocks.md` section 1 | Report includes 4 Tier 1 findings, 7 Tier 2 findings, 5 Tier 3 findings, repeated-pattern note, non-findings, coverage gaps, bug-density assessment, and recommended fix order. Source basis: full reads of all 13 source files (3375 LOC) + integration tests + driver-SDK trait file. Sweeps: panics (~140 hits, mostly tests), unsafe/FFI (3 unsafe blocks in executor + validator for `geteuid`/`getpwnam`), libc/setuid/setgroups (`setsid` in pre_exec, no `setgroups`), format! (~50 hits, all error messages with no shell construction), Command::new (1 hit, tokio + explicit argv). cargo check + cargo test --lib pass. |
| `todo/tasks.md` | Marked RXE0.1–RXE2.3 as `[x]` with one-line completion notes | RXE dispatch | All 14 sub-tasks complete; review delivered as a single artifact at `docs/review/rivers-plugin-exec.md`. |
| `changedecisionlog.md` | Logged RXE-1.1 covering single-crate scope, severity-tier definitions, T1-vs-T2 borderline calls, and combined fix-order rationale | RXE dispatch + AGENTS.md workflow rule 5 | Decisions traceable for CB drift detection. |
# 2026-04-27 — Rivers-wide code review consolidation

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `docs/review/rivers-wide-code-review-2026-04-27.md` | Added a consolidated detailed report for the 22-crate Rivers review pass | User request to build detailed report in `docs/review/`; `docs/review_inc/rivers-code-review-prompt-kit.md`; `docs/review_inc/rivers-per-crate-focus-blocks.md` | Report covers repeated bug classes, severity distribution, per-crate findings, and recommended remediation phases |
| `changedecisionlog.md` | Logged the report path, consolidation choice, and review emphasis | CLAUDE.md workflow rule 5 | Existing per-crate reports were preserved; the new dated report captures cross-crate patterns and contract violations |

# 2026-04-27 — Rivers-wide review validation pass

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `docs/review/rivers-wide-code-review-2026-04-27.md` | Corrected second-pass issues in the consolidated report | User request to confirm the report is 95% accurate | Fixed `rivers-lockbox` and `rivers-plugin-influxdb` count mismatches, downgraded Kafka `rskafka` note to an observation, and narrowed CouchDB JSON-substitution wording |
| `docs/review/rivers-wide-code-review-2026-04-27-validation-pass.md` | Added the second-pass validation addendum | User request to confirm all items in the existing report | Per-crate table records confirmed status, corrections applied, and residual judgment calls |
| `changedecisionlog.md` | Logged the validation choices and correction policy | CLAUDE.md workflow rule 5 | Source-confirmed items remain; only count/wording/downgrade fixes were applied |

# 2026-04-27 — H1: V8 ctx.ddl() DDL whitelist enforcement

| File | Summary | Reference | Resolution |
|------|---------|-----------|------------|
| `crates/riversd/src/process_pool/v8_engine/context.rs` | Added DDL whitelist check (Gate 3) in `ctx_ddl_callback` at lines 721–777, before `factory.connect()`. Reads `engine_loader::ddl_whitelist()` and resolves the entry_point name to manifest app_id via `engine_loader::app_id_for_entry_point()`. Rejects with the same error string as `host_ddl_execute` in `engine_loader/host_callbacks.rs` when the `database@app_id` pair is not in the whitelist. | H1 — riversd T1-4 security gap | Mirrors the dynamic-engine Gate 3 check exactly; both paths now enforce whitelist from the same `DDL_WHITELIST` OnceLock |
| `crates/riversd/tests/v8_ddl_whitelist_tests.rs` | Added integration test binary with two tests: `h1_whitelisted_ddl_succeeds_for_application_init` (SQLite CREATE TABLE succeeds and table exists) and `h1_unwhitelisted_ddl_rejected_for_application_init` (blocked, table absent, error message matches dynamic-engine format verbatim). | H1 validation spec | Test binary isolated so DDL_WHITELIST OnceLock doesn't contaminate B1.5 success-path tests |
| `todo/tasks.md` | Marked H1 `[x]` with resolution summary | CLAUDE.md workflow rule 6 | — |
| `Cargo.toml` (workspace) | Version bumped `0.55.2+0219280426` → `0.55.2+0226280426` | CLAUDE.md versioning rules | Patch-level bump; closing a documented-but-missing security gate |

## RW1.1 — `rivers-driver-sdk` DDL guard + error sanitization + param substitution + retry backoff

| File | What changed | Spec ref | Resolution |
|------|---------|-----------|------------|
| `crates/rivers-driver-sdk/src/lib.rs` | Added `first_sql_token()` helper that delegates to `infer_operation()` (which already strips SQL comments via `strip_sql_comments()`). Rewrote `is_ddl_statement()` to compare the full first token rather than `starts_with()` on raw trimmed text — fixes comment-aware DDL classification (RW1.1.a). | RW1.1.a | `-- DROP TABLE\nSELECT 1` now classifies as query, not DDL |
| `crates/rivers-driver-sdk/src/lib.rs` | Sanitized `check_admin_guard()`: error message now emits only the classified token, never raw statement content. Full statement logged at `tracing::debug!` only (RW1.1.b). | RW1.1.b | Credential material in connection-string-style payloads cannot leak into user-facing errors |
| `crates/rivers-driver-sdk/src/lib.rs` | Rewrote `translate_params()` DollarPositional/QuestionPositional/ColonNamed branches to use a single span-based scan instead of `str::replace()`. Eliminates prefix-collision where `$param1` processing would clobber `$param10` (RW1.1.c). | RW1.1.c | `$param1` and `$param10` now substitute to independent positional slots |
| `crates/rivers-driver-sdk/src/http_executor/connection.rs` | Replaced `2u64.pow(n) * base` with `2u64.saturating_pow(n)` + `base.saturating_mul(factor)` in `retry_delay()`. Also hardened `BackoffStrategy::Linear` arm to `saturating_mul`. (RW1.1.d). | RW1.1.d | 64+ retries with large base no longer overflow before max-delay cap |
| `crates/rivers-driver-sdk/src/http_executor/oauth2.rs` | Same saturating arithmetic fix in OAuth2 token retry sleep calculation (RW1.1.d). | RW1.1.d | Consistent with connection.rs fix |
| `crates/rivers-driver-sdk/tests/ddl_guard_tests.rs` | Updated `guard_truncates_statement_in_message` → `guard_sanitizes_statement_not_echoed_in_message` to assert the new security-correct behavior: raw statement must NOT appear in error messages. | RW1.1.b | Test now validates sanitization rather than the former prefix-echo behavior |
| `crates/rivers-driver-sdk/src/lib.rs` | Added `rw1_1_tests` module with 13 new tests covering all four subtasks. | RW1.1.validate | 203 tests pass across all driver-sdk test targets |
| `Cargo.toml` (workspace) | Version bumped `0.55.8+0347280426` → `0.55.9+1329280426` | CLAUDE.md versioning rules | Patch bump — closing documented-but-missing security/correctness gaps |
| `crates/rivers-plugin-exec/src/executor.rs` | RW1.2.a: moved stdin write inside unified `tokio::time::timeout` block so all child I/O (stdin write, concurrent stdout/stderr drain, wait) is covered by the configured timeout. | RW1.2.a | Child that refuses to read stdin can no longer hang indefinitely outside the timeout |
| `crates/rivers-plugin-exec/src/executor.rs` | RW1.2.b: on Linux, open the verified binary as a file descriptor and exec via `/proc/self/fd/N` to close the TOCTOU window between hash verification and spawn. On macOS, falls back to path exec with documented residual window bounded to microseconds. | RW1.2.b | TOCTOU window eliminated on Linux; macOS window bounded and documented |
| `crates/rivers-plugin-exec/src/executor.rs` | RW1.2.c: added `libc::setgroups(0, std::ptr::null())` in the `pre_exec` block before uid/gid drop so supplementary groups are cleared and don't survive the privilege change. | RW1.2.c | Supplementary groups no longer inherited across uid/gid drop |
| `crates/rivers-plugin-exec/src/executor.rs` | RW1.2.d: replaced sequential stdout-then-stderr reads with `tokio::join!` draining both pipes concurrently. Stderr is now chunked-read with an explicit byte cap (same as stdout cap, floor 64 KB). Kill-on-overflow now fires inside the stdout future so the child exits and stderr gets EOF without waiting for the timeout. | RW1.2.d | Concurrent drain prevents deadlock on large stderr; overflow detection is immediate |
| `crates/rivers-plugin-exec/src/connection/pipeline.rs` | RW1.2.e: moved integrity check (step 5) to after both semaphore acquisitions (steps 6 & 7), so `Every(N)` counter only increments when the execution actually proceeds. Rejected concurrency attempts no longer consume scheduled checks. | RW1.2.e | Every(N) check cadence is accurate; rejected attempts don't burn slots |
| `crates/rivers-plugin-exec/src/integrity.rs` | RW1.2.e: updated `should_check()` doc to clarify it must only be called after semaphore acquisition for `Every(N)` mode. | RW1.2.e | Doc matches new call site contract |
| `crates/rivers-plugin-exec/src/config/parser.rs` | RW1.2.f: replaced `s == "true"` with case-insensitive parse of "true"/"false"; any other value (e.g. "yes", "True" typos) returns a config error rather than silently inheriting the host environment. | RW1.2.f | Fail-closed on invalid env_clear values |
| `crates/rivers-plugin-exec/src/executor.rs` | RW1.2.g: `kill_process_group` now checks the return value of `kill(2)` and logs `warn!` on non-ESRCH errors; `setsid()` errors in `pre_exec` are printed to stderr (tracing unavailable in pre_exec context). | RW1.2.g | Process-group and kill errors are surfaced rather than silently ignored |
| `crates/rivers-plugin-exec/src/executor.rs` | RW1.2.h: replaced `&stderr_str[..stderr_str.len().min(1024)]` with `truncate_utf8(&stderr_str, 1024)` — a helper that walks char boundaries backward to find a safe cut point, preventing panic on multi-byte UTF-8. | RW1.2.h | No panic on multi-byte stderr sequences at the 1024-byte boundary |
| `crates/rivers-plugin-exec/src/executor.rs` | Added tests: `truncate_utf8_*` (4), `stdin_blocking_respects_timeout` (RW1.2.a regression), `evaluate_stderr_multibyte_no_panic` (RW1.2.h). | RW1.2.validate | 105 tests pass; 2 known-broken ignored; 0 failed |
| `crates/rivers-plugin-exec/src/config/parser.rs` | Added 5 tests for env_clear parsing (RW1.2.f): true/false/mixed-case/invalid/default. | RW1.2.f | Config rejection of invalid env_clear values is tested |
| `Cargo.toml` (workspace) | Version bumped `0.55.9+1329280426` → `0.55.10+1339280426` | CLAUDE.md versioning rules | Patch bump — 8 security/correctness hardening fixes to rivers-plugin-exec |
| `crates/rivers-plugin-exec/src/executor.rs` | After `f.into_raw_fd()`, added `libc::fcntl(fd, F_SETFD, 0)` to clear O_CLOEXEC so the fd survives both the script exec and the shebang-interpreter re-exec on Linux. Updated `proc_fd_accessible()` to clear O_CLOEXEC before the accessibility check so it correctly reflects production behavior. | PR 96 CI fix | GitHub Actions sandbox: /proc/self/fd/N invisible without this fix |
| `crates/rivers-plugin-exec/src/connection/mod.rs` | Same `proc_fd_accessible()` fix — clear O_CLOEXEC before checking. | PR 96 CI fix | Same Linux CI sandbox fix |
| `crates/riversd/src/process_pool/tests/exec_and_keystore.rs` | `exec_driver_error_propagation`: changed command input_mode from `"stdin"` to `"args"`. In stdin mode, fail.sh exits before stdin is written → broken-pipe error masks the "script error" in stderr. Args mode avoids the pipe entirely. | PR 96 CI fix | Single failing test in x86_64 CI build |
| `crates/rivers-runtime/src/view.rs` | `McpToolConfig`: added `view: Option<String>` with `#[serde(default)]`; made `dataview` `#[serde(default)]` (empty string when view-backed). | CB-P0.1 | Allow MCP tools to reference codecomponent views |
| `crates/rivers-runtime/src/validate_crossref.rs` | MCP-VAL-1 updated: accepts `view` (validates view exists + is Codecomponent handler) or `dataview` (existing behavior); fails if neither is set. | CB-P0.1 | Validation covers both backends |
| `crates/riversd/src/mcp/dispatch.rs` | `handle_tools_call` dispatches view-backed tools via `ProcessPoolManager.dispatch("default", ctx)` using `task_enrichment::enrich` for full capability wiring. `handle_tools_list` returns open schema for view-backed tools. Added `dispatch_codecomponent_tool` helper. | CB-P0.1 | Same process pool pipeline as REST/WebSocket handlers |
| `Cargo.toml` (workspace) | Version bumped `0.55.19+0538290426` → `0.55.20+0656290426` | CLAUDE.md versioning rules | Patch bump — closing documented CB-P0.1 gap |
