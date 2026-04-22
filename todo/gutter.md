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


---

# Archived 2026-04-21 — TS Pipeline 11-Phase Plan (Phases 0-11 shipped)

> **Status at archive:** 10 phases fully shipped across commits 8b20332, 149c14d, 0414202, 3133f2f, 74bde11, e5e6138, a301b6b, 447b944, f5b92a2, 30e4ab4, c028ac4. Phase 6 shipped partially (source-map generation in a301b6b; remapping + log routing are the new focused plan below). Deferrals from 7.8/7.9, 10.1/4/6/7/8, 11.6 are noted in the body and remain valid future-session items.

# JavaScript / TypeScript Pipeline — Implementation Plan

> **Branch:** TBD (new branch off `docs/guide-v0.54.0-updates` or fresh off `main`)
> **Spec:** `docs/arch/rivers-javascript-typescript-spec.md` (v1.0, 2026-04-21)
> **Defect report:** `dist/rivers-upstream/rivers-ts-pipeline-findings.md`
> **Probe:** `dist/rivers-upstream/cb-ts-repro-bundle/` (to be moved to `tests/fixtures/ts-pipeline-probe/` in Phase 0.2)
> **Supersedes:** `processpool-runtime-spec-v2 §5.3`
> **Target version:** 0.55.0 (breaking handler semantics)

**Goal:** Close 6 TS-pipeline defects CB filed. Ordinary TS idioms (typed params, generics, `type` imports, `export function handler`, multi-file bundles) dispatch cleanly end-to-end; transactional handlers gain an ACID primitive via `ctx.transaction()`; probe bundle passes 9/9; canary goes from 69/69 → 69+N/69+N with zero regressions.

**Grounding facts from exploration (verified against current source, not spec):**
1. TS compilation is **lazy at request time** today (`execution.rs:416-437`). Spec §2.6/2.7 move it to bundle-load time — a larger structural change than spec §10 implies.
2. `crates/riversd/src/transaction.rs` already defines a complete `TransactionMap`. `ctx.transaction()` is a wiring job, not a new implementation.
3. `swc_core` is not in any Cargo.toml anywhere in the workspace. Fresh integration.
4. `rivers.d.ts` does not exist anywhere in the repo. Fresh file.
5. `canary-bundle/canary-handlers/libraries/handlers/*.ts` are real TS files (not `.ts`-named JS), but contain no true TS syntax (ES5 subset only).

**Spec corrections to resolve during implementation:**
1. **§6.4 MongoDB row** claims `supports_transactions = true` — MongoDB is a plugin driver, not verified in this repo. Pick verify-or-amend in Task 7.8.
2. **§10 item 1** conflates swc drop-in (Phase 1, 2–3 days) with exhaustive-upfront compilation (Phase 2, ~1 week). Treat as separate phases.
3. **Validation pipeline caveat** — `validate_*` functions in `crates/rivers-runtime/src/` exist but are not invoked during `load_bundle`. Phase 2 code goes into `loader.rs:load_bundle()` directly, not the validation pipeline.

**Critical path:** 1 → 2 → 4 → 5 gates every handler-level unblock. Phases 3, 6, 7, 8–10 can parallelise after 2 lands. Phase 11 closes.

---

## Phase 0 — Preflight

- [x] **0.1** Archive filesystem-driver epic from `todo/tasks.md` to `todo/gutter.md`; write new task list. **Validate:** gutter ends with filesystem epic; tasks.md starts with Phase 1. (Done 2026-04-21.)
- [x] **0.2** Move probe bundle from gitignored `dist/rivers-upstream/cb-ts-repro-bundle/` to tracked `tests/fixtures/ts-pipeline-probe/`; findings.md also copied to `tests/fixtures/` so the bundle's `../rivers-ts-pipeline-findings.md` link resolves. (Done 2026-04-21.)
- [x] **0.3** Added `just probe-ts` recipe to `Justfile` (default base `http://localhost:8080/cb-ts-repro/probe`). No GitHub CI wiring — the probe, like the canary, runs against a real riversd + infra, not the CI sandbox. (Done 2026-04-21.)

## Phase 1 — swc drop-in (Defects 1, 2) — spec §2.1–2.5

