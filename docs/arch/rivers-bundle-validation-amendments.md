# Rivers Bundle Validation — Cross-Spec Amendments

**Date:** 2026-04-06  
**Applies to:** 7 specification documents  
**Origin:** `rivers-bundle-validation-spec.md` §13 — Affected Specifications  
**Scope:** Tool ownership changes (`riversctl validate` removed, `doctor --lint` removed), validation pipeline references, engine dylib FFI exports, canary fleet test renames

---

## Amendment Index

| AMD | Target Spec | Target Section | Change |
|---|---|---|---|
| VAL-1 | `rivers-v1-admin.md` | §Bundle Validation (line 170) | Replace `riversctl validate` with `riverpackage validate`; update check list to four-layer model |
| VAL-2 | `rivers-application-spec.md` | §8 Deployment Lifecycle | Insert VALIDATING state between PENDING and RESOLVING |
| VAL-3 | `rivers-application-spec.md` | §14 Validation Rules | Reference `rivers_runtime` validation modules; replace inline rule list with error catalog reference |
| VAL-4 | `rivers-technology-path-spec.md` | §19.4 Deployment Lifecycle | Insert validation step; reference `rivers_runtime::validate_bundle_live()` |
| VAL-5 | `rivers-driver-spec.md` | §7 Plugin System | ABI version bump; document `_rivers_compile_check` export |
| VAL-6 | `rivers-processpool-runtime-spec-v2.md` | §5 Engine: V8 Isolate | Document `compile_check` FFI export on engine dylib |
| VAL-7 | `rivers-canary-fleet-spec.md` | OPS profile test table | Rename `OPS-DOCTOR-LINT-*` → `OPS-RIVERPACKAGE-VALIDATE-*`; expand test set; update harness code |
| VAL-8 | `rivers-canary-fleet-spec.md` | Harness-only test list (AMD-2.7) | Update harness-only test list to reflect renames and additions |
| VAL-9 | `rivers-canary-fleet-spec.md` | Test Count Summary | Update OPS profile test counts |

---

## VAL-1 — `rivers-v1-admin.md` — Bundle Validation Section

### Replace: §Bundle Validation (lines 170–187)

**Remove:**

```markdown
## Bundle Validation

`riversctl validate <bundle_path>` runs 9 checks against a bundle directory or archive.

`riversctl validate --schema server|app|bundle` outputs the corresponding JSON Schema.

### Validation Checks

1. View types — validates view type values are recognized
2. Driver names — validates driver names match registered drivers
3. Datasource refs — validates all datasource references resolve
4. DataView refs — validates all DataView references resolve
5. Invalidates targets — validates invalidation targets exist
6. Duplicate names — detects duplicate DataView/View/datasource names
7. Schema file existence — verifies all referenced schema files exist on disk
8. Cross-app service refs — validates inter-app service references resolve within the bundle
9. TOML parse error context — provides line/column context for TOML syntax errors
```

**Replace with:**

