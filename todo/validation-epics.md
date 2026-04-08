# Validation Layer — Epic & Sprint Breakdown

**Spec:** `docs/arch/rivers-bundle-validation-spec.md`
**Amendments:** `docs/arch/rivers-bundle-validation-amendments.md`
**Friction:** `docs/arch/rivers-bundle-validation-friction.md`
**Schema:** `docs/arch/rivers-driver-schema-validation-spec.md`

---

## Epic 1: Foundation (ValidationReport + Error Codes + Formatters)

**Goal:** Shared types that all layers depend on.
**Crate:** `rivers-runtime`
**Depends on:** Nothing

### Sprint 1.1 — ValidationReport types
- [ ] Create `crates/rivers-runtime/src/validate_result.rs`
- [ ] `ValidationError` struct: code, severity, message, file_path, toml_path, suggestion
- [ ] `ValidationSeverity` enum: Error, Warning, Info
- [ ] `ValidationReport` struct: layers map, summary (pass/fail/warn counts, exit_code)
- [ ] Error code constants: S001-S010, E001-E005, X001-X013, C001-C008, L001-L005, W001-W004
- [ ] Export from `lib.rs`
- [ ] Unit tests for report builder

### Sprint 1.2 — Text + JSON formatters
- [ ] Create `crates/rivers-runtime/src/validate_format.rs`
- [ ] Text formatter: `[PASS]`/`[FAIL]`/`[WARN]`/`[SKIP]` per check (spec §8)
- [ ] JSON formatter: stable contract matching spec §8 (`summary`, `layers`, `results[]`)
- [ ] "did you mean?" suggestion helper (Levenshtein distance ≤ 2)
- [ ] Unit tests for both formatters

---

## Epic 2: Layer 1 — Structural TOML Validation

**Goal:** Catch typos, unknown keys, missing fields, type mismatches at parse time.
**Crate:** `rivers-runtime`
**Depends on:** E1

### Sprint 2.1 — deny_unknown_fields + TOML parsing
- [ ] Create `crates/rivers-runtime/src/validate_structural.rs`
- [ ] Add `#[serde(deny_unknown_fields)]` to all config structs (per FR-1 field tables)
  - BundleManifest, AppManifest, ResourceDatasource, ResourceKeystore
  - AppDataConfig, DatasourceConfig, DataViewConfig, ApiViewConfig
- [ ] Custom deserializer wrapper that captures unknown field names for "did you mean?"
- [ ] Tests: valid TOML passes, unknown key fails with suggestion

### Sprint 2.2 — Field value validation
- [ ] appId UUID format validation (S007)
- [ ] bundleVersion semver validation (S009)
- [ ] app_type enum validation ("service", "main") (S008)
- [ ] nopassword vs credentials_source mutual exclusion (S006)
- [ ] Required field presence checks (S003)
- [ ] Tests for each error code

---

## Epic 3: Layer 2 — Resource Existence

**Goal:** Verify all referenced files exist on disk.
**Crate:** `rivers-runtime`
**Depends on:** E1

### Sprint 3.1 — File existence checks
- [ ] Create `crates/rivers-runtime/src/validate_existence.rs`
- [ ] Handler module files (.js, .ts, .wasm) (E001)
- [ ] Init handler modules (E001)
- [ ] Schema JSON files (E001) — already partially implemented, migrate
- [ ] SPA root_path and index_file (E002)
- [ ] App directory existence (E003)
- [ ] manifest.toml, resources.toml, app.toml per app (E004, E005)
- [ ] Tests with temp bundle fixtures (per FR-9)

---

## Epic 4: Layer 3 — Cross-Reference Validation

**Goal:** Verify internal references resolve and are consistent.
**Crate:** `rivers-runtime`
**Depends on:** E1

### Sprint 4.1 — Datasource + DataView references
- [ ] Create `crates/rivers-runtime/src/validate_crossref.rs`
- [ ] DataView → datasource reference resolves (X001)
- [ ] View handler → resources[] resolve to declared datasources (X003)
- [ ] Invalidates targets exist as DataView names (X004)
- [ ] Migrate existing checks from `validate.rs`
- [ ] Tests

### Sprint 4.2 — Uniqueness + consistency
- [ ] Duplicate appId across apps (X006)
- [ ] Duplicate datasource names within app (X007)
- [ ] Duplicate DataView names within app (X007)
- [ ] Service dependency → appId resolves within bundle (X005)
- [ ] x-type matches driver name (X011)
- [ ] nopassword=true but credentials_source set (X012)
- [ ] Views exist (warn if empty — W004) (X013)
- [ ] Tests

---

## Epic 5: CLI + Removals

**Goal:** Wire validation into `riverpackage validate`, remove old commands.
**Crate:** `riverpackage`, `riversctl`
**Depends on:** E1-E4

### Sprint 5.1 — Upgrade riverpackage validate
- [ ] Replace current `cmd_validate()` with full 4-layer pipeline
- [ ] Add `--format text|json` flag (default: text)
- [ ] Add `--config <path>` flag for engine discovery (Layer 4)
- [ ] Wire `validate_bundle_full()` → format → print
- [ ] Exit codes: 0 (pass), 1 (errors), 2 (config error), 3 (internal error)
- [ ] Tests

### Sprint 5.2 — Remove old commands
- [ ] Delete `crates/riversctl/src/commands/validate.rs`
- [ ] Remove `validate` match arm from `riversctl/src/main.rs`
- [ ] Remove `--lint` flag from `doctor.rs` (keep `--fix`)
- [ ] Remove `lint_app_conventions()` function
- [ ] Update help text
- [ ] Update `riversctl` docs

