# Rivers Bundle Validation — Friction Resolution & Handoff Guide

**Date:** 2026-04-06  
**Purpose:** Resolve ambiguities and friction points in `rivers-bundle-validation-spec.md` and `rivers-bundle-validation-amendments.md` before Claude Code implementation.  
**Audience:** Claude Code (agentic implementation)

---

## FR-1 — Complete Allowed Field Sets per Config Struct

The spec defines required fields but not the full allowed set. `deny_unknown_fields` will reject any key not in the struct. Claude Code needs the exhaustive list.

### Bundle `manifest.toml` — `BundleManifest`

| Field | Type | Required | Default |
|---|---|---|---|
| `bundleName` | String | yes | — |
| `bundleVersion` | String | yes | — |
| `source` | String | yes | — |
| `apps` | Vec\<String\> | yes | — |

No optional fields. This struct is tight.

### Per-app `manifest.toml` — `AppManifest`

| Field | Type | Required | Default |
|---|---|---|---|
| `appName` | String | yes | — |
| `description` | String | no | None |
| `version` | String | yes | — |
| `type` | String | yes | — |
| `appId` | String | yes | — |
| `entryPoint` | String | yes | — |
| `appEntryPoint` | String | no | None |
| `source` | String | yes | — |
| `spa` | SpaConfig | no | None |
| `init` | InitConfig | no | None |

### `SpaConfig` (nested in AppManifest)

| Field | Type | Required | Default |
|---|---|---|---|
| `root` | String | yes | — |
| `indexFile` | String | yes | — |
| `fallback` | bool | no | true |
| `maxAge` | u64 | no | 86400 |

### `InitConfig` (nested in AppManifest)

| Field | Type | Required | Default |
|---|---|---|---|
| `module` | String | yes | — |
| `entrypoint` | String | yes | — |

### Per-app `resources.toml` — `ResourcesConfig`

| Field | Type | Required | Default |
|---|---|---|---|
| `datasources` | Vec\<DatasourceDecl\> | no | [] |
| `services` | Vec\<ServiceDecl\> | no | [] |

### `DatasourceDecl`

| Field | Type | Required | Default |
|---|---|---|---|
| `name` | String | yes | — |
| `driver` | String | yes | — |
| `x-type` | String | yes | — |
| `lockbox` | String | conditional | — |
| `nopassword` | bool | no | false |
| `required` | bool | yes | — |
| `host` | String | no | None |
| `port` | u16 | no | None |
| `database` | String | no | None |
| `username` | String | no | None |
| `password` | String | no | None |
| `service` | String | no | None |

**Note:** `host`, `port`, `database`, `username`, `password` are driver-specific connection fields. They appear in `resources.toml` for drivers that use inline credentials (dev/test) rather than LockBox. The struct must allow them. `service` is used by the HTTP driver for service resolution.

**Implementation guidance:** Use `#[serde(deny_unknown_fields)]` on the struct. The field set above is exhaustive. Any key not in this list is a hard error.

### `ServiceDecl`

| Field | Type | Required | Default |
|---|---|---|---|
| `name` | String | yes | — |
| `appId` | String | yes | — |
| `required` | bool | yes | — |

### Per-app `app.toml` — `AppConfig`

This is the largest struct. Top-level tables:

| Table | Type | Required |
|---|---|---|
| `[data]` | DataConfig | no |
| `[api]` | ApiConfig | no |

### `DataConfig`

| Field | Type | Required |
|---|---|---|
| `dataviews` | HashMap\<String, DataViewConfig\> | no |

### `DataViewConfig`

| Field | Type | Required | Default |
|---|---|---|---|
| `name` | String | yes | — |
| `datasource` | String | yes | — |
| `query` | String | yes | — |
| `parameters` | Vec\<ParameterConfig\> | no | [] |
| `caching` | CachingConfig | no | None |
| `max_rows` | u32 | no | 1000 |
| `invalidates` | Vec\<String\> | no | [] |
| `get_schema` | String | no | None |
| `post_schema` | String | no | None |
| `put_schema` | String | no | None |
| `delete_schema` | String | no | None |

### `ParameterConfig`

| Field | Type | Required | Default |
|---|---|---|---|
| `name` | String | yes | — |
| `type` | String | yes | — |
| `required` | bool | no | false |
| `default` | Value | no | None |
| `location` | String | no | None |

### `CachingConfig`

| Field | Type | Required | Default |
|---|---|---|---|
| `ttl_seconds` | u64 | yes | — |

### `ApiConfig`

| Field | Type | Required |
|---|---|---|
| `views` | HashMap\<String, ViewConfig\> | no |

### `ViewConfig`