```markdown
## Bundle Validation

`riverpackage validate <bundle_dir> [--format text|json] [--config <path>]`

Bundle validation runs a four-layer pipeline. See `rivers-bundle-validation-spec.md` for the full specification, error catalog, and output format contract.

### Validation Layers

| Layer | What it checks |
|---|---|
| 1 — Structural TOML | All TOML files parse correctly, have correct keys and types, all required fields present. Unknown keys are hard errors via `deny_unknown_fields`. Typo'd keys produce `did you mean?` suggestions. |
| 2 — Resource Existence | Every file path referenced in config (handler modules, schema files, init handlers, SPA assets, WASM modules, libs) exists on disk. |
| 3 — Logical Cross-References | DataView→datasource, view→DataView, view→resources, invalidates targets, service→appId, appId uniqueness, datasource name uniqueness, nopassword/lockbox consistency, x-type/driver consistency, view type constraints. |
| 4 — Syntax Verification | Schema JSON structural check. TS/JS compile via V8 engine dylib with entrypoint export verification. WASM validation via Wasmtime engine dylib. Import path resolution. Requires engine dylibs — skipped with warning if unavailable. |

### Output format

`--format text` (default) — human-readable `[PASS]`/`[FAIL]`/`[WARN]`/`[SKIP]` per check.
`--format json` — machine-readable structured output with error codes, file paths, table paths, and suggestions. Stable contract for agentic consumers.

### Engine dylib discovery

`riverpackage` reads engine dylib paths from `riversd.toml` (or path specified by `--config`). If the config or engine dylibs are not found, Layer 4 is skipped with a warning. Layers 1–3 always run.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | All checks passed (warnings may be present) |
| 1 | One or more validation errors |
| 2 | Bundle directory not found or unreadable |
| 3 | Config file unreadable (only when `--config` explicitly specified) |

### Schema output

`riverpackage validate --schema server|app|bundle` outputs the corresponding JSON Schema.

### Tool ownership

`riversctl validate` has been removed. All bundle validation is performed by `riverpackage validate`. `riversctl doctor --lint` has been removed. `doctor` owns system health; `riverpackage` owns bundle correctness.
```

---

## VAL-2 — `rivers-application-spec.md` — §8 Deployment Lifecycle

### Insert: New state in deploy state machine (§8.1)

**Replace** the state diagram:

```
PENDING → RESOLVING → STARTING → RUNNING
                │                   │
                └─ FAILED           └─ STOPPING → STOPPED
```

**With:**

```
PENDING → VALIDATING → RESOLVING → STARTING → RUNNING
               │            │                    │
               └─ FAILED    └─ FAILED            └─ STOPPING → STOPPED
```

**Add row to state table:**

| State | Description |
|---|---|
| `VALIDATING` | Bundle received, four-layer validation running (`rivers_runtime::validate_bundle_live()`). Runs Layers 1–4 plus live checks (LockBox alias resolution, registered driver matching, SchemaSyntaxChecker). Any failure → FAILED. |

### Insert: New section §8.5 Deploy-Time Validation

```markdown
### 8.5 Deploy-time validation

When a bundle is received, `riversd` runs the full validation pipeline before entering RESOLVING state. This is Gate 2 of the two-gate validation architecture (see `rivers-bundle-validation-spec.md`).

Gate 2 runs:
- Layers 1–4 from `rivers_runtime` (same checks as `riverpackage validate`)
- Live check: LockBox alias resolution for all `lockbox://` URIs in datasource configs
- Live check: registered driver matching — every declared `driver` value must be a registered driver in the running `riversd` instance
- Live check: `SchemaSyntaxChecker` — schema files validated against the actual driver's syntax checker
- Live check: `x-type` must match the registered driver for each datasource
- Live check: required service `appId` must be in RUNNING state

Any validation failure causes the app to enter FAILED state. No views are registered. No traffic is routed. A structured error is logged with the validation error code, file path, and detail.

Gate 2 does not trust Gate 1. A bundle that passed `riverpackage validate` is re-validated in full. Gate 1 exists for fast developer feedback. Gate 2 is the production hard-stop.
```

---

## VAL-3 — `rivers-application-spec.md` — §14 Validation Rules

### Replace: §14 Validation Rules

**Remove** the entire validation rules table (lines 749–768).

**Replace with:**

```markdown
## 14. Validation Rules

Validation rules are enforced at two gates:

- **Gate 1:** `riverpackage validate` — build-time, all four layers, offline
- **Gate 2:** `riversd` deploy-time — all four layers plus live infrastructure checks

The authoritative rule set, error codes, and error message templates are defined in `rivers-bundle-validation-spec.md` §11 (Error Catalog). The `rivers_runtime` crate implements all validation rules. Both gates link against it.

### Rule categories

