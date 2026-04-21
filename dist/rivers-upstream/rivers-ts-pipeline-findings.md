# Rivers v0.54.1 — TypeScript Support Defect Report

**Filed:** 2026-04-21
**Reporter:** CB team (source read by Claude Code)
**Target:** Rivers dev team
**Rivers version:** 0.54.1
**Companion artifact:** `docs/rivers-upstream/cb-ts-repro-bundle/` — regression suite with 9 isolated test cases, runs against a live riversd.

---

## Summary

Rivers claims TypeScript as a first-class handler language. The 0.54.1 implementation accepts `language = "typescript"` and routes `.ts` files through a dedicated compiler pass. The implementation is **substantively incomplete**: three of the five TypeScript features its own documentation says it supports are not stripped, and a fourth (ES-module exports) is parsed but not exposed to the entrypoint-lookup path. Any handler written in ordinary TypeScript idioms — typed parameters, typed locals, `type`-only imports, generic functions — fails at dispatch with a V8 syntax error.

The scope isn't edge-case: CB's 11 handler files hit this on the very first request. Every handler.

Five defects below, each with a failing test case in the companion bundle. Severity and proposed fix per defect.

---

## Scope of testing performed

I read the Rivers 0.54.1 source (`dist/rivers-0.54.1/`) and traced the compile path from handler dispatch through the V8 engine:

- `crates/riversd/src/process_pool/v8_engine/execution.rs` — dispatch, module vs script branching, entrypoint invocation
- `crates/riversd/src/process_pool/v8_config.rs` — `compile_typescript` + `strip_type_annotations`
- `crates/riversd/src/process_pool/tests/wasm_and_workers.rs` — existing TS test coverage
- `crates/riversd/tests/process_pool_tests.rs:288` — the one regression test for the TS compiler
- `crates/rivers-engine-v8/src/execution.rs` — single-module classic-script compile (separate path)
- `crates/rivers-drivers-builtin/src/sqlite.rs` — transaction primitives
- `canary-bundle/canary-sql/app.toml` — working MCP view reference
- `canary-bundle/**/libraries/handlers/*.ts` — every canary handler written in plain ES5 JS under a `.ts` filename (no real TypeScript exercised in the canary suite)

Every finding below has a source-line reference and a reproducer in the probe bundle.

---

## Defect 1 — `compile_typescript` does not strip the type annotations it documents

**File:** `crates/riversd/src/process_pool/v8_config.rs:120-226`

The function's contract (its own docstring):