| Field | Type | Required | Default |
|---|---|---|---|
| `path` | String | yes | — |
| `method` | String | yes | — |
| `view_type` | String | yes | — |
| `response_format` | String | no | "envelope" |
| `auth` | String | no | "session" |
| `handler` | HandlerConfig | yes | — |
| `parameter_mapping` | ParameterMappingConfig | no | None |
| `process_pool` | String | no | "default" |
| `libs` | Vec\<String\> | no | [] |
| `datasources` | Vec\<String\> | no | [] |
| `dataviews` | Vec\<String\> | no | [] |
| `allow_outbound_http` | bool | no | false |
| `allow_env_vars` | bool | no | false |
| `session_stage` | String | no | "before_pre_process" |
| `methods` | HashMap\<String, MethodConfig\> | no | None |

**Note on `methods` vs top-level:** Views can define handler at top level OR per-method under `[api.views.X.methods.POST.handler]`. Both patterns must be accepted.

### `HandlerConfig`

| Field | Type | Required | Default |
|---|---|---|---|
| `type` | String | yes | — |
| `dataview` | String | conditional | — |
| `language` | String | conditional | — |
| `module` | String | conditional | — |
| `entrypoint` | String | conditional | — |
| `resources` | Vec\<String\> | no | [] |

`type = "dataview"` requires `dataview`. `type = "codecomponent"` requires `language`, `module`, `entrypoint`.

### `ParameterMappingConfig`

| Field | Type | Required |
|---|---|---|
| `query` | HashMap\<String, String\> | no |
| `path` | HashMap\<String, String\> | no |
| `body` | HashMap\<String, String\> | no |
| `header` | HashMap\<String, String\> | no |

---

## FR-2 — Engine Trait: Use Existing Architecture, Not New Trait

**Problem:** The spec describes an `EnginePlugin` trait that doesn't exist. The actual engine architecture uses `Worker` trait + `WorkerFactory` for runtime, and engine dylibs are loaded via the existing plugin loading mechanism (§7 of driver spec). There is no separate `EnginePlugin` trait.

**Resolution:** `compile_check` is a standalone FFI function exported by the engine dylib. It does not go on the `Worker` trait (which is a runtime execution contract) or on a new `EnginePlugin` trait. It is a pure function — no state, no isolate lifecycle, no pool.

The engine dylib already exports:
- `_rivers_abi_version() -> u32`
- Engine-specific registration (V8 registers as engine type `"v8"`, Wasmtime as `"wasmtime"`)

Add:
- `_rivers_compile_check(source, filename) -> JSON string`
- `_rivers_free_string(ptr)` — free the JSON result

**The Rust-side trait in the spec (`EnginePlugin`) should be removed from the spec.** Replace with documentation of the two new FFI exports. `rivers_runtime` loads the dylib, resolves the two symbols, and calls them directly. No trait abstraction needed — there's no polymorphism at play. `riverpackage` knows it's calling V8 for `.ts`/`.js` and Wasmtime for `.wasm` based on file extension.

**Spec amendment needed:** Section 5 of `rivers-bundle-validation-spec.md` — replace the `EnginePlugin` trait with the FFI-only interface:

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
        let result: CompileCheckFFI = serde_json::from_str(json_str)?;
        unsafe { (self.free_string_fn)(json_ptr) };
        // Convert to CompileCheckResult or CompileCheckError
        Ok(result.into())
    }
}
```

---

## FR-3 — FFI Boundary: JSON Serialization (Option A)

**Locked.** `_rivers_compile_check` returns a heap-allocated JSON `*const c_char`. Caller frees via `_rivers_free_string`.

Success response:
```json
{"ok": true, "exports": ["onCreateOrder", "default"]}
```

Error response:
```json
{"ok": false, "error": {"filename": "orders.ts", "line": 14, "column": 8, "message": "Unexpected token"}}
```

Null line/column in JSON when not available (e.g., WASM validation errors that don't have line numbers).

---

## FR-4 — Config Path: `riversd.toml`, Not `config/rivers.toml`

**Problem:** The spec says `config/rivers.toml`. The actual config file is `riversd.toml`.

**Resolution:** All references to `config/rivers.toml` in both spec documents become `riversd.toml`.

Engine paths go in a new `[engines]` section in `riversd.toml`:

```toml
[engines]
v8       = "/usr/lib/rivers/librivers_engine_v8.dylib"
wasmtime = "/usr/lib/rivers/librivers_engine_wasm.dylib"
```

`riverpackage` discovery sequence:
1. `--config` flag (if specified)
2. `RIVERS_CONFIG` env var (if set)
3. `riversd.toml` in CWD
4. Not found → Layer 4 skipped with warning W003

This matches `riversd`'s own config discovery pattern documented in the admin guide.

---

## FR-5 — `rivers_validate` Crate vs `rivers_runtime` Dependency

**Problem:** Validation logic currently lives in `rivers_runtime` (`validate_bundle()`, `validate_known_drivers()`, `load_bundle()`). Creating `rivers_validate` as a new crate needs a clean dependency direction.

**Resolution:** Minimize disruption. Do NOT create a new crate. Put the new validation code in `rivers_runtime` alongside the existing validation functions.

```
crates/rivers-runtime/src/
├── lib.rs
├── loader.rs          # existing — load_bundle(), load_server_config()
├── validate.rs        # existing — validate_bundle(), validate_known_drivers()
├── validate_structural.rs   # NEW — Layer 1
├── validate_existence.rs    # NEW — Layer 2
├── validate_crossref.rs     # NEW — Layer 3
├── validate_syntax.rs       # NEW — Layer 4
├── validate_engine.rs       # NEW — engine dylib loading
├── validate_result.rs       # NEW — ValidationReport, error types
└── validate_format.rs       # NEW — text and JSON formatters
```

The existing `validate_bundle()` and `validate_known_drivers()` become thin wrappers that call into the new layer functions. No existing call sites break. New callers use the layer functions directly.

`riverpackage` already depends on `rivers_runtime`. `riversd` already depends on `rivers_runtime`. No new crate dependency needed.

The public API surface:

```rust
// In rivers_runtime