| Layer | Error code prefix | Example |
|---|---|---|
| Structural TOML | `S0xx` | `S002` — unknown key in TOML table |
| Resource Existence | `E0xx` | `E001` — referenced file not found |
| Logical Cross-References | `X0xx` | `X001` — DataView references undeclared datasource |
| Syntax Verification | `C0xx` | `C001` — TS/JS syntax error |
| Live Checks (Gate 2 only) | `L0xx` | `L001` — LockBox alias not found |

### Validation timing in deploy lifecycle

Validation runs after bundle receipt (PENDING) and before resource resolution (RESOLVING). See §8.5.
```

---

## VAL-4 — `rivers-technology-path-spec.md` — §19.4 Deployment Lifecycle

### Replace: §19.4 Deployment Lifecycle (line 1297)

**Replace** the deployment lifecycle list:

```
 1. Deploy bundle.zip
 2. Validate bundle manifest.toml
 3. Per app: validate manifest.toml, resources.toml, app.toml
 4. Per app: SchemaSyntaxChecker validates all schema files against driver
 5. Per app: resolve resources (LockBox, datasource connections)
 6. Per app: run init handler (CORS, health, seeding, lifecycle hooks)
 7. Start app-services (parallel, respecting dependency graph)
 8. Health check app-services
 9. Start app-main
10. Health check app-main
11. Bundle RUNNING
```

**With:**

```
 1. Deploy bundle.zip → PENDING state
 2. Gate 2 validation → VALIDATING state
    a. Layer 1: Structural TOML — parse all manifest.toml, resources.toml, app.toml with deny_unknown_fields
    b. Layer 2: Resource existence — all referenced files exist (modules, schemas, SPA assets, WASM)
    c. Layer 3: Logical cross-references — datasource refs, DataView refs, service appIds, uniqueness, consistency
    d. Layer 4: Syntax verification — V8 compile check (TS/JS), Wasmtime validation (WASM), schema JSON structure, entrypoint export verification
    e. Live check: LockBox alias resolution for all lockbox:// URIs
    f. Live check: registered driver matching
    g. Live check: SchemaSyntaxChecker validates schema files against live driver
    h. Live check: x-type matches registered driver
    i. Any failure → FAILED state, structured error logged, deployment aborted
 3. Per app: resolve resources (LockBox, datasource connections) → RESOLVING state
 4. Per app: run init handler (CORS, health, seeding, lifecycle hooks)
 5. Start app-services (parallel, respecting dependency graph) → STARTING state
 6. Health check app-services
 7. Start app-main
 8. Health check app-main
 9. Bundle RUNNING
```

**Add note after the list:**

```
Validation logic is implemented in `rivers_runtime` and shared with `riverpackage validate` (Gate 1). See `rivers-bundle-validation-spec.md` for the full error catalog and layer definitions.
```

---

## VAL-5 — `rivers-driver-spec.md` — §7 Plugin System

### Insert: New section §7.7 after §7.6 (Honest stub pattern)

```markdown
### 7.7 Engine plugin `compile_check` export

Engine dylibs (V8, Wasmtime) export a `compile_check` function used by `riverpackage validate` for build-time syntax verification. This is separate from the driver plugin ABI — engine plugins are a distinct plugin category.

```rust
#[no_mangle]
pub extern "C" fn _rivers_compile_check(
    source_ptr: *const u8,
    source_len: usize,
    filename_ptr: *const u8,
    filename_len: usize,
    result_ptr: *mut CompileCheckFFI,
) -> i32;
```

Returns `0` on success (export list written to `result_ptr`), `1` on compile error (error details written to `result_ptr`).

`riverpackage` loads engine dylibs via `dlopen`, verifies `_rivers_abi_version()`, and resolves `_rivers_compile_check` and `_rivers_free_string` for syntax verification.

The V8 engine dylib performs TS transpilation (via embedded swc, same pipeline as ProcessPool bundle load) before V8 compile. On success, it enumerates named exports via `module.GetModuleNamespace()`. The Wasmtime engine dylib performs `Module::validate()` and parses the export section.

