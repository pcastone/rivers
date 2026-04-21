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

### 5.1 — V8 regression tests ✅ (covered by v8_bridge_tests.rs)
- [x] `ctx_app_id_is_uuid_not_slug` covers `regression_app_id_not_empty`
- [x] `console_delegates_to_rivers_log` done

### 5.2 — Middleware/dispatch tests ✅
- [x] `security_headers_tests.rs` — 3 tests (all 5 headers, error sanitization, header blocklist)
- [x] `config_validation_tests.rs` — 8 tests (defaults, session cookie, DDL whitelist, canary parsing)
- [x] Found and fixed: ddl_whitelist in canary TOML was silently ignored (section ordering bug)

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

---

# APPENDED 2026-04-16 — Previous tasks.md contents (bundle validation + platform standards alignment)

# Tasks — Epic 1: Foundation — ValidationReport + Error Codes + Formatters

> **Branch:** `feature/art-of-possible`
> **Source:** `docs/arch/rivers-bundle-validation-spec.md` (Sections 8, 9, 11, Appendix A)
> **Goal:** Create foundational types and formatters for the 4-layer bundle validation pipeline

---

## Sprint 1.1 — ValidationReport types (`validate_result.rs`)

- [x] 1. Create `validate_result.rs` with `ValidationSeverity` enum (Error, Warning, Info)
- [x] 2. `ValidationStatus` enum (Pass, Fail, Warn, Skip) for individual results
- [x] 3. `ValidationResult` struct (status, file, message, error_code, table_path, field, suggestion, line, column, exports, etc.)
- [x] 4. `LayerResults` struct (passed, failed, skipped count + results vec)
- [x] 5. `ValidationReport` struct (bundle_name, bundle_version, layers map, summary)
- [x] 6. `ValidationSummary` struct (total_passed, total_failed, total_skipped, total_warnings, exit_code)
- [x] 7. Error code constants: S001-S010, E001-E005, X001-X013, C001-C008, L001-L005, W001-W004
- [x] 8. Builder methods: `report.add_result(layer, result)`, `report.exit_code()`, `report.has_errors()`
- [x] 9. Unit tests for report builder

## Sprint 1.2 — Text + JSON formatters (`validate_format.rs`)

- [x] 10. Text formatter matching spec section 8.1 output format
- [x] 11. JSON formatter matching spec section 8.2 contract
- [x] 12. `did_you_mean()` Levenshtein helper (distance <= 2)
- [x] 13. Unit tests for both formatters and Levenshtein helper

## Integration

- [x] 14. Export modules from `lib.rs`
- [x] 15. `cargo check -p rivers-runtime` passes
- [x] 16. `cargo test -p rivers-runtime -- validate_result validate_format` passes

---

## Validation

- `cargo check -p rivers-runtime` — compiles clean
- `cargo test -p rivers-runtime -- validate_result validate_format` — all tests pass

---

# Platform Standards Alignment — Task Plan

**Spec:** `docs/arch/rivers-platform-standards-alignment-spec.md`
**Status:** Planning — tasks organized by spec rollout phases

---

## Phase 1 — OpenAPI + Probes (P0)

### OpenAPI Support (spec §4)

- [ ] Write child execution spec `docs/arch/rivers-openapi-spec.md` from §4
- [ ] Add `OpenApiConfig` struct (`enabled`, `path`, `title`, `version`, `include_playground`) to `rivers-runtime/src/view.rs`
- [ ] Add view metadata fields: `summary`, `description`, `tags`, `operation_id`, `deprecated` to `ApiViewConfig`
- [ ] Add to structural validation known fields in `validate_structural.rs`
- [ ] Create `crates/riversd/src/openapi.rs` — walk REST views, DataView params, schemas → produce OpenAPI 3.1 JSON
- [ ] Map DataView parameter types to OpenAPI `in: path/query/header` from parameter_mapping; map schemas to request/response bodies
- [ ] Register `GET /<bundle>/<app>/openapi.json` route when `api.openapi.enabled = true`
- [ ] Validation: unique `operation_id` per app; no duplicate path+method; fail if enabled but cannot generate
- [ ] Unit tests for OpenAPI generation; integration test with address-book-bundle
- [ ] Tutorial: `docs/guide/tutorials/tutorial-openapi.md`

### Liveness/Readiness/Startup Probes (spec §5)

- [ ] Write child execution spec `docs/arch/rivers-probes-spec.md` from §5
- [ ] Add `ProbesConfig` struct (`enabled`, `live_path`, `ready_path`, `startup_path`) to `rivers-core-config`
- [ ] Add `probes` to known `[base]` fields in structural validation
- [ ] Implement `/live` handler — always 200 unless catastrophic (process alive, not deadlocked)
- [ ] Implement `/ready` handler — 200 when bundle loaded, required datasources connected, pools healthy; 503 otherwise
- [ ] Implement `/startup` handler — 503 until initialization complete, then 200
- [ ] Add startup-complete flag to `AppContext`, set after bundle wiring completes
- [ ] Tests: each probe response; failing datasource → /ready returns 503
- [ ] Add probe configuration to admin guide

---

## Phase 2 — OTel + Transaction Completion (P1)

### OpenTelemetry Trace Export (spec §6)

- [ ] Write child execution spec `docs/arch/rivers-otel-spec.md` from §6
- [ ] Add `OtelConfig` struct (`enabled`, `service_name`, `service_version`, `environment`, `exporter`, `endpoint`, `headers`, `sample_ratio`, `propagate_w3c`) to `rivers-core-config`
- [ ] Add `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry` to workspace dependencies
- [ ] Create spans: HTTP receive → route match → guard/auth → DataView execute → response write
- [ ] Span attributes: `http.method`, `http.route`, `http.status_code`, `rivers.app`, `rivers.dataview`, `rivers.driver`, `rivers.trace_id`
- [ ] W3C propagation: extract `traceparent`/`tracestate` inbound, inject on outbound HTTP driver requests
- [ ] Failure policy: OTel export failures log warning, never block requests
- [ ] Initialize OTel exporter at startup in `server/lifecycle.rs`
- [ ] Tests: verify spans created for request lifecycle; verify W3C headers propagated
- [ ] Tutorial: `docs/guide/tutorials/tutorial-otel.md`

### Runtime Transaction & Batch Completion (spec §7)

- [ ] Gap analysis: compare §7 against current implementation (Connection trait, TransactionMap, Rivers.db.batch stubs)
- [ ] Wire `host_db_begin/commit/rollback/batch` callbacks to actual pool acquisition and TransactionMap
- [ ] Implement batch `onError` policy: `fail_fast` (default) and `continue` modes per §7.4
- [ ] Verify auto-rollback on handler exit without commit
- [ ] Integration tests: Postgres transaction roundtrip via handler; batch insert with partial failure
- [ ] Verify existing canary transaction tests pass end-to-end

---

## Phase 3 — Standards-Based Auth (P1)

### JWT / OIDC / API Key Auth Providers (spec §8)

- [ ] Write child execution spec `docs/arch/rivers-auth-providers-spec.md` from §8
- [ ] Add `AuthProviderConfig` enum (JWT, OIDC, APIKey) to `rivers-core-config`
- [ ] Add `auth_config` to `ApiViewConfig` with `provider`, `required_scopes`, `required_roles`, claim fields
- [ ] JWT provider: validate signature (RS256/ES256), check `iss`/`aud`/`exp`, extract claims → `ctx.auth`
- [ ] OIDC provider: discover JWKS from `/.well-known/openid-configuration`, cache keys, validate tokens
- [ ] API key provider: lookup hashed key in StorageEngine
- [ ] Authorization: check `required_scopes` and `required_roles` against token claims
- [ ] Add `ctx.auth` object to handler context (subject, scopes, roles, claims)
- [ ] Compatibility: `auth = "none"` / `auth = "session"` unchanged; new `auth = "jwt"` / `"oidc"` / `"api_key"`
- [ ] Security: HTTPS required for JWT/OIDC; tokens never logged; JWKS cached with TTL
- [ ] Tests: JWT validation with test keys; OIDC discovery mock; API key lookup
- [ ] Tutorial: `docs/guide/tutorials/tutorial-api-auth.md`

---

## Phase 4 — AsyncAPI (P2)

### AsyncAPI Support (spec §9)

- [ ] Write child execution spec `docs/arch/rivers-asyncapi-spec.md` from §9
- [ ] Add `AsyncApiConfig` struct (`enabled`, `path`, `title`, `version`)
- [ ] Create `crates/riversd/src/asyncapi.rs` — walk MessageConsumer, SSE, WebSocket views → produce AsyncAPI 3.0 JSON
- [ ] Kafka/RabbitMQ/NATS: map consumer subscriptions to AsyncAPI channels with message schemas
- [ ] SSE: map SSE views to AsyncAPI channels (optional in v1)
- [ ] WebSocket: map WebSocket views to AsyncAPI channels (optional in v1)
- [ ] Register `GET /<bundle>/<app>/asyncapi.json` when enabled
- [ ] Validation: broker consumers must have schemas; SSE/WS optional
- [ ] Tests: unit tests for AsyncAPI generation from broker configs
- [ ] Add to developer guide

---

## Phase 5 — Polish (Future)

- [ ] OpenAPI HTML playground (Swagger UI / ReDoc)
- [ ] OTel metrics signal (bridge Prometheus → OTel)
- [ ] OTel log signal (bridge tracing → OTel logs)
- [ ] Richer AsyncAPI bindings (Kafka headers, AMQP routing keys)

---

## Cross-Cutting Rules (spec §10)

- [ ] All new features opt-in by default (`enabled = false` or absent)
- [ ] No new feature breaks existing bundles
- [ ] All new config fields have sensible defaults
- [ ] Error responses follow existing `ErrorResponse` envelope format
- [ ] Validation runs at startup (fail-fast), not at request time

---

## Open Questions (spec §12)

Decisions for implementation:

1. Bundle-level aggregate OpenAPI/AsyncAPI → defer to v2
2. `/ready` degradation → fail on any required datasource failure + open circuit breakers
3. OTel v1 → traces only; metrics/logs deferred to Phase 5
4. `Rivers.db.batch` partial failure → `fail_fast` only in v1
5. `ctx.auth` vs `ctx.session` → introduce `ctx.auth` as new object
6. AsyncAPI SSE/WS → start with brokers only, SSE/WS optional
7. OpenAPI strictness → permissive (omit missing schemas, don't invent them)


---

# Archived 2026-04-21 — Filesystem Driver + OperationDescriptor Epic

> **Status at archive:** canary FILESYSTEM profile 7/7 passing (commit 09c4025); docs + version bump committed (20febbe). 157 `- [ ]` checkbox items were not individually ticked in tasks.md before archive — epic is complete in code, only the checkbox bookkeeping was skipped. Preserved verbatim below for audit trail.

# Filesystem Driver + OperationDescriptor Framework — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `filesystem` built-in driver (eleven typed operations, chroot-sandboxed) and the `OperationDescriptor` framework that lets any driver expose a typed JS method surface via V8 proxy codegen, per `docs/arch/rivers-filesystem-driver-spec.md`.

**Architecture:** Two layered additions. (1) A framework-level `OperationDescriptor` catalog on `DatabaseDriver` with a default empty slice — opt-in, backward-compatible. (2) A built-in `filesystem` driver registering eleven operations, performing direct in-worker I/O (no IPC) through a new `DatasourceToken::Direct` variant, with startup-time root canonicalization and runtime-time path + symlink validation.

**Tech Stack:** Rust (`std::fs`, `std::path`), `async-trait`, `glob` + `regex` (new workspace deps), `base64` + `serde_json` + `tempfile` (already in workspace), `rusty_v8` (existing engine-v8 crate).

**Spec:** `docs/arch/rivers-filesystem-driver-spec.md` (v1.0, 2026-04-16)

**Branch:** `feature/filesystem-driver`

**Workflow note (per `CLAUDE.md`):**
- Mark each task complete as you go.
- Log decisions in `todo/changedecisionlog.md`, completed sections in `todo/changelog.md`.
- Commit after each logical task group (TDD pairs: test + impl).
- Canary must still pass end-to-end before merge.

---

## File Structure (locked up-front)

**Create:**
- `crates/rivers-driver-sdk/src/operation_descriptor.rs` — new types (`Param`, `ParamType`, `OpKind`, `OperationDescriptor`).
- `crates/rivers-drivers-builtin/src/filesystem.rs` — driver + connection + op dispatcher.
- `crates/rivers-drivers-builtin/src/filesystem/ops.rs` — eleven operation implementations.
- `crates/rivers-drivers-builtin/src/filesystem/chroot.rs` — root resolution, path validation, symlink rejection.
- `crates/rivers-drivers-builtin/src/filesystem/catalog.rs` — static `FILESYSTEM_OPERATIONS` slice.
- `crates/rivers-drivers-builtin/tests/filesystem_tests.rs` — integration tests.
- `canary-bundle/canary-filesystem/` — new canary app (mirrors `canary-sql` pattern).
- `docs/guide/tutorials/tutorial-filesystem-driver.md` — tutorial.

**Modify:**
- `crates/rivers-driver-sdk/src/traits.rs` — re-export from operation_descriptor, add `operations()` default method to `DatabaseDriver`.
- `crates/rivers-driver-sdk/src/lib.rs` — pub mod export.
- `crates/rivers-drivers-builtin/src/lib.rs` — `mod filesystem;` + register in `register_builtin_drivers`.
- `crates/rivers-runtime/src/process_pool/types.rs` — extend `DatasourceToken` with `Direct` variant.
- `crates/rivers-engine-v8/src/execution.rs` — typed-proxy codegen path when token is `Direct`.
- `crates/rivers-engine-v8/src/task_context.rs` — plumb Direct token into isolate setup.
- `Cargo.toml` (workspace root) — add `glob`, `regex` workspace deps.
- `canary-bundle/manifest.toml` — register `canary-filesystem` app.
- `docs/arch/rivers-feature-inventory.md` — §6.1 filesystem bullet, §6.6 OperationDescriptor bullet.

---

# Phase 1 — OperationDescriptor Framework

These tasks add the framework-level types with **zero behavior change for existing drivers** (empty default slice). Ship this phase first and independently — it compiles green, all existing tests pass, and nothing in the runtime changes.

---

### Task 1: Create `operation_descriptor.rs` with `ParamType` + `Param`

**Files:**
- Create: `crates/rivers-driver-sdk/src/operation_descriptor.rs`
- Modify: `crates/rivers-driver-sdk/src/lib.rs` (add `pub mod operation_descriptor;`)

- [ ] **Step 1: Write the failing test**

Create `crates/rivers-driver-sdk/src/operation_descriptor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_required_builder_sets_required_true_and_no_default() {
        let p = Param::required("path", ParamType::String);
        assert_eq!(p.name, "path");
        assert!(p.required);
        assert!(p.default_value.is_none());
    }

    #[test]
    fn param_optional_builder_sets_required_false_with_default() {
        let p = Param::optional("encoding", ParamType::String, "utf-8");
        assert_eq!(p.name, "encoding");
        assert!(!p.required);
        assert_eq!(p.default_value, Some("utf-8"));
    }

    #[test]
    fn paramtype_variants_are_distinct() {
        // Prove all five variants exist and can be constructed
        let _ = ParamType::String;
        let _ = ParamType::Integer;
        let _ = ParamType::Float;
        let _ = ParamType::Boolean;
        let _ = ParamType::Any;
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **FAIL** — module does not exist yet.

- [ ] **Step 3: Write the minimal implementation**

Prepend to `crates/rivers-driver-sdk/src/operation_descriptor.rs` (above the `mod tests`):

```rust
//! Typed operation catalog types for the V8 proxy codegen framework.
//!
//! Any driver may declare a slice of `OperationDescriptor` to expose typed
//! JS methods on `ctx.datasource("name")`. Drivers that do not declare a
//! catalog continue to use the standard `Query` / `execute()` pipeline.

/// Parameter type for JS-side validation before IPC dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Float,
    Boolean,
    /// Accepts string, number, boolean, array, or object.
    Any,
}

/// Single parameter in an operation signature.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: &'static str,
    pub param_type: ParamType,
    pub required: bool,
    pub default_value: Option<&'static str>,
}

impl Param {
    pub const fn required(name: &'static str, param_type: ParamType) -> Self {
        Param { name, param_type, required: true, default_value: None }
    }

    pub const fn optional(
        name: &'static str,
        param_type: ParamType,
        default: &'static str,
    ) -> Self {
        Param { name, param_type, required: false, default_value: Some(default) }
    }
}
```

Modify `crates/rivers-driver-sdk/src/lib.rs` — add near the top, next to other `pub mod` lines:

```rust
pub mod operation_descriptor;
pub use operation_descriptor::{OpKind, OperationDescriptor, Param, ParamType};
```

(The `OpKind` / `OperationDescriptor` re-exports will fail to compile until Task 2 adds them — that's fine, we'll add them next.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p rivers-driver-sdk operation_descriptor --no-run 2>&1 | head -40`
Expected: compile error on `OpKind`, `OperationDescriptor` re-export (not yet defined). The three `Param*` tests can compile once we fix the re-export in Task 2.

Temporary unblocking: in `lib.rs` narrow the re-export to what exists today:

```rust
pub use operation_descriptor::{Param, ParamType};
```

Then run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **3/3 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/operation_descriptor.rs crates/rivers-driver-sdk/src/lib.rs
git commit -m "feat(driver-sdk): add ParamType and Param types for operation catalog"
```

**Validation:**
- `cargo test -p rivers-driver-sdk operation_descriptor` → **3 passing**.
- `cargo build -p rivers-driver-sdk` → exit 0.
- Grep shows zero callers of `Param::required` yet (future phase wires them in).

---

### Task 2: Add `OpKind` + `OperationDescriptor` types

**Files:**
- Modify: `crates/rivers-driver-sdk/src/operation_descriptor.rs`
- Modify: `crates/rivers-driver-sdk/src/lib.rs` (restore full re-export)

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `crates/rivers-driver-sdk/src/operation_descriptor.rs`:

```rust
    #[test]
    fn operation_descriptor_read_builder_sets_kind_read() {
        static PARAMS: &[Param] = &[
            Param::required("path", ParamType::String),
        ];
        let desc = OperationDescriptor::read("readFile", PARAMS, "Read file contents");
        assert_eq!(desc.name, "readFile");
        assert_eq!(desc.kind, OpKind::Read);
        assert_eq!(desc.params.len(), 1);
        assert_eq!(desc.description, "Read file contents");
    }

    #[test]
    fn operation_descriptor_write_builder_sets_kind_write() {
        static PARAMS: &[Param] = &[
            Param::required("path", ParamType::String),
            Param::required("content", ParamType::String),
        ];
        let desc = OperationDescriptor::write("writeFile", PARAMS, "Write file");
        assert_eq!(desc.kind, OpKind::Write);
        assert_eq!(desc.params.len(), 2);
    }

    #[test]
    fn opkind_eq() {
        assert_eq!(OpKind::Read, OpKind::Read);
        assert_ne!(OpKind::Read, OpKind::Write);
    }
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **FAIL** — `OpKind` / `OperationDescriptor` not defined.

- [ ] **Step 3: Implement**

Append to `crates/rivers-driver-sdk/src/operation_descriptor.rs` (before `#[cfg(test)]`):

```rust
/// Classifies an operation as read or write for DDL security alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Read,
    Write,
}

/// Describes a single typed operation a driver exposes to handlers.
#[derive(Clone, Debug)]
pub struct OperationDescriptor {
    pub name: &'static str,
    pub kind: OpKind,
    pub params: &'static [Param],
    pub description: &'static str,
}

impl OperationDescriptor {
    pub const fn read(
        name: &'static str,
        params: &'static [Param],
        description: &'static str,
    ) -> Self {
        OperationDescriptor { name, kind: OpKind::Read, params, description }
    }

    pub const fn write(
        name: &'static str,
        params: &'static [Param],
        description: &'static str,
    ) -> Self {
        OperationDescriptor { name, kind: OpKind::Write, params, description }
    }
}
```

Restore full re-export in `crates/rivers-driver-sdk/src/lib.rs`:

```rust
pub use operation_descriptor::{OpKind, OperationDescriptor, Param, ParamType};
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p rivers-driver-sdk operation_descriptor`
Expected: **6/6 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/operation_descriptor.rs crates/rivers-driver-sdk/src/lib.rs
git commit -m "feat(driver-sdk): add OpKind and OperationDescriptor types"
```

**Validation:**
- `cargo test -p rivers-driver-sdk operation_descriptor` → **6 passing**.
- `cargo build --workspace` → exit 0 (no existing crate breaks).

---

### Task 3: Add `operations()` default to `DatabaseDriver` trait

**Files:**
- Modify: `crates/rivers-driver-sdk/src/traits.rs`

- [ ] **Step 1: Write the failing test**

Append at the bottom of `crates/rivers-driver-sdk/src/traits.rs` (inside the existing `#[cfg(test)]` block, or create one if absent):

```rust
#[cfg(test)]
mod operations_default_tests {
    use super::*;
    use async_trait::async_trait;

    struct NoOpsDriver;

    #[async_trait]
    impl DatabaseDriver for NoOpsDriver {
        fn name(&self) -> &str { "noops" }
        async fn connect(
            &self,
            _params: &ConnectionParams,
        ) -> Result<Box<dyn Connection>, DriverError> {
            unimplemented!("test-only driver")
        }
    }

    #[test]
    fn default_operations_returns_empty_slice() {
        let driver = NoOpsDriver;
        assert_eq!(driver.operations().len(), 0);
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-driver-sdk operations_default`
Expected: **FAIL** — method `operations` not found.

- [ ] **Step 3: Implement**

In `crates/rivers-driver-sdk/src/traits.rs`, locate the `DatabaseDriver` trait (currently around line 563) and add:

```rust
    /// Returns the typed operation catalog for V8 proxy codegen.
    ///
    /// Default: empty — driver uses standard `Query`/`execute()` dispatch.
    /// Override to declare typed methods available on `ctx.datasource("name")`.
    fn operations(&self) -> &[crate::OperationDescriptor] {
        &[]
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-driver-sdk operations_default`
Expected: **1/1 PASS**.

Also run the broader test suite to confirm backward compat:
Run: `cargo test -p rivers-driver-sdk`
Expected: all previously-passing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-driver-sdk/src/traits.rs
git commit -m "feat(driver-sdk): add DatabaseDriver::operations() with empty default"
```

**Validation:**
- `cargo build --workspace` → exit 0 (no existing driver breaks — default method kicks in).
- `cargo test --workspace --no-fail-fast 2>&1 | tail -20` → summary shows no new failures (new assertions only).

---

### Task 4: Backward-compat sweep

**Files:**
- No code changes — a verification task only, per CLAUDE.md "check in before executing" philosophy applied to outputs.

- [ ] **Step 1: Compile the full workspace**

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: no errors. Faker, memcached, postgres, mysql, sqlite, redis, eventbus, rps_client drivers all build with the new trait method's default.

- [ ] **Step 2: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -30`
Expected: test count increased by exactly 7 (four from Task 1/2, one from Task 3, two from later ops-body expansions if any — but we haven't added those yet, so count is 7). Previously-passing count unchanged; no regressions.

Log the exact counts in `todo/changelog.md`:

```markdown
### 2026-04-16 — OperationDescriptor framework baseline
- Files: crates/rivers-driver-sdk/src/{operation_descriptor.rs,traits.rs,lib.rs}
- Summary: new types + opt-in trait method; existing drivers unaffected.
- Spec: rivers-filesystem-driver-spec.md §2.
- Test delta: +7 passing, 0 regressions.
```

- [ ] **Step 3: Commit the changelog entry**

```bash
git add todo/changelog.md
git commit -m "docs(changelog): OperationDescriptor framework baseline"
```

**Validation:**
- No new failing test.
- No existing driver trait impl required source edits.

---

# Phase 2 — Filesystem Driver Foundation (Chroot + Connection)

These tasks stand up the driver skeleton with **no operations wired yet**. Every task hardens the chroot boundary before any I/O ever runs.

---

### Task 5: Add `glob` and `regex` workspace deps

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Inspect current workspace deps**

Run: `Grep("glob\\|regex", path="Cargo.toml", context=1)`
Expected: neither dep present.

- [ ] **Step 2: Add the deps**

Edit `Cargo.toml` workspace `[workspace.dependencies]` block, append:

```toml
glob = "0.3"
regex = "1.10"
```

- [ ] **Step 3: Verify resolution**

Run: `cargo tree -p rivers-driver-sdk 2>&1 | grep -E '^\\s*(glob|regex)' | head -5`
Expected: crates resolved (may be empty until a crate actually consumes them — Task 19/20 will).

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add glob + regex workspace deps for filesystem driver"
```

**Validation:**
- `cargo tree` shows both deps in the resolved graph.
- No existing crate breaks.

---

### Task 6: Scaffold `FilesystemDriver` + `FilesystemConnection` shells

**Files:**
- Create: `crates/rivers-drivers-builtin/src/filesystem.rs`
- Create: `crates/rivers-drivers-builtin/src/filesystem/mod.rs` (if we choose folder layout) — we'll use a single file for now and split in later tasks.
- Modify: `crates/rivers-drivers-builtin/src/lib.rs` (add `mod filesystem;`)

- [ ] **Step 1: Write the failing test**

Create `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
//! Filesystem driver — chroot-sandboxed direct-I/O driver.
//!
//! Spec: docs/arch/rivers-filesystem-driver-spec.md

use async_trait::async_trait;
use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, Query, QueryResult,
};
use std::path::PathBuf;