/// Gate 1: offline validation (riverpackage)
pub fn validate_bundle_full(config: &ValidationConfig) -> ValidationReport;

/// Gate 2: offline + live checks (riversd)
pub fn validate_bundle_live(
    config: &ValidationConfig,
    lockbox: &dyn LockBoxResolver,
    drivers: &DriverFactory,
) -> ValidationReport;

/// Existing — preserved for backward compatibility, calls validate_bundle_full() internally
pub fn validate_bundle(bundle: &LoadedBundle) -> Vec<String>;
pub fn validate_known_drivers(bundle: &LoadedBundle, drivers: &[String]) -> Vec<String>;
```

---

## FR-6 — swc Dependency: Lives in the Engine Dylib

**Problem:** V8 compile check needs TS transpilation via swc. Where does swc live?

**Resolution:** swc stays inside the V8 engine dylib. The `_rivers_compile_check` FFI function handles transpilation internally — same code path as ProcessPool bundle load. `rivers_runtime` (and therefore `riverpackage`) never touches swc. It sends raw `.ts` source bytes across the FFI boundary and gets back a compile result. The engine dylib decides whether transpilation is needed based on the filename extension.

This means:
- `rivers_runtime` has no swc dependency
- `riverpackage` has no swc dependency
- The V8 engine dylib owns the full pipeline: detect TS → transpile via swc → compile via V8 → enumerate exports
- Zero risk of a second transpilation path

---

## FR-7 — VALIDATING State: Locate the State Machine

**Problem:** Adding VALIDATING between PENDING and RESOLVING requires finding every state match and transition in the codebase.

**Resolution for Claude Code:** The deploy state machine is in `crates/riversd/src/`. Specific search targets:

```bash
# Find the state enum
grep -rn "enum.*Deploy\|PENDING\|RESOLVING\|STARTING\|RUNNING\|FAILED\|STOPPING\|STOPPED" crates/riversd/src/

# Find state transitions
grep -rn "PENDING\|DeployState\|deploy_state\|set_state\|transition" crates/riversd/src/

