# cb-ts-repro — Rivers TypeScript Support Regression Suite

Minimal Rivers bundle that acts as a **regression test suite for TypeScript support**. Nine endpoints, each probing one behavior. Cases currently failing against Rivers 0.54.1 are the defects to close; cases currently passing are the behaviors to keep passing.

## Purpose

Rivers supports TypeScript as a first-class handler language. The 0.54.1 implementation is substantively incomplete: ordinary TypeScript idioms (typed parameters, typed locals, `type` imports, generics, `export function`, shared `_lib` helpers) fail at dispatch. This bundle is the smallest runnable artifact that isolates each failure for a Rivers-side fix.

Companion defect report: [`../rivers-ts-pipeline-findings.md`](../rivers-ts-pipeline-findings.md)

## Layout

```
cb-ts-repro/
├── manifest.toml
├── probe/
│   ├── manifest.toml
│   ├── resources.toml
│   ├── app.toml                    — 9 views, each a case probe
│   ├── schemas/answer.schema.json
│   └── libraries/handlers/
│       ├── case-a.js               PASS baseline (plain JS)
│       ├── case-b.ts               FAIL parameter type annotation
│       ├── case-c.ts               FAIL variable type annotation
│       ├── case-d.ts               FAIL `type` import
│       ├── case-e.ts               FAIL generic type parameter
│       ├── case-f.ts               FAIL multi-module import
│       ├── case-g.ts               FAIL export-default module semantics
│       ├── case-h.ts               PASS working TS shape
│       └── case-i.js               PASS ctx.dataview reference
└── run-probe.sh                    curl each case, print status + body
```

## Run

```bash
mkdir -p probe/data
sqlite3 probe/data/probe.db "SELECT 1" >/dev/null

# in repo root Cargo.toml context:
riverpackage validate .
riverpackage preflight .

# start riversd with this bundle:
# (adapt paths for your setup)
cat > /tmp/probe-riversd.toml <<EOF
bundle_path = "$(pwd)"
[base]
host = "0.0.0.0"
port = 8080
log_level = "info"
[storage_engine]
backend = "memory"
EOF

riversd --config /tmp/probe-riversd.toml &
./run-probe.sh
```

## Test Matrix (Rivers v0.54.1 current → target behavior)

| Case | Description | Current | Target | Defect |
|------|---|---|---|---|
| A | plain JS baseline | **pass** | pass | — |
| B | `function handler(ctx: any)` parameter annotation | **fail** | pass | #1 |
| C | `const x: number = 42` variable annotation | **fail** | pass | #1 |
| D | `import { type X, foo } from './...'` `type`-only hoist | **fail** | pass | #2 |
| E | `function identity<T>(x: T): T` generic | **fail** | pass | #1 |
| F | `import { foo } from './sibling'` multi-module | **fail** | pass | #4 |
| G | `export function handler(ctx)` single-file module | **fail** | pass | #3 |
| H | TS using only currently-working features | **pass** | pass | — |
| I | `ctx.data.*` pre-fetched + `ctx.dataview()` | **pass** | pass | — |

Defect numbers refer to `../rivers-ts-pipeline-findings.md`. Closing defect #1 (swc stripper) alone flips B/C/E to pass, defect #2 flips D, defect #3 flips G, defect #4 flips F.

## Done Criteria

The Rivers fix is complete when all 9 cases return HTTP 200 with `"outcome": "pass"` in the JSON body. Once that holds, the bundle can live in Rivers' own test suite as a regression gate — any future change to the TS compilation pipeline must keep all 9 green.

## Root Causes (with source references)

All references are to paths under `dist/rivers-0.54.1/`.

### 1. Docstring ⇄ implementation drift in `compile_typescript`

`crates/riversd/src/process_pool/v8_config.rs:120-133` documents five TS features supported by the stripper:

```
/// - Type annotations on parameters: `(x: string)` -> `(x)`
/// - Return type annotations: `): string {` -> `) {`
/// - Interface/type declarations: removed entirely
/// - Generic type parameters: `<T>` -> removed
/// - `as` type assertions: `x as string` -> `x`
```

`strip_type_annotations()` at line 195-226 only implements **two of the five**: return-type and `as` assertion. Parameter annotations, variable annotations, and generics are silently passed through.

The regression-test at `crates/riversd/tests/process_pool_tests.rs:288-296` verifies `compile_typescript("const x: number = 42;", ...)` returns output containing "const x" — it does not assert the `": number"` is removed. The test passes with a no-op stripper.

**Effect:** every handler that types a parameter, a variable, or a generic hits V8 and errors.

### 2. `import { type X, ... }` unhandled

`compile_typescript` is line-based and does not parse import lists. A type-only hoist inside a named-imports clause is passed through verbatim. `is_module_syntax()` then sees `import ` and routes to `compile_module`, which rejects the TS syntax.

### 3. Multi-module resolve callback returns `None`

`crates/riversd/src/process_pool/v8_engine/execution.rs:62-69`:

```rust
let instantiate_result = module.instantiate_module(
    tc,
    |_context, _specifier, _import_attributes, _referrer| {
        // V1: reject all imports -- single-module only
        None
    },
);
```

Documented V1 limitation. `_lib.ts`-style shared-helper patterns are impossible; shared code must be inlined into every handler file or exposed via a Rivers global (which `_lib.ts` is not).

### 4. Module namespace vs. global scope for exports

`crates/riversd/src/process_pool/v8_engine/execution.rs:222-224`:

```
// For modules, exported functions are set on the module namespace.
// For V1: the module must set the entrypoint on the global scope
// (e.g., via a side effect or `globalThis.handler = handler`).
```

A handler that uses `export function handler(ctx) { ... }` is instantiated but `call_entrypoint("handler")` looks on the global scope, where the export is not present. Handler must either avoid `export` or explicitly set `globalThis.handler = handler`.

## Recommended Fixes

Listed in order of impact. All contained to `crates/riversd/src/process_pool/`. See `../rivers-ts-pipeline-findings.md` for full severity/effort rationale.

### A. Swap hand-rolled stripper for swc_core (closes defects #1 and #2)

`swc_core` is the canonical Rust TypeScript toolchain (used by Next.js, Deno, most TS-over-V8 runtimes). Minimal integration — strip-types mode only, no bundling:

```toml
# crates/riversd/Cargo.toml
swc_core = { version = "0.90", features = ["ecma_parser_typescript", "ecma_transforms_typescript"] }
```

Replace `compile_typescript()` with a swc strip-types pass. Drops ~150 LOC of hand-rolled parsing, closes defects #1 and #2 in one change, and handles the TypeScript that doesn't exist yet (future syntax releases).

Existing canary handlers stay working — swc is a superset of what the hand-rolled stripper accepts.

**Effort:** 2–3 days. Offered below as a CB-team contribution.

### B. `globalThis.*` auto-hoist for named exports (closes defect #3)

In `execute_as_module`, after successful evaluation, walk the module namespace and set each exported function on `globalThis`. Single-file ES modules become valid handlers with no user ceremony.

Rough shape:

```rust
let ns = module.get_module_namespace();
for key in ns.own_property_names() {
    let val = ns.get(scope, key)?;
    if val.is_function() {
        global.set(scope, key, val);
    }
}
```

~20 LOC. **Effort:** 1 day. Flips case G to pass.

### C. Expose `ctx.transaction()` (closes defect #5)

`rivers-drivers-builtin/src/sqlite.rs` has `begin_transaction` / `commit_transaction` / `rollback_transaction`. These are not exposed through the v8 ctx surface. For any handler that makes 2+ writes (CB's sync.ts does 26), invariants are at-risk under concurrent requests.

Minimal shape:

```javascript
ctx.transaction(function() {
    ctx.dataview("insert_parent", { ... });
    ctx.dataview("insert_child",  { ... });
    // throw → rollback; normal return → commit
});
```

Implementation: wrap the driver's begin/commit/rollback, bound to one datasource at a time (no XA).

### D. Multi-module `import` resolution (closes defect #4)

Flagged in source comments as a V2 feature. For CB-scale bundles, shared `_lib.ts` helpers are load-bearing; right now each handler must inline ~280 LOC of helpers or skip the feature. A filesystem-rooted resolver (`./libraries/handlers/_lib.ts` resolves to that file, scoped to the bundle subtree) closes case F. **Effort:** ~1 week.

## Suggested order

| Step | Effort | Cases flipped |
|---|---|---|
| A (swc stripper) | 2–3 days | B, C, D, E |
| B (globalThis auto-hoist) | 1 day | G |
| D (multi-module imports) | ~1 week | F |
| C (ctx.transaction) | 2 days | (race safety; no probe case) |

Steps A + B close 5 of 6 failing cases in ~4 days. All 9 cases green takes ~1.5 weeks focused. CB team offers to take step A as a contributed Rivers PR — see `../rivers-ts-pipeline-findings.md` for details.
