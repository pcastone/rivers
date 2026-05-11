# Rivers Bundle Validation Specification

**Document Type:** Implementation Specification  
**Version:** 1.0  
**Status:** Design — Pre-Implementation  
**Scope:** Bundle validation pipeline, `riverpackage validate`, `riversd` deploy-time validation, engine dylib `compile_check` FFI export  
**Depends On:** Application Spec (bundle structure, manifests), Technology Path Spec (deployment lifecycle), ProcessPool Spec (engine dylibs), Driver Spec (plugin ABI), LockBox Spec (alias resolution)  
**Supersedes:** `riversctl validate` (removed), `riversctl doctor --lint` (removed)  
**Origin:** IPAM team malformed TOML incident — no guardrail caught structurally invalid config files

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Design Principles](#2-design-principles)
3. [Two-Gate Architecture](#3-two-gate-architecture)
4. [Validation Layers](#4-validation-layers)
5. [Engine Dylib FFI — `compile_check`](#5-engine-dylib-ffi--compile_check)
6. [Engine Dylib Discovery](#6-engine-dylib-discovery)
7. [CLI Surface](#7-cli-surface)
8. [Output Formats](#8-output-formats)
9. [Validation Modules in `rivers_runtime`](#9-validation-modules-in-rivers_runtime)
10. [Tool Ownership Changes](#10-tool-ownership-changes)
11. [Error Catalog](#11-error-catalog)
12. [Graceful Degradation](#12-graceful-degradation)
13. [Affected Specifications](#13-affected-specifications)

---

## 1. Problem Statement

The IPAM team deployed bundles with malformed TOML files that passed through the entire pipeline without detection. No existing gate caught the error because:

- TOML parse errors produce a generic `toml::de::Error` with no structural context. A file that parses as valid TOML but has wrong keys, wrong value types, or missing required fields is silently accepted.
- Referenced files (modules, schemas) are not existence-checked until runtime dispatch.
- TS/JS handler files with syntax errors reach V8 at request time. The first user to hit the endpoint discovers the bug.
- Validation logic is scattered across `riversctl validate`, `riversctl doctor --lint`, `riverpackage validate`, and `riversd` deploy-time — with inconsistent coverage and no single authoritative gate.

---

## 2. Design Principles

### 2.1 Two gates, one library

All validation logic lives in `rivers_runtime` as new validation modules alongside the existing `validate_bundle()` and `validate_known_drivers()`. `riverpackage` and `riversd` both link against `rivers_runtime`. No duplicated validation code. No drift. No new crate dependency.

### 2.2 Gate 2 is a superset of Gate 1

Anything `riverpackage validate` catches, `riversd` deploy-time also catches. Gate 1 exists for fast developer feedback. Gate 2 is the production hard-stop. Neither trusts the other.

### 2.3 Fail early, fail completely

Validation runs all layers and collects all errors before returning. It does not stop at the first error. The developer sees every problem in one pass.

### 2.4 Engine dylib is optional at Gate 1

`riverpackage` dylinks engine dylibs for syntax checking. If the dylib is not discoverable, Layers 1–3 still run. Layer 4 emits a skip warning. A CI pipeline without the engine installed still gets structural and logical validation.

### 2.5 One tool owns bundle validation

`riverpackage validate` is the sole CLI surface for bundle validation. `riversctl validate` is removed. `riversctl doctor --lint` is removed. `doctor` owns system health (file permissions, TLS certs, PID files, log directories). `riverpackage` owns bundle correctness.

---

## 3. Two-Gate Architecture

### 3.1 Gate 1 — `riverpackage validate` (build-time)

Runs before the zip is created. Offline — no running `riversd`, no LockBox, no live drivers. Catches every error that can be caught without infrastructure.

```
Developer edits bundle
    │
    ▼
riverpackage validate <bundle_dir>
    │
    ├─ Layer 1: Structural TOML
    ├─ Layer 2: Resource Existence
    ├─ Layer 3: Logical Cross-References
    └─ Layer 4: Syntax Verification (requires engine dylibs)
    │
    ▼
Pass → riverpackage pack
Fail → fix errors, re-validate
```

### 3.2 Gate 2 — `riversd` deploy-time (production)

Runs when the bundle is received (step 3 in the deployment lifecycle). Re-runs all four layers, then adds live infrastructure checks. Hard-stops deployment on any failure.

```
bundle.zip received by riversd
    │
    ▼
Gate 2 validation
    │
    ├─ Layer 1: Structural TOML
    ├─ Layer 2: Resource Existence
    ├─ Layer 3: Logical Cross-References
    ├─ Layer 4: Syntax Verification
    │
    ├─ Live Check: LockBox alias resolution
    ├─ Live Check: Registered driver matching
    ├─ Live Check: SchemaSyntaxChecker against live driver
    │
    ▼
Pass → continue to RESOLVING state
Fail → app enters FAILED state, structured error logged
```

### 3.3 Gate relationship

| Check | Gate 1 | Gate 2 |
|---|---|---|
| TOML structural validation | ✓ | ✓ |
| File existence | ✓ | ✓ |
| Logical cross-references | ✓ | ✓ |
| TS/JS/WASM syntax + export check | ✓ (if engine dylib available) | ✓ |
| LockBox alias resolution | — | ✓ |
| Registered driver matching | — | ✓ |
| SchemaSyntaxChecker (driver-specific) | — | ✓ |
| `x-type` vs live driver match | — | ✓ |

---

## 4. Validation Layers

### 4.1 Layer 1 — Structural TOML

**Purpose:** Every TOML file in the bundle is syntactically valid, has the correct keys, correct value types, and all required fields present.

**Implementation:** `#[serde(deny_unknown_fields)]` on all config structs. Unknown keys are hard errors, not silent ignores. Required fields enforced by serde's `#[serde(…)]` attributes — a missing required field fails deserialization with field name and file context.

**Files validated per app:**

| File | Required | Config struct |
|---|---|---|
| `manifest.toml` (bundle root) | yes | `BundleManifest` |
| `manifest.toml` (per app) | yes | `AppManifest` |
| `resources.toml` (per app) | yes | `ResourcesConfig` |
| `app.toml` (per app) | yes | `AppConfig` |

**Bundle-level `manifest.toml` required fields:**

| Field | Type | Constraint |
|---|---|---|
| `bundleName` | string | non-empty |
| `bundleVersion` | string | valid semver |
| `apps` | string[] | non-empty, each element matches a directory name in the bundle |

**Per-app `manifest.toml` required fields:**

| Field | Type | Constraint |
|---|---|---|
| `appName` | string | non-empty |
| `appId` | string | valid UUID v4 |
| `type` | string | `"app-main"` or `"app-service"` |
| `entryPoint` | string | valid URL with scheme, host, port |

**Per-app `resources.toml` structural rules:**

| Field | Type | Constraint |
|---|---|---|
| `datasources[].name` | string | non-empty, unique within app |
| `datasources[].driver` | string | non-empty |
| `datasources[].x-type` | string | non-empty |
| `datasources[].lockbox` | string | required unless `nopassword = true` |
| `datasources[].required` | boolean | — |
| `services[].name` | string | non-empty |
| `services[].appId` | string | valid UUID |
| `services[].required` | boolean | — |

**Per-app `app.toml` structural rules:**

- `[data.dataviews.*]` — each DataView has `name` (string), `datasource` (string), `query` (string)
- `[api.views.*]` — each view has `path` (string), `method` (string), `view_type` (string, must be `Rest`, `Websocket`, `ServerSentEvents`, `MessageConsumer`, `Mcp`, `Cron`, or `OTLP`); optional `auth` (string, must be `none` or `session` if present — `OTLP` views accept only `none`). Values outside these closed sets emit `S005` with a did-you-mean hint (Sprint 2026-05-09 Track 2; `Cron` added Track 3; `OTLP` added Sprint 2026-05-XX Track O1).
- Handler definitions: `type` is `"dataview"` or `"codecomponent"`; CodeComponent requires `language`, `module`, `entrypoint`
- `nopassword = true` and `lockbox` are mutually exclusive — presence of both is a hard error
- `nopassword` absent and `lockbox` absent is a hard error (for non-`nopassword` drivers)

**Error context:** Every structural error includes the file path, the TOML table path (e.g., `api.views.create_order.handler`), the field name, and the expected vs actual type or constraint.

### 4.2 Layer 2 — Resource Existence

**Purpose:** Every file path referenced in config actually exists on disk within the bundle.

**Checks:**

| Reference source | Referenced path | Resolution root |
|---|---|---|
| CodeComponent handler `module` | `libraries/handlers/orders.ts` | `{app_dir}/` |
| DataView `query` (ending in `.json`) | `schemas/item.schema.json` | `{app_dir}/` |
| Init handler `init.module` | `handlers/init.ts` | `{app_dir}/libraries/` |
| SPA `index_file` | `index.html` | `{app_dir}/libraries/spa/` |
| WASM handler `module` | `libraries/compute/processor.wasm` | `{app_dir}/` |
| View `libs[]` entries | `lodash.js`, `validator.wasm` | `{app_dir}/libraries/` |

**All module paths are resolved relative to the app directory root** — the directory containing `manifest.toml` and `resources.toml`. This matches the module resolution rule in the Application Spec §13.1.

**Error context:** File path, reference source (which config key pointed to it), app name.

### 4.3 Layer 3 — Logical Cross-References

**Purpose:** Internal references between config files resolve correctly. The dependency graph is consistent.

**Checks:**

| Check | Rule | Error |
|---|---|---|
| DataView → datasource | Every `datasource` field in `[data.dataviews.*]` matches a `name` in `resources.toml` `[[datasources]]` | `DataView '{dv}' references datasource '{ds}' not declared in {app}/resources.toml` |
| View → DataView | Every `handler.dataview` reference matches a declared DataView in `app.toml` | `View '{view}' references dataview '{dv}' not declared in {app}/app.toml` |
| View → resources | Every `handler.resources[]` entry matches a declared datasource `name` in `resources.toml` | `View '{view}' handler resource '{res}' not declared in {app}/resources.toml` |
| Invalidates → target | Every `invalidates` target matches a declared DataView name | `Invalidates target '{target}' does not exist in {app}` |
| Service → app | Every `services[].appId` in `resources.toml` matches an `appId` in some app's `manifest.toml` within the bundle | `Service '{svc}' references appId '{id}' not found in bundle` |
| Bundle manifest → dirs | Every entry in bundle `manifest.toml` `apps[]` matches a directory in the bundle | `App directory '{name}' listed in manifest but not found in bundle` |
| appId uniqueness | No two apps in the bundle share an `appId` | `Duplicate appId '{id}' in {appA} and {appB}` |
| Datasource name uniqueness | No two datasources within the same app share a `name` | `Duplicate datasource name '{name}' in {app}/resources.toml` |
| `nopassword` / `lockbox` consistency | `nopassword = true` → `lockbox` field absent. `nopassword` absent → `lockbox` field present and non-empty | `Datasource '{name}': nopassword=true but lockbox is also set` or `Datasource '{name}': lockbox is required when nopassword is not set` |
| `x-type` / `driver` consistency | `x-type` value equals `driver` value for built-in drivers | `Datasource '{name}': x-type '{xt}' does not match driver '{drv}'` |
| View type constraints | `handler.type = "dataview"` only on `view_type = "Rest"` | `View '{view}': dataview handler only valid for view_type=Rest` |
| WebSocket method | `view_type = "Websocket"` requires `method = "GET"` | `View '{view}': method must be GET when view_type=Websocket` |
| SSE method | `view_type = "ServerSentEvents"` requires `method = "GET"` | `View '{view}': method must be GET when view_type=ServerSentEvents` |
| SPA on app-service | `spa` config block present on `type = "app-service"` | `SPA config is only valid on app-main` |
| Init handler on manifest | If `init` declared, both `init.module` and `init.entrypoint` must be present | `Init handler declared but missing module or entrypoint` |

### 4.4 Layer 4 — Syntax Verification

**Purpose:** Code artifacts and schema files are syntactically valid before deployment. Catches errors that would otherwise surface at first request dispatch.

**Requires:** Engine dylibs loaded via `riversd.toml`. If not available, this layer is skipped with a warning.

#### 4.4.1 Schema JSON validation

Schema `.json` files are validated structurally:

- Valid JSON (parse check)
- Has `type` field (string)
- If `properties` present, it is an object
- If `required` present, it is an array of strings
- Each string in `required` matches a key in `properties`
- Each property has a `type` field

This is not driver-specific schema validation (that's a Gate 2 live check via `SchemaSyntaxChecker`). This catches malformed JSON and structurally broken schemas before they reach a driver.

#### 4.4.2 TS/JS compile check via V8 engine dylib

For every file referenced as a CodeComponent handler (`language` = `typescript`, `ts`, `javascript`, `js`):

1. Load the V8 engine dylib
2. Call `compile_check(source, filename)` on the engine trait
3. V8 compiles the module (TS files transpiled by embedded swc first, same as ProcessPool)
4. On successful compile, verify that the declared `entrypoint` function name exists as a named export via `module.GetModuleNamespace()` property check
5. Report syntax errors with file, line, column, and message
6. Report missing exports with the declared entrypoint name and the actual export list

**The compile check is read-only.** No code is executed. No side effects. The module is compiled and its exports are inspected, then discarded.

#### 4.4.3 WASM validation via Wasmtime engine dylib

For every file referenced as a WASM handler (`language` = `wasm`):

1. Load the Wasmtime engine dylib
2. Call `compile_check(bytes, filename)` on the engine trait
3. Wasmtime performs validation-only parse — checks WASM magic bytes, section headers, type validation
4. Verify that the declared `entrypoint` function name exists in the module's export list
5. Report validation errors and missing exports

#### 4.4.4 Cross-module import resolution

For TS/JS files, scan source for `import` statements with relative paths (`./` or `../`). For each relative import:

1. Resolve the path relative to the importing file
2. Verify the target file exists within the same app's `libraries/` directory
3. If the resolved path escapes `{app}/libraries/`, report error C004 (cross-app import)
4. If the target file does not exist, report error C005

**Scope:** Only relative paths are checked. Bare specifiers (e.g., `import lodash from "lodash"`) are skipped — those are resolved at runtime by the `libs` declaration in the view config. Layer 3 already validates that `libs[]` entries exist as files. Absolute paths (starting with `/`) are an unconditional error.

Import parsing uses a simple line-by-line scan for `from "./..."` and `from "../..."` patterns. Dynamic `import()` expressions are ignored — they fail at runtime via the capability model (no dynamic imports allowed).

Import resolution does not compile imported modules recursively — it only verifies the file exists. The compile check on the importing module will catch type-level import errors.

---

## 5. Engine Dylib FFI — `compile_check`

### 5.1 FFI exports

Each engine dylib (V8, Wasmtime) exports two new functions alongside the existing `_rivers_abi_version`:

```rust
/// Compile-only check. No execution. Returns heap-allocated JSON string.
/// Caller frees via _rivers_free_string.
#[no_mangle]
pub extern "C" fn _rivers_compile_check(
    source_ptr: *const u8,
    source_len: usize,
    filename_ptr: *const u8,
    filename_len: usize,
) -> *const c_char;

/// Free a string returned by _rivers_compile_check.
#[no_mangle]
pub extern "C" fn _rivers_free_string(ptr: *const c_char);
```

No trait abstraction. `riverpackage` loads the dylib via `libloading`, resolves these two symbols, and calls them directly. File extension determines which engine dylib to call (`.ts`/`.js` → V8, `.wasm` → Wasmtime).

### 5.2 JSON response contract

Success:
```json
{"ok": true, "exports": ["onCreateOrder", "default"]}
```

Error:
```json
{"ok": false, "error": {"filename": "orders.ts", "line": 14, "column": 8, "message": "Unexpected token"}}
```

`line` and `column` are `null` when not available (e.g., WASM validation errors without line context).

### 5.3 Caller-side handle

`rivers_runtime` wraps the FFI in a safe handle:

```rust
// In rivers_runtime::validate_engine
struct EngineHandle {
    _lib: libloading::Library,
    compile_check_fn: unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> *const c_char,
    free_string_fn: unsafe extern "C" fn(*const c_char),
}

impl EngineHandle {
    fn compile_check(&self, source: &[u8], filename: &str) -> Result<CompileCheckResult, CompileCheckError> {
        let json_ptr = unsafe {
            (self.compile_check_fn)(
                source.as_ptr(), source.len(),
                filename.as_ptr(), filename.len(),
            )
        };
        let json_str = unsafe { CStr::from_ptr(json_ptr) }.to_str()?;
        let result: CompileCheckResponse = serde_json::from_str(json_str)?;
        unsafe { (self.free_string_fn)(json_ptr) };
        result.into()
    }
}
```

### 5.4 V8 implementation

1. Detect filename extension — if `.ts`, transpile TS → JS via embedded swc (same pipeline as ProcessPool bundle load). If `.js`, use source directly.
2. Create a throwaway V8 isolate
3. Compile as ES module via `v8::Module::compile()`
4. On success: instantiate minimally, call `module.GetModuleNamespace()`, enumerate own properties → export list
5. On failure: extract V8 `TryCatch` exception message, line, column
6. Serialize result as JSON, return heap-allocated `*const c_char`
7. Destroy isolate. No state persists.

swc stays inside the V8 engine dylib. `rivers_runtime` and `riverpackage` have no swc dependency. Raw source bytes (TS or JS) cross the FFI boundary. The engine dylib decides whether transpilation is needed.

### 5.5 Wasmtime implementation

1. Validate WASM bytes via `wasmtime::Module::validate(&engine, bytes)` — checks magic bytes, section headers, type validation
2. On success: parse export section, collect exported function names
3. On failure: extract Wasmtime error message
4. Serialize result as JSON, return heap-allocated `*const c_char`

### 5.6 ABI version bump

The ABI version check (`_rivers_abi_version`) is bumped to reflect the new FFI exports. Engine dylibs built against the previous ABI version will fail the version check — `riverpackage` skips Layer 4 for the affected engine with a warning.

---

## 6. Engine Dylib Discovery

### 6.1 Config source

`riverpackage` reads engine dylib paths from `riversd.toml`. This is the same config file used by `riversd`.

```toml
[engines]
v8       = "/usr/lib/rivers/librivers_engine_v8.dylib"
wasmtime = "/usr/lib/rivers/librivers_engine_wasm.dylib"
```

### 6.2 Discovery sequence

1. `--config` flag (if specified)
2. `RIVERS_CONFIG` env var (if set)
3. `riversd.toml` in CWD
4. Not found → Layer 4 skipped with warning W003
5. Parse `[engines]` section
6. For each engine, attempt `dlopen` on the declared path
7. Verify `_rivers_abi_version()` matches current SDK version
8. Resolve `_rivers_compile_check` and `_rivers_free_string` symbols

### 6.3 Failure modes

| Failure | Behavior |
|---|---|
| `riversd.toml` not found | Layer 4 skipped with warning |
| `[engines]` section absent | Layer 4 skipped with warning |
| Engine dylib path not found | That engine's checks skipped with warning; other engines proceed |
| ABI version mismatch | Hard error — version skew between `riverpackage` and engine dylib |
| `_rivers_compile_check` symbol missing | Hard error — engine dylib too old, needs rebuild |

### 6.4 File extension mapping

| Handler `language` | Engine |
|---|---|
| `typescript`, `ts`, `typescript_strict`, `ts_strict` | V8 |
| `javascript`, `js`, `javascript_v8`, `js_v8` | V8 |
| `wasm` | Wasmtime |

If the required engine is not loaded for a given handler's language, that handler's syntax check is skipped with a warning.

---

## 7. CLI Surface

### 7.1 `riverpackage validate`

```bash
riverpackage validate <bundle_dir> [--format text|json] [--config <path>]
```

| Flag | Default | Description |
|---|---|---|
| `<bundle_dir>` | (required) | Path to bundle directory |
| `--format` | `text` | Output format: `text` for human-readable, `json` for machine-readable |
| `--config` | `riversd.toml` | Path to Rivers config file (for engine dylib discovery) |

### 7.2 Exit codes

| Code | Meaning |
|---|---|
| 0 | All checks passed (warnings may be present) |
| 1 | One or more validation errors |
| 2 | Bundle directory not found or unreadable |
| 3 | Config file unreadable (only when `--config` explicitly specified) |

Warnings (skipped checks, unknown drivers) do not affect the exit code. Only errors produce exit code 1.

---

## 8. Output Formats

### 8.1 Text format (`--format text`)

```
Rivers Bundle Validation — orders-platform v1.4.2
==================================================

Layer 1: Structural TOML
  [PASS] manifest.toml — bundle manifest valid
  [PASS] orders-service/manifest.toml — app manifest valid
  [FAIL] orders-service/app.toml — unknown key 'veiew_type' in [api.views.list_orders]
  [FAIL] orders-service/resources.toml — missing required field 'driver' in [[datasources]][1]

Layer 2: Resource Existence
  [PASS] orders-service/libraries/handlers/orders.ts — exists
  [FAIL] orders-service/libraries/handlers/fulfillment.ts — file not found
         referenced by: api.views.fulfill_order.handler.module

Layer 3: Logical Cross-References
  [PASS] DataView 'order_list' → datasource 'orders-db' — resolved
  [FAIL] DataView 'user_lookup' → datasource 'users-db' — not declared in orders-service/resources.toml

Layer 4: Syntax Verification
  [PASS] orders-service/libraries/handlers/orders.ts — compiles, export 'onCreateOrder' found
  [FAIL] orders-service/libraries/handlers/init.ts — SyntaxError: Unexpected token at line 14, column 8
  [SKIP] WASM checks — wasmtime engine dylib not available

RESULT: 4 errors, 0 warnings
```

### 8.2 JSON format (`--format json`)

```json
{
  "bundle_name": "orders-platform",
  "bundle_version": "1.4.2",
  "timestamp": "2026-04-06T14:23:01.847Z",
  "layers": {
    "structural_toml": {
      "passed": 2,
      "failed": 2,
      "results": [
        {
          "status": "pass",
          "file": "manifest.toml",
          "message": "bundle manifest valid"
        },
        {
          "status": "fail",
          "file": "orders-service/app.toml",
          "table_path": "api.views.list_orders",
          "field": "veiew_type",
          "message": "unknown key 'veiew_type'",
          "suggestion": "did you mean 'view_type'?"
        },
        {
          "status": "fail",
          "file": "orders-service/resources.toml",
          "table_path": "datasources[1]",
          "field": "driver",
          "message": "missing required field 'driver'"
        }
      ]
    },
    "resource_existence": {
      "passed": 1,
      "failed": 1,
      "results": [
        {
          "status": "fail",
          "file": "orders-service/libraries/handlers/fulfillment.ts",
          "referenced_by": "api.views.fulfill_order.handler.module",
          "app": "orders-service",
          "message": "file not found"
        }
      ]
    },
    "logical_cross_references": {
      "passed": 1,
      "failed": 1,
      "results": [
        {
          "status": "fail",
          "source": "data.dataviews.user_lookup",
          "target": "users-db",
          "target_type": "datasource",
          "app": "orders-service",
          "message": "datasource 'users-db' not declared in orders-service/resources.toml"
        }
      ]
    },
    "syntax_verification": {
      "passed": 1,
      "failed": 1,
      "skipped": 1,
      "results": [
        {
          "status": "pass",
          "file": "orders-service/libraries/handlers/orders.ts",
          "exports": ["onCreateOrder"],
          "entrypoint_verified": true
        },
        {
          "status": "fail",
          "file": "orders-service/libraries/handlers/init.ts",
          "error_type": "SyntaxError",
          "line": 14,
          "column": 8,
          "message": "Unexpected token"
        },
        {
          "status": "skip",
          "engine": "wasmtime",
          "reason": "engine dylib not available"
        }
      ]
    }
  },
  "summary": {
    "total_passed": 5,
    "total_failed": 4,
    "total_skipped": 1,
    "total_warnings": 0,
    "exit_code": 1
  }
}
```

### 8.3 JSON schema contract

The JSON output structure is stable. Fields may be added but never removed or renamed. Agentic consumers can rely on `summary.exit_code`, `layers.*.results[].status`, and the error detail fields.

---

## 9. Validation Modules in `rivers_runtime`

### 9.1 Location

New validation modules are added to the existing `rivers_runtime` crate. No new crate is created. Both `riverpackage` and `riversd` already depend on `rivers_runtime`.

```
crates/rivers-runtime/src/
├── lib.rs                       # existing
├── loader.rs                    # existing — load_bundle(), load_server_config()
├── validate.rs                  # existing — validate_bundle(), validate_known_drivers()
├── validate_structural.rs       # NEW — Layer 1: TOML structural validation
├── validate_existence.rs        # NEW — Layer 2: file existence checks
├── validate_crossref.rs         # NEW — Layer 3: logical cross-reference checks
├── validate_syntax.rs           # NEW — Layer 4: compile check via engine dylibs
├── validate_engine.rs           # NEW — engine dylib loading (EngineHandle)
├── validate_result.rs           # NEW — ValidationReport, error types, error codes
└── validate_format.rs           # NEW — text and JSON output formatters
```

### 9.2 Public API

```rust
pub struct ValidationConfig {
    pub bundle_dir: PathBuf,
    pub engines: Option<EngineConfig>,   // None = skip Layer 4
}

pub struct EngineConfig {
    pub v8_path: Option<PathBuf>,
    pub wasmtime_path: Option<PathBuf>,
}

/// Gate 1: offline validation (riverpackage)
pub fn validate_bundle_full(config: &ValidationConfig) -> ValidationReport;

/// Gate 2: offline + live checks (riversd)
pub fn validate_bundle_live(
    config: &ValidationConfig,
    lockbox: &dyn LockBoxResolver,
    drivers: &DriverFactory,
) -> ValidationReport;

/// Existing — preserved for backward compatibility, calls into Layer 1-3 internally
pub fn validate_bundle(bundle: &LoadedBundle) -> Vec<String>;
pub fn validate_known_drivers(bundle: &LoadedBundle, drivers: &[String]) -> Vec<String>;

pub struct ValidationReport {
    pub bundle_name: String,
    pub bundle_version: String,
    pub layers: LayerResults,
    pub live_checks: Option<LiveCheckResults>,  // Gate 2 only
    pub summary: Summary,
}

impl ValidationReport {
    pub fn format_text(&self) -> String;
    pub fn format_json(&self) -> String;
    pub fn exit_code(&self) -> i32;
    pub fn has_errors(&self) -> bool;
}
```

### 9.3 Consumers

| Consumer | Function | Context |
|---|---|---|
| `riverpackage validate` | `validate_bundle_full()` | Build-time CLI |
| `riversd` deploy-time | `validate_bundle_live()` | Runtime, after bundle receipt |
| Existing callers | `validate_bundle()` (unchanged signature) | Backward compatibility |

---

## 10. Tool Ownership Changes

### 10.1 Removed: `riversctl validate`

The `validate` subcommand is removed from `riversctl`. All bundle validation moves to `riverpackage validate`. The nine checks previously in `riversctl validate` are subsumed by Layers 1–3 in `rivers_runtime`.

### 10.2 Removed: `riversctl doctor --lint`

The `--lint` flag is removed from `riversctl doctor`. `doctor` retains its system health role:

- File permissions
- TLS certificate health and expiry
- PID file state
- Log directory existence
- LockBox keystore integrity

`doctor` does not load, parse, or validate bundles.

### 10.3 Retained: `riversctl doctor --fix`

`--fix` continues to auto-repair system health issues (missing log dirs, missing TLS certs, permission corrections). Unrelated to bundle validation.

### 10.4 Migration

| Old command | New command |
|---|---|
| `riversctl validate <bundle>` | `riverpackage validate <bundle>` |
| `riversctl doctor --lint` | `riverpackage validate <bundle>` |
| `riversctl validate --schema server\|app\|bundle` | `riverpackage validate --schema server\|app\|bundle` (moved) |

---

## 11. Error Catalog

### 11.1 Layer 1 — Structural errors

| Code | Message template |
|---|---|
| `S001` | `{file}: TOML parse error at line {line}, column {col}: {detail}` |
| `S002` | `{file}: unknown key '{key}' in [{table_path}]` |
| `S003` | `{file}: missing required field '{field}' in [{table_path}]` |
| `S004` | `{file}: wrong type for '{field}' in [{table_path}] — expected {expected}, got {actual}` |
| `S005` | `{file}: invalid value for '{field}' — {constraint}` |
| `S006` | `{file}: nopassword=true and lockbox are mutually exclusive in datasource '{name}'` |
| `S007` | `{file}: lockbox is required when nopassword is not set in datasource '{name}'` |
| `S008` | `{app}/manifest.toml: appId '{value}' is not a valid UUID` |
| `S009` | `{app}/manifest.toml: type must be 'app-main' or 'app-service', got '{value}'` |
| `S010` | `manifest.toml: bundleVersion '{value}' is not valid semver` |

### 11.2 Layer 2 — Existence errors

| Code | Message template |
|---|---|
| `E001` | `{path}: file not found — referenced by {config_key} in {app}` |
| `E002` | `{app}: directory listed in bundle manifest but not found` |
| `E003` | `{app}: missing manifest.toml` |
| `E004` | `{app}: missing resources.toml` |
| `E005` | `{app}: missing app.toml` |

### 11.3 Layer 3 — Cross-reference errors

| Code | Message template |
|---|---|
| `X001` | `DataView '{dv}' references datasource '{ds}' not declared in {app}/resources.toml` |
| `X002` | `View '{view}' references dataview '{dv}' not declared in {app}/app.toml` |
| `X003` | `View '{view}' handler resource '{res}' not declared in {app}/resources.toml` |
| `X004` | `Invalidates target '{target}' does not exist in {app}` |
| `X005` | `Service '{svc}' references appId '{id}' not found in bundle` |
| `X006` | `Duplicate appId '{id}' in {appA} and {appB}` |
| `X007` | `Duplicate datasource name '{name}' in {app}/resources.toml` |
| `X008` | `View '{view}': dataview handler only valid for view_type=Rest` |
| `X009` | `View '{view}': method must be GET when view_type=Websocket` |
| `X010` | `View '{view}': method must be GET when view_type=ServerSentEvents` |
| `X011` | `SPA config is only valid on app-main in {app}` |
| `X012` | `Init handler declared but missing module or entrypoint in {app}` |
| `X013` | `Datasource '{name}': x-type '{xt}' does not match driver '{drv}'` |

### 11.4 Layer 4 — Syntax errors

| Code | Message template |
|---|---|
| `C001` | `{file}: SyntaxError at line {line}, column {col}: {message}` |
| `C002` | `{file}: entrypoint '{name}' not found in exports — available: [{exports}]` |
| `C003` | `{file}: WASM validation failed: {message}` |
| `C004` | `{file}: import '{path}' resolves outside {app}/libraries/ — cross-app imports not permitted` |
| `C005` | `{file}: import '{path}' target file not found` |
| `C006` | `{file}: invalid JSON in schema — {parse_error}` |
| `C007` | `{file}: schema missing 'type' field` |
| `C008` | `{file}: schema 'required' array references property '{prop}' not in 'properties'` |

### 11.5 Warnings (do not affect exit code)

| Code | Message template |
|---|---|
| `W001` | `Unknown driver '{driver}' in {app}/resources.toml — cannot verify at build time` |
| `W002` | `Layer 4 skipped — engine dylib not available for {engine}` |
| `W003` | `Layer 4 skipped — riversd.toml not found` |
| `W004` | `{app}: no views defined — check [api.views.*] (not [views.*])` |
| `W005` | `{file}: skip_introspect = true on a DataView with a GET query — likely a misconfiguration` |
| `W006` | `{file}: subscribable = true on an MCP resource whose bound DataView has no GET method` |
| `W007` | `{file}: cursor_key is set but the query has no ORDER BY clause (cursor pagination requires deterministic ordering)` |
| `W008` | `{file}: transaction=true on a DataView backed by a driver that does not support transactions` |
| `W009` | `{file}: guard_view target has auth = "session" — sessions don't exist when the guard runs` |
| `W010` | `{file}: view has both guard = true (server-wide auth gate) and guard_view = "..." (per-view gate)` |
| `W011` | `Cron views declared with a node-local storage backend ({backend}); multi-instance dedupe does not work` |
| `W012` | `OTLP view: {field} = {value} is unusually large (OTLP/HTTP recommends 4); accepting but flagging` |

### 11.5.1 X-OTLP-N marker convention

OTLP-view-specific structural failures are emitted as `S005` (the existing invalid-value code) with a `[X-OTLP-N]` marker prepended to the message. The markers correspond to spec rules in `docs/arch/rivers-otlp-view-spec.md` §9 and let docs/agents grep for specific OTLP failure modes without introducing per-rule error codes:

| Marker | Condition |
|---|---|
| `[X-OTLP-1]` | `path` ends with `/v1/{metrics,logs,traces}` — operator is mounting OTLP under a non-OTLP view type |
| `[X-OTLP-2]` | Neither `handlers.{metrics,logs,traces}` nor a single `handler` declared, OR an unknown handler signal name |
| `[X-OTLP-3]` | `auth` is anything other than `"none"` (OTLP is stateless; use `guard_view` for bearer-style auth) |
| `[X-OTLP-4]` | `streaming = true` (OTLP/HTTP is unary) |
| `[X-OTLP-5]` | Both single `handler` and `handlers.*` declared (mutually exclusive) |
| `[X-OTLP-6]` | A field from another view type's domain is declared on an OTLP view |
| `[W-OTLP-1]` | `max_body_mb > 16` — emitted as `W012`, not `S005` |

`[X-OTLP-7]` and `[X-OTLP-8]` (Layer 3 module/entrypoint resolution for the `handlers.*` table) land when the dispatcher ships — see `rivers-otlp-view-spec.md` §14.3.

### 11.6 Gate 2 live check errors

| Code | Message template |
|---|---|
| `L001` | `Required resource '{name}' lockbox alias '{alias}' not found in keystore` |
| `L002` | `Driver '{driver}' not registered in this riversd instance` |
| `L003` | `Schema validation failed for {file} against {driver}: {detail}` |
| `L004` | `x-type '{xt}' does not match registered driver '{drv}' for datasource '{name}'` |
| `L005` | `Required service '{svc}' (appId: {id}) is not running` |

---

## 12. Graceful Degradation

### 12.1 Degradation hierarchy

| Missing component | Layers that run | Layers skipped |
|---|---|---|
| Everything available | 1, 2, 3, 4 | — |
| Engine dylibs not found | 1, 2, 3 | 4 |
| V8 dylib only missing | 1, 2, 3, 4 (WASM only) | 4 (TS/JS only) |
| Wasmtime dylib only missing | 1, 2, 3, 4 (TS/JS only) | 4 (WASM only) |
| `riversd.toml` not found | 1, 2, 3 | 4 |

### 12.2 CI pipeline without engine dylibs

A minimal CI pipeline can run `riverpackage validate` without engine dylibs installed. Layers 1–3 catch structural, existence, and logical errors. Layer 4 is skipped with warnings. The exit code is 0 if Layers 1–3 pass — skipped checks do not produce errors.

### 12.3 Full developer workstation

A developer with the full Rivers installation (riversd + engine dylibs) gets all four layers. This is the recommended configuration for development.

---

## 13. Affected Specifications

This specification amends the following documents:

| Spec | Section | Change |
|---|---|---|
| `rivers-application-spec.md` | §14 Validation Rules | Reference `rivers_runtime` validation modules as authoritative source; remove inline rule list in favor of error catalog reference |
| `rivers-application-spec.md` | §8 Deployment Lifecycle | Add reference to Gate 2 validation between bundle receipt and RESOLVING state |
| `rivers-technology-path-spec.md` | §19.4 Deployment Lifecycle | Step 3 references `rivers_runtime::validate_bundle_live()` |
| `rivers-v1-admin.md` | Bundle Validation section | Replace `riversctl validate` documentation with `riverpackage validate`; update validation check list to reference four-layer model |
| `rivers-canary-fleet-spec.md` | OPS profile tests | `OPS-DOCTOR-LINT-PASS` and `OPS-DOCTOR-LINT-FAIL` become `OPS-RIVERPACKAGE-VALIDATE-PASS` and `OPS-RIVERPACKAGE-VALIDATE-FAIL` |
| `rivers-driver-spec.md` | §7 Plugin System | ABI version bump; document `_rivers_compile_check` export |
| `rivers-processpool-runtime-spec-v2.md` | Engine trait | Document `compile_check` method addition |

---

## Appendix A — `did you mean?` Suggestions

When Layer 1 encounters an unknown key, the validator computes Levenshtein distance against known keys for the same table level. If a known key is within distance 2, it is offered as a suggestion:

```
[FAIL] orders-service/app.toml — unknown key 'veiew_type' in [api.views.list_orders]
       did you mean 'view_type'?
```

This targets the exact class of error from the IPAM incident — a typo in a key name that silently passes TOML parsing but produces no config effect.

---

## Appendix B — Validation Sequence Diagram

```
riverpackage validate <bundle_dir>
    │
    ├─ Read riversd.toml
    │   ├─ Found → parse [engines] section
    │   └─ Not found → engines = None (Layer 4 will skip)
    │
    ├─ Load engine dylibs (if available)
    │   ├─ V8: dlopen → abi_version check → resolve compile_check
    │   └─ Wasmtime: dlopen → abi_version check → resolve compile_check
    │
    ├─ Layer 1: Structural TOML
    │   ├─ Parse bundle manifest.toml → BundleManifest (deny_unknown_fields)
    │   └─ Per app:
    │       ├─ Parse manifest.toml → AppManifest
    │       ├─ Parse resources.toml → ResourcesConfig
    │       └─ Parse app.toml → AppConfig
    │
    ├─ Layer 2: Resource Existence
    │   └─ Per app:
    │       ├─ Check handler module paths
    │       ├─ Check schema file paths
    │       ├─ Check init handler module path
    │       ├─ Check SPA assets
    │       └─ Check lib entries
    │
    ├─ Layer 3: Logical Cross-References
    │   ├─ DataView → datasource resolution
    │   ├─ View → DataView resolution
    │   ├─ View → resource resolution
    │   ├─ Invalidates → target resolution
    │   ├─ Service → appId resolution
    │   ├─ Bundle manifest → directory resolution
    │   ├─ appId uniqueness
    │   ├─ Datasource name uniqueness
    │   ├─ nopassword/lockbox consistency
    │   ├─ x-type/driver consistency
    │   └─ View type constraint checks
    │
    ├─ Layer 4: Syntax Verification (if engines available)
    │   ├─ Schema JSON: parse + structural check
    │   ├─ TS/JS: V8 compile_check → export verification
    │   ├─ WASM: Wasmtime compile_check → export verification
    │   └─ Import path resolution (file existence)
    │
    └─ Format output (text or JSON) → exit code
```
