# Tutorial: Bundle Validation

Validate your Rivers bundle before deployment using `riverpackage validate`. This catches configuration errors, missing files, broken references, and syntax problems in a single pass.

## Quick Start

```bash
riverpackage validate my-bundle/
```

## What Validation Checks

The validation pipeline runs four layers:

| Layer | What it checks |
|-------|----------------|
| 1 — Structural TOML | All TOML files parse, have correct keys and types, required fields present. Unknown keys produce "did you mean?" suggestions. |
| 2 — Resource Existence | Every referenced file (handlers, schemas, init modules, SPA assets) exists on disk. |
| 3 — Cross-References | DataView→datasource, view→DataView, invalidates targets, service→appId, appId uniqueness, driver/x-type consistency, view type constraints. |
| 4 — Syntax Verification | Schema JSON structure, JS/TS compilation via V8, WASM validation via Wasmtime, import path resolution. Requires engine dylibs. |

## Reading Text Output

```
Rivers Bundle Validation — my-app v1.0.0
=========================================

Layer 1: Structural TOML
  [PASS] manifest.toml — bundle manifest valid
  [PASS] my-app/manifest.toml — app manifest valid
  [FAIL] my-app/app.toml — unknown key 'veiew_type' in [api.views.list]
         did you mean 'view_type'?

Layer 2: Resource Existence
  [PASS] my-app/schemas/item.schema.json — exists

Layer 3: Logical Cross-References
  [PASS] DataView 'items' → datasource 'data' — resolved

Layer 4: Syntax Verification
  [SKIP] Layer 4 skipped — engine dylibs not configured

RESULT: 1 error, 0 warnings
```

## JSON Output

```bash
riverpackage validate my-bundle/ --format json
```

Outputs a structured JSON object with `layers`, `summary`, and per-check `results[]`. Stable contract for CI/CD integration. See `rivers-bundle-validation-spec.md` §8.2 for the full schema.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All checks passed (warnings may be present) |
| 1 | One or more validation errors |
| 2 | Bundle directory not found or unreadable |

## Common Errors and Fixes

### S002 — Unknown key (typo)

```
[FAIL] my-app/app.toml — unknown key 'veiew_type'
       did you mean 'view_type'?
```

Fix: correct the typo in your TOML file.

### E001 — File not found

```
[FAIL] my-app/libraries/handlers/orders.ts — file not found
       referenced by: api.views.create_order.handler.module
```

Fix: create the missing file or fix the path in `app.toml`.

### X001 — Unknown datasource

```
[FAIL] DataView 'users' → datasource 'users-db' — not declared
```

Fix: add the datasource to `resources.toml` and `app.toml`, or fix the name.

### S008 — Invalid appId

```
[FAIL] my-app/manifest.toml — appId 'not-a-uuid' is not a valid UUID
```

Fix: generate a proper UUID v4 for appId.

## Engine Dylib Setup (Layer 4)

Layer 4 requires engine dylibs for JS/TS compilation and WASM validation. Without them, Layer 4 is skipped with a warning — Layers 1-3 still run.

To enable Layer 4, point to the engine dylibs via `--config`:

```bash
riverpackage validate my-bundle/ --config /path/to/riversd.toml
```

The `riversd.toml` should have an `[engines]` section:

```toml
[engines]
dir = "/opt/rivers/lib"
```

Or explicit paths:

```toml
[engines]
v8       = "/opt/rivers/lib/librivers_engine_v8.dylib"
wasmtime = "/opt/rivers/lib/librivers_engine_wasm.dylib"
```

## Error Code Reference

| Prefix | Layer | Examples |
|--------|-------|---------|
| S0xx | Structural TOML | S001 parse error, S002 unknown key, S003 missing field |
| E0xx | Resource Existence | E001 file not found, E003 missing manifest.toml |
| X0xx | Cross-References | X001 unknown datasource, X005 unknown appId |
| C0xx | Syntax Verification | C001 syntax error, C006 invalid schema JSON |
| W0xx | Warnings | W003 engine dylibs not available |

Full error catalog: `rivers-bundle-validation-spec.md` §11.