# Find admin API serialization of state
grep -rn "serde.*DeployState\|Serialize.*Deploy\|deploy.*state.*json" crates/riversd/src/
```

The change is:
1. Add `Validating` variant to the enum
2. Insert transition: `Pending → Validating` (when bundle is received)
3. Insert transition: `Validating → Resolving` (when validation passes)
4. Insert transition: `Validating → Failed` (when validation fails)
5. Call `rivers_runtime::validate_bundle_full()` (or `validate_bundle_live()` with live context) in the transition from Pending to Validating
6. Serialize as `"VALIDATING"` in admin API responses

---

## FR-8 — `riversctl doctor --lint` Removal

**Implementation steps:**

1. Open `crates/riversctl/src/commands/doctor.rs`
2. Remove the `--lint` flag parsing (`"--lint" => { lint_mode = true; }`)
3. Remove the `--app` flag parsing (only used by lint)
4. Remove the entire `if lint_mode { ... }` block
5. Remove the `lint_app_conventions()` helper function
6. Remove the `use rivers_runtime::{load_bundle, validate_bundle, validate_known_drivers}` imports if they become unused
7. Verify `doctor --fix` still works (it doesn't touch lint code)
8. Update help text to remove `--lint` from the usage string

**Do not touch:** `doctor --fix`, `doctor` (no flags), permission checks, TLS cert checks, log directory checks, PID file checks. These are system health — they stay.

---

## FR-9 — Test Fixtures: Build Bundles From Scratch

**Problem:** Harness tests that create bundles via `riverpackage init` then mutate files are fragile. If `init` output changes, tests break.

**Resolution:** Tests create bundle directories from scratch using inline content. No dependency on `riverpackage init`.

Helper function pattern:

```rust
/// Create a minimal valid bundle for testing.
/// Returns the bundle directory path.
fn create_test_bundle(dir: &Path, name: &str) -> PathBuf {
    let bundle_dir = dir.join(name);
    let app_dir = bundle_dir.join(name);
    let schemas_dir = app_dir.join("schemas");
    let handlers_dir = app_dir.join("libraries/handlers");
    std::fs::create_dir_all(&schemas_dir).unwrap();
    std::fs::create_dir_all(&handlers_dir).unwrap();

    // Bundle manifest
    std::fs::write(bundle_dir.join("manifest.toml"), format!(r#"
bundleName = "{name}"
bundleVersion = "1.0.0"
source = "test"
apps = ["{name}"]
"#)).unwrap();

    // App manifest
    std::fs::write(app_dir.join("manifest.toml"), format!(r#"
appName = "{name}"
appId = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
type = "app-service"
version = "1.0.0"
entryPoint = "http://0.0.0.0:9200"
source = "test"
"#)).unwrap();

    // Resources
    std::fs::write(app_dir.join("resources.toml"), r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true
"#).unwrap();

    // App config
    std::fs::write(app_dir.join("app.toml"), r#"
[data.dataviews.items]
name       = "items"
datasource = "data"
query      = "schemas/item.schema.json"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#).unwrap();

    // Schema
    std::fs::write(schemas_dir.join("item.schema.json"), r#"
{"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}
"#).unwrap();

    bundle_dir
}

/// Inject a specific defect into a test bundle.
enum BundleDefect {
    UnknownTomlKey { file: &'static str, key: &'static str, value: &'static str },
    MissingHandlerModule { module_path: &'static str },
    UndeclaredDatasource { dataview_name: &'static str, bad_datasource: &'static str },
    SyntaxErrorTs { filename: &'static str, content: &'static str },
    WrongExport { filename: &'static str, content: &'static str, wrong_entrypoint: &'static str },
}
```

Each negative test calls `create_test_bundle()` then applies a specific `BundleDefect`. No mutation of `init` output. Bundle structure is inline and stable.

The existing `OPS-RIVERPACKAGE-INIT` test still exercises `riverpackage init` — that test validates the scaffolding tool. The *validation* tests don't depend on it.

---

## FR-10 — Import Resolution: Relative Paths Only

**Problem:** Layer 4 import resolution encounters bare specifiers (`import lodash from "lodash"`) and won't know whether to error, warn, or ignore.

**Resolution:** Layer 4 import resolution checks **relative paths only**. The rule:

- Starts with `./` or `../` → resolve relative to the importing file, verify target exists within `{app}/libraries/`, error if not found or if it escapes the app boundary
- Starts with `/` → absolute path, error unconditionally (paths must be relative)
- Anything else (bare specifier: `"lodash"`, `"@org/pkg"`) → skip silently. These are resolved at runtime by the `libs` declaration in the view config. Layer 3 already validates that `libs[]` entries exist as files.

This connects the dot between Layer 3 (`libs[]` file existence) and Layer 4 (import resolution):
- Layer 3 checks that declared `libs` files exist
- Layer 4 checks that relative imports between handler files resolve
- Bare specifiers are the `libs` mechanism's responsibility — Layer 4 doesn't second-guess it

**Import parsing:** Use a simple regex or string scan for `import` and `from` statements. Do NOT compile the module to find imports — that's circular (compile check is what we're doing). A line-by-line scan for `from "..."` and `from '...'` after `import` is sufficient. Dynamic `import()` expressions are ignored (they'll fail at runtime via the capability model anyway — no dynamic imports allowed).

---

## Application Order for Claude Code

Recommended implementation sequence:

1. **FR-5** — Add new validation modules to `rivers_runtime` (no existing code changes)
2. **FR-1** — Add `deny_unknown_fields` to config structs using the field sets above. Fix any tests that break (they reveal real gaps).
3. **FR-8** — Remove `--lint` from doctor. Small, surgical.
4. **FR-4** — Add `[engines]` section support to config parser
5. **FR-2 + FR-3** — Add `_rivers_compile_check` and `_rivers_free_string` FFI exports to V8 and Wasmtime engine dylibs
6. **FR-6** — Verify swc is called inside V8 dylib's compile_check (should be natural if built on existing transpile code)
7. **FR-7** — Add VALIDATING state to deploy state machine
8. **FR-9 + FR-10** — Write test fixtures and harness tests last, after the validation code works