pub struct FilesystemDriver;

pub struct FilesystemConnection {
    pub root: PathBuf,
}

#[async_trait]
impl DatabaseDriver for FilesystemDriver {
    fn name(&self) -> &str {
        "filesystem"
    }

    async fn connect(
        &self,
        _params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        Err(DriverError::NotImplemented("FilesystemDriver::connect — Task 11".into()))
    }
}

#[async_trait]
impl Connection for FilesystemConnection {
    async fn execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::NotImplemented("FilesystemConnection::execute — Task 26".into()))
    }

    async fn ddl_execute(&mut self, _q: &Query) -> Result<QueryResult, DriverError> {
        Err(DriverError::Forbidden(
            "filesystem driver does not support ddl_execute".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_name_is_filesystem() {
        assert_eq!(FilesystemDriver.name(), "filesystem");
    }

    #[test]
    fn operations_default_empty_for_now() {
        // Until Task 14 wires the catalog, operations() returns empty via default.
        assert!(FilesystemDriver.operations().is_empty());
    }
}
```

Modify `crates/rivers-drivers-builtin/src/lib.rs`, add near other `mod` lines:

```rust
pub mod filesystem;
```

- [ ] **Step 2: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **2/2 PASS**.

- [ ] **Step 3: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs crates/rivers-drivers-builtin/src/lib.rs
git commit -m "feat(drivers-builtin): scaffold FilesystemDriver + FilesystemConnection shells"
```

**Validation:**
- `cargo test -p rivers-drivers-builtin filesystem::tests` → **2 passing**.
- `cargo build --workspace` → exit 0.

---

### Task 7: Implement `resolve_root` with TDD

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec reference: §5.1. Behavior: must be absolute, must canonicalize, must be a directory.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
    use tempfile::TempDir;

    #[test]
    fn resolve_root_rejects_relative_path() {
        let err = FilesystemDriver::resolve_root("./relative").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("absolute"),
            "expected 'absolute' in error, got: {msg}"
        );
    }

    #[test]
    fn resolve_root_rejects_nonexistent_path() {
        let err = FilesystemDriver::resolve_root("/does/not/exist/for/real").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("does not exist") || msg.contains("not accessible"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_root_rejects_file_path() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("not_a_dir.txt");
        std::fs::write(&file_path, b"hi").unwrap();
        let err = FilesystemDriver::resolve_root(file_path.to_str().unwrap()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not a directory"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_root_canonicalizes_valid_directory() {
        let dir = TempDir::new().unwrap();
        let resolved = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **FAIL** — `resolve_root` not defined.

- [ ] **Step 3: Implement**

Add to `impl FilesystemDriver` in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
impl FilesystemDriver {
    pub fn resolve_root(database: &str) -> Result<PathBuf, DriverError> {
        let path = PathBuf::from(database);

        if !path.is_absolute() {
            return Err(DriverError::Connection(format!(
                "filesystem root must be absolute path, got: {database}"
            )));
        }

        let canonical = std::fs::canonicalize(&path).map_err(|e| {
            DriverError::Connection(format!(
                "filesystem root does not exist or is not accessible: {database} — {e}"
            ))
        })?;

        if !canonical.is_dir() {
            return Err(DriverError::Connection(format!(
                "filesystem root is not a directory: {}",
                canonical.display()
            )));
        }

        Ok(canonical)
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **6/6 PASS** (2 existing + 4 new).

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement resolve_root — absolute + canonical + directory check"
```

**Validation:**
- All 6 filesystem tests pass.
- `tempfile` dep already available (workspace dep).

---

### Task 8: Implement `resolve_path` chroot enforcement

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec: §5.2. Must reject absolute paths, canonicalize relative paths, and verify `canonical.starts_with(&self.root)`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    fn test_connection() -> (TempDir, FilesystemConnection) {
        let dir = TempDir::new().unwrap();
        let root = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        (dir, FilesystemConnection { root })
    }

    #[test]
    fn resolve_path_rejects_absolute_unix() {
        let (_dir, conn) = test_connection();
        let err = conn.resolve_path("/etc/passwd").unwrap_err();
        assert!(format!("{err}").contains("absolute paths not permitted"));
    }

    #[test]
    fn resolve_path_rejects_parent_escape() {
        let (_dir, conn) = test_connection();
        let err = conn.resolve_path("../../../etc/passwd").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("escapes datasource root") || msg.contains("does not exist"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn resolve_path_accepts_valid_relative() {
        let (dir, conn) = test_connection();
        std::fs::write(dir.path().join("hello.txt"), b"hi").unwrap();
        let resolved = conn.resolve_path("hello.txt").unwrap();
        assert!(resolved.starts_with(&conn.root));
    }

    #[test]
    fn resolve_path_normalizes_backslashes() {
        // On Unix this behaves like a literal; purpose is documentation — real
        // Windows coverage comes via CI.
        let (dir, conn) = test_connection();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("b.txt"), b"x").unwrap();
        let resolved = conn.resolve_path("a\\b.txt").unwrap();
        assert!(resolved.starts_with(&conn.root));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **FAIL** — `resolve_path` not defined.

- [ ] **Step 3: Implement**

Add to `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
impl FilesystemConnection {
    pub fn resolve_path(&self, relative: &str) -> Result<PathBuf, DriverError> {
        let normalized = relative.replace('\\', "/");

        let bytes = normalized.as_bytes();
        let is_windows_drive =
            bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
        if normalized.starts_with('/') || is_windows_drive {
            return Err(DriverError::Query(
                "absolute paths not permitted — all paths relative to datasource root".into(),
            ));
        }

        let joined = self.root.join(&normalized);
        let canonical = canonicalize_for_op(&joined)?;

        if !canonical.starts_with(&self.root) {
            return Err(DriverError::Forbidden(
                "path escapes datasource root".into(),
            ));
        }

        reject_symlinks_within(&self.root, &canonical)?;
        Ok(canonical)
    }
}

fn canonicalize_for_op(path: &std::path::Path) -> Result<PathBuf, DriverError> {
    // For nonexistent paths (writeFile, mkdir), canonicalize the deepest existing
    // ancestor, then append the remaining segments. This preserves chroot checks
    // while letting write ops target paths that do not yet exist.
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let base = std::fs::canonicalize(&existing).map_err(|e| {
        DriverError::Query(format!("could not canonicalize ancestor of path: {e}"))
    })?;
    let mut out = base;
    for piece in tail.into_iter().rev() {
        out.push(piece);
    }
    Ok(out)
}

fn reject_symlinks_within(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<(), DriverError> {
    // Walk from root forward, checking every intermediate component.
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut current = root.to_path_buf();
    for comp in rel.components() {
        current.push(comp);
        if !current.exists() {
            break;
        }
        let is_symlink = current
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            return Err(DriverError::Forbidden(format!(
                "symlink detected in path: {}",
                current.display()
            )));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **10/10 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement resolve_path with chroot + symlink rejection"
```

**Validation:**
- All 10 filesystem tests pass.
- `resolve_path` is pure (no I/O side effects beyond canonicalization).
- Manual probe: `cargo test resolve_path_rejects_parent_escape -- --nocapture` shows clean output.

---

### Task 9: Unit test — symlink rejection (Unix-gated)

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_symlink_inside_root() {
        use std::os::unix::fs::symlink;
        let (dir, conn) = test_connection();
        let target = dir.path().join("real");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, dir.path().join("link")).unwrap();

        let err = conn.resolve_path("link").unwrap_err();
        assert!(format!("{err}").contains("symlink detected"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_symlink_pointing_outside_root() {
        use std::os::unix::fs::symlink;
        let (dir, conn) = test_connection();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), dir.path().join("escape")).unwrap();

        let err = conn.resolve_path("escape/file.txt").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symlink detected") || msg.contains("escapes datasource root"),
            "unexpected error: {msg}"
        );
    }
```

- [ ] **Step 2: Run — expect PASS**

Task 8 already implemented symlink rejection. Run to confirm.

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **12/12 PASS** on Unix; 10/10 on Windows (cfg-gated).

- [ ] **Step 3: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "test(filesystem): add symlink rejection unit tests (unix-gated)"
```

**Validation:**
- Unix: 12 filesystem tests pass, the two new symlink tests included.
- On macOS (darwin host): test goes green because we cfg(unix) gate.

---

### Task 10: Wire `connect()` to build `FilesystemConnection`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Write the failing test**

Append:

```rust
    #[tokio::test]
    async fn connect_returns_connection_with_resolved_root() {
        let dir = TempDir::new().unwrap();
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: dir.path().to_str().unwrap().to_string(),
            username: String::new(),
            password: String::new(),
        };
        let driver = FilesystemDriver;
        let conn = driver.connect(&params).await.unwrap();
        // Dry-probe: we don't yet have execute(), but we should at least compile + connect.
        drop(conn);
    }

    #[tokio::test]
    async fn connect_fails_on_nonexistent_root() {
        let params = ConnectionParams {
            host: String::new(),
            port: 0,
            database: "/does/not/exist/nowhere".into(),
            username: String::new(),
            password: String::new(),
        };
        let err = FilesystemDriver.connect(&params).await.unwrap_err();
        assert!(format!("{err}").contains("does not exist") || format!("{err}").contains("not accessible"));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: **FAIL** — `connect` still returns `NotImplemented`.

- [ ] **Step 3: Implement**

Replace the `connect` body in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let root = Self::resolve_root(&params.database)?;
        Ok(Box::new(FilesystemConnection { root }))
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: all passing (14 on Unix / 12 on Windows).

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): wire connect() to resolve_root + FilesystemConnection"
```

**Validation:**
- Async connect test passes.
- Driver name `"filesystem"` established and stable.

---

### Task 11: Register `FilesystemDriver` in `register_builtin_drivers`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/lib.rs`

- [ ] **Step 1: Locate registration fn**

Run: `Grep("fn register_builtin_drivers", type=rust, path=\"crates/rivers-drivers-builtin\")`

Read that file.

- [ ] **Step 2: Write the failing test**

Add to `crates/rivers-drivers-builtin/src/lib.rs` under a `#[cfg(test)]` block (create if missing):

```rust
#[cfg(test)]
mod registration_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CaptureRegistrar {
        names: Arc<Mutex<Vec<String>>>,
    }

    impl DriverRegistrar for CaptureRegistrar {
        fn register_database_driver(
            &mut self,
            driver: std::sync::Arc<dyn rivers_driver_sdk::DatabaseDriver>,
        ) {
            self.names.lock().unwrap().push(driver.name().to_string());
        }
    }

    #[test]
    fn filesystem_driver_is_registered() {
        let mut reg = CaptureRegistrar::default();
        register_builtin_drivers(&mut reg);
        let names = reg.names.lock().unwrap().clone();
        assert!(
            names.iter().any(|n| n == "filesystem"),
            "expected 'filesystem' in registered driver names: {names:?}"
        );
    }
}
```

- [ ] **Step 3: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin registration_tests`
Expected: **FAIL**.

- [ ] **Step 4: Register**

In `crates/rivers-drivers-builtin/src/lib.rs`, inside `register_builtin_drivers`, add:

```rust
registrar.register_database_driver(std::sync::Arc::new(filesystem::FilesystemDriver));
```

- [ ] **Step 5: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin registration_tests`
Expected: **PASS**.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-drivers-builtin/src/lib.rs
git commit -m "feat(filesystem): register FilesystemDriver in register_builtin_drivers"
```

**Validation:**
- Driver is discoverable by name `"filesystem"` at runtime.

---

# Phase 3 — Operation Catalog + Implementations

Eleven operations, each landed test-first. Operations live in dedicated modules to keep files small.

---

### Task 12: Declare `FILESYSTEM_OPERATIONS` catalog

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Write the failing test**

Append:

```rust
    #[test]
    fn catalog_has_eleven_operations() {
        assert_eq!(FilesystemDriver.operations().len(), 11);
    }

    #[test]
    fn catalog_contains_all_expected_names() {
        let names: Vec<&str> = FilesystemDriver
            .operations()
            .iter()
            .map(|o| o.name)
            .collect();
        for expected in [
            "readFile", "readDir", "stat", "exists", "find", "grep",
            "writeFile", "mkdir", "delete", "rename", "copy",
        ] {
            assert!(names.contains(&expected), "missing op: {expected}");
        }
    }

    #[test]
    fn read_ops_have_opkind_read() {
        for op in FilesystemDriver.operations() {
            let is_read = matches!(op.name, "readFile" | "readDir" | "stat" | "exists" | "find" | "grep");
            let is_write = matches!(op.name, "writeFile" | "mkdir" | "delete" | "rename" | "copy");
            match (is_read, is_write) {
                (true, false) => assert_eq!(op.kind, OpKind::Read, "{}", op.name),
                (false, true) => assert_eq!(op.kind, OpKind::Write, "{}", op.name),
                _ => panic!("unclassified op: {}", op.name),
            }
        }
    }
```

Add the `OpKind` import: `use rivers_driver_sdk::OpKind;`.

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::catalog`
Expected: **FAIL**.

- [ ] **Step 3: Implement**

Append to `crates/rivers-drivers-builtin/src/filesystem.rs` (module-level):

```rust
use rivers_driver_sdk::{OpKind, OperationDescriptor, Param, ParamType};

static FILESYSTEM_OPERATIONS: &[OperationDescriptor] = &[
    // Reads
    OperationDescriptor::read(
        "readFile",
        &[
            Param::required("path", ParamType::String),
            Param::optional("encoding", ParamType::String, "utf-8"),
        ],
        "Read file contents — utf-8 returns string, base64 returns base64-encoded string",
    ),
    OperationDescriptor::read(
        "readDir",
        &[Param::required("path", ParamType::String)],
        "List directory entries — filenames only",
    ),
    OperationDescriptor::read(
        "stat",
        &[Param::required("path", ParamType::String)],
        "File/directory metadata",
    ),
    OperationDescriptor::read(
        "exists",
        &[Param::required("path", ParamType::String)],
        "Returns boolean existence",
    ),
    OperationDescriptor::read(
        "find",
        &[
            Param::required("pattern", ParamType::String),
            Param::optional("max_results", ParamType::Integer, "1000"),
        ],
        "Recursive glob search",
    ),
    OperationDescriptor::read(
        "grep",
        &[
            Param::required("pattern", ParamType::String),
            Param::optional("path", ParamType::String, "."),
            Param::optional("max_results", ParamType::Integer, "1000"),
        ],
        "Regex search across files",
    ),
    // Writes
    OperationDescriptor::write(
        "writeFile",
        &[
            Param::required("path", ParamType::String),
            Param::required("content", ParamType::String),
            Param::optional("encoding", ParamType::String, "utf-8"),
        ],
        "Write file — creates parent dirs, overwrites if exists",
    ),
    OperationDescriptor::write(
        "mkdir",
        &[Param::required("path", ParamType::String)],
        "Create directory recursively",
    ),
    OperationDescriptor::write(
        "delete",
        &[Param::required("path", ParamType::String)],
        "Delete file or recursively delete directory",
    ),
    OperationDescriptor::write(
        "rename",
        &[
            Param::required("oldPath", ParamType::String),
            Param::required("newPath", ParamType::String),
        ],
        "Rename/move within root",
    ),
    OperationDescriptor::write(
        "copy",
        &[
            Param::required("src", ParamType::String),
            Param::required("dest", ParamType::String),
        ],
        "Copy file or recursively copy directory",
    ),
];
```

Then override `operations()` on the trait impl:

```rust
    fn operations(&self) -> &[OperationDescriptor] {
        FILESYSTEM_OPERATIONS
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::catalog`
Expected: **3/3 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): declare FILESYSTEM_OPERATIONS catalog (11 ops)"
```

**Validation:**
- Catalog visible via `FilesystemDriver.operations()`.
- Names + kinds match spec §6.1 exactly.

---

### Task 13: Operation dispatcher in `Connection::execute`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Behavior: route on `Query::operation`. We wire up an empty match and add per-op branches in later tasks.

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn execute_unknown_operation_returns_notimpl() {
        let (_dir, mut conn) = test_connection();
        let q = Query {
            operation: "nope".into(),
            target: String::new(),
            parameters: Default::default(),
            statement: String::new(),
        };
        let err = conn.execute(&q).await.unwrap_err();
        assert!(
            matches!(err, DriverError::NotImplemented(_) | DriverError::Unsupported(_)),
            "unexpected variant: {err:?}"
        );
    }
```

- [ ] **Step 2: Run — expect FAIL**

Compile error — `Query` is constructed with defaults. If `QueryValue` derive is missing for `HashMap::default()`, adjust. Otherwise FAIL on execution returning `NotImplemented(\"...Task 26\")` (which this test expects).

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::execute_unknown`
Expected: **PASS** (existing skeleton returns NotImplemented, which the test accepts).

- [ ] **Step 3: Replace the placeholder execute**

Replace `Connection::execute` body:

```rust
    async fn execute(&mut self, q: &Query) -> Result<QueryResult, DriverError> {
        match q.operation.as_str() {
            // Reads (Tasks 15–20)
            "readFile" => Err(DriverError::NotImplemented("readFile — Task 15".into())),
            "readDir" => Err(DriverError::NotImplemented("readDir — Task 16".into())),
            "stat" => Err(DriverError::NotImplemented("stat — Task 17".into())),
            "exists" => Err(DriverError::NotImplemented("exists — Task 18".into())),
            "find" => Err(DriverError::NotImplemented("find — Task 19".into())),
            "grep" => Err(DriverError::NotImplemented("grep — Task 20".into())),
            // Writes (Tasks 21–25)
            "writeFile" => Err(DriverError::NotImplemented("writeFile — Task 21".into())),
            "mkdir" => Err(DriverError::NotImplemented("mkdir — Task 22".into())),
            "delete" => Err(DriverError::NotImplemented("delete — Task 23".into())),
            "rename" => Err(DriverError::NotImplemented("rename — Task 24".into())),
            "copy" => Err(DriverError::NotImplemented("copy — Task 25".into())),
            other => Err(DriverError::Unsupported(format!(
                "unknown filesystem operation: {other}"
            ))),
        }
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: all filesystem tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): stub execute dispatcher routing by operation name"
```

**Validation:**
- Unknown op → `Unsupported`.
- Known op → `NotImplemented` with task pointer.

---

### Task 14: Implement `readFile` (utf-8 + base64)

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §6.3. Default encoding `"utf-8"`. `"base64"` → `base64::engine::general_purpose::STANDARD.encode`. Unknown encoding → `DriverError::Query`.

- [ ] **Step 1: Write the failing test**

```rust
    use rivers_driver_sdk::QueryValue;

    fn mkq(op: &str, params: &[(&str, QueryValue)]) -> Query {
        let mut parameters = std::collections::HashMap::new();
        for (k, v) in params {
            parameters.insert(k.to_string(), v.clone());
        }
        Query {
            operation: op.into(),
            target: String::new(),
            parameters,
            statement: String::new(),
        }
    }

    #[tokio::test]
    async fn read_file_utf8_returns_string() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let q = mkq("readFile", &[("path", QueryValue::String("a.txt".into()))]);
        let result = conn.execute(&q).await.unwrap();
        // Result shape: single-row single-column "content"
        let content = extract_scalar_string(&result);
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn read_file_base64_returns_b64_string() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("b.bin"), &[0xff, 0x00, 0xfe]).unwrap();
        let q = mkq(
            "readFile",
            &[
                ("path", QueryValue::String("b.bin".into())),
                ("encoding", QueryValue::String("base64".into())),
            ],
        );
        let result = conn.execute(&q).await.unwrap();
        let content = extract_scalar_string(&result);
        assert_eq!(content, "/wD+"); // base64 of 0xff 0x00 0xfe
    }

    #[tokio::test]
    async fn read_file_unknown_encoding_errors() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let q = mkq(
            "readFile",
            &[
                ("path", QueryValue::String("a.txt".into())),
                ("encoding", QueryValue::String("ebcdic".into())),
            ],
        );
        let err = conn.execute(&q).await.unwrap_err();
        assert!(format!("{err}").contains("unsupported encoding"));
    }

    // Helper for tests — extract single string scalar from QueryResult.
    fn extract_scalar_string(r: &QueryResult) -> String {
        // Specific shape wired by op impl: rows = [[QueryValue::String(content)]]
        let row = r.rows.first().expect("expected one row");
        let val = row.first().expect("expected one column");
        match val {
            QueryValue::String(s) => s.clone(),
            other => panic!("expected String, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run — expect FAIL** (NotImplemented)

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::read_file`
Expected: **FAIL**.

- [ ] **Step 3: Implement**

Replace the `readFile` arm:

```rust
            "readFile" => ops::read_file(self, q).await,
```

Create a new submodule. Inline for now, split to `filesystem/ops.rs` once it grows. At the bottom of `filesystem.rs`:

```rust
mod ops {
    use super::*;
    use base64::Engine;
    use rivers_driver_sdk::{Query, QueryResult, QueryValue};

    fn get_string<'a>(q: &'a Query, key: &str) -> Option<&'a str> {
        match q.parameters.get(key) {
            Some(QueryValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub async fn read_file(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("readFile: required parameter 'path' missing".into())
        })?;
        let encoding = get_string(q, "encoding").unwrap_or("utf-8");
        let path = conn.resolve_path(rel)?;
        let bytes = tokio::task::spawn_blocking({
            let path = path.clone();
            move || std::fs::read(&path)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        let content = match encoding {
            "utf-8" => String::from_utf8(bytes).map_err(|e| {
                DriverError::Query(format!("file is not valid utf-8: {e}"))
            })?,
            "base64" => base64::engine::general_purpose::STANDARD.encode(&bytes),
            other => {
                return Err(DriverError::Query(format!(
                    "unsupported encoding: {other}"
                )));
            }
        };

        Ok(QueryResult {
            columns: vec!["content".into()],
            rows: vec![vec![QueryValue::String(content)]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }

    pub fn map_io_error(e: std::io::Error) -> DriverError {
        use std::io::ErrorKind::*;
        match e.kind() {
            NotFound => DriverError::Query(format!("not found: {e}")),
            PermissionDenied => DriverError::Query(format!("permission denied: {e}")),
            _ => DriverError::Internal(format!("I/O error: {e}")),
        }
    }
}
```

(If `QueryResult` field names differ — confirm via `Grep("struct QueryResult", type=rust, path=\"crates/rivers-driver-sdk\")` and adjust.)

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::read_file`
Expected: **3/3 PASS**.

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement readFile (utf-8 + base64)"
```

**Validation:**
- UTF-8 happy path passes.
- Base64 round-trip exact.
- Unknown encoding → clean error.

---

### Task 15: Implement `readDir`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn read_dir_returns_entry_names() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        let q = mkq("readDir", &[("path", QueryValue::String(".".into()))]);
        let result = conn.execute(&q).await.unwrap();
        let mut names: Vec<String> = result
            .rows
            .iter()
            .map(|r| match &r[0] {
                QueryValue::String(s) => s.clone(),
                _ => panic!(),
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "b".to_string()]);
    }
```

- [ ] **Step 2: Run — FAIL**
- [ ] **Step 3: Implement**

Append to `mod ops`:

```rust
    pub async fn read_dir(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("readDir: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        let entries: Vec<String> = tokio::task::spawn_blocking({
            let path = path.clone();
            move || -> Result<Vec<String>, std::io::Error> {
                let mut out = Vec::new();
                for entry in std::fs::read_dir(&path)? {
                    out.push(entry?.file_name().to_string_lossy().to_string());
                }
                Ok(out)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        Ok(QueryResult {
            columns: vec!["name".into()],
            rows: entries
                .into_iter()
                .map(|n| vec![QueryValue::String(n)])
                .collect(),
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire the arm: `"readDir" => ops::read_dir(self, q).await,`.

- [ ] **Step 4: Run — PASS**
- [ ] **Step 5: Commit**

```bash
git commit -am "feat(filesystem): implement readDir"
```

**Validation:** 1 new test passes.

---

### Task 16: Implement `stat`

**Files:** same file.

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn stat_file_returns_metadata() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("f.txt"), b"hello").unwrap();
        let q = mkq("stat", &[("path", QueryValue::String("f.txt".into()))]);
        let result = conn.execute(&q).await.unwrap();
        assert_eq!(result.rows.len(), 1);
        let cols = &result.columns;
        for expected in ["size", "mtime", "atime", "ctime", "isFile", "isDirectory", "mode"] {
            assert!(cols.iter().any(|c| c == expected), "missing col: {expected}");
        }
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn stat(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("stat: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        let md = tokio::task::spawn_blocking({
            let p = path.clone();
            move || std::fs::metadata(&p)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        fn to_iso(t: std::time::SystemTime) -> String {
            let secs = t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            // minimal ISO — avoid adding chrono dep if not already present
            use std::fmt::Write as _;
            let mut s = String::new();
            let _ = write!(s, "{secs}");
            s
        }
        let size = QueryValue::Integer(md.len() as i64);
        let mtime = QueryValue::String(to_iso(md.modified().unwrap_or(std::time::UNIX_EPOCH)));
        let atime = QueryValue::String(to_iso(md.accessed().unwrap_or(std::time::UNIX_EPOCH)));
        let ctime = QueryValue::String(to_iso(md.created().unwrap_or(std::time::UNIX_EPOCH)));
        let is_file = QueryValue::Boolean(md.is_file());
        let is_dir = QueryValue::Boolean(md.is_dir());

        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            QueryValue::Integer(md.permissions().mode() as i64)
        };
        #[cfg(not(unix))]
        let mode = QueryValue::Integer(0);

        Ok(QueryResult {
            columns: vec![
                "size".into(), "mtime".into(), "atime".into(), "ctime".into(),
                "isFile".into(), "isDirectory".into(), "mode".into(),
            ],
            rows: vec![vec![size, mtime, atime, ctime, is_file, is_dir, mode]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire the arm. Follow TDD pattern.

- [ ] **Step 4/5:** Run + commit.

```bash
git commit -am "feat(filesystem): implement stat"
```

**Validation:** all seven stat columns appear.

**Note on timestamp format:** we emit epoch seconds as a string for v1. If a later task adds a date lib to the workspace, we can upgrade to ISO 8601 without breaking callers — the handler API (`info.mtime`) is opaque today.

---

### Task 17: Implement `exists`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn exists_returns_true_for_present_false_for_absent() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("yes.txt"), "").unwrap();
        let q = mkq("exists", &[("path", QueryValue::String("yes.txt".into()))]);
        assert!(matches!(
            conn.execute(&q).await.unwrap().rows[0][0],
            QueryValue::Boolean(true)
        ));
        let q2 = mkq("exists", &[("path", QueryValue::String("nope.txt".into()))]);
        assert!(matches!(
            conn.execute(&q2).await.unwrap().rows[0][0],
            QueryValue::Boolean(false)
        ));
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn exists(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("exists: required parameter 'path' missing".into())
        })?;
        // Resolve may error with "escapes root" — that still counts as "not visible"; return false.
        let ok = match conn.resolve_path(rel) {
            Ok(p) => tokio::task::spawn_blocking(move || p.exists())
                .await
                .unwrap_or(false),
            Err(DriverError::Forbidden(_)) => false,
            Err(e) => return Err(e),
        };
        Ok(QueryResult {
            columns: vec!["exists".into()],
            rows: vec![vec![QueryValue::Boolean(ok)]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement exists"
```

**Validation:** absent file → false; present → true; chroot-escaping path → false (not error).

---

### Task 18: Implement `find` (glob) with truncation

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn find_returns_relative_paths_and_truncation() {
        let (dir, mut conn) = test_connection();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
        }
        let q = mkq(
            "find",
            &[
                ("pattern", QueryValue::String("*.txt".into())),
                ("max_results", QueryValue::Integer(3)),
            ],
        );
        let r = conn.execute(&q).await.unwrap();
        // Expected shape: row[0] = results array, row[1] = truncated bool
        // Implementation choice: two columns on a single row.
        assert_eq!(r.columns, vec!["results", "truncated"]);
        let row = &r.rows[0];
        match &row[0] {
            QueryValue::Array(v) => assert!(v.len() <= 3),
            other => panic!("expected Array, got {other:?}"),
        }
        assert!(matches!(row[1], QueryValue::Boolean(true)));
    }
```

- [ ] **Step 2/3: Implement**

Add to `Cargo.toml` of `rivers-drivers-builtin` (in the `[dependencies]` table):
```toml
glob = { workspace = true }
```

Append to `mod ops`:

```rust
    pub async fn find(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let pattern = get_string(q, "pattern").ok_or_else(|| {
            DriverError::Query("find: required parameter 'pattern' missing".into())
        })?;
        let max = match q.parameters.get("max_results") {
            Some(QueryValue::Integer(n)) => (*n).max(0) as usize,
            _ => 1000,
        };
        let root = conn.root.clone();
        let pattern_owned = pattern.to_string();
        let (results, truncated) = tokio::task::spawn_blocking(move || {
            let full_pattern = format!("{}/**/{}", root.display(), pattern_owned);
            let mut out = Vec::new();
            let mut truncated = false;
            if let Ok(paths) = glob::glob(&full_pattern) {
                for entry in paths.flatten() {
                    if let Ok(rel) = entry.strip_prefix(&root) {
                        out.push(rel.to_string_lossy().to_string());
                        if out.len() > max {
                            out.pop();
                            truncated = true;
                            break;
                        }
                    }
                }
            }
            (out, truncated)
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?;

        Ok(QueryResult {
            columns: vec!["results".into(), "truncated".into()],
            rows: vec![vec![
                QueryValue::Array(
                    results.into_iter().map(QueryValue::String).collect(),
                ),
                QueryValue::Boolean(truncated),
            ]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement find (glob) with truncation"
```

**Validation:** 5 files, max_results=3 → `results.len() <= 3`, truncated=true.

---

### Task 19: Implement `grep` (regex) with truncation

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn grep_finds_matching_lines() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "foo\nTODO: bar\nbaz").unwrap();
        let q = mkq(
            "grep",
            &[
                ("pattern", QueryValue::String("TODO".into())),
                ("path", QueryValue::String(".".into())),
                ("max_results", QueryValue::Integer(10)),
            ],
        );
        let r = conn.execute(&q).await.unwrap();
        // Shape: results = Array of Object{file, line, content}, plus truncated bool
        assert_eq!(r.columns, vec!["results", "truncated"]);
    }
```

- [ ] **Step 2/3: Implement**

Add `regex = { workspace = true }` to `rivers-drivers-builtin/Cargo.toml`.

Append to `mod ops`:

```rust
    pub async fn grep(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let pattern = get_string(q, "pattern").ok_or_else(|| {
            DriverError::Query("grep: required parameter 'pattern' missing".into())
        })?;
        let rel_path = get_string(q, "path").unwrap_or(".");
        let max = match q.parameters.get("max_results") {
            Some(QueryValue::Integer(n)) => (*n).max(0) as usize,
            _ => 1000,
        };
        let base = conn.resolve_path(rel_path)?;
        let re = regex::Regex::new(pattern).map_err(|e| {
            DriverError::Query(format!("grep: invalid regex: {e}"))
        })?;

        let (hits, truncated) = tokio::task::spawn_blocking({
            let root = conn.root.clone();
            move || {
                let mut hits = Vec::new();
                let mut truncated = false;
                walk_files(&base, &root, &mut |rel_path, contents| {
                    for (i, line) in contents.lines().enumerate() {
                        if re.is_match(line) {
                            hits.push((rel_path.clone(), i + 1, line.to_string()));
                            if hits.len() > max {
                                hits.pop();
                                truncated = true;
                                return false;
                            }
                        }
                    }
                    true
                });
                (hits, truncated)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?;

        let results = QueryValue::Array(
            hits.into_iter()
                .map(|(file, line, content)| {
                    QueryValue::Json(serde_json::json!({
                        "file": file,
                        "line": line,
                        "content": content,
                    }))
                })
                .collect(),
        );
        Ok(QueryResult {
            columns: vec!["results".into(), "truncated".into()],
            rows: vec![vec![results, QueryValue::Boolean(truncated)]],
            affected_rows: 0,
            last_insert_id: None,
        })
    }

    fn walk_files(
        start: &std::path::Path,
        root: &std::path::Path,
        visit: &mut impl FnMut(String, String) -> bool,
    ) {
        let mut stack = vec![start.to_path_buf()];
        while let Some(p) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    // Binary detect: null in first 8192 bytes
                    let head_len = bytes.len().min(8192);
                    if bytes[..head_len].contains(&0) {
                        continue;
                    }
                    let Ok(text) = String::from_utf8(bytes) else { continue };
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    if !visit(rel, text) {
                        return;
                    }
                }
            }
        }
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement grep with binary skip + truncation"
```

**Validation:** finds TODO line in a.txt; binary file skipped.

---

### Task 20: Implement `writeFile` (utf-8 + base64, mkdir -p parent)

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn write_file_creates_parent_dirs_and_writes_utf8() {
        let (dir, mut conn) = test_connection();
        let q = mkq(
            "writeFile",
            &[
                ("path", QueryValue::String("deep/nested/out.txt".into())),
                ("content", QueryValue::String("hello".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        let read = std::fs::read_to_string(dir.path().join("deep/nested/out.txt")).unwrap();
        assert_eq!(read, "hello");
    }

    #[tokio::test]
    async fn write_file_base64_decodes_to_bytes() {
        let (dir, mut conn) = test_connection();
        let q = mkq(
            "writeFile",
            &[
                ("path", QueryValue::String("b.bin".into())),
                ("content", QueryValue::String("/wD+".into())),
                ("encoding", QueryValue::String("base64".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        let bytes = std::fs::read(dir.path().join("b.bin")).unwrap();
        assert_eq!(bytes, vec![0xff, 0x00, 0xfe]);
    }
```

- [ ] **Step 2/3: Implement**

Append:

```rust
    pub async fn write_file(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("writeFile: required parameter 'path' missing".into())
        })?;
        let content = get_string(q, "content").ok_or_else(|| {
            DriverError::Query("writeFile: required parameter 'content' missing".into())
        })?;
        let encoding = get_string(q, "encoding").unwrap_or("utf-8");
        let path = conn.resolve_path(rel)?;

        let bytes: Vec<u8> = match encoding {
            "utf-8" => content.as_bytes().to_vec(),
            "base64" => base64::engine::general_purpose::STANDARD
                .decode(content)
                .map_err(|e| DriverError::Query(format!("base64 decode: {e}")))?,
            other => {
                return Err(DriverError::Query(format!(
                    "unsupported encoding: {other}"
                )));
            }
        };

        tokio::task::spawn_blocking({
            let path = path.clone();
            move || -> std::io::Result<()> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, bytes)
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;

        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement writeFile (utf-8 + base64, recursive mkdir)"
```

**Validation:** deep/nested/out.txt written; binary round-trip via base64 exact.

---

### Task 21: Implement `mkdir`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn mkdir_is_recursive_and_idempotent() {
        let (dir, mut conn) = test_connection();
        let q = mkq("mkdir", &[("path", QueryValue::String("a/b/c".into()))]);
        conn.execute(&q).await.unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
        // idempotent
        conn.execute(&q).await.unwrap();
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn mkdir(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("mkdir: required parameter 'path' missing".into())
        })?;
        let path = conn.resolve_path(rel)?;
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&path))
            .await
            .map_err(|e| DriverError::Internal(format!("join: {e}")))?
            .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 0,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement mkdir (recursive, idempotent)"
```

**Validation:** nested + repeated mkdir both succeed.

---

### Task 22: Implement `delete`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn delete_removes_file_and_directory_recursively() {
        let (dir, mut conn) = test_connection();
        std::fs::create_dir_all(dir.path().join("d/e")).unwrap();
        std::fs::write(dir.path().join("d/e/f.txt"), "").unwrap();
        std::fs::write(dir.path().join("g.txt"), "").unwrap();

        conn.execute(&mkq("delete", &[("path", QueryValue::String("d".into()))]))
            .await.unwrap();
        assert!(!dir.path().join("d").exists());
        conn.execute(&mkq("delete", &[("path", QueryValue::String("g.txt".into()))]))
            .await.unwrap();
        assert!(!dir.path().join("g.txt").exists());

        // idempotent — deleting nonexistent is not an error
        conn.execute(&mkq("delete", &[("path", QueryValue::String("g.txt".into()))]))
            .await.unwrap();
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn delete(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let rel = get_string(q, "path").ok_or_else(|| {
            DriverError::Query("delete: required parameter 'path' missing".into())
        })?;
        // resolve_path fails on nonexistent if the whole chain is missing; handle silently.
        let path = match conn.resolve_path(rel) {
            Ok(p) => p,
            Err(DriverError::Query(_)) => {
                return Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    affected_rows: 0,
                    last_insert_id: None,
                })
            }
            Err(e) => return Err(e),
        };
        tokio::task::spawn_blocking({
            let p = path.clone();
            move || -> std::io::Result<()> {
                if !p.exists() {
                    return Ok(());
                }
                if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                }
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement delete (idempotent, recursive for dirs)"
```

**Validation:** file, dir, and repeated delete all succeed.

---

### Task 23: Implement `rename`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn rename_moves_file_within_root() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("old.txt"), "x").unwrap();
        let q = mkq(
            "rename",
            &[
                ("oldPath", QueryValue::String("old.txt".into())),
                ("newPath", QueryValue::String("new.txt".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        assert!(!dir.path().join("old.txt").exists());
        assert!(dir.path().join("new.txt").exists());
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn rename(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let old_rel = get_string(q, "oldPath").ok_or_else(|| {
            DriverError::Query("rename: required parameter 'oldPath' missing".into())
        })?;
        let new_rel = get_string(q, "newPath").ok_or_else(|| {
            DriverError::Query("rename: required parameter 'newPath' missing".into())
        })?;
        let old_p = conn.resolve_path(old_rel)?;
        let new_p = conn.resolve_path(new_rel)?;
        tokio::task::spawn_blocking(move || std::fs::rename(&old_p, &new_p))
            .await
            .map_err(|e| DriverError::Internal(format!("join: {e}")))?
            .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement rename"
```

**Validation:** file moved within root.

---

### Task 24: Implement `copy`

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn copy_file_byte_level() {
        let (dir, mut conn) = test_connection();
        std::fs::write(dir.path().join("a.txt"), "data").unwrap();
        let q = mkq(
            "copy",
            &[
                ("src", QueryValue::String("a.txt".into())),
                ("dest", QueryValue::String("b.txt".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "data");
    }

    #[tokio::test]
    async fn copy_directory_recursively() {
        let (dir, mut conn) = test_connection();
        std::fs::create_dir_all(dir.path().join("src/sub")).unwrap();
        std::fs::write(dir.path().join("src/sub/f.txt"), "x").unwrap();
        let q = mkq(
            "copy",
            &[
                ("src", QueryValue::String("src".into())),
                ("dest", QueryValue::String("dst".into())),
            ],
        );
        conn.execute(&q).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("dst/sub/f.txt")).unwrap(), "x");
    }
```

- [ ] **Step 2/3: Implement**

```rust
    pub async fn copy(
        conn: &FilesystemConnection,
        q: &Query,
    ) -> Result<QueryResult, DriverError> {
        let src_rel = get_string(q, "src").ok_or_else(|| {
            DriverError::Query("copy: required parameter 'src' missing".into())
        })?;
        let dest_rel = get_string(q, "dest").ok_or_else(|| {
            DriverError::Query("copy: required parameter 'dest' missing".into())
        })?;
        let src = conn.resolve_path(src_rel)?;
        let dest = conn.resolve_path(dest_rel)?;
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if src.is_dir() {
                copy_dir_recursive(&src, &dest)
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&src, &dest).map(|_| ())
            }
        })
        .await
        .map_err(|e| DriverError::Internal(format!("join: {e}")))?
        .map_err(map_io_error)?;
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    fn copy_dir_recursive(
        src: &std::path::Path,
        dst: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if from.is_dir() {
                copy_dir_recursive(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }
```

Wire arm + commit.

```bash
git commit -am "feat(filesystem): implement copy (file + recursive directory)"
```

**Validation:** file copy + directory copy both pass.

---

### Task 25: `extra` config — `max_file_size` and `max_depth`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §8.4.

- [ ] **Step 1: Test**

```rust
    #[tokio::test]
    async fn write_file_enforces_max_file_size() {
        let dir = TempDir::new().unwrap();
        let mut conn = FilesystemConnection {
            root: FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap(),
            max_file_size: 10,
            max_depth: 100,
        };
        let big = "a".repeat(100);
        let q = mkq(
            "writeFile",
            &[
                ("path", QueryValue::String("big.txt".into())),
                ("content", QueryValue::String(big)),
            ],
        );
        let err = conn.execute(&q).await.unwrap_err();
        assert!(format!("{err}").contains("exceeds max_file_size"));
    }
```

(Earlier `test_connection` helper must be updated to build the struct with new fields.)

- [ ] **Step 2/3: Implement**

Change `FilesystemConnection`:

```rust
pub struct FilesystemConnection {
    pub root: PathBuf,
    pub max_file_size: u64,
    pub max_depth: usize,
}
```

Update `test_connection` helper to supply defaults:

```rust
    fn test_connection() -> (TempDir, FilesystemConnection) {
        let dir = TempDir::new().unwrap();
        let root = FilesystemDriver::resolve_root(dir.path().to_str().unwrap()).unwrap();
        (dir, FilesystemConnection { root, max_file_size: 50 * 1024 * 1024, max_depth: 100 })
    }
```

Update `connect()`:

```rust
    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let root = Self::resolve_root(&params.database)?;
        // TODO(future task): plumb extra config via params.extra
        Ok(Box::new(FilesystemConnection {
            root,
            max_file_size: 50 * 1024 * 1024,
            max_depth: 100,
        }))
    }
```

Inside `ops::write_file`, before write:

```rust
        if (bytes.len() as u64) > conn.max_file_size {
            return Err(DriverError::Query(format!(
                "file exceeds max_file_size: {} bytes",
                bytes.len()
            )));
        }
```

Inside `ops::read_file`, after reading bytes:

```rust
        if (bytes.len() as u64) > conn.max_file_size {
            return Err(DriverError::Query(format!(
                "file exceeds max_file_size: {} bytes",
                bytes.len()
            )));
        }
```

Wire `max_depth` into `walk_files` in `grep`:

```rust
    fn walk_files_bounded(
        start: &std::path::Path,
        root: &std::path::Path,
        max_depth: usize,
        visit: &mut impl FnMut(String, String) -> bool,
    ) {
        let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(start.to_path_buf(), 0)];
        while let Some((p, depth)) = stack.pop() {
            if depth > max_depth { continue; }
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push((path, depth + 1));
                } else if let Ok(bytes) = std::fs::read(&path) {
                    let head_len = bytes.len().min(8192);
                    if bytes[..head_len].contains(&0) { continue; }
                    let Ok(text) = String::from_utf8(bytes) else { continue };
                    let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string();
                    if !visit(rel, text) { return; }
                }
            }
        }
    }
```

Call `walk_files_bounded(&base, &root, conn.max_depth, &mut |…|)` from `grep`.

- [ ] **Step 4: Run — PASS**

Run: `cargo test -p rivers-drivers-builtin filesystem::tests`
Expected: all passing.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(filesystem): enforce max_file_size and max_depth"
```

**Validation:** oversized write rejected with clean error.

---

### Task 26: Rename `delete` idempotency — test

- [ ] **Step 1/2/3:** Already covered in Task 22 test.
  Verify the idempotent branch is present.

Run: `cargo test -p rivers-drivers-builtin filesystem::tests::delete_removes`
Expected: **PASS**.

No code change. Skip commit.

**Validation:** no-op pass.

---

# Phase 4 — Direct I/O Token + V8 Typed Proxy

This phase introduces `DatasourceToken::Direct` and wires the V8 isolate to generate typed methods. It's the most cross-cutting phase — start here only after Phase 3 is green.

---

### Task 27: Extend `DatasourceToken` with `Direct` variant

**Files:**
- Modify: `crates/rivers-runtime/src/process_pool/types.rs`

- [ ] **Step 1: Read current definition**

Run: `Read crates/rivers-runtime/src/process_pool/types.rs`

Confirm current shape: `pub struct DatasourceToken(pub String);` plus `ResolvedDatasource { driver_name, params }`.

- [ ] **Step 2: Write the failing test**

Append to the same file under `#[cfg(test)]`:

```rust
#[cfg(test)]
mod direct_token_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn pooled_token_constructs() {
        let t = DatasourceToken::pooled("pool-42");
        assert!(matches!(t, DatasourceToken::Pooled { .. }));
    }

    #[test]
    fn direct_token_carries_driver_and_root() {
        let t = DatasourceToken::direct("filesystem", PathBuf::from("/tmp/x"));
        match t {
            DatasourceToken::Direct { driver, root } => {
                assert_eq!(driver, "filesystem");
                assert_eq!(root, PathBuf::from("/tmp/x"));
            }
            _ => panic!("expected Direct variant"),
        }
    }
}
```

- [ ] **Step 3: Run — expect FAIL**

Run: `cargo test -p rivers-runtime direct_token_tests`
Expected: **FAIL** — the struct is not yet an enum.

- [ ] **Step 4: Implement — enum conversion**

Replace:

```rust
pub struct DatasourceToken(pub String);
```

with:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatasourceToken {
    /// Pool-backed — resolves to host-side connection pool by id.
    Pooled { pool_id: String },
    /// Self-contained — worker performs I/O directly with the given resource handle.
    Direct { driver: String, root: std::path::PathBuf },
}

impl DatasourceToken {
    pub fn pooled(pool_id: impl Into<String>) -> Self {
        DatasourceToken::Pooled { pool_id: pool_id.into() }
    }

    pub fn direct(driver: impl Into<String>, root: std::path::PathBuf) -> Self {
        DatasourceToken::Direct { driver: driver.into(), root }
    }
}
```

- [ ] **Step 5: Migrate call sites**

Run: `Grep("DatasourceToken\\(", type=rust)` — expect every construction site to break compilation.

Run: `Grep("DatasourceToken", type=rust)` — list every consumer.

For each call site:
- Construction `DatasourceToken("xyz".into())` → `DatasourceToken::pooled("xyz")`.
- Pattern matches on `DatasourceToken(s)` → `DatasourceToken::Pooled { pool_id: s }`.

Commit each crate's migration as its own commit:

```bash
git commit -am "refactor(<crate>): migrate DatasourceToken to enum"
```

- [ ] **Step 6: Run — expect PASS**

Run: `cargo test -p rivers-runtime direct_token_tests`
Expected: **2/2 PASS**.

Run: `cargo build --workspace`
Expected: exit 0.

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -30`
Expected: no regressions.

- [ ] **Step 7: Final commit**

```bash
git commit -am "feat(runtime): introduce DatasourceToken::Direct variant"
```

**Validation:**
- All workspace tests pass.
- No `DatasourceToken(` pattern remains anywhere except inside the enum definition.

---

### Task 28: Emit `Direct` token for `filesystem` driver at dispatch time

**Files:**
- Modify: `crates/rivers-runtime/src/process_pool/` — wherever `ResolvedDatasource` → `DatasourceToken` translation happens.

- [ ] **Step 1: Locate translation site**

Run: `Grep("ResolvedDatasource", type=rust, path=\"crates/rivers-runtime\")`.

Identify the function that builds the `DatasourceToken` for a handler invocation from a `ResolvedDatasource`. Call it `resolve_token_for_dispatch` or similar.

- [ ] **Step 2: Write the failing test**

Add a unit test in that module:

```rust
    #[test]
    fn filesystem_driver_yields_direct_token() {
        let rd = ResolvedDatasource {
            driver_name: "filesystem".into(),
            params: ConnectionParams {
                host: String::new(),
                port: 0,
                database: "/tmp".into(),
                username: String::new(),
                password: String::new(),
            },
        };
        let tok = resolve_token_for_dispatch(&rd);
        assert!(matches!(tok, DatasourceToken::Direct { ref driver, .. } if driver == "filesystem"));
    }

    #[test]
    fn other_drivers_yield_pooled_token() {
        let rd = ResolvedDatasource {
            driver_name: "postgres".into(),
            params: ConnectionParams {
                host: "localhost".into(),
                port: 5432,
                database: "db".into(),
                username: "u".into(),
                password: "p".into(),
            },
        };
        let tok = resolve_token_for_dispatch(&rd);
        assert!(matches!(tok, DatasourceToken::Pooled { .. }));
    }
```

- [ ] **Step 3: Run — FAIL**

Run the test — should fail either because fn doesn't exist yet or because it always returns `Pooled`.

- [ ] **Step 4: Implement**

Branch on `driver_name == "filesystem"` (fast path; add generic `driver.is_direct()` flag in the future):

```rust
pub fn resolve_token_for_dispatch(rd: &ResolvedDatasource) -> DatasourceToken {
    if rd.driver_name == "filesystem" {
        return DatasourceToken::Direct {
            driver: "filesystem".into(),
            root: std::path::PathBuf::from(&rd.params.database),
        };
    }
    DatasourceToken::Pooled { pool_id: format!("{}:{}", rd.driver_name, rd.params.database) }
}
```

- [ ] **Step 5: Run — PASS**

Run: `cargo test -p rivers-runtime resolve_token_for_dispatch`
Expected: **2/2 PASS**.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(runtime): emit DatasourceToken::Direct for filesystem driver"
```

**Validation:**
- Filesystem → Direct; all other drivers → Pooled.

---

### Task 29 (decomposed): V8 Direct-dispatch typed proxy

**Decomposition rationale:** Original Task 29 bundled five cross-cutting concerns (thread-local plumbing, catalog lookup, host fn, JS codegen, integration harness) into one commit. Breaking it into 29a–29f keeps each commit focused and individually reviewable. The V8 engine is **statically linked** into `riversd` (default feature `static-engines`), so the live code lives under `crates/riversd/src/process_pool/v8_engine/` — there is no C-ABI to cross, no `ENGINE_ABI_VERSION` bump, and no `HostCallbacks` extension needed. `rivers-drivers-builtin` is reachable through `rivers-core`'s `drivers` feature.

Task 30 (ParamType validation) is absorbed into 29f.

---

### Task 29a: Thread-local `DirectDatasource` registry

**Files:**
- Modify: `crates/riversd/src/process_pool/v8_engine/task_locals.rs`
- Modify: `crates/riversd/src/process_pool/v8_engine/execution.rs` (setup + teardown)

Today the V8 task locals store env/store/trace-id. Add a per-task map of direct datasources so the host fn (29c) can look up `{driver, root, lazy-initialized Connection}` without crossing any ABI.

- [ ] **Step 1: Define `DirectDatasource` struct**

In `task_locals.rs`:

```rust
pub(crate) struct DirectDatasource {
    pub driver: String,
    pub root: std::path::PathBuf,
    // Lazily built on first use; reused across ops within the task.
    pub connection: std::cell::RefCell<Option<Box<dyn rivers_driver_sdk::Connection>>>,
}
```

- [ ] **Step 2: Add thread-local**

```rust
thread_local! {
    pub(crate) static TASK_DIRECT_DATASOURCES:
        RefCell<HashMap<String, DirectDatasource>> = RefCell::new(HashMap::new());
}
```

- [ ] **Step 3: Wire setup/teardown**

Extend `setup_task_locals`/`clear_task_locals` to populate/clear the new map from `TaskContext.datasources`, filtering for `DatasourceToken::Direct`.

- [ ] **Step 4: Unit test**

Add a test verifying:
- After setup with a `Direct` token, the map has one entry with correct driver + root.
- After teardown, the map is empty.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(v8): thread-local registry for direct datasources"
```

**Validation:** map round-trips. No V8 interaction yet.

---

### Task 29b: `catalog_for(driver_name)` helper

**Files:**
- Create: `crates/riversd/src/process_pool/v8_engine/catalog.rs`
- Modify: `mod.rs` to expose it.

- [ ] **Step 1: Write the function**

```rust
use rivers_driver_sdk::OperationDescriptor;
use rivers_drivers_builtin::filesystem::FILESYSTEM_OPERATIONS;

pub(crate) fn catalog_for(driver: &str) -> Option<&'static [OperationDescriptor]> {
    match driver {
        "filesystem" => Some(FILESYSTEM_OPERATIONS),
        _ => None,
    }
}
```

Need to confirm `FILESYSTEM_OPERATIONS` is `pub`. It's currently `static`, scoped to the module — expose as `pub static FILESYSTEM_OPERATIONS` if needed.

- [ ] **Step 2: Unit tests**

- `catalog_for("filesystem")` returns `Some` with 11 descriptors.
- `catalog_for("postgres")` returns `None`.

- [ ] **Step 3: Commit**

```bash
git commit -am "feat(v8): catalog_for helper maps driver → OperationDescriptor slice"
```

**Validation:** 2 unit tests pass.

---

### Task 29c: V8 host fn `__rivers_direct_dispatch`

**Files:**
- Modify: `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` (or a new `direct_dispatch.rs`)
- Ensure it's registered on every isolate's global (where `Rivers.*` already lives).

Signature from JS: `__rivers_direct_dispatch(name: string, operation: string, parameters: object) → any`

Body:
1. Pull `(name, operation, parameters)` out of the V8 args.
2. Look up `DirectDatasource` in the thread-local from 29a. Throw V8 `TypeError` if missing.
3. Lazy-init `FilesystemConnection` via the driver's `connect()` — use `FilesystemDriver::resolve_root(root)` path we already built. Cache into the `RefCell`.
4. Build a `Query { operation, target: "", parameters: <HashMap from V8 object>, statement: "" }`.
5. Run `connection.execute(&query).await` — since the V8 callback is synchronous, run via `tokio::runtime::Handle::block_on` or the existing sync-wait pattern used by other V8 host fns in `rivers_global.rs`.
6. Marshal `QueryResult` → V8 value:
   - Single-row results with a `content` column → unwrap to the string/value directly (matches ergonomic JS expectations, e.g. `readFile` returns `string`).
   - Multi-row results → array of objects.
   - Non-scalar shape (find/grep) → object containing `results` + `truncated`.
   - Decision rule: if `column_names == ["content"]`, unwrap; else if single row, return the row as an object; else return array.

- [ ] **Step 1: Scaffold the callback**

Register on the global under a non-guessable name (keeps handlers from calling it directly — the proxy owns the contract).

- [ ] **Step 2: Marshaling helpers**

Write `query_value_to_v8` + `v8_to_query_value` if not already present; check `rivers_global.rs` and `datasource.rs` for existing equivalents and reuse.

- [ ] **Step 3: Unit test via V8 isolate harness**

Spawn an isolate, populate thread-local with a `Direct` token pointing at a `TempDir`, write `hello.txt` with content `"world"`, run:

```js
__rivers_direct_dispatch("fs", "readFile", {path: "hello.txt"})
```

Assert returned V8 string is `"world"`.

Also test an error path: missing datasource name → `TypeError`.

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(v8): __rivers_direct_dispatch host fn for direct drivers"
```

**Validation:** isolate-level round trip works without any proxy codegen.

---

### Task 29d: Typed-proxy codegen in `datasource.rs`

**Files:**
- Modify: `crates/riversd/src/process_pool/v8_engine/datasource.rs`
- May reference `catalog.rs` (29b) and thread-local (29a).

Current `ctx.datasource(name)` goes through `ctx_datasource_build_callback` (lines 14+ in `datasource.rs`) which routes to the pooled `.fromQuery().build()` flow. We branch:

1. At call time, peek thread-local for `Direct` token under `name`.
2. If `Direct`: emit a V8 object with one method per descriptor from `catalog_for(driver)`. Each method:
   - Validates each arg by `Param.ParamType` with `typeof`/`Array.isArray` guards, throwing `TypeError` with `"<operation>: '<param>' must be <type>"`.
   - Fills defaults for optional params.
   - Calls `__rivers_direct_dispatch(name, operation, {param1, param2, ...})`.
3. Fall through to existing pooled behavior otherwise.

Two implementation choices:
- **(A) Compile a JS string template** once per datasource and cache. Simpler; debuggable via `Script::compile`.
- **(B) Build each method as a native `v8::Function::new`** with closure over operation metadata. More lines of Rust but no JS source parsing.

**Recommendation:** start with (A). Template looks like:

```js
const proxy = {};
{{#each descriptors}}
proxy.{{name}} = function({{params}}) {
    {{#each params_with_type}}
    {{type_guard}}
    {{/each}}
    {{#each optional_params}}
    if ({{name}} === undefined) {{name}} = {{default_literal}};
    {{/each}}
    return __rivers_direct_dispatch("{{ds_name}}", "{{op_name}}", { {{params}} });
};
{{/each}}
proxy
```

Keep the code-gen in Rust as a `String` builder — no templating engine dependency needed.

- [ ] **Step 1: Emit proxy for a single op (`readFile`) end-to-end**

Proves the mechanism. Build the JS string, `v8::Script::compile`, run, return the resulting object.

- [ ] **Step 2: Generalize to all 11 filesystem ops**

Loop over descriptors. Handle `ParamType::Integer` vs `String` vs `Boolean` vs `Json`. For defaults, render integers literally, strings as JSON-encoded strings.

- [ ] **Step 3: Wire branch in `ctx_datasource_build_callback`**

If thread-local holds a `Direct` entry for the requested name, return the proxy; else current pooled path.

- [ ] **Step 4: Unit test (proxy shape)**

Without running a real op, assert that `ctx.datasource("fs").readFile` is a `function`, that `ctx.datasource("fs").__proto__` contains all 11 method names.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(v8): typed proxy codegen for DatasourceToken::Direct"
```

**Validation:** 11 methods on the proxy object, each callable.

---

### Task 29e: Integration test — `typed_proxy_readfile_roundtrip`

**Files:**
- Create or extend: `crates/riversd/tests/` file that already exercises V8 dispatch, or add a new one dedicated to direct drivers.

- [ ] **Step 1: Author test**

- Spawn the full ProcessPool dispatch path (same harness pattern other integration tests use).
- Register a datasource resolved as `filesystem` pointing at `TempDir`.
- Write `hello.txt` = `"world"` in the tempdir.
- Execute inline handler: `export function run(ctx) { return ctx.datasource("fs").readFile("hello.txt"); }`
- Assert result is `"world"`.

- [ ] **Step 2: Commit**

```bash
git commit -am "test(v8): integration test for typed proxy readFile round-trip"
```

**Validation:** end-to-end proves the full stack: dispatch → thread-local → proxy codegen → host fn → `FilesystemConnection.execute` → return value.

---

### Task 29f: ParamType validation + negative cases (absorbs Task 30)

**Files:**
- Extend test file from 29e.
- If codegen gaps: tighten `datasource.rs` (29d).

- [ ] **Step 1: Tests**

- `ctx.datasource("fs").readFile(42)` → throws `TypeError` with `"must be a string"` in message, before dispatch.
- `ctx.datasource("fs").readFile()` (missing required) → `TypeError`.
- `ctx.datasource("fs").find("*.txt")` with `max_results` omitted → uses default 1000, dispatch succeeds.

- [ ] **Step 2: Tighten codegen if any test fails**

- [ ] **Step 3: Commit**

```bash
git commit -am "test(v8): typed proxy arg validation + defaults"
```

**Validation:** no dispatch happens on invalid input; defaults are applied correctly.

---

**Sequence:** 29a → 29b → 29c → 29d → 29e → 29f. Each commit independently reviewable. Every task touches files inside `riversd` only — no cross-crate ABI work.

---

# Phase 5 — Canary Fleet + Docs

---

### Task 31: Scaffold `canary-filesystem` app

**Files:**
- Create: `canary-bundle/canary-filesystem/manifest.toml`
- Create: `canary-bundle/canary-filesystem/resources.toml`
- Create: `canary-bundle/canary-filesystem/app.toml`
- Create: `canary-bundle/canary-filesystem/libraries/handlers/filesystem.ts`
- Modify: `canary-bundle/manifest.toml` (register app)

- [ ] **Step 1: Copy structure from `canary-sql`**

Run: `Bash cp -R canary-bundle/canary-sql canary-bundle/canary-filesystem`

Edit the copied files:
- `manifest.toml`: change `appId` (fresh UUID) and `name = "canary-filesystem"`. Assign a unique port.
- `resources.toml`: declare one `[[resources]]` with `name = "fs"`, `x-type = "filesystem"`, `nopassword = true`.
- `app.toml`: declare one datasource block `[[data.datasources]]` with `name = "fs"`, `driver = "filesystem"`, `database = "/tmp/canary-fs-root"` (or use `$CANARY_FS_ROOT` env var).
- Replace the SQL views with a single `[api.views.fs_roundtrip]` pointing to a handler `handlers/filesystem.ts`.

- [ ] **Step 2: Write handler**

Create `canary-bundle/canary-filesystem/libraries/handlers/filesystem.ts`:

```typescript
export function run(ctx: any): void {
    const fs = ctx.datasource("fs");

    // Write
    fs.writeFile("round/trip.txt", "hello canary");

    // Read back
    const got = fs.readFile("round/trip.txt");
    if (got !== "hello canary") {
        ctx.resdata = { name: "fs_roundtrip", status: "fail", error: `got ${got}` };
        return;
    }

    // Chroot escape must fail
    try {
        fs.readFile("../../etc/passwd");
        ctx.resdata = { name: "fs_roundtrip", status: "fail", error: "chroot bypass!" };
        return;
    } catch (_e) { /* expected */ }

    // Cleanup
    fs.delete("round");

    ctx.resdata = { name: "fs_roundtrip", status: "pass", timing_ms: 0 };
}
```

- [ ] **Step 3: Register in fleet manifest**

Edit `canary-bundle/manifest.toml`:

```toml
[[apps]]
name = "canary-filesystem"
path = "canary-filesystem"
```

- [ ] **Step 4: Validate the bundle**

Run: `./target/debug/riverpackage validate canary-bundle`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add canary-bundle/canary-filesystem/ canary-bundle/manifest.toml
git commit -m "test(canary): add canary-filesystem app (CRUD + chroot-escape probe)"
```

**Validation:**
- `riverpackage validate canary-bundle` → success.
- `riversd` loads bundle without error.

---

### Task 32: Canary run + green gate

**Files:** none (operational).

- [ ] **Step 1: Deploy and start**

Follow `CLAUDE.md` workflow:
```bash
cargo deploy ./dist/canary-run
./dist/canary-run/bin/riversctl start --foreground &
sleep 3
```

- [ ] **Step 2: Hit the canary view**

```bash
curl -ks https://localhost:<canary-port>/api/fs_roundtrip
```
Expected JSON: `{"name":"fs_roundtrip","status":"pass", ...}`

- [ ] **Step 3: Verify no regressions in other canaries**

Run whatever aggregation endpoint exists (`canary-main`), assert all profiles still green.

- [ ] **Step 4: Log run in changelog**

Append to `todo/changelog.md`:
```markdown
### 2026-04-XX — Filesystem driver canary green
- canary-filesystem CRUD roundtrip + chroot escape probe: PASS
- Full fleet: XX/XX passing.
```

**Validation:**
- Canary passes 100%.
- No regressions elsewhere.

---

### Task 33: Update `rivers-feature-inventory.md`

**Files:**
- Modify: `docs/arch/rivers-feature-inventory.md`

- [ ] **Step 1: Add filesystem bullet to §6.1**

Insert after the Faker line:

```
- **Filesystem** (std::fs): chroot-sandboxed directory access, eleven typed operations,
  direct I/O in worker process, no credentials required
```

- [ ] **Step 2: Add OperationDescriptor bullet to §6.6**

Insert new bullet:

```
- `OperationDescriptor` — driver-declared typed operation catalog for V8 proxy codegen.
  Drivers that declare operations get typed JS methods on `ctx.datasource("name")`
  instead of the pseudo DataView builder. Framework-level feature — any driver can opt in.
```

- [ ] **Step 3: Commit**

```bash
git add docs/arch/rivers-feature-inventory.md
git commit -m "docs(inventory): add filesystem driver and OperationDescriptor bullets"
```

**Validation:**
- `Grep("Filesystem", path=\"docs/arch/rivers-feature-inventory.md\")` → 1 new hit.

---

### Task 34: Tutorial — `tutorial-filesystem-driver.md`

**Files:**
- Create: `docs/guide/tutorials/tutorial-filesystem-driver.md`

Contents cover:
1. Minimal `resources.toml` + `app.toml` datasource declaration.
2. Simple handler: `ctx.datasource("fs").readFile("config.json")`.
3. All eleven operations with one-line examples.
4. Chroot model — what escapes and how errors surface.
5. Edge cases: binary via base64, `max_file_size`, `find` glob patterns.
6. Link to the spec for details.

- [ ] **Step 1: Write tutorial**
- [ ] **Step 2: Verify every code example compiles by running through `riverpackage validate` on a throwaway bundle**
- [ ] **Step 3: Commit**

```bash
git add docs/guide/tutorials/tutorial-filesystem-driver.md
git commit -m "docs(tutorial): filesystem driver walkthrough"
```

**Validation:**
- Tutorial readable top-to-bottom.
- Code snippets valid TypeScript handler shapes.

---

# Phase 6 — Hardening + Sign-Off

---

### Task 35: Error-model mapping test sweep

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §10 — confirm each error shape maps to the declared `DriverError` variant with the declared message pattern.

- [ ] **Step 1: Table-driven test**

```rust
    #[tokio::test]
    async fn error_mapping_table() {
        let (dir, mut conn) = test_connection();

        // Not found
        let err = conn.execute(&mkq("readFile", &[("path", QueryValue::String("nope.txt".into()))])).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(ref m) if m.contains("not found")));

        // Absolute path
        let err = conn.execute(&mkq("readFile", &[("path", QueryValue::String("/etc/passwd".into()))])).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(ref m) if m.contains("absolute paths not permitted")));

        // Escape
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        // NOTE: '../../outside' canonicalizes outside root
        let err = conn.execute(&mkq("readFile", &[("path", QueryValue::String("../../outside".into()))])).await.unwrap_err();
        assert!(matches!(err, DriverError::Forbidden(_) | DriverError::Query(_)));

        // Unsupported encoding
        std::fs::write(dir.path().join("e.txt"), "x").unwrap();
        let err = conn.execute(&mkq(
            "readFile",
            &[("path", QueryValue::String("e.txt".into())),
              ("encoding", QueryValue::String("utf-16".into()))]
        )).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(ref m) if m.contains("unsupported encoding")));
    }
```

- [ ] **Step 2: Run — PASS**
- [ ] **Step 3: Commit**

```bash
git commit -am "test(filesystem): table-driven error-mapping coverage"
```

**Validation:**
- All 4 error classes observed.

---

### Task 36: `admin_operations()` returns empty

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs`

Spec §11.

- [ ] **Step 1: Test**

```rust
    #[test]
    fn admin_operations_is_empty() {
        let conn = FilesystemConnection { root: std::path::PathBuf::from("/tmp"), max_file_size: 0, max_depth: 0 };
        assert!(conn.admin_operations().is_empty());
    }
```

- [ ] **Step 2: Run — FAIL**
- [ ] **Step 3: Implement**

Add `fn admin_operations(&self) -> &[&str] { &[] }` on `Connection` impl (or whichever trait surface the DDL guard uses — confirm via `Grep("admin_operations", type=rust, path=\"crates/rivers-driver-sdk\")`).

- [ ] **Step 4: PASS**
- [ ] **Step 5: Commit**

```bash
git commit -am "feat(filesystem): admin_operations returns empty (spec §11)"
```

**Validation:**
- Filesystem ops never require `ddl_execute`.

---

### Task 37: Full workspace green sweep + changelog update

**Files:**
- Modify: `todo/changelog.md`, `todo/changedecisionlog.md`

- [ ] **Step 1: Full test run**

Run: `cargo test --workspace --no-fail-fast 2>&1 | tail -50`

Expected: 0 failures. Record the total test count delta in `changelog.md`:

```markdown
### 2026-04-XX — Filesystem driver + OperationDescriptor framework landed
- New crates touched: rivers-driver-sdk, rivers-drivers-builtin, rivers-runtime, rivers-engine-v8.
- New tests: +~45 (driver ops, chroot, proxy codegen, canary roundtrip).
- Spec: rivers-filesystem-driver-spec.md §1–§12.
- Shaping: no new shaping decisions required.
- Canary: canary-filesystem green.
```

- [ ] **Step 2: Decision log**

Append to `changedecisionlog.md` entries for any deviations (e.g. epoch-seconds `mtime` instead of ISO-8601 to avoid adding `chrono` — confirm this is okay or open a follow-up).

- [ ] **Step 3: Commit**

```bash
git add todo/changelog.md todo/changedecisionlog.md
git commit -m "docs(changelog): filesystem driver sign-off"
```

**Validation:**
- Full workspace tests green.
- CI canary job green (if wired).

---

### Task 38: Open-question sweep + follow-ups

**Files:** none.

- [ ] **Step 1: Walk the spec once more**

Re-read `docs/arch/rivers-filesystem-driver-spec.md` and check each section's requirement against a concrete task.

Known deferred items (file a follow-up issue for each; do NOT ship them in this PR):
- `mtime`/`atime`/`ctime` in ISO-8601 (requires `chrono` or `time` workspace dep) — currently epoch-seconds string.
- Windows NTFS junction tests (needs CI runner; add to tracking sheet).
- Concurrent-write stress tests for canary fleet.
- Tutorial updates if `ctx.datasource("fs")` API differs in practice.

- [ ] **Step 2: Open issues in `bugs/` or `todo/` as appropriate**
- [ ] **Step 3: Commit follow-ups**

```bash
git commit -am "chore: record filesystem driver follow-ups"
```

**Validation:**
- Every spec section has either a completed task or a tracked follow-up.

---

# Self-Review Checklist

**Spec coverage map (quick pass):**

| Spec section | Implemented by |
|---|---|
| §2 OperationDescriptor types | Tasks 1–2 |
| §2.2 `operations()` default | Task 3 |
| §2.3 Backward compat | Task 4 |
| §3 V8 typed proxy | Tasks 29–30 |
| §3.3 ParamType validation | Task 30 |
| §4 Filesystem driver shell | Tasks 6, 10, 11 |
| §5 Chroot security | Tasks 7–9, 25 |
| §5.5 UTF-8 paths | Implicit in std::fs use |
| §6 Operation catalog | Task 12 |
| §6.3 readFile encodings | Task 14 |
| §6.4 readDir | Task 15 |
| §6.5 stat (+ Windows mode=0) | Task 16 |
| §6.6 exists | Task 17 |
| §6.7 find | Task 18 |
| §6.8 grep | Task 19 |
| §6.9 writeFile | Task 20 |
| §6.10 mkdir | Task 21 |
| §6.11 delete | Task 22, 26 |
| §6.12 rename | Task 23 |
| §6.13 copy | Task 24 |
| §7 Direct I/O path | Tasks 27–29 |
| §8 Configuration | Task 25 |
| §9 JS handler API | Tasks 29, 31 (canary) |
| §10 Error model | Task 35 |
| §11 Admin ops empty | Task 36 |
| §12 Implementation notes (cross-platform) | Tasks 7–9 (symlinks, path norm), 16 (mode), deferred follow-ups |
| §12.4 Testing | Throughout |
| §12.5 Canary | Tasks 31–32 |
| §12.5 (second) Feature inventory | Task 33 |

**Placeholder scan:** Each task has real code for real files. Unknowns flagged explicitly:
- `Connection::execute` route table uses `NotImplemented(...Task N)` in scaffolding — removed as ops land.
- `resolve_token_for_dispatch` location is pinpointed via a `Grep` in Task 28, not assumed.
- V8 test harness (Task 29) may need scaffolding — noted in Step 3.

**Type consistency:** `FilesystemConnection` gains `max_file_size` and `max_depth` in Task 25 — `test_connection` helper is updated in-place in the same task. No later task references the old 1-field form.

---

## Execution Handoff

Two options:

**1. Subagent-Driven (recommended):** I dispatch a fresh subagent per task, review between tasks for fast iteration.

**2. Inline Execution:** Execute tasks in this session with batched checkpoints.

Which approach?
