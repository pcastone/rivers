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

### 1.1 — Add `test-case` crate to workspace dependencies
- [ ] Add `test-case = "3"` to `[workspace.dependencies]` in root Cargo.toml
- [ ] Add `test-case = { workspace = true }` to `rivers-drivers-builtin/Cargo.toml` `[dev-dependencies]`
- [ ] Verify: `cargo test -p rivers-drivers-builtin --no-run`

### 1.2 — Create driver conformance test harness
- [ ] Create `crates/rivers-drivers-builtin/tests/conformance/mod.rs`
  - `skip_unless_cluster()` guard
  - `test_connection_params()` for all drivers (postgres, mysql, sqlite, redis, mongodb, elasticsearch, couchdb, cassandra, ldap)
  - `make_connection()` async factory
  - `ordered_params()` helper
  - SQL DDL/DML helper functions (`setup_test_table`, `make_insert_query`, `make_select_by_zname_query`, `make_update_query`, `make_delete_query`, `cleanup_test_row`)
  - NoSQL helpers (`make_write_query`, `make_read_query`)
- [ ] Verify: `cargo test -p rivers-drivers-builtin --test conformance -- --list`

### 1.3 — Create V8 bridge test isolate factory
- [ ] Create `crates/riversd/tests/bridge/mod.rs` (TestIsolate struct)
  - `TestIsolate::new()` — creates V8 isolate with production injection paths, mock backends
  - `.with_ctx(app_id, trace_id, node_id, env)` — inject metadata
  - `.with_request(json)` — inject mock HTTP request
  - `.with_session(claims)` — inject mock session
  - `.with_dataview_capture()` — mock dataview that records calls
  - `.with_entry_point(name)` — set DV namespace (AMD-1.2)
  - `.with_store()` — mock store with namespace enforcement
  - `.eval(js) -> String` — run JS, return result
  - `.eval_json(js) -> Value` — run JS, parse JSON result
  - `.dataview_calls() -> Vec<(String, Value)>` — get captured calls
- [ ] Verify: `cargo test -p riversd --test bridge -- --list`

---

## Phase 2 — Driver Conformance Matrix (Strategy 1)

Each test uses `#[test_case]` to run against multiple drivers. SQLite runs without cluster; others need `RIVERS_TEST_CLUSTER=1`.

### 2.1 — Parameter binding tests (BUG-004, Issue #54)
- [ ] `param_binding.rs` — `param_binding_order_independent` (postgres, mysql, sqlite, cassandra)
- [ ] `param_binding.rs` — `param_binding_same_prefix` (postgres, mysql, sqlite)
- [ ] `param_binding.rs` — `param_binding_empty_params` (postgres, mysql, sqlite, redis)
- [ ] Verify: SQLite tests pass locally without cluster

### 2.2 — DDL guard tests (BUG-001)
- [ ] `ddl_guard.rs` — `ddl_rejected_on_execute` (postgres DROP/CREATE/ALTER/TRUNCATE, mysql DROP/CREATE, sqlite DROP/CREATE)
- [ ] `ddl_guard.rs` — `ddl_detection_edge_cases` (whitespace, comments, case variants — SQLite only, no cluster)
- [ ] Verify: all DDL variants return `DriverError::Forbidden`

### 2.3 — Admin operation guard tests (BUG-001)
- [ ] `admin_guard.rs` — `admin_op_rejected` (redis FLUSHDB/FLUSHALL/CONFIG SET, mongodb drop_collection/create_index, elasticsearch delete_index)
- [ ] Verify: all admin ops return `DriverError::Forbidden`

### 2.4 — CRUD lifecycle tests
- [ ] `crud_lifecycle.rs` — `full_crud_lifecycle` (postgres, mysql, sqlite)
- [ ] `crud_lifecycle.rs` — `nosql_write_read_cycle` (redis, mongodb, elasticsearch, couchdb)
- [ ] Verify: insert → select → update → select → delete → select cycle

### 2.5 — NULL and type coercion tests
- [ ] `null_handling.rs` — `null_value_round_trip` (postgres, mysql, sqlite)
- [ ] Verify: NULL survives round-trip, not coerced to "" or 0

### 2.6 — max_rows truncation tests (BUG-017)
- [ ] `max_rows.rs` — `result_truncated_at_max_rows` (postgres, mysql, sqlite)
- [ ] Verify: LIMIT clause respected

---

## Phase 3 — V8 Bridge Contract Tests (Strategy 2)

No server, no cluster. Pure V8 isolate with mock backends.