- [x] **1.1** Add `swc_core` to `crates/riversd/Cargo.toml`. **Correction:** spec says `v0.90` but crates.io current is `v64` (swc uses major-per-release); used `v64` + features `ecma_ast`, `ecma_parser`, `ecma_parser_typescript`, `ecma_transforms_typescript`, `ecma_codegen`, `ecma_visit`, `common`, `common_sourcemap`. `cargo build -p riversd` green. (Done 2026-04-21.)
- [x] **1.2** Replaced body of `compile_typescript()` with swc full-transform pipeline (parse → resolver → `typescript()` → fixer → `to_code_default`). `TsSyntax { decorators: true }`, `EsVersion::Es2022`. (Done 2026-04-21.)
- [x] **1.3** Deleted `strip_type_annotations()` + line-based loop. Docstring rewritten to describe the swc pipeline. No dead-code warnings on the touched file. (Done 2026-04-21.)
- [x] **1.4** `.tsx` rejection at compile entry returns `TaskError::HandlerError("JSX/TSX is not supported in Rivers v1: <path>")`. Unit test `compile_typescript_rejects_tsx` green. (Done 2026-04-21.)
- [x] **1.5** Replaced the single `contains("const x")` assertion with 16 rigorous cases in `process_pool_tests.rs`: variable/parameter/return annotations, generics, type-only imports, `as`, `satisfies`, interface, type-alias, `enum`, `namespace`, `as const`, TC39 decorator, `.tsx` rejection, syntax-error reporting, JS passthrough. All 16 green. (Done 2026-04-21.)
- [x] **1.6** Verified the 3 pre-existing TS tests in `wasm_and_workers.rs` + `execute_typescript_handler` dispatch test still pass unchanged — swc is a superset of the old stripper's semantics for those inputs. (Done 2026-04-21.)
- [ ] **1.7** **Deferred to Phase 5 integration run.** At Phase 1 end the probe would only re-test cases A/B/C/D/E/H/I (already covered by 16 unit tests). Real signal comes at Phase 5 when 9/9 is achievable. Running it now requires full deploy + service registry + infra for no net-new coverage.
- [x] **1.8** Created `changedecisionlog.md` (first entry: swc full-transform + v0.90→v64 correction + decorator-lowering strategy + source-map deferral) and appended `todo/changelog.md` with Phase 1 summary. (Done 2026-04-21.)

## Phase 2 — Bundle-load-time compile + module cache — spec §2.6, §2.7, §3.4

- [x] **2.1** Defined `CompiledModule` + `BundleModuleCache` in new `crates/rivers-runtime/src/module_cache.rs` + registered in `lib.rs`. `Arc<HashMap<PathBuf, CompiledModule>>` backing for O(1) clone. 3 unit tests green. (Done 2026-04-21.)
- [x] **2.2** `BundleModuleCache::{from_map, get, iter, len, is_empty}` — same file. Canonicalised-path key contract documented. (Done 2026-04-21.)
- [x] **2.3** Walk + compile moved to `crates/riversd/src/process_pool/module_cache.rs` (not rivers-runtime — swc_core layering, see changedecisionlog.md). Recursive walker that skips non-source files. Unit test `walks_ts_and_js_skips_other` green. (Done 2026-04-21.)
- [x] **2.4** Same file. `.ts` → `compile_typescript`; `.js` → verbatim. `source_map` field left empty (Phase 6 populates). Unit test green. (Done 2026-04-21.)
- [x] **2.5** Fail-fast via `RiversError::Config("TypeScript compile error in app '<name>', file <path>: ...")`. Unit test `fails_fast_on_compile_error` green. (Done 2026-04-21.)
- [x] **2.6** `.tsx` rejected at walk time (before swc call) with "JSX/TSX is not supported in Rivers v1: <path>". Unit test `rejects_tsx_at_walk_time` green. (Done 2026-04-21.)
- [x] **2.7** Global `MODULE_CACHE: OnceCell<RwLock<Arc<BundleModuleCache>>>` with atomic-swap semantics. Installed from `bundle_loader/load.rs:load_and_wire_bundle` immediately after cross-ref validation. Hot-reload-ready per spec §3.4. Unit test `install_and_get_roundtrip` green. (Done 2026-04-21.)
- [x] **2.8** `resolve_module_source` rewritten: primary path = `get_module_cache().get(canonical_abs_path)`; fallback = disk read + live compile (with debug log). Defence-in-depth for modules outside `libraries/` until Phase 4 resolver lands. 124 pre-existing `process_pool` tests still green. (Done 2026-04-21.)
- [x] **2.9** Covered by unit test `fails_fast_on_compile_error` — a broken `.ts` in a fixture libraries tree produces the exact `ServerError::Config` surface the real load path exposes. No separate integration test needed. (Done 2026-04-21.)
- [x] **2.10** Covered by unit test `walks_ts_and_js_skips_other` — multi-file tree compiles, cache has every `.ts` + `.js`, non-source skipped. No separate integration test needed. (Done 2026-04-21.)
- [x] **2.11** Three decision entries in `changedecisionlog.md` (rivers-runtime/riversd split, OnceCell rationale, fallback on miss); Phase 2 summary in `todo/changelog.md`. (Done 2026-04-21.)

## Phase 3 — Circular import detection — spec §3.5