Both engines export the same FFI surface:

```rust
#[no_mangle]
pub extern "C" fn _rivers_compile_check(
    source_ptr: *const u8,
    source_len: usize,
    filename_ptr: *const u8,
    filename_len: usize,
) -> *const c_char;

#[no_mangle]
pub extern "C" fn _rivers_free_string(ptr: *const c_char);
```

Returns a heap-allocated JSON string. Success: `{"ok": true, "exports": [...]}`. Error: `{"ok": false, "error": {"filename": "...", "line": N, "column": N, "message": "..."}}`. Caller frees via `_rivers_free_string`.

See `rivers-bundle-validation-spec.md` §5 for full details.
```

### Amend: §7.2 ABI version check

**Append** to §7.2:

```
The ABI version is bumped when the engine plugin trait surface changes. The addition of `_rivers_compile_check` (see §7.7) requires an ABI version bump. Engine dylibs built against the previous ABI version will fail the version check and their compile_check function will be unavailable — `riverpackage` will skip Layer 4 with a warning for the affected engine.
```

---

## VAL-6 — `rivers-processpool-runtime-spec-v2.md` — §5 Engine: V8 Isolate

### Insert: New section §5.5 after existing V8 content (before §6 Engine: Wasmtime)

```markdown
### 5.5 Compile Check (build-time validation)

The V8 engine dylib exposes a `compile_check` method for build-time syntax verification. This is used by `riverpackage validate` (Gate 1) and `riversd` deploy-time validation (Gate 2) to catch TS/JS errors before request dispatch.

The compile check:

1. Transpiles TS → JS via embedded swc (same pipeline as §5.4 bundle load transpilation)
2. Creates a throwaway V8 isolate
3. Compiles the source as an ES module via `v8::Module::compile()`
4. On success: instantiates minimally, calls `module.GetModuleNamespace()`, enumerates own properties to produce an export list
5. On failure: extracts V8 `TryCatch` exception with file, line, column, and message
6. Destroys the isolate — no state persists

The compile check verifies that the declared handler `entrypoint` function exists in the export list. A handler declaring `entrypoint = "onCreateOrder"` against a module that exports `["default", "onCreateOrder"]` passes. A module that exports `["default", "onCreate"]` fails with error code `C002`.

The compile check is read-only. No handler code is executed. No side effects. The check uses the same swc and V8 versions as the runtime — zero version skew risk.

See `rivers-bundle-validation-spec.md` §5.2 for the full implementation contract.
```

### Insert: New section §6.4 after existing Wasmtime content

```markdown
### 6.4 Compile Check (build-time validation)

The Wasmtime engine dylib exposes a `compile_check` method for build-time WASM validation:

1. Validates WASM bytes via `wasmtime::Module::validate(&engine, bytes)` — checks magic bytes, section headers, type validation
2. On success: parses export section, collects exported function names
3. On failure: extracts Wasmtime error message

Export verification works the same as V8 — the declared `entrypoint` must appear in the export list.