### Sprint 5.3 — Backward compatibility
- [ ] Keep `validate_bundle()` and `validate_known_drivers()` as thin wrappers
- [ ] Ensure `riversd` deploy path still calls validation (uses new modules)
- [ ] Integration test: `riverpackage validate address-book-bundle/` passes

---

## Epic 6: Layer 4 — Engine FFI + Syntax Verification

**Goal:** TypeScript/JavaScript compile check, WASM validation, schema JSON validation.
**Crates:** `rivers-engine-v8`, `rivers-engine-wasm`, `rivers-runtime`
**Depends on:** E1, E5

### Sprint 6.1 — Engine dylib FFI contract
- [ ] Create `crates/rivers-runtime/src/validate_engine.rs`
- [ ] `EngineHandle` struct with libloading symbol resolution (per FR-2)
- [ ] Discovery: read `[engines]` from config, scan lib/ dir
- [ ] Load `_rivers_compile_check` and `_rivers_free_string` symbols
- [ ] JSON request/response serialization (per FR-3)
- [ ] Graceful fallback: skip Layer 4 with W002 warning if engines unavailable

### Sprint 6.2 — V8 compile_check export
- [ ] Add `_rivers_compile_check` to `crates/rivers-engine-v8/src/lib.rs`
- [ ] TS → JS transpilation via internal swc (per FR-6)
- [ ] JS syntax validation via V8::Script::Compile
- [ ] Export enumeration from compiled script
- [ ] JSON response: `{"ok":true,"exports":[...]}` or `{"ok":false,"error":{...}}`
- [ ] Add `_rivers_free_string` for heap cleanup
- [ ] Tests

### Sprint 6.3 — Wasmtime compile_check export
- [ ] Add `_rivers_compile_check` to `crates/rivers-engine-wasm/src/lib.rs`
- [ ] WASM module validation via `wasmtime::Module::validate`
- [ ] Export enumeration from WASM module
- [ ] JSON response matching V8 contract
- [ ] Add `_rivers_free_string`
- [ ] Tests

### Sprint 6.4 — Syntax validation module
- [ ] Create `crates/rivers-runtime/src/validate_syntax.rs`
- [ ] Schema JSON validation: parse, check type field, validate field types (C006-C008)
- [ ] Handler module compile check via engine FFI (C001-C002)
- [ ] Export verification: handler entrypoint exists in exports (C002)
- [ ] Import path resolution: relative paths only (per FR-10) (C004-C005)
- [ ] WASM validation: module parse + export check (C003)
- [ ] Tests with fixture .js/.ts/.wasm files

---

## Epic 7: Gate 2 — Deploy-Time Live Validation

**Goal:** Validate with live infrastructure (lockbox, drivers, running services).
**Crate:** `riversd`
**Depends on:** E1-E4

### Sprint 7.1 — VALIDATING state
- [ ] Add `VALIDATING` to deploy state machine (per FR-7, VAL-2)
- [ ] Insert between `PENDING` and `RESOLVING` in `crates/riversd/src/`
- [ ] Log state transition: `app → VALIDATING`
- [ ] On validation failure: app → `FAILED` with collected errors

### Sprint 7.2 — validate_bundle_live()
- [ ] Implement `validate_bundle_live()` in `rivers-runtime`
- [ ] LockBox alias existence check (L001)
- [ ] Driver name → registered driver check (L002)
- [ ] Schema syntax check with live driver `check_schema_syntax()` (L003)
- [ ] x-type → driver type match (L004)
- [ ] Required service health check (L005)
- [ ] Wire into `crates/riversd/src/bundle_loader/load.rs` after config parse

---

## Epic 8: Canary + Documentation

**Goal:** Update canary tests, tutorials, and all affected docs.
**Depends on:** E5

### Sprint 8.1 — Canary test updates
- [ ] Rename `OPS-DOCTOR-LINT-*` → `OPS-VALIDATE-*` (per VAL-7)
- [ ] Add 10 new validation tests in canary-ops (per VAL-9):
  - OPS-VALIDATE-PASS, OPS-VALIDATE-STRUCTURAL-FAIL
  - OPS-VALIDATE-EXISTENCE-FAIL, OPS-VALIDATE-CROSSREF-FAIL
  - OPS-VALIDATE-SYNTAX-FAIL, OPS-VALIDATE-JSON-FORMAT
  - OPS-VALIDATE-EXIT-CODE, OPS-VALIDATE-DID-YOU-MEAN
  - OPS-VALIDATE-ENGINE-SKIP, OPS-VALIDATE-GATE2-LIVE
- [ ] Update canary-fleet-spec test counts (107 → 116)

### Sprint 8.2 — Tutorial + guide updates
- [ ] Create `docs/guide/tutorials/tutorial-bundle-validation.md`
  - How to validate a bundle before deployment
  - Reading validation output (text + JSON)
  - Common errors and how to fix them
  - Engine dylib setup for Layer 4
- [ ] Update `docs/guide/cli.md` — replace `riversctl validate` with `riverpackage validate`
- [ ] Update `docs/guide/installation.md` — validation in deploy workflow
- [ ] Update `docs/guide/developer.md` — validation in app development workflow
- [ ] Update `docs/guide/AI/rivers-skill.md` — new validation commands
- [ ] Update `docs/guide/AI/rivers-app-development.md` — validation step

### Sprint 8.3 — Spec cross-references
- [ ] Apply amendments VAL-1 through VAL-6 to affected spec docs
- [ ] Update `docs/arch/rivers-feature-inventory.md` with validation layer
- [ ] Update `CLAUDE.md` with validation commands and modules
- [ ] Update `README.md` quick reference