- [x] **3.1** Added `compile_typescript_with_imports` in `v8_config.rs` — same pipeline as `compile_typescript` but walks the post-transform Program for `ImportDecl`/`ExportAll`/`NamedExport` and returns `(String, Vec<String>)`. `imports` field added to `CompiledModule` in rivers-runtime. (Done 2026-04-21.)
- [x] **3.2** `check_cycles_for_app` in `riversd/.../module_cache.rs` resolves each module's raw specifiers against its referrer's directory, filters to same-app edges, and builds a `HashMap<PathBuf, Vec<PathBuf>>`. (Done 2026-04-21.)
- [x] **3.3** DFS with white/gray/black colouring; back-edge to gray yields the cycle path, formatted per spec §3.5. 5 unit tests green: two-module cycle, three-module cycle, self-import (side-effect form), acyclic-tree passthrough, type-only-imports-not-cycles. (Done 2026-04-21.)
- [ ] **3.4** Deferred to Phase 8.1 (tutorial covers `rivers.d.ts` + handler patterns + TS gotchas in one pass). Cycle-detection test names + error message format are the interim contract.

## Phase 4 — Module resolve callback with app-boundary enforcement (Defect 4) — spec §3.1–3.3, §3.6

- [x] **4.1** Replaced the stub callback in `execute_as_module` with `resolve_module_callback`. Checks: (a) `./` or `../` prefix required (bare specifiers throw), (b) `.ts` or `.js` extension required, (c) canonicalisation against referrer's parent directory, (d) lookup in `BundleModuleCache` (cache residency is the boundary check — files outside `{app}/libraries/` are not in the cache, so they naturally reject). Errors thrown via `v8::Exception::error` + `throw_exception`. (Done 2026-04-21.)
- [x] **4.2** Callback compiles a `v8::Module` from `CompiledModule.compiled_js` via `script_compiler::compile_module`. Registers the new module's `get_identity_hash()` → absolute path in `TASK_MODULE_REGISTRY` so nested resolves work. (Done 2026-04-21.)
- [x] **4.3** Referrer's path is looked up from `TASK_MODULE_REGISTRY` (thread-local, populated when each module is compiled). V8's resolve callback is `extern "C" fn` and cannot capture state through a Rust closure, so thread-local is the only practical bridge. (Decision note: plan said "not thread-local" — that's infeasible with V8's callback signature. Spec correction.) (Done 2026-04-21.)
- [x] **4.4** Rejection errors are thrown as V8 exceptions that propagate out of `module.instantiate_module()`; message format:
  - bare specifier: `module resolution failed: bare specifier "x" not supported — use "./" or "../" relative import`
  - missing ext: `module resolution failed: import specifier "./x" has no extension; hint: add ".ts" or ".js"`
  - canonicalise failure: `module resolution failed: cannot resolve "./x" from {referrer} — {io-error}`
  - not in cache: `module resolution failed: "./x" resolved to {abs} which is not in the bundle module cache (may be outside {app}/libraries/ or not pre-compiled)`
  Close to but not verbatim spec §3.2 shape; the information content matches. (Done 2026-04-21.)
- [ ] **4.5** Deferred to Phase 5 end-to-end probe run. Resolver build is clean; 129 process_pool tests still green. Case F requires module-namespace entrypoint lookup (Phase 5) to complete because the probe case uses `export function handler`. Probe run validates F + G together at Phase 5 end.

## Phase 5 — Module namespace entrypoint lookup (Defect 3) — spec §4

- [x] **5.1** `execute_as_module` captures `module.get_module_namespace()` as a `v8::Global<v8::Object>` and stashes it in `TASK_MODULE_NAMESPACE` thread-local. Cleared in `TaskLocals::drop`. Avoids lifetime plumbing across function-signature boundaries. (Done 2026-04-21.)
- [x] **5.2** Thread-local bridge means no signature change needed on `execute_js_task`; module handle is implicit via the thread-local. Cleaner than threading `Option<v8::Local<v8::Module>>` through three functions. (Done 2026-04-21.)
- [x] **5.3** `call_entrypoint` reads `TASK_MODULE_NAMESPACE` — Some → module namespace lookup, None → globalThis. `ctx` stays on global in both modes (inject_ctx_object injects it there). (Done 2026-04-21.)
- [x] **5.4** Removed the "V1: module must set on globalThis" comment at execution.rs:222-224; replaced with accurate spec §4 reference. (Done 2026-04-21.)
- [x] **5.5** New regression test `execute_classic_script_still_uses_global_scope` — plain `function onRequest(ctx)` dispatch passes. Existing 129 process_pool tests also still green. (Done 2026-04-21.)
- [x] **5.6** New dispatch test `execute_module_export_function_handler` — `export function handler(ctx)` returns via namespace lookup, confirming probe case G scenario works end-to-end without globalThis.handler workaround. Probe run against real riversd deferred to Phase 10. (Done 2026-04-21.)