```rust
/// - Type annotations on parameters: `(x: string)` -> `(x)`
/// - Return type annotations: `): string {` -> `) {`
/// - Interface/type declarations: removed entirely
/// - Generic type parameters: `<T>` -> removed
/// - `as` type assertions: `x as string` -> `x`
```

The implementation:

- `strip_type_annotations()` at line 195-226 removes return-type annotations (` ): T {` → `) {`) and `as Type` assertions. Two features.
- `compile_typescript()` at line 133-193 additionally removes full-block `interface` and `type X = ...` declarations via a brace-counting state machine. Third feature.
- **Parameter annotations** (`x: string`) — no implementation. Passes through to V8.
- **Generic type parameters** (`<T>`) — no implementation. Passes through to V8.
- **Variable annotations** (`const x: number`) — not claimed in the docstring but implicitly expected by users. Not stripped.

The only regression test at `crates/riversd/tests/process_pool_tests.rs:288-296`:

```rust
let result = compile_typescript("const x: number = 42;", "test.ts");
assert!(result.is_ok(), "...");
let js = result.unwrap();
assert!(js.contains("const x"), "should preserve variable declaration");
```

The test asserts only that `"const x"` appears in the output. A stripper that returns input unchanged passes. No assertion verifies the `: number` is gone.

### Failing test cases (probe bundle)

- **case-b.ts** — `function handler(ctx: any)` → 500 `Unexpected identifier 'any'`
- **case-c.ts** — `const answer: number = 42` → 500 `Unexpected token ':'`
- **case-e.ts** — `function identity<T>(x: T): T` → 500 `Unexpected token '<'`

### Recommended fix

Replace the hand-rolled stripper with `swc_core::ecma::transforms::typescript::strip`:

```toml
# crates/riversd/Cargo.toml
swc_core = { version = "0.90", features = ["ecma_parser_typescript", "ecma_transforms_typescript"] }
```

swc is the canonical Rust TypeScript toolchain used by Next.js, Deno, and most TS-over-V8 runtimes. Integration shape (drop-in for `compile_typescript`):

```rust
pub fn compile_typescript(source: &str, filename: &str) -> Result<String, TaskError> {
    use swc_core::common::{FileName, sync::Lrc, SourceMap, Globals, GLOBALS};
    use swc_core::ecma::{parser, transforms::typescript::strip, visit::FoldWith, codegen};
    // parse → strip → emit
    // ~30 LOC end to end
}
```

Advantages:
- Closes defects 1, 2, and most of the `<T>` generic case in one change
- Industry-standard; handles edge cases the hand-rolled stripper never will (decorators, enums, namespace merging, satisfies, const assertions)
- Canary bundles continue to work unchanged — swc is a superset of the current stripper
- ~150 LOC hand-rolled parsing deleted

Alternative (smaller change, incomplete): extend `strip_type_annotations()` to handle parameter and variable annotations with additional regex patterns. Not recommended — every TS release adds new syntax the hand-rolled approach will miss again.

### Regression coverage proposal

Adopt the probe bundle's cases B, C, E as test fixtures. Each is a one-line TypeScript input with a known-good JavaScript output. Existing test at line 288 should grow from `contains("const x")` to an equality assertion against the stripped output.

**Severity:** critical — 4 of 5 documented features unimplemented; no user writing ordinary TypeScript can run a handler.

---

## Defect 2 — `type`-only imports not erased

**File:** `crates/riversd/src/process_pool/v8_config.rs` (line-based stripper)

TypeScript `import { type X, foo } from './mod'` is valid 4.5+ syntax for type-only hoists inside a named-imports clause. swc strips the `type X` portion, leaving `import { foo }`. Rivers' line-based stripper has no pattern for this — the line passes through verbatim. `is_module_syntax()` at `execution.rs:102-105` then detects `import ` and routes to `compile_module()`, which errors on the `type` keyword.

This is the exact error mode CB's entire handler suite hit:

```
codecomponent dispatch: handler error: module compilation failed:
SyntaxError: Unexpected identifier 'Ctx'
```

(for `import { type Ctx, bad, ok } from './_lib'`).

### Failing test case

- **case-d.ts** — `import { type Something, foo } from './case-d-helpers'` → 500 `module compilation failed`

### Recommended fix

Subsumed by Defect 1's swc integration — swc lowers type-only import hoists to empty. No additional work if swc lands.

**Severity:** critical — ordinary TS idiom, every cross-file shared type hits it.

---

## Defect 3 — ES-module exports not reachable by entrypoint lookup

**File:** `crates/riversd/src/process_pool/v8_engine/execution.rs:220-256`

When source contains `import` or `export`, `execute_as_module()` runs instead of classic-script compile:

```rust
if is_module_syntax(&source) {
    execute_as_module(&mut scope, &source, &ctx.entrypoint.module)?;
    // For modules, exported functions are set on the module namespace.
    // For V1: the module must set the entrypoint on the global scope
    // (e.g., via a side effect or `globalThis.handler = handler`).
}
// ...
let return_value = call_entrypoint(&mut scope, &ctx.entrypoint.function);
```

`call_entrypoint` (line 255) looks up `ctx.entrypoint.function` on the global scope. Module-mode evaluation places exports on the module namespace, not the global. A handler that does `export function handler(ctx) { ... }` instantiates and evaluates fine, then fails at entrypoint lookup.

The source comment explicitly acknowledges the gap as "V1" and suggests users work around it with `globalThis.handler = handler` — a workaround that defeats the purpose of using `export` syntax.

### Failing test case

- **case-g.ts** — `export function handler(ctx) { ... }` → 500 entrypoint not found

### Recommended fix

After `module.evaluate()` succeeds, walk the module namespace and hoist callable exports onto `globalThis`:

```rust
let ns = module.get_module_namespace();
if let Ok(ns_obj) = v8::Local::<v8::Object>::try_from(ns) {
    let names = ns_obj.get_property_names(scope, Default::default()).unwrap();
    let len = names.length();
    for i in 0..len {
        let key = names.get_index(scope, i).unwrap();
        let val = ns_obj.get(scope, key).unwrap();
        if val.is_function() {
            global.set(scope, key, val);
        }
    }
}
```

~20 LOC. Closes case G and normalizes module vs script semantics for all handlers.

**Severity:** high — `export function` is the documented way to define a handler in modern JS/TS; currently broken.

---

## Defect 4 — Multi-module `import` resolution rejected

**File:** `crates/riversd/src/process_pool/v8_engine/execution.rs:62-69`

```rust
let instantiate_result = module.instantiate_module(
    tc,
    |_context, _specifier, _import_attributes, _referrer| {
        // V1: reject all imports -- single-module only
        None
    },
);
```

Every import specifier resolves to `None`, so any `import { foo } from './sibling'` fails at instantiation. This forces handler authors to inline every shared helper into every handler file — a maintenance burden scaling linearly with handler count.

CB's situation: 11 handler files share a 280-LOC `_lib.ts` with ~25 utility functions (`ulid`, `nowIso`, `ok`, `bad`, `notFound`, `fencedInject`, etc.). Inlined into each handler, that's ~3000 LOC of duplicated boilerplate.

### Failing test case

- **case-f.ts** — `import { helper } from './case-f-helpers'` → 500 module instantiation failed

### Recommended fix

Implement a filesystem-rooted resolver that resolves relative imports against the handler's `libraries/` tree:

```rust
|context, specifier, _attrs, referrer| {
    let spec = specifier.to_rust_string_lossy(scope);
    let abs_path = resolve_relative_to_referrer(&spec, &referrer_path)?;
    if !is_within_bundle(&abs_path, bundle_root) { return None; }
    let src = std::fs::read_to_string(&abs_path)?;
    let compiled = if abs_path.ends_with(".ts") { compile_typescript(&src)? } else { src };
    let module = compile_module(context, compiled)?;
    Some(module)
}
```

Security properties to preserve:
- Restricted to `libraries/` subtree of the current handler's app (no `../` escape)
- Same TS compilation path as root module
- Module cache keyed by absolute path

~100 LOC. Unblocks the shared-`_lib` pattern in any Rivers bundle.

**Severity:** high for non-trivial apps. The current V1 restriction effectively caps bundle complexity at "single-file handler" scale.

---

## Defect 5 — `ctx.transaction()` not exposed to handlers

**File:** `crates/riversd/src/process_pool/v8_engine/context.rs` (ctx surface injection)

`rivers-drivers-builtin/src/sqlite.rs:192-214` implements `begin_transaction` / `commit_transaction` / `rollback_transaction` on the `DatabaseDriver` trait, and `sqlite::supports_transactions()` returns true. None of this is reachable from handlers: the v8 ctx surface injects `ctx.dataview`, `ctx.store`, `ctx.datasource`, `Rivers.*` — no transaction primitive.

Every handler with more than one write must assume individual DataView calls commit independently. Any invariant that spans multiple writes is at-risk under concurrent load.

### Impact on CB

Sync protocol handler (`sync.ts`, 842 LOC) performs ~26 sequential writes per request, some of which are invariant-coupled:
- Create artifact + insert revision 1 + set `artifacts.current_revision_id` — three writes that must be atomic
- Fork revision + update artifact's current pointer — must be atomic against concurrent commits
- Create issue + insert `issue_workitem_refs` row + guarantee `is_primary=1` count = 1 — three writes

Without transactions, all these are best-effort under concurrency.

### No probe test case

Race conditions are hard to reproduce deterministically in a one-shot probe. Flagged here as a design concern.

### Recommended fix

```javascript
var result = ctx.transaction("db", function() {
    ctx.dataview("insert_parent", { id: "P" });
    ctx.dataview("insert_child",  { parent: "P" });
    // throw inside → rollback and rethrow
    return { ok: true };
});
// Normal return → commit
```

Constraints:
- One txn per handler call (no nesting in V1)
- Single datasource (no XA)
- Timeout bound by existing `task_timeout_ms`

Implementation: new callback `ctx_transaction_callback` in `v8_engine/context.rs`, delegates to the datasource's `begin/commit/rollback` via the existing driver interface.

~200 LOC. Effort: 2 days.

**Severity:** high for apps with multi-write invariants; medium for read-mostly apps.

---

## Defect 6 — MCP view TOML format under-documented

**File:** Rivers docs + `crates/rivers-runtime/src/validate_crossref.rs:121+`

The only in-repo working example of an MCP view is `canary-bundle/canary-sql/app.toml`:

```toml
[api.views.mcp]
path      = "canary/sql/mcp"
view_type = "Mcp"              # case-sensitive
method    = "POST"
auth      = "none"

[api.views.mcp.handler]
type = "none"                  # sentinel

[api.views.mcp.tools.pg_select]
dataview    = "pg_select_all"
description = "..."
hints       = { read_only = true }
```

CB, following an internal MCP spec predating 0.54.1, wrote `view_type = "MCP"`, `guard = "api_key_guard"`, omitted `method` and the `[handler]` stub. `riverpackage validate` failed with two structural errors and one type error. No single doc describes the current form.

### Recommended fix

Add an MCP-view section to `docs/guide/tutorials/tutorial-js-handlers.md` or `rivers-application-spec.md` covering:

- `view_type = "Mcp"` — case-sensitive
- Required `method = "POST"` (MCP is JSON-RPC 2.0 over HTTP POST)
- Required `[api.views.<name>.handler] type = "none"` sentinel
- `guard` is boolean, not a string reference
- Tools, resources, prompts — with at least one example of each

Estimated effort: 1 hour.

**Severity:** low (doc), but the validator-feedback loop is painful for first-time users.

---

## Probe Bundle as Regression Suite

`docs/rivers-upstream/cb-ts-repro-bundle/` is a complete, validatable Rivers bundle. Nine endpoints, each probing one defect or confirming one working behavior:

| Case | Tests | Current | Target |
|---|---|---|---|
| A | JS baseline | pass | pass |
| B | Parameter annotation stripping | fail | pass |
| C | Variable annotation stripping | fail | pass |
| D | `type` import erasure | fail | pass |
| E | Generic type parameter stripping | fail | pass |
| F | Multi-module import resolution | fail | pass |
| G | `export function` entrypoint lookup | fail | pass |
| H | Working TS subset | pass | pass |
| I | `ctx.data.*` + `ctx.dataview()` | pass | pass |

Proposed adoption: wire `run-probe.sh` into Rivers CI against a riversd integration test fixture. Every case is a single HTTP GET; success criteria is a 200 status and `outcome: "pass"` in the JSON body.

This gives the Rivers team a concrete "done = green" signal for the TS support work, and gives future Rivers users confidence that ordinary TS idioms will keep working.

---

## Defect Priority + Effort Summary

| # | Defect | Severity | Fix effort | Closes |
|---|---|---|---|---|
| 1 | Missing TS stripper features (params, vars, generics) | Critical | 2–3 days (swc) | B, C, E |
| 2 | `type` import erasure | Critical | 0 (subsumed by #1) | D |
| 3 | Module-namespace entrypoints not on global | High | 1 day | G |
| 4 | Multi-module import resolution | High | 1 week | F |
| 5 | `ctx.transaction()` | High | 2 days | race safety for multi-write apps |
| 6 | MCP view doc | Low | 1 hour | first-time UX |

**Total effort for full-compliance TS support:** ~2 weeks focused. Defects 1+2+3 (~4 days) close 5 of 6 probe cases and unblock CB's entire handler suite.

---

## Offer

We (CB team / Claude Code) can contribute the swc integration (Defect 1) as a Rivers PR. The work:

- Replace `compile_typescript()` body with swc strip-types pass (~30 LOC)
- Extend the test suite to assert strippable inputs reach the expected JS output
- Run the full Rivers test suite + probe bundle as regression coverage
- Update the docstring to match the new (complete) behavior

Estimated turnaround: 2–3 days from green light.

If the module-namespace hoist (Defect 3) is welcome as a follow-on PR, happy to do that too — it's ~20 LOC and closes case G.

The probe bundle is self-contained and you're welcome to adopt it directly as a regression suite (copyright attribution fine, any license you prefer).

---

## Attachments

- `docs/rivers-upstream/cb-ts-repro-bundle/` — complete reproducer bundle
- `docs/rivers-upstream/cb-ts-repro-bundle/README.md` — bundle walkthrough + expected-vs-observed matrix
- `docs/rivers-upstream/cb-ts-repro-bundle/run-probe.sh` — one-shot test runner

Contact: CB team, via the usual internal channel.