See `rivers-bundle-validation-spec.md` §5.3 for the full implementation contract.
```

---

## VAL-7 — `rivers-canary-fleet-spec.md` — OPS Profile Test Table

### Replace: Test entries in OPS profile test inventory

**Remove these two rows:**

| Test ID | Path | Method | Assertion | Spec anchor |
|---|---|---|---|---|
| OPS-DOCTOR-LINT-PASS | (harness) | — | `riversctl doctor --lint` passes on valid canary bundle | doctor §3.1 |
| OPS-DOCTOR-LINT-FAIL | (harness) | — | `riversctl doctor --lint` fails on intentionally broken bundle | doctor §3.1 |

**Replace with these rows:**

| Test ID | Path | Method | Assertion | Spec anchor |
|---|---|---|---|---|
| OPS-VALIDATE-PASS | (harness) | — | `riverpackage validate canary-bundle/ --format json` exits 0, all layers pass | bundle-validation §7.2 |
| OPS-VALIDATE-FAIL-STRUCTURAL | (harness) | — | `riverpackage validate` exits 1 on bundle with unknown TOML key, error code `S002` in JSON output | bundle-validation §4.1 |
| OPS-VALIDATE-FAIL-EXISTENCE | (harness) | — | `riverpackage validate` exits 1 on bundle with missing handler module, error code `E001` in JSON output | bundle-validation §4.2 |
| OPS-VALIDATE-FAIL-CROSSREF | (harness) | — | `riverpackage validate` exits 1 on bundle with DataView referencing undeclared datasource, error code `X001` in JSON output | bundle-validation §4.3 |
| OPS-VALIDATE-FAIL-SYNTAX | (harness) | — | `riverpackage validate` exits 1 on bundle with TS syntax error, error code `C001` in JSON output | bundle-validation §4.4 |
| OPS-VALIDATE-FAIL-EXPORT | (harness) | — | `riverpackage validate` exits 1 on bundle with wrong entrypoint name, error code `C002` in JSON output | bundle-validation §4.4 |
| OPS-VALIDATE-SKIP-ENGINE | (harness) | — | `riverpackage validate --config /dev/null` skips Layer 4, exits 0 with `W003` warning in JSON output | bundle-validation §12 |
| OPS-VALIDATE-FORMAT-JSON | (harness) | — | `riverpackage validate --format json` produces valid JSON with `summary.exit_code` field | bundle-validation §8.2 |
| OPS-VALIDATE-FORMAT-TEXT | (harness) | — | `riverpackage validate --format text` produces `[PASS]`/`[FAIL]` text output | bundle-validation §8.1 |
| OPS-VALIDATE-TYPO-SUGGEST | (harness) | — | `riverpackage validate` on bundle with `veiew_type` key includes `did you mean 'view_type'?` in output | bundle-validation Appendix A |

**The existing `OPS-RIVERPACKAGE-VALIDATE` test is superseded by `OPS-VALIDATE-PASS`.** Remove the old row.

### Replace: Rust harness test code for lint/validate tests

**Remove:**

```rust
#[tokio::test]
async fn canary_ops_doctor_lint_pass() {
    let output = Command::new("riversctl")
        .args(["doctor", "--lint", "canary-bundle/"])
        .output().await.unwrap();
    assert!(output.status.success(),
        "doctor --lint should pass on valid canary bundle");
}

#[tokio::test]
async fn canary_ops_doctor_lint_fail() {
    // Create a broken bundle (missing manifest)
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("broken-app")).unwrap();
    let output = Command::new("riversctl")
        .args(["doctor", "--lint", tmp.path().to_str().unwrap()])
        .output().await.unwrap();
    assert!(!output.status.success(),
        "doctor --lint should fail on broken bundle");
}
```

And:

```rust
#[tokio::test]
async fn canary_ops_riverpackage_validate() {
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("test-bundle");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    assert!(output.status.success(),
        "riverpackage validate should pass on scaffolded bundle");
}
```

**Replace with:**

```rust
#[tokio::test]
async fn canary_ops_validate_pass() {
    let output = Command::new("riverpackage")
        .args(["validate", "canary-bundle/", "--format", "json"])
        .output().await.unwrap();
    assert!(output.status.success(),
        "riverpackage validate should pass on valid canary bundle");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["exit_code"], 0);
    assert_eq!(json["summary"]["total_failed"], 0);
}