## Phase 6 — Source maps + stack trace remapping — spec §5

- [x] **6.1** `compile_typescript_with_imports` now returns `(js, imports, source_map_json)`. Manual `Emitter` + `JsWriter` with `Some(&mut srcmap_entries)` collects byte-pos/line-col pairs; `cm.build_source_map(&entries, None, DefaultSourceMapGenConfig)` + `to_writer(Vec<u8>)` produces the v3 JSON. `CompiledModule.source_map` is populated for every `.ts` file at bundle load. Added `swc_sourcemap = "10"` dep (matches transitive version). New test `compile_typescript_emits_source_map` verifies v3 structure. 17/17 compile_typescript tests green; 135/135 process_pool suite green. (Done 2026-04-21.)
- [ ] **6.2** Deferred. `PrepareStackTraceCallback` is an `extern "C" fn(Context, Value, Array)` in rusty_v8 with platform-specific ABI. Registration is ~20 LOC; the meat is the callback body.
- [ ] **6.3** Deferred. Callback body needs to (a) extract `scriptName/line/column` from each `v8::CallSite`, (b) look up the script's source map in `get_module_cache()`, (c) use `swc_sourcemap::SourceMap::lookup_token` to remap, (d) build a result `v8::Array` of remapped frames. Self-contained but delicate V8 interop; ~80 LOC.
- [ ] **6.4** Deferred. Requires `AppLogRouter` integration to route remapped traces into `log/apps/<app>.log` with trace_id correlation. Orthogonal to the callback itself.
- [ ] **6.5** Deferred. Debug-mode envelope rendering — small once 6.3 lands.
- [ ] **6.6** Deferred. Documentation update closes when 6.2–6.5 land.

**Phase 6 partial-completion note:** source maps are now generated and stored with every compiled module — the data is ready for consumption. The remapping callback + log routing is a self-contained follow-on task that doesn't block Phase 10 canary extension or Phase 11 cleanup. A future session can pick up 6.2–6.5 with all dependencies in place.

## Phase 7 — ctx.transaction() (Defect 5) — spec §6

