# Tasks — Unit Test Infrastructure

> **Branch:** `test-coverage`
> **Source:** `docs/bugs/rivers-unit-test-spec.md` + `rivers-unit-test-amd1.md` + `docs/reports/test-coverage-audit.md`
> **Goal:** Implement test infrastructure from spec, covering 33/38 bugs + feature inventory gaps
> **Current:** 1,940 tests across 27 crates. 0/13 critical bugs had unit tests before discovery.
>
> **Critical gaps (0 tests):** DataView engine, Tiered cache, Schema validation, V8 bridge contracts, V8 security, Config validation, Boot parity

---

## Phase 1 — Test Harness Foundation

These create the shared infrastructure that all later tests depend on.

### 1.1 — Add `test-case` crate to workspace dependencies ✅
### 1.2 — Create driver conformance test harness ✅ (19 tests)
### 1.3 — V8 bridge test harness ✅ (via v8_bridge_tests.rs — uses ProcessPoolManager dispatch, not TestIsolate)

---

## Phase 2 — Driver Conformance Matrix (Strategy 1) ✅

19 tests implemented in `conformance_tests.rs`:
- DDL guard: 12 tests (8 SQLite + 4 cluster) — BUG-001 ✅
- CRUD lifecycle: 3 tests (1 SQLite + 2 cluster) ✅
- Param binding: 4 tests (2 SQLite + 2 cluster) — BUG-004 ✅

Remaining (cluster-only, deferred until podman available):
- [ ] Admin guard tests (redis, mongodb, elasticsearch)
- [ ] NULL handling round-trip
- [ ] max_rows truncation

---

## Phase 3 — V8 Bridge Contract Tests (Strategy 2) ✅

21 tests implemented in `v8_bridge_tests.rs`:
- ctx.* injection: trace_id, app_id (UUID not slug), node_id, env, resdata ✅
- ctx.request: all fields, query field name (BUG-012), ghost field rejection ✅
- Rivers.*: log, crypto (random, hash, hmac, timing-safe), ghost API detection ✅
- Console: delegates to Rivers.log ✅
- V8 security: codegen blocked (BUG-003), timeout (BUG-002), heap (BUG-006) ✅
- ctx.store: set/get/del round-trip, reserved prefix rejection ✅

Remaining (need TestIsolate for mock dataview capture):
- [ ] ctx.dataview() param forwarding with capture (BUG-008)
- [ ] ctx.dataview() namespace resolution with capture (BUG-009)
- [ ] Store TTL type validation (BUG-021)

---

## Phase 4 — AMD-1 Additions (Boot Parity + Module Resolution) ✅

4 tests in `boot_parity_tests.rs`:
- no_ssl_path_has_all_subsystem_init_calls (BUG-005 regression) ✅
- tls_path_has_all_subsystem_init_calls (sanity check) ✅
- module_path_resolution_exists_in_bundle_loader (BUG-013) ✅
- storage_engine_config_has_memory_default ✅

---

## Phase 5 — Regression Gate + Console Fix

### 5.1 — V8 regression tests
- [ ] `v8_regression.rs` — `regression_pr48_dataview_params_not_dropped`
- [ ] `v8_regression.rs` — `regression_app_id_not_empty`
- [ ] Update `console_not_available` test → `console_delegates_to_rivers_log` (Rivers provides console intentionally)

### 5.2 — Middleware/dispatch tests
- [ ] `error_sanitization.rs` — verify error responses don't contain driver names, IPs, file paths
- [ ] `security_headers.rs` — verify HSTS, X-Content-Type-Options, X-Frame-Options present
- [ ] `view_dispatch.rs` — `ctx.app_id` populated correctly from manifest UUID

---

## Phase 6 — Feature Inventory Gaps (0-test areas)

These features from `rivers-feature-inventory.md` have zero or near-zero test coverage.

### 6.1 — DataView engine tests (Feature 3.1 — 0 tests)
- [ ] `crates/rivers-runtime/tests/dataview_engine_tests.rs`
  - DataView execution with faker datasource (no cluster needed)
  - Parameter passing through DataView to driver
  - DataView registry lookup (namespaced keys)
  - max_rows truncation at engine level
  - `invalidates` list triggers cache clear on write
  - Operation inference from SQL first token (SHAPE-7)

### 6.2 — Tiered cache tests (Feature 3.3 — 0 tests)
- [ ] `crates/rivers-runtime/tests/cache_tests.rs`
  - L1 LRU eviction when memory limit exceeded
  - L1 returns `Arc<QueryResult>` (pointer, not clone)
  - L1 entry count safety valve (100K)
  - L2 skip when result exceeds `l2_max_value_bytes`
  - Cache key derivation: BTreeMap → serde_json → SHA-256 → hex (SHAPE-3)
  - Cache invalidation by view name
  - `NoopDataViewCache` fallback when unconfigured

### 6.3 — Schema validation chain tests (Feature 4.1-4.8 — 0 tests)
- [ ] `crates/rivers-driver-sdk/tests/schema_validation_tests.rs`
  - SchemaSyntaxChecker: valid schema accepted
  - SchemaSyntaxChecker: missing required fields rejected
  - SchemaSyntaxChecker: invalid types rejected
  - Validator: type mismatch caught at request time
  - Validator: missing required field caught
  - Validator: constraint violations (min/max/pattern)
  - Per-driver validation: Redis schema vs Postgres schema different shapes

### 6.4 — Config validation tests (Feature 17 — 5 tests)
- [ ] `crates/rivers-core-config/tests/config_validation_tests.rs`
  - Environment variable substitution `${VAR}`
  - All validation rules from spec table (feature inventory §17.4)
  - Invalid TOML rejected with clear errors
  - Missing required sections caught
  - DDL whitelist format validation
  - Session cookie validation (http_only enforcement)

### 6.5 — Security headers tests (Feature 1.5 — 1 test)
- [ ] `crates/riversd/tests/security_headers_tests.rs`
  - X-Content-Type-Options: nosniff present
  - X-Frame-Options: DENY present
  - X-XSS-Protection present
  - Referrer-Policy present
  - Vary: Origin on CORS responses
  - Handler header blocklist: Set-Cookie, access-control-*, host silently dropped

### 6.6 — Pipeline stage isolation tests (Feature 2.2)
- [ ] `crates/riversd/tests/pipeline_tests.rs`
  - pre_process fires before DataView execution
  - handlers fire after DataView, can modify ctx.resdata
  - post_process fires after handlers, side-effect only
  - on_error fires on any stage failure
  - Sequential execution order (SHAPE-12)

### 6.7 — Cross-app session propagation tests (Feature 7.5 — 0 tests)
- [ ] `crates/riversd/tests/session_propagation_tests.rs`
  - Authorization header forwarded from app-main to app-service
  - X-Rivers-Claims header carries claims
  - Session scope preserved across app boundaries

---

## Validation

After all phases:
- [ ] `cargo test -p rivers-drivers-builtin` — conformance matrix (SQLite without cluster)
- [ ] `cargo test -p riversd` — bridge, boot, bundle, regression tests
- [ ] `RIVERS_TEST_CLUSTER=1 cargo test -p rivers-drivers-builtin` — full cluster tests (when available)
- [ ] All 33 bug-sourced tests mapped in coverage table