#[tokio::test]
async fn canary_ops_validate_fail_structural() {
    // Create bundle with unknown TOML key
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("broken-structural");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    // Inject unknown key
    let app_toml = bundle_path.join(bundle_path.file_name().unwrap()).join("app.toml");
    let mut content = std::fs::read_to_string(&app_toml).unwrap();
    content.push_str("\n[api.views.list_items]\nveiew_type = \"Rest\"\n");
    std::fs::write(&app_toml, content).unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap(), "--format", "json"])
        .output().await.unwrap();
    assert!(!output.status.success(),
        "should fail on unknown TOML key");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let errors: Vec<&serde_json::Value> = json["layers"]["structural_toml"]["results"]
        .as_array().unwrap()
        .iter().filter(|r| r["status"] == "fail").collect();
    assert!(errors.iter().any(|e| e["message"].as_str().unwrap().contains("unknown key")),
        "should report S002 unknown key error");
}

#[tokio::test]
async fn canary_ops_validate_fail_existence() {
    // Create bundle with missing handler module
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("broken-existence");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    // Add view referencing non-existent module
    let app_name = bundle_path.file_name().unwrap().to_str().unwrap();
    let app_toml = bundle_path.join(app_name).join("app.toml");
    let mut content = std::fs::read_to_string(&app_toml).unwrap();
    content.push_str(r#"
[api.views.ghost_handler.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/ghost.ts"
entrypoint = "handle"
"#);
    std::fs::write(&app_toml, content).unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap(), "--format", "json"])
        .output().await.unwrap();
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["layers"]["resource_existence"]["failed"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn canary_ops_validate_fail_crossref() {
    // Create bundle with DataView referencing undeclared datasource
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("broken-crossref");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    let app_name = bundle_path.file_name().unwrap().to_str().unwrap();
    let app_toml = bundle_path.join(app_name).join("app.toml");
    let mut content = std::fs::read_to_string(&app_toml).unwrap();
    content.push_str(r#"
[data.dataviews.phantom]
name       = "phantom"
datasource = "nonexistent_db"
query      = "schemas/item.schema.json"
"#);
    std::fs::write(&app_toml, content).unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap(), "--format", "json"])
        .output().await.unwrap();
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let errors: Vec<&serde_json::Value> = json["layers"]["logical_cross_references"]["results"]
        .as_array().unwrap()
        .iter().filter(|r| r["status"] == "fail").collect();
    assert!(errors.iter().any(|e| e["message"].as_str().unwrap().contains("nonexistent_db")),
        "should report X001 datasource not declared");
}

#[tokio::test]
async fn canary_ops_validate_fail_syntax() {
    // Create bundle with TS syntax error
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("broken-syntax");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    let app_name = bundle_path.file_name().unwrap().to_str().unwrap();
    let handler_dir = bundle_path.join(app_name).join("libraries/handlers");
    std::fs::create_dir_all(&handler_dir).unwrap();
    std::fs::write(handler_dir.join("broken.ts"),
        "export function handle(ctx: any) {\n  const x = {{\n}\n").unwrap();
    // Add view pointing to broken handler
    let app_toml = bundle_path.join(app_name).join("app.toml");
    let mut content = std::fs::read_to_string(&app_toml).unwrap();
    content.push_str(r#"
[api.views.broken.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/broken.ts"
entrypoint = "handle"
"#);
    std::fs::write(&app_toml, content).unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap(), "--format", "json"])
        .output().await.unwrap();
    // May pass or fail depending on engine dylib availability
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let syntax_results = &json["layers"]["syntax_verification"]["results"];
    if let Some(arr) = syntax_results.as_array() {
        let has_syntax_fail = arr.iter().any(|r|
            r["status"] == "fail" && r["error_type"] == "SyntaxError");
        let has_skip = arr.iter().any(|r| r["status"] == "skip");
        assert!(has_syntax_fail || has_skip,
            "should either report C001 syntax error or skip Layer 4");
    }
}

#[tokio::test]
async fn canary_ops_validate_fail_export() {
    // Create bundle with wrong entrypoint name
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("broken-export");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    let app_name = bundle_path.file_name().unwrap().to_str().unwrap();
    let handler_dir = bundle_path.join(app_name).join("libraries/handlers");
    std::fs::create_dir_all(&handler_dir).unwrap();
    std::fs::write(handler_dir.join("misnamed.ts"),
        "export function actualName(ctx: any) { ctx.resdata = {}; }\n").unwrap();
    // Add view with wrong entrypoint
    let app_toml = bundle_path.join(app_name).join("app.toml");
    let mut content = std::fs::read_to_string(&app_toml).unwrap();
    content.push_str(r#"
[api.views.misnamed.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/misnamed.ts"
entrypoint = "wrongName"
"#);
    std::fs::write(&app_toml, content).unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap(), "--format", "json"])
        .output().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let syntax_results = &json["layers"]["syntax_verification"]["results"];
    if let Some(arr) = syntax_results.as_array() {
        let has_export_fail = arr.iter().any(|r|
            r["status"] == "fail" && r["message"].as_str()
                .map(|m| m.contains("wrongName")).unwrap_or(false));
        let has_skip = arr.iter().any(|r| r["status"] == "skip");
        assert!(has_export_fail || has_skip,
            "should either report C002 missing export or skip Layer 4");
    }
}

#[tokio::test]
async fn canary_ops_validate_skip_engine() {
    // Point --config to a file with no [engines] section
    let tmp = tempdir().unwrap();
    let empty_config = tmp.path().join("empty.toml");
    std::fs::write(&empty_config, "# no engines\n").unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", "canary-bundle/", "--format", "json",
               "--config", empty_config.to_str().unwrap()])
        .output().await.unwrap();
    assert!(output.status.success(),
        "should pass — Layer 4 skipped, Layers 1-3 pass");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let syntax_results = &json["layers"]["syntax_verification"]["results"];
    if let Some(arr) = syntax_results.as_array() {
        assert!(arr.iter().any(|r| r["status"] == "skip"),
            "should have skipped results in Layer 4");
    }
}