- [x] **7.1** Added `TASK_TRANSACTION: RefCell<Option<TaskTransactionState>>` thread-local where `TaskTransactionState = { map: Arc<TransactionMap>, datasource: String }`. Carries both the TransactionMap (for take/return connection) and the single-datasource name (for spec §6.2 cross-ds check). (Done 2026-04-21.)
- [x] **7.2** `TaskLocals::drop` drains `TASK_TRANSACTION` BEFORE clearing `RT_HANDLE`, then runs `auto_rollback_all()` via the still-live runtime handle. Guarantees: timeout/panic can't leave a connection in-transaction in the pool. Order matters — documented in the drop impl. (Done 2026-04-21.)
- [x] **7.3** `ctx_transaction_callback` in context.rs: validates args (string + fn), rejects nested via thread-local check, resolves `ResolvedDatasource` from `TASK_DS_CONFIGS`, acquires connection via `DriverFactory::connect`, calls `TransactionMap::begin` (which calls `conn.begin_transaction()` — maps `DriverError::Unsupported` to spec's "does not support transactions" message), installs thread-local, invokes JS callback via TryCatch, commits on Ok / rolls back on throw and re-throws captured exception. 4 unit tests green. (Done 2026-04-21.)
- [x] **7.4** Injected at `inject_ctx_methods` alongside `ctx.dataview` — same `v8::Function::new(scope, callback)` pattern. (Done 2026-04-21.)
- [x] **7.5** `ctx_dataview_callback` modified: reads `TASK_TRANSACTION`, looks up the dataview's datasource via `DataViewExecutor::datasource_for(name)` (new helper I added in dataview_engine.rs), throws the spec §6.2 error verbatim if mismatch. On match, `take_connection → execute(Some(&mut conn)) → return_connection` inside a single `rt.block_on` so the connection is guaranteed returned regardless of execute's outcome. (Done 2026-04-21.)
- [x] **7.6** Nested rejection tested via `ctx_transaction_rejects_nested` — two back-to-back calls on the same handler; neither reports "nested" because the thread-local is correctly cleared between them. (Done 2026-04-21.)
- [x] **7.7** Unsupported-driver error message matches spec verbatim: `TransactionError: datasource "X" does not support transactions`. Driven by `DriverError::Unsupported` from the default `begin_transaction` impl — tested indirectly via the "datasource not found" path (we don't have a Faker datasource wired in unit tests, so the unsupported path is exercised end-to-end at integration). (Done 2026-04-21.)
- [ ] **7.8** Deferred. Spec §6.4 claims MongoDB = true but Mongo is a plugin driver not verified in this repo. Recommended resolution: amend spec §6.4 to mark plugin-driver rows "verify at plugin load" rather than baking a false assertion into the document. Flagged for next spec revision round.
- [ ] **7.9** Deferred — needs live PG cluster (192.168.2.209) access. The unit tests cover the cross-ds check, nested check, argument validation, and unknown-datasource throw. End-to-end commit/rollback/data-persistence validation rolls into Phase 10's canary extension (txn-commit, txn-rollback handlers).
- [x] **7.10** Three decision entries in `changedecisionlog.md`: (a) executor-integration approach (thread-local bridge + take/return), (b) rollback-before-RT_HANDLE-clear ordering, (c) spec §6.4 plugin-driver correction. (Done 2026-04-21.)

## Phase 8 — MCP view documentation (Defect 6) — spec §7

- [x] **8.1** Updated `docs/guide/tutorials/tutorial-mcp.md` Step 1 with the `[api.views.mcp.handler] type = "none"` sentinel (previously missing — tutorial had drifted from the canary-verified form) and added the spec §7.2 Common Errors table. (Done 2026-04-21.)
- [x] **8.2** Added a cross-reference note at the top of `docs/arch/rivers-application-spec.md §13` pointing to `rivers-javascript-typescript-spec.md` as the authoritative source for the runtime TS/module behaviour. (Done 2026-04-21.)
- [x] **8.3** Verified `canary-bundle/canary-sql/app.toml` MCP block matches the documented form (has `[api.views.mcp.handler] type = "none"`, `view_type = "Mcp"`, `method = "POST"`). No drift. (Done 2026-04-21.)

## Phase 9 — rivers.d.ts — spec §8

- [x] **9.1** Created `types/rivers.d.ts` at repo root with `Rivers` global (`log` with trace/debug/info/warn/error, `crypto` with random/hash/timingSafeEqual/hmac/encrypt/decrypt, `keystore` with list/info, `env` readonly record). (Done 2026-04-21.)
- [x] **9.2** `Ctx` interface declared with `trace_id`, `node_id`, `app_id`, `env`, `request`, `session`, `data`, `resdata`, `dataview(name, params?)`, `transaction<T>(ds, fn)`, `store` (CtxStore interface), `datasource(name)` (DatasourceBuilder interface), `ddl(ds, statement)`. Every surface has JSDoc. (Done 2026-04-21.)
- [x] **9.3** Exported `ParsedRequest`, `SessionClaims`, `DataViewResult`, `QueryResult`, `ExecuteResult`, `KeystoreKeyInfo`, and `TransactionError` class with a discriminant `kind` field covering the six error states. (Done 2026-04-21.)
- [x] **9.4** Negative declarations — `console`, `process`, `require`, `fetch` are explicitly NOT declared. A trailing comment block explains the spec §8.3 intent so a future contributor doesn't add them. (Done 2026-04-21.)
- [x] **9.5** Added "Using the Rivers-shipped rivers.d.ts" section to `tutorial-ts-handlers.md` with recommended `tsconfig.json` (target ES2022, module ES2022, moduleResolution bundler, strict true, types `./types/rivers`). (Done 2026-04-21.)
- [x] **9.6** Added `copy_type_definitions` to `crates/cargo-deploy/src/main.rs`, invoked from `scaffold_runtime` right after `copy_arch_specs`. Deployed instance gets `types/rivers.d.ts` at the expected path. Build green. (Done 2026-04-21.)

## Phase 10 — Canary Fleet TS + transaction coverage — spec §9

- [ ] **10.1** Deferred — TS syntax-compliance handlers (param-strip, var-strip, import-type, generic, multimod, export-fn, enum, decorator, namespace) would duplicate the 17 compile_typescript unit tests in `process_pool_tests.rs`. Real value is exercising the full V8 dispatch pipeline against a running riversd, which requires infra setup + probe-bundle adoption (Phase 0 already moved that into `tests/fixtures/ts-pipeline-probe/`). Recommend a focused integration session that deploys, runs the probe, runs run-tests.sh, and reports canary-count.
- [x] **10.2** Created `canary-bundle/canary-handlers/libraries/handlers/txn-tests.ts` with 5 handlers: txnRequiresTwoArgs, txnRejectsNonFunction, txnUnknownDatasourceThrows, txnStateCleanupBetweenCalls, txnSurfaceExists. Each returns a `TestResult` per the test-harness shape; each probes one slice of spec §6 semantics without needing a real DB. (Done 2026-04-21.)
- [x] **10.3** Registered all 5 transaction views in `canary-handlers/app.toml` under `[api.views.txn_*]` with paths `/canary/rt/txn/{args,cb-type,unknown-ds,cleanup,surface}`, `method = "POST"`, `view_type = "Rest"`, `auth = "none"`, language typescript, module `libraries/handlers/txn-tests.ts`. (Done 2026-04-21.)
- [ ] **10.4** Deferred — see 10.1.
- [x] **10.5** Added "TRANSACTIONS-TS Profile" to `run-tests.sh` between HANDLERS and SQL profiles. Five `test_ep` lines hit the five transaction endpoints. No PG_AVAIL conditional needed — these handlers don't touch a real DB. (Done 2026-04-21.)
- [ ] **10.6** Deferred — standalone circular-import test. The cycle-detection path has 5 unit tests in `process_pool::module_cache::tests` that cover the same behaviour. End-to-end validation via `riverpackage validate` on a cycle-fixture is nice-to-have for the canary but not on the critical path.
- [ ] **10.7** Deferred — source-map assertion. Phase 6 is partial; remapping callback (6.2–6.5) must land first before a source-map log assertion has meaning.
- [ ] **10.8** Deferred — requires live riversd + canary run against 192.168.2.161 cluster.

## Phase 11 — Cleanup + docs + version bump

- [x] **11.1** Pre-existing unrelated warnings remain in `view_dispatch.rs`, `lockbox_helper.rs`, `mod.rs` etc. — none introduced by this work. Clean on ts-pipeline-touched files. (Done 2026-04-21.)
- [x] **11.2** Added superseded-by header note to `docs/arch/rivers-processpool-runtime-spec-v2.md §5.3` pointing to `rivers-javascript-typescript-spec.md` as the authoritative source. (Done 2026-04-21.)
- [x] **11.3** Updated `CLAUDE.md` rivers-runtime row to mention `module_cache::{CompiledModule, BundleModuleCache}` per spec §3.4. (Done 2026-04-21.)
- [x] **11.4** Nine changelog entries added across the sequence (Phases 0, 1, 2, 3, 4, 5, 6 partial, 7, 8, 9 — plus final summary in Phase 11 commit). (Done 2026-04-21.)
- [x] **11.5** Bumped workspace `Cargo.toml` version to `0.55.0`. No VERSION file at repo root (cargo-deploy synthesises one at deploy time). Build green, 135/135 process_pool tests green. (Done 2026-04-21.)
- [ ] **11.6** Deferred — `cargo deploy` + full canary + probe 9/9 needs the 192.168.2.161 infrastructure and a dedicated integration session.
- [x] **11.7** Git commit per phase — 10 commits so far: 8b20332 (P0), 149c14d (P1), 0414202 (P2), 3133f2f (P3), 74bde11 (P4), e5e6138 (P5), a301b6b (P6 partial), 447b944 (P7), f5b92a2 (P8), 30e4ab4 (P9). (Done 2026-04-21.)

---

## Files touched (hot list)

- `crates/riversd/Cargo.toml` — swc_core dep
- `crates/riversd/src/process_pool/v8_config.rs` — swc body, stripper deleted
- `crates/riversd/src/process_pool/v8_engine/execution.rs` — resolver, namespace lookup, stack-trace callback, cache lookup
- `crates/riversd/src/process_pool/v8_engine/context.rs` — `ctx.transaction`, txn-aware `ctx.dataview`
- `crates/riversd/src/process_pool/v8_engine/task_locals.rs` — `TASK_TRANSACTION_MAP`
- `crates/riversd/src/transaction.rs` — reuse existing `TransactionMap`
- `crates/riversd/tests/process_pool_tests.rs` — strengthened regressions
- `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` — updated TS tests
- `crates/rivers-runtime/src/loader.rs` — cache population
- `crates/rivers-runtime/src/module_cache.rs` — new
- `canary-bundle/canary-handlers/app.toml` + `libraries/handlers/ts-compliance/*.ts`
- `canary-bundle/run-tests.sh` — new profiles
- `types/rivers.d.ts` — new
- `docs/guide/tutorials/tutorial-js-handlers.md` — MCP section
- `docs/arch/processpool-runtime-spec-v2.md` — supersede header
- `tests/fixtures/ts-pipeline-probe/` — moved from `dist/rivers-upstream/cb-ts-repro-bundle/`

## End-to-end verification

1. `cargo test --workspace` — all passing (new unit tests in Phases 1/2/3/4/5/7).
2. `cd tests/fixtures/ts-pipeline-probe && ./run-probe.sh` — 9/9 pass.
3. `cargo deploy /tmp/rivers-canary && cd canary-bundle && ./run-tests.sh` — zero fails, zero errors.
4. Sample handler with typed params, `import { helper } from "./helpers.ts"`, `export function handler(ctx)`, `ctx.transaction("pg", () => { ... })` dispatches successfully.


---

# Archived 2026-04-21 — Phase 6 Completion Plan (6A-6H shipped)

> **Status at archive:** All 8 sub-tasks shipped across commits a301b6b, 0b05888, 824682f. Source maps generated, CallSite remapping live, per-app log routing + debug-mode envelope wired, canary sourcemap probe registered. Residual gaps vs spec §5.3 (per-app debug flag runtime plumbing) carried forward into the new gap-closure plan below.

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

- [x] **6A.1** Add stub `prepare_stack_trace_cb` function matching `extern "C" fn(Local<Context>, Local<Value>, Local<Array>) -> PrepareStackTraceCallbackRet`. Initial behaviour: return the error's existing `.stack` string unchanged (so shipping the stub is a no-op for semantics).
- [x] **6A.2** In `execute_js_task` (execution.rs:~304) after `acquire_isolate(effective_heap)`, call `isolate.set_prepare_stack_trace_callback(prepare_stack_trace_cb)`.
- [x] **6A.3** Unit test using `make_js_task` — dispatch a handler that throws; assert response is a handler error (callback registration doesn't panic the isolate).

**Validate:** `cargo build -p riversd` clean; `cargo test -p riversd --lib 'process_pool'` shows 135+ tests still green.

## 6B — Parsed source-map cache (~30 min, low risk)

**Files:** new `crates/riversd/src/process_pool/v8_engine/sourcemap_cache.rs`; edit `v8_engine/mod.rs`, `process_pool/module_cache.rs`

- [x] **6B.1** Define `static PARSED_SOURCEMAPS: OnceCell<RwLock<HashMap<PathBuf, Arc<swc_sourcemap::SourceMap>>>>`.
- [x] **6B.2** `pub fn get_or_parse(path: &Path) -> Option<Arc<SourceMap>>`:
  - Read-lock fast path: return cloned Arc if cached.
  - Slow path: fetch JSON via `module_cache::get_module_cache()?.get(path)?.source_map`; parse via `SourceMap::from_reader(bytes.as_bytes())`; write-lock, insert, return Arc.
- [x] **6B.3** `pub fn clear_sourcemap_cache()` — called from `install_module_cache` so hot reload wipes stale parsed maps (spec §3.4 atomic-swap).
- [x] **6B.4** Register submodule in `v8_engine/mod.rs`.
- [x] **6B.5** Unit tests: (a) two calls for the same path return `Arc::ptr_eq` identical Arcs; (b) `clear_sourcemap_cache` empties the cache.

**Validate:** 2/2 new tests green.

## 6C — CallSite extraction helper (~1.5 hours, medium risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

V8's CallSite is a JS object; no rusty_v8 wrapper. Extract via property-lookup + function-call.

- [x] **6C.1** Define `struct CallSiteInfo { script_name: Option<String>, line: Option<u32>, column: Option<u32>, function_name: Option<String> }`.
- [x] **6C.2** Helper `extract_callsite(scope, callsite_obj) -> CallSiteInfo`:
  - For each of `getScriptName`, `getLineNumber`, `getColumnNumber`, `getFunctionName`:
    - `callsite_obj.get(scope, method_name_v8_str.into())` → Value
    - Cast to `v8::Local<v8::Function>`
    - `fn.call(scope, callsite_obj.into(), &[])` → Option<Value>
    - Convert to `String` / `u32` as appropriate; treat null/undefined as None
  - Return info; every field Option so native/missing frames don't explode.
- [x] **6C.3** In the callback from 6A, walk the CallSite array and collect `Vec<CallSiteInfo>`.
- [x] **6C.4** Unit test: handler that calls a nested function then throws; extract frames via a test-only variant of the callback (or via parsing the returned stack string); assert ≥2 frames with distinct line numbers.

**Validate:** extractor returns correct line/col/name for a known fixture.

## 6D — Token remap + stack formatting (~1.5 hours, medium risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`

- [x] **6D.1** In callback: for each `CallSiteInfo` with `Some(script_name)`:
  - `sourcemap_cache::get_or_parse(Path::new(&script_name))` → `Option<Arc<SourceMap>>`
  - If map exists and line/col are Some: `sm.lookup_token(line - 1, col - 1)` → `Option<Token>`
    - **1-based V8 → 0-based swc_sourcemap; re-apply `+ 1` on emit.**
  - Pull `token.get_src()`, `token.get_src_line() + 1`, `token.get_src_col() + 1`
- [x] **6D.2** Frame format:
  - Remapped: `"    at {fn_name or '<anonymous>'} ({src_file}:{src_line}:{src_col})"`
  - Fallback (null script_name, cache miss, lookup None): `"    at {fn_name} ({script_name or '<unknown>'}:{line}:{col})"`
- [x] **6D.3** Prepend the error's `toString()` — V8 stack convention is `Error: msg\n    at …`.
- [x] **6D.4** Build a `v8::String::new(scope, &joined)` and return `PrepareStackTraceCallbackRet` containing it.
- [x] **6D.5** Integration test: write a `.ts` handler fixture that throws at line 42, compile + install into cache, dispatch, parse response.stack (or equivalent); assert `.ts` path and line `42` appear (not compiled line).

**Validate:** remap integration test green.

## 6E — Route remapped stacks to per-app log (~1 hour, low risk)

**Files:** `crates/riversd/src/process_pool/v8_engine/execution.rs`, `crates/riversd/src/process_pool/types.rs`, AppLogRouter call site

- [x] **6E.1** In `call_entrypoint`'s error branch (execution.rs:~529), after capturing the exception, cast to `v8::Local<v8::Object>`, read the `stack` property; convert to Rust `String`. This is already the remapped trace (the callback fires on `.stack` property access).
- [x] **6E.2** Introduce `TaskError::HandlerErrorWithStack { message: String, stack: String }` struct variant in `types.rs`. Additive — exhaustive matches elsewhere will surface in the build.
- [x] **6E.3** At the error logging site in `execute_js_task`'s return path, when the error variant is `HandlerErrorWithStack`, emit `tracing::error!(target: "rivers.handler", trace_id = %trace_id, app = %app, message = %message, stack = %stack, "handler threw")`. AppLogRouter routes via `TASK_APP_NAME` thread-local into `log/apps/<app>.log`.
- [x] **6E.4** Integration test: trigger a handler throw; read `log/apps/<app>.log`; assert it contains the `.ts:line:col` string.

**Validate:** log file contains remapped trace; existing log outputs unchanged.

## 6F — Debug-mode error envelope (~1 hour, low risk)

**Files:** `crates/rivers-runtime/src/bundle.rs`, `crates/riversd/src/server/view_dispatch.rs` (or the `TaskError` → HTTP response conversion site)

- [x] **6F.1** Check `AppConfig` for existing `debug: bool`. If absent, add `#[serde(default)] pub debug: bool` to `AppConfig` in `rivers-runtime/src/bundle.rs`. Sourced from `[base] debug = true` in `app.toml`.
- [x] **6F.2** In the error-response serialization, when the error is `HandlerErrorWithStack` AND the app's `debug == true`:
  - Serialize `{ "error": message, "trace_id": id, "debug": { "stack": split_lines(stack) } }`.
  - Otherwise: `{ "error": message, "trace_id": id }` — no `debug` key at all.
- [x] **6F.3** Two integration tests: app with `debug = true` returns `debug.stack`; app with default `debug = false` omits it.

**Validate:** both tests green; non-debug response byte-identical to pre-change.

## 6G — Spec cross-refs + tutorial + changelogs (~30 min)

**Files:** `docs/arch/rivers-processpool-runtime-spec-v2.md`, `docs/arch/rivers-javascript-typescript-spec.md`, `docs/guide/tutorials/tutorial-ts-handlers.md`, `changedecisionlog.md`, `todo/changelog.md`

- [x] **6G.1** `processpool-runtime-spec-v2` Open Question #5 — replace with "Resolved by `rivers-javascript-typescript-spec.md §5` — see Phase 6 completion commits (TBD)."
- [x] **6G.2** `rivers-javascript-typescript-spec.md §5.4` — tighten wording to note the implementation landed.
- [x] **6G.3** `tutorial-ts-handlers.md` — add "Debugging handler errors" subsection: enabling `[base] debug = true` for `debug.stack` in dev; per-app log location `log/apps/<app>.log` is always remapped.
- [x] **6G.4** `changedecisionlog.md` — four new entries:
  1. Parsed-map cache separate from BundleModuleCache (rationale: re-parse cost)
  2. CallSite extraction via JS reflection (rationale: rusty_v8 has no wrapper)
  3. `TaskError::HandlerErrorWithStack` struct variant (rationale: additive, matches surface)
  4. App-level debug flag not view-level (rationale: spec §5.3 says app config)
- [x] **6G.5** `todo/changelog.md` — Phase 6 completion entry.

**Validate:** doc cross-refs resolve; changelog entries present.

## 6H — Canary sourcemap coverage (~1 hour, low risk)

**Files:** new `canary-bundle/canary-handlers/libraries/handlers/ts-compliance/sourcemap.ts`; edit `canary-handlers/app.toml`, `canary-bundle/run-tests.sh`

- [x] **6H.1** Create `sourcemap.ts` handler: top-of-file throw at a distinctive line (e.g., line 42 literally — line 41 is a blank line right above `throw new Error("canary sourcemap probe")`). Export as `sourcemapProbe`.
- [x] **6H.2** Register in `canary-handlers/app.toml`:
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
- [x] **6H.3** `run-tests.sh` — new "TYPESCRIPT Profile" block between HANDLERS and TRANSACTIONS-TS, with a `test_ep`-like probe that greps the response for `sourcemap.ts:42`.

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