### 3.1 — ctx.* injection tests
- [ ] `ctx_injection.rs` — `ctx_trace_id_injected`
- [ ] `ctx_injection.rs` — `ctx_app_id_injected_and_not_empty`
- [ ] `ctx_injection.rs` — `ctx_node_id_injected`
- [ ] `ctx_injection.rs` — `ctx_env_injected`
- [ ] `ctx_injection.rs` — `ctx_session_is_object_with_claims`
- [ ] `ctx_injection.rs` — `ctx_session_undefined_when_no_session`
- [ ] `ctx_injection.rs` — `ctx_request_has_all_fields`
- [ ] `ctx_injection.rs` — `ctx_resdata_writable_and_becomes_response`
- [ ] AMD-1.3: `ctx_injection.rs` — `regression_bug012_request_field_names_match_spec`
- [ ] AMD-1.3: `ctx_injection.rs` — `request_object_has_all_spec_fields`
- [ ] AMD-1.3: `ctx_injection.rs` — `request_object_no_alias_fields` (ghost field detection)
- [ ] AMD-1.4: `ctx_injection.rs` — `regression_bug010_app_id_is_uuid_not_slug`

### 3.2 — ctx.dataview() bridge tests (BUG-008, BUG-009)
- [ ] `dataview_bridge.rs` — `dataview_params_not_dropped` (regression PR #48)
- [ ] `dataview_bridge.rs` — `dataview_params_type_fidelity`
- [ ] `dataview_bridge.rs` — `dataview_empty_params`
- [ ] `dataview_bridge.rs` — `dataview_no_params_arg`
- [ ] AMD-1.2: `dataview_bridge.rs` — `regression_bug009_dataview_name_namespaced`
- [ ] AMD-1.2: `dataview_bridge.rs` — `dataview_already_namespaced_not_double_prefixed`

### 3.3 — ctx.store namespace tests (BUG-015, BUG-021)
- [ ] `store_bridge.rs` — `store_get_set_del_roundtrip`
- [ ] `store_bridge.rs` — `store_rejects_session_namespace`
- [ ] `store_bridge.rs` — `store_rejects_csrf_namespace`
- [ ] `store_bridge.rs` — `store_rejects_cache_namespace`
- [ ] `store_bridge.rs` — `store_rejects_all_reserved_prefixes`
- [ ] AMD-1.6: `store_bridge.rs` — `regression_bug021_store_ttl_accepts_number`
- [ ] AMD-1.6: `store_bridge.rs` — `store_ttl_rejects_object`
- [ ] AMD-1.6: `store_bridge.rs` — `store_ttl_zero_or_negative_behavior`

### 3.4 — Rivers.* API tests
- [ ] `rivers_api.rs` — `rivers_log_exists_and_callable`
- [ ] `rivers_api.rs` — `rivers_crypto_random_hex_returns_correct_length`
- [ ] `rivers_api.rs` — `rivers_crypto_random_hex_not_deterministic`
- [ ] `rivers_api.rs` — `rivers_crypto_hash_password_and_verify`
- [ ] `rivers_api.rs` — `rivers_crypto_hmac_deterministic`
- [ ] `rivers_api.rs` — `all_spec_rivers_apis_exist` (ghost API detection)
- [ ] `rivers_api.rs` — `all_spec_ctx_methods_exist` (ghost API detection)

### 3.5 — V8 security tests (BUG-002, BUG-003, BUG-006, BUG-007)
- [ ] `security/timeout.rs` — `infinite_loop_terminates_within_timeout`
- [ ] `security/codegen_blocked.rs` — `eval_is_blocked`
- [ ] `security/codegen_blocked.rs` — `function_constructor_is_blocked`
- [ ] `security/heap_limit.rs` — `massive_allocation_does_not_crash_process`
- [ ] `security/timing_safe.rs` — `timing_safe_equal_returns_true_for_equal`
- [ ] `security/timing_safe.rs` — `timing_safe_equal_returns_false_for_unequal`
- [ ] `security/timing_safe.rs` — `timing_safe_equal_returns_false_for_different_length`

---

## Phase 4 — AMD-1 Additions (Boot Parity + Module Resolution)

### 4.1 — Boot path parity tests (AMD-1.1, BUG-005)
- [ ] Create `crates/riversd/tests/boot/no_ssl_boot.rs` — `no_ssl_boot_has_all_subsystems`
- [ ] Create `crates/riversd/tests/boot/boot_parity.rs` — `boot_parity_tls_vs_no_ssl`
- [ ] Both boot paths must have: StorageEngine, SessionManager, CsrfManager, EventBus, engine registry, host context
- [ ] Verify: test fails if either path is missing a subsystem

### 4.2 — Module path resolution tests (AMD-1.5, BUG-013)
- [ ] Create `crates/riversd/tests/bundle/module_resolution.rs`
- [ ] `module_paths_resolved_to_absolute_after_bundle_load` — verify all module paths are absolute
- [ ] `module_resolution_independent_of_cwd` — load from non-bundle CWD, verify paths resolve
- [ ] Create test fixture bundle at `crates/riversd/tests/fixtures/test-bundle/`

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