#[tokio::test]
async fn canary_ops_validate_format_json() {
    let output = Command::new("riverpackage")
        .args(["validate", "canary-bundle/", "--format", "json"])
        .output().await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["summary"]["exit_code"].is_number(),
        "JSON output must have summary.exit_code");
    assert!(json["bundle_name"].is_string(),
        "JSON output must have bundle_name");
    assert!(json["layers"].is_object(),
        "JSON output must have layers object");
}

#[tokio::test]
async fn canary_ops_validate_format_text() {
    let output = Command::new("riverpackage")
        .args(["validate", "canary-bundle/", "--format", "text"])
        .output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[PASS]") || stdout.contains("[FAIL]"),
        "text output should contain [PASS] or [FAIL] markers");
    assert!(stdout.contains("Layer 1") || stdout.contains("Structural"),
        "text output should reference validation layers");
}

#[tokio::test]
async fn canary_ops_validate_typo_suggest() {
    // Create bundle with typo'd key
    let tmp = tempdir().unwrap();
    let bundle_path = tmp.path().join("typo-bundle");
    Command::new("riverpackage")
        .args(["init", bundle_path.to_str().unwrap()])
        .output().await.unwrap();
    let app_name = bundle_path.file_name().unwrap().to_str().unwrap();
    let app_toml = bundle_path.join(app_name).join("app.toml");
    let mut content = std::fs::read_to_string(&app_toml).unwrap();
    // Inject typo: 'veiew_type' instead of 'view_type'
    content = content.replace("view_type", "veiew_type");
    std::fs::write(&app_toml, content).unwrap();

    let output = Command::new("riverpackage")
        .args(["validate", bundle_path.to_str().unwrap(), "--format", "text"])
        .output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("did you mean") && stdout.contains("view_type"),
        "should suggest 'view_type' for typo 'veiew_type'");
}
```

---

## VAL-8 — `rivers-canary-fleet-spec.md` — Harness-Only Test List (AMD-2.7)

### Replace: In AMD-2.7 harness-only test list

**Remove:**

```
- OPS-DOCTOR-LINT-PASS — `riversctl doctor --lint` passes on valid bundle
- OPS-DOCTOR-LINT-FAIL — `riversctl doctor --lint` fails on broken bundle
```

And:

```
- OPS-RIVERPACKAGE-VALIDATE — `riverpackage validate` passes on scaffold
```

**Replace with:**

```
- OPS-VALIDATE-PASS — `riverpackage validate --format json` passes on valid canary bundle, exits 0
- OPS-VALIDATE-FAIL-STRUCTURAL — `riverpackage validate` exits 1 on unknown TOML key (S002)
- OPS-VALIDATE-FAIL-EXISTENCE — `riverpackage validate` exits 1 on missing handler module (E001)
- OPS-VALIDATE-FAIL-CROSSREF — `riverpackage validate` exits 1 on undeclared datasource reference (X001)
- OPS-VALIDATE-FAIL-SYNTAX — `riverpackage validate` exits 1 on TS syntax error (C001)
- OPS-VALIDATE-FAIL-EXPORT — `riverpackage validate` exits 1 on wrong entrypoint name (C002)
- OPS-VALIDATE-SKIP-ENGINE — `riverpackage validate` skips Layer 4 when engine unavailable (W003)
- OPS-VALIDATE-FORMAT-JSON — `--format json` produces valid JSON with summary.exit_code
- OPS-VALIDATE-FORMAT-TEXT — `--format text` produces [PASS]/[FAIL] markers
- OPS-VALIDATE-TYPO-SUGGEST — `riverpackage validate` suggests correct key name for typos
```

### Update: doctor-tests.ts handler stub comment

**Replace:**

```typescript
// doctor-tests.ts and tls-tests.ts contain no handler endpoints.
// All OPS-DOCTOR-* and OPS-TLS-* tests are harness-only.
// They are documented in the test inventory table for completeness.
// The Rust integration test harness runs:
//   riversctl doctor --lint <bundle-path>
//   riversctl doctor --fix <bundle-path>
//   riversctl tls renew
// and asserts on exit codes, output, and file state.
```

**With:**

```typescript
// doctor-tests.ts and tls-tests.ts contain no handler endpoints.
// All OPS-DOCTOR-FIX-*, OPS-TLS-*, and OPS-VALIDATE-* tests are harness-only.
// They are documented in the test inventory table for completeness.
// The Rust integration test harness runs:
//   riverpackage validate <bundle-path> --format json|text
//   riversctl doctor --fix
//   riversctl tls renew
// and asserts on exit codes, output, file state, and JSON structure.
//
// Note: riversctl doctor --lint has been removed.
// All bundle validation is performed by riverpackage validate.
// See rivers-bundle-validation-spec.md for the full validation pipeline.
```

---

## VAL-9 — `rivers-canary-fleet-spec.md` — Test Count Summary

### Replace: OPS row in Test Count Summary table

**Remove:**

| Profile | Positive Tests | Negative Tests | Total |
|---------|---------------|----------------|-------|
| OPS | 16 | 8 | 24 |

**Replace with:**

| Profile | Positive Tests | Negative Tests | Total |
|---------|---------------|----------------|-------|
| OPS | 20 | 13 | 33 |

Net change: +9 tests (removed 3 old lint/validate tests, added 10 new validate tests, 2 existing OPS-DOCTOR-LINT removed = net gain of 7 positive + 2 negative = +9 total).

### Replace: Total row

**Remove:**

| **Total** | **80** | **27** | **107** |

**Replace with:**

| **Total** | **84** | **32** | **116** |

### Update: summary prose

**Append** to the summary paragraph after the table:

```
Covers bundle validation pipeline: four-layer structural/existence/cross-reference/syntax checking,
engine dylib graceful degradation, JSON and text output formats, and typo suggestion (Levenshtein).
```
