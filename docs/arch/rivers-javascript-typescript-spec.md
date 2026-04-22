# Rivers — JavaScript & TypeScript Pipeline Specification

**Version:** 1.0
**Date:** 2026-04-21
**Status:** Draft
**Supersedes:** processpool-runtime-spec-v2 §5.3 (TypeScript Compilation)
**Cross-references:** processpool-runtime-spec-v2, rivers-application-spec §13, rivers-app-development

---

## 1. Purpose

This specification defines the JavaScript and TypeScript compilation pipeline, module resolution system, and related handler API additions for Rivers v1. It replaces the incomplete TypeScript compilation described in processpool-runtime-spec-v2 §5.3 with an industry-grade implementation based on the `swc` compiler toolchain.

### 1.1 Origin

Rivers 0.54.1 shipped a hand-rolled TypeScript stripper (`v8_config.rs:120-226`) that implements two of five documented stripping features. The defect report `rivers-ts-pipeline-findings.md` catalogs six defects — four critical/high — with nine reproducible test cases. Ordinary TypeScript idioms (typed parameters, type-only imports, generics, ES module exports) fail at dispatch with V8 syntax errors.

The existing specifications (feature-inventory §9.1, processpool-runtime-spec-v2 §5.3, shaping task 24.11) already mandate `swc`. This specification formalizes the full design.

### 1.2 Scope

| In scope | Out of scope |
|----------|-------------|
| swc compiler integration | JSX/TSX support (deferred) |
| Full TypeScript transform | `node_modules` resolution |
| ES module resolution within `libraries/` | npm package support |
| Source map generation and consumption | Bundler integration (esbuild) |
| Module namespace entrypoint lookup | WASM module imports |
| `ctx.transaction()` handler API | Cross-app code sharing |
| MCP view TOML documentation | Nested transactions / XA |

---

## 2. TypeScript Compilation

### 2.1 Compiler

Rivers MUST use `swc_core` for all TypeScript-to-JavaScript compilation. The hand-rolled `strip_type_annotations()` and `compile_typescript()` functions in `v8_config.rs` MUST be deleted and replaced with a single swc-based compilation function.

```toml
# crates/riversd/Cargo.toml — pinned at the crates.io-current major.
# swc_core uses major-per-release versioning; any reasonably recent
# major works provided the feature flags below resolve. v64 is what
# the reference implementation pins against.
swc_core = { version = "64", features = [
    "ecma_ast",
    "ecma_parser",
    "ecma_parser_typescript",
    "ecma_transforms_typescript",
    "ecma_codegen",
    "ecma_visit",
    "common",
    "common_sourcemap",
] }
# Direct source-map serialization (matches swc_core's transitive).
swc_sourcemap = "10"
```

### 2.2 Transform Mode

Rivers MUST use **full transform**, not strip-only. The swc `typescript::typescript()` transform pass performs:

- Type annotation erasure (parameters, variables, return types, generics)
- `type`-only import erasure
- `as` type assertions removal
- `satisfies` operator removal
- `interface` and `type` alias removal
- `enum` lowering to IIFE-wrapped objects
- `const enum` inlining at call sites
- `namespace` merging into nested objects
- `const` assertions processing

Rivers MUST NOT use the strip-only mode (`typescript::strip`). Strip-only leaves enums and namespaces in the output, producing V8 syntax errors.

TC39 Stage 3 decorators are **parsed but not lowered** by `typescript::typescript()` — decorator lowering lives in `swc_ecma_transforms_proposal::decorators`, a separate pass. See §2.3 for how decorator syntax reaches V8.

### 2.3 Decorator Support

Rivers MUST support **TC39 Stage 3 decorators only**. Legacy decorators (`experimentalDecorators`) MUST NOT be supported.

Decorator handling in Rivers is parse-and-pass-through: the swc parser accepts decorator syntax, the `typescript()` transform leaves decorators intact (it only strips type-layer TypeScript), and V8 executes them natively under its Stage 3 decorator support. The reference swc parser configuration is:

```rust
use swc_core::ecma::parser::{Syntax, TsSyntax};

let syntax = Syntax::Typescript(TsSyntax {
    decorators: true,           // accept Stage 3 decorator syntax
    ..Default::default()
});
```

No additional swc pass is required — specifically, there is no `decorator_version` option on `typescript::typescript()` in current swc_core (that API lives on the separate `swc_ecma_transforms_proposal::decorators` pass, which Rivers does NOT apply). If the pinned V8 ever drops native Stage 3 decorator support, Rivers would insert the proposal-decorators pass between `typescript()` and `fixer()` at that time.

If a handler uses legacy decorator semantics (`emitDecoratorMetadata`, `experimentalDecorators`), the swc parser accepts the syntax but V8's Stage 3 execution semantics apply. Rivers documentation MUST state that only TC39 Stage 3 decorators are supported.

### 2.4 ES Target

Rivers MUST target **ES2022** as the compilation floor. swc will lower any syntax above ES2022 to ES2022-compatible output. Syntax at or below ES2022 passes through untouched.

ES2022 includes: `class` fields, private methods, `Array.at()`, top-level `await`, `Object.hasOwn()`, RegExp `/d` flag, `Error.cause`.

This MUST match the ES feature set supported by the pinned `rusty_v8` version. If the pinned V8 supports features above ES2022, those will also work (passed through by swc), but ES2022 is the guaranteed minimum.

### 2.5 JSX

JSX and TSX MUST NOT be supported in v1. Files with `.tsx` extension MUST be rejected at bundle load with a clear error:

```
JSX/TSX is not supported in Rivers v1: {app}/{path}
```

### 2.6 Compilation Timing

TypeScript compilation MUST occur at **bundle load time**, not at request time. This is unchanged from the existing specification.

The compilation pipeline at bundle load:

1. Walk all `.ts` files in every app's `libraries/` directory
2. Compile each file through the swc full transform
3. Store `(compiled_js, source_map)` in the bundle cache, keyed by absolute path within the bundle
4. If any file fails to compile, the **entire bundle fails to load** — no partial loading

At dispatch time, the worker loads pre-compiled JavaScript from the bundle cache. There is no per-request transpilation.

### 2.7 Exhaustive Upfront Compilation

**All** `.ts` files in `libraries/` MUST be compiled at bundle load, regardless of whether they are referenced by a view configuration. This ensures:

- Syntax errors are caught at deploy time, not at first request
- The module cache is fully populated before any request arrives
- No cold-start compilation latency on first request to a particular handler

A `.ts` file that compiles successfully but is never imported by any handler is harmless — the compiled output sits in the cache unused.

---

## 3. Module Resolution

### 3.1 Resolution Algorithm

Rivers MUST implement **Deno-style explicit extension resolution**. All import specifiers MUST include the file extension:

```typescript
// VALID — explicit extension
import { validateOrder } from "./shared/models.ts";
import { formatResponse } from "../utils/format.js";

// INVALID — missing extension
import { validateOrder } from "./shared/models";    // rejected
import { formatResponse } from "../utils/format";   // rejected
```

Rivers MUST NOT implement Node-style extension inference. There is no probing for `.ts`, `.js`, `/index.ts`, or `/index.js`. The specifier is resolved exactly as written.

If an import specifier lacks an extension, the module resolver MUST reject it with a clear error:

```
module resolution failed: import specifier "./shared/models" has no extension
  in {app}/libraries/handlers/orders.ts
  hint: use "./shared/models.ts" or "./shared/models.js"
```

### 3.2 Resolution Scope

Import paths MUST resolve within the same app's `libraries/` directory. This is unchanged from rivers-application-spec §13.2.

The resolver MUST enforce a **chroot-like boundary** on the app's directory root:

- Relative imports are resolved against the referrer's directory
- The resolved absolute path MUST be within `{app}/libraries/`
- Path traversal beyond the app root (`../../other-app/`) MUST be rejected
- Bare specifiers (`import x from "lodash"`) MUST be rejected — no `node_modules` resolution
- Absolute paths (`/etc/passwd`) MUST be rejected

Rejection error:

```
module resolution failed: "{specifier}" resolves outside app boundary
  in {app}/libraries/handlers/orders.ts
  resolved to: {resolved_path}
  boundary: {app}/libraries/
```

### 3.3 TypeScript Imports

When the resolver loads a `.ts` file, it MUST compile it through the same swc pipeline (§2) before passing the compiled JavaScript to V8. In practice, the compiled output is already in the bundle cache (§2.6), so the resolver performs a cache lookup, not a live compilation.

`.js` files are loaded verbatim with no compilation step.

### 3.4 Module Cache

The module cache is keyed by absolute path within the bundle. Each entry stores:

```rust
struct CompiledModule {
    /// The original TypeScript source (for error reporting)
    source_path: String,
    /// swc-compiled JavaScript
    compiled_js: String,
    /// Source map (JSON string) for stack trace remapping
    source_map: String,
}
```

The cache is populated exhaustively at bundle load (§2.7) and is immutable for the lifetime of the loaded bundle. Hot reload replaces the entire cache atomically.

### 3.5 Circular Import Detection

Rivers MUST detect circular imports at bundle load time and reject the bundle. Detection occurs during the exhaustive compilation walk (§2.7):

1. As each module is compiled, its import statements are extracted
2. A dependency graph is built across all modules in each app
3. If a cycle is detected, the bundle fails to load with:

```
circular import detected in {app}:
  libraries/handlers/a.ts
    → libraries/shared/b.ts
    → libraries/helpers/c.ts
    → libraries/handlers/a.ts
```

Rivers MUST NOT allow V8's native circular module behavior. The structural enforcement catches subtle `undefined` binding bugs at deploy time rather than at runtime.

Cycle detection operates per-app. Cross-app imports are already prohibited (§3.2), so cross-app cycles are structurally impossible.

### 3.6 V8 Module Resolve Callback

The V8 module resolver callback implements §3.1 through §3.5:

```rust
fn resolve_module_callback(
    context: v8::Local<v8::Context>,
    specifier: v8::Local<v8::String>,
    _import_attributes: v8::Local<v8::FixedArray>,
    referrer: v8::Local<v8::Module>,
) -> Option<v8::Local<v8::Module>> {
    let spec = specifier.to_rust_string_lossy(scope);

    // 1. Reject bare specifiers
    if !spec.starts_with("./") && !spec.starts_with("../") {
        // error: bare specifier not supported
        return None;
    }

    // 2. Reject missing extension
    if !has_known_extension(&spec) {
        // error: explicit extension required
        return None;
    }

    // 3. Resolve relative to referrer
    let resolved = resolve_relative(&spec, &referrer_path);

    // 4. Enforce app boundary
    if !is_within_boundary(&resolved, &app_libraries_root) {
        // error: resolves outside app boundary
        return None;
    }

    // 5. Lookup in bundle cache (already compiled at load time)
    let compiled = bundle_cache.get(&resolved)?;

    // 6. Compile V8 module from cached JS
    let v8_module = compile_v8_module(context, &compiled.compiled_js, &resolved);
    Some(v8_module)
}
```

---

## 4. Entrypoint Lookup

### 4.1 Module Namespace Lookup

Rivers MUST look up the handler entrypoint function on the **module namespace object**, not on `globalThis`. This replaces the current `call_entrypoint` implementation which only checks global scope.

The dispatch flow:

1. Determine if source is module or classic script (presence of `import`/`export` keywords)
2. **Module path:** evaluate as ES module → look up entrypoint on module namespace
3. **Classic script path:** evaluate as script → look up entrypoint on `globalThis`

```rust
let entrypoint_fn = if is_module {
    // Module: look up on namespace
    let ns = module.get_module_namespace();
    let ns_obj = v8::Local::<v8::Object>::try_from(ns)?;
    let key = v8::String::new(scope, &entrypoint_name)?;
    ns_obj.get(scope, key.into())
} else {
    // Classic script: look up on globalThis
    let global = context.global(scope);
    let key = v8::String::new(scope, &entrypoint_name)?;
    global.get(scope, key.into())
};
```

### 4.2 No Global Hoisting

Rivers MUST NOT hoist module exports onto `globalThis`. Only the declared entrypoint (from `app.toml` handler config) is accessed from the module namespace. All other exports remain on the namespace and are not visible to the Rivers runtime.

This means:

- `export function handler(ctx)` — reachable as entrypoint if declared in `app.toml`
- `export function helperFn()` — importable by other modules, not visible to Rivers runtime
- Side effects at module top level execute during evaluation but do not register on global scope

### 4.3 Backward Compatibility

Classic script handlers (plain JavaScript with no `import`/`export`) continue to work exactly as before — function lookup on `globalThis`. The module namespace path only activates when the source contains ES module syntax.

The `globalThis.handler = handler` workaround documented in the current source code is no longer necessary and SHOULD be removed from documentation.

---

## 5. Source Maps

### 5.1 Generation

swc MUST generate source maps as a side product of every TypeScript compilation. Source maps are stored alongside compiled JavaScript in the bundle cache (§3.4).

Source map generation is not optional — it is always on. The overhead is negligible at bundle load time and the maps are only consulted on error paths.

### 5.2 Stack Trace Remapping

Rivers MUST register a V8 `SetPrepareStackTraceCallback` that intercepts `Error.stack` construction. The callback:

1. Receives structured `v8::StackFrame` objects (script name, line, column)
2. Looks up the source map for the script name in the bundle cache
3. Remaps line and column numbers to original TypeScript positions
4. Returns the corrected stack trace string

```rust
fn prepare_stack_trace_callback(
    scope: &mut v8::HandleScope,
    error: v8::Local<v8::Value>,
    callsites: v8::Local<v8::Array>,
) -> v8::Local<v8::Value> {
    // For each callsite:
    //   1. Get script name, line, column
    //   2. Look up source map in bundle cache
    //   3. Map to original position
    //   4. Format as "{original_file}:{original_line}:{original_col}"
}
```

### 5.3 Error Reporting

When a handler throws an uncaught exception, the error reported to the client (in non-debug mode) MUST NOT include stack traces. The remapped stack trace is written to the **Rivers structured log** at `error` level, correlated with the request trace ID.

In debug mode (`[base] debug = true` in the app's `app.toml`), the remapped stack trace MAY be included in the error response envelope. The envelope matches Rivers' standard `ErrorResponse` shape (spec §18 / SHAPE-2): `{code, message, trace_id, details?}`. The stack appears under `details.stack` as an array of pre-formatted frame strings.

```json
{
  "code": 500,
  "message": "handler error: handler 'createOrder' threw: TypeError: Cannot read property 'name' of undefined",
  "trace_id": "abc-123",
  "details": {
    "stack": [
      "at processOrder (libraries/handlers/orders.ts:47:12)",
      "at handler (libraries/handlers/orders.ts:12:5)"
    ]
  }
}
```

Non-debug responses omit the `details` key entirely — the envelope reduces to `{code, message, trace_id}`.

### 5.4 Resolves

This section closes processpool-runtime-spec-v2 Open Question #5 ("TypeScript source maps").

---

## 6. Handler Transaction API

### 6.1 Surface

Rivers MUST expose `ctx.transaction()` on the handler context object:

```javascript
var result = ctx.transaction("datasource_name", function() {
    ctx.dataview("insert_parent", { id: "P" });
    ctx.dataview("insert_child",  { parent: "P" });
    return { ok: true };
});
```

**Behavior:**

- Normal return from the callback → **commit**
- Exception thrown inside the callback → **rollback**, exception re-thrown to handler
- The callback receives no arguments — `ctx.dataview()` calls inside the callback are implicitly scoped to the open transaction

### 6.2 Constraints

| Constraint | Rule |
|-----------|------|
| Nesting | Prohibited. Calling `ctx.transaction()` inside a transaction callback MUST throw `TransactionError: nested transactions not supported` |
| Datasource scope | Single datasource per transaction. The `datasource_name` argument identifies which connection holds the transaction |
| Cross-datasource calls | Any `ctx.dataview()` call inside the transaction block that routes to a **different** datasource than the named one MUST throw `TransactionError: dataview "{name}" uses datasource "{ds}" which differs from transaction datasource "{txn_ds}"` |
| Timeout | Bound by the existing `task_timeout_ms`. No separate transaction timeout. If the handler times out, the transaction is rolled back as part of worker cleanup |
| Driver support | `ctx.transaction()` MUST throw `TransactionError: datasource "{name}" does not support transactions` if the driver's `supports_transactions()` returns `false` |

### 6.3 Implementation

New host function `ctx_transaction_callback` in `v8_engine/context.rs`:

1. Resolve `datasource_name` to the driver connection
2. Call `driver.begin_transaction()` on the connection
3. Set a thread-local flag indicating the active transaction and its datasource
4. Invoke the JavaScript callback
5. On normal return: call `driver.commit_transaction()`
6. On exception: call `driver.rollback_transaction()`, re-throw
7. Clear the thread-local transaction flag

The thread-local flag is checked by `ctx.dataview()` dispatch:

- If no transaction active: execute normally (auto-commit per call)
- If transaction active: verify the dataview's backing datasource matches the transaction datasource. If mismatch → throw. If match → execute within the open transaction

### 6.4 Drivers with Transaction Support

Runtime enforcement is authoritative: a driver's `supports_transactions()` return (or the `Unsupported` error from its default `begin_transaction` impl) determines whether `ctx.transaction()` accepts the datasource. The table below is informational — built-in driver rows are verified in `crates/rivers-drivers-builtin/`; plugin rows should be verified by inspecting each plugin crate or by invoking `ctx.transaction()` at plugin load time.

| Driver | `supports_transactions()` | Source of truth | Notes |
|--------|--------------------------|-----------------|-------|
| PostgreSQL | `true` | `crates/rivers-drivers-builtin/src/postgres.rs` | `BEGIN` / `COMMIT` / `ROLLBACK` |
| MySQL | `true` | `crates/rivers-drivers-builtin/src/mysql.rs` | `START TRANSACTION` / `COMMIT` / `ROLLBACK` |
| SQLite | `true` | `crates/rivers-drivers-builtin/src/sqlite.rs` | `BEGIN IMMEDIATE` / `COMMIT` / `ROLLBACK` |
| Faker / EventBus / Memcached | `false` | `crates/rivers-drivers-builtin/` | No transaction semantics |
| Redis | `false` | `crates/rivers-drivers-builtin/src/redis/driver.rs` | `MULTI/EXEC` is pipelining, not ACID transactions |
| MongoDB | plugin — verify at plugin load | `crates/rivers-plugin-mongodb` | Client session transactions (would require replica set) |
| CouchDB | plugin — verify at plugin load | `crates/rivers-plugin-couchdb` | No native transaction support expected |
| Elasticsearch | plugin — verify at plugin load | `crates/rivers-plugin-elasticsearch` | No transaction support expected |
| Cassandra | plugin — verify at plugin load | `crates/rivers-plugin-cassandra` | LWTs are not general transactions |
| LDAP | plugin — verify at plugin load | `crates/rivers-plugin-ldap` | No transaction support expected |
| Kafka | plugin — verify at plugin load | `crates/rivers-plugin-kafka` | Producer transactions are a different model |

---

## 7. MCP View TOML Format

### 7.1 Required Structure

An MCP view in `app.toml` MUST follow this exact structure:

```toml
[api.views.mcp_endpoint]
path      = "/app/path/mcp"
view_type = "Mcp"                    # Case-sensitive: "Mcp", not "MCP" or "mcp"
method    = "POST"                   # Required: MCP is JSON-RPC 2.0 over HTTP POST
auth      = "none"                   # Or a guard reference

[api.views.mcp_endpoint.handler]
type = "none"                        # Required sentinel — MCP dispatch is internal

# Tools — expose DataViews as MCP tools
[api.views.mcp_endpoint.tools.tool_name]
dataview    = "dataview_reference"
description = "Human-readable description for the AI model"
hints       = { read_only = true }   # Optional MCP tool hints

# Resources — expose DataViews as MCP resources (optional)
[api.views.mcp_endpoint.resources.resource_name]
dataview    = "dataview_reference"
description = "Human-readable description"
uri         = "resource://app/resource_name"

# Prompts — expose markdown templates as MCP prompts (optional)
[api.views.mcp_endpoint.prompts.prompt_name]
template    = "libraries/prompts/prompt_name.md"
description = "Human-readable description"
```

### 7.2 Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `invalid view_type` | Used `"MCP"` or `"mcp"` | Use `"Mcp"` (capital M, lowercase cp) |
| `missing method` | Omitted `method` field | Add `method = "POST"` |
| `missing handler` | Omitted `[handler]` section | Add `[handler] type = "none"` |
| `invalid guard type` | Used `guard = "guard_name"` (string) | `guard` is boolean — use auth patterns from view layer spec |

### 7.3 Guard Configuration

MCP endpoints use the same auth/guard configuration as any other view. The `auth` field accepts the standard values: `"none"`, `"session"`, or a guard reference per the view layer specification.

---

## 8. `rivers.d.ts` — API Type Definitions

### 8.1 Purpose

Rivers MUST ship a `rivers.d.ts` file that declares the complete TypeScript type surface available inside handlers. This enables IDE autocomplete, type checking, and inline documentation for handler authors.

### 8.2 Distribution

The file lives at `types/rivers.d.ts` in the Rivers repository and is included in every release artifact (binary distribution, container image, documentation site).

Handler authors reference it in their `tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "strict": true,
    "types": ["./types/rivers"]
  }
}
```

### 8.3 Contents

The `.d.ts` file MUST declare:

- `Rivers` global object (`Rivers.log`, `Rivers.crypto`, `Rivers.keystore`, `Rivers.env`) — every namespace that the V8 host layer actually injects. See `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` for the authoritative injection surface. Spec-listed namespaces that are NOT injected today (`Rivers.db`, `Rivers.view`, `Rivers.http`) MUST NOT be declared — typed stubs for surfaces that don't exist would let the type checker accept calls that fail at runtime. If a future release adds these namespaces at the host layer, this spec section and `rivers.d.ts` update together.
- `ViewContext` interface (the type of the `ctx` argument passed to every handler)
- `ParsedRequest` interface
- `QueryResult` and `ExecuteResult` interfaces
- `ctx.transaction()` signature and `TransactionError`
- `ctx.dataview()` signature
- `ctx.datasource()` signature
- `ctx.store` (StorageEngine) interface
- All capability-gated surfaces with JSDoc comments indicating when each is available

The file MUST NOT declare `console`, `process`, `require`, `fetch` (unless `allow_outbound_http`), or any other global that Rivers does not inject. This ensures the type checker catches calls to unavailable APIs at development time.

---

## 9. Canary Fleet Integration

### 9.1 Probe Bundle Adoption

The probe bundle from `rivers-ts-pipeline-findings.md` (cases A through I) MUST be adopted as a regression suite in the Rivers CI pipeline. Each case is a single HTTP endpoint with a pass/fail JSON verdict.

### 9.2 Canary Extension

The `canary-handlers` app in the Canary Fleet SHOULD be extended with TypeScript-specific test cases that exercise:

| Test ID | Tests | Probe case |
|---------|-------|------------|
| RT-TS-PARAM-STRIP | Parameter type annotation stripping | B |
| RT-TS-VAR-STRIP | Variable type annotation stripping | C |
| RT-TS-IMPORT-TYPE | `type`-only import erasure | D |
| RT-TS-GENERIC | Generic type parameter stripping | E |
| RT-TS-MULTIMOD | Multi-module import resolution | F |
| RT-TS-EXPORT-FN | `export function` entrypoint lookup | G |
| RT-TS-ENUM | `enum` lowering | new |
| RT-TS-DECORATOR | TC39 decorator lowering | new |
| RT-TS-NAMESPACE | `namespace` merging | new |
| RT-TS-CIRCULAR | Circular import rejection at bundle load | new |
| RT-TS-SOURCEMAP | Source map stack trace remapping | new |
| RT-TXN-COMMIT | `ctx.transaction()` commit on return | new |
| RT-TXN-ROLLBACK | `ctx.transaction()` rollback on throw | new |
| RT-TXN-CROSS-DS | Cross-datasource error inside transaction | new |
| RT-TXN-NESTED | Nested transaction error | new |
| RT-TXN-UNSUPPORTED | Transaction on non-transactional driver error | new |

---

## 10. Implementation Checklist

Ordered by dependency. Each item lists the defect(s) it closes from `rivers-ts-pipeline-findings.md`.

| # | Work Item | Effort | Closes |
|---|-----------|--------|--------|
| 1 | Replace `compile_typescript()` with swc full transform | 2–3 days | Defect 1 (param, var, generic stripping), Defect 2 (type import erasure) |
| 2 | Source map generation + `PrepareStackTraceCallback` | 1–2 days | Open Question #5 |
| 3 | Module resolve callback with boundary enforcement | 3–4 days | Defect 4 (multi-module resolution) |
| 4 | Circular import detection at bundle load | 1 day | — |
| 5 | Module namespace entrypoint lookup | 1 day | Defect 3 (export function entrypoint) |
| 6 | `ctx.transaction()` host function | 2 days | Defect 5 (handler transactions) |
| 7 | MCP view documentation | 1 hour | Defect 6 (MCP TOML format) |
| 8 | `rivers.d.ts` type definitions | 1 day | — |
| 9 | Canary Fleet TS test cases | 2 days | — |
| 10 | Delete hand-rolled stripper, update docstrings | 0.5 day | — |

**Total estimated effort:** ~2.5 weeks focused.

**Critical path:** Items 1 → 3 → 5 unblock all handler development. Items 2 and 4 can proceed in parallel after item 1.

---

## 11. Decision Log

All decisions locked during specification development.

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| 1 | Full transform, not strip-only | swc `typescript::strip` | Strip-only leaves enums, decorators, namespaces as V8 syntax errors. First user to write `enum Status {}` files the same bug report |
| 2 | Deno-style explicit extensions | Node-style extension inference | No filesystem probing, deterministic resolution, fits Rivers' explicit-over-implicit philosophy |
| 3 | Exhaustive upfront compilation | Lazy per-request compilation | Compile errors at deploy time, not first request. Full cache population eliminates cold-start latency |
| 4 | ES2022 target floor | ESNext, ES2020 | Safe floor matching V8 capabilities. Modern enough for class fields, top-level await. swc passes through anything V8 handles natively |
| 5 | TC39 decorators only | Legacy `experimentalDecorators`, configurable per-bundle | Legacy is a dead-end standard. One decorator flavor eliminates configuration surface |
| 6 | JSX deferred | Include (trivial to enable) | Handlers return data, not markup. No compelling v1 use case. Trivial to add later |
| 7 | Circular imports rejected at bundle load | Allow (V8 native behavior) | Structural enforcement over subtle runtime `undefined` bugs. Deploy-time failure with clear error |
| 8 | Import depth cap skipped | Fixed cap (e.g., 32) | `libraries/` boundary already constrains tree depth. Artificial cap adds a knob without value |
| 9 | Module namespace lookup, no global hoist | Hoist entrypoint only, hoist all exports | Deno model. Cleanest separation — namespace for modules, globalThis for classic scripts. No leak of helper exports onto global |
| 10 | `ctx.transaction()` uses ambient ctx | Explicit `txn` argument | Simpler API — same `ctx.dataview()` calls, implicitly scoped. No new object to learn |
| 11 | No nesting, single datasource, `task_timeout_ms` bound | Savepoints, XA | v1 simplicity. Covers the dominant case (multiple writes to one database). Nesting and XA are v2 if needed |
| 12 | Cross-datasource calls inside transaction throw | Pass-through (non-transactional) | Rivers philosophy: fail loud. Don't let the developer think they have a guarantee they don't |
| 13 | MCP view documentation included | Separate doc effort | Small scope, high impact on first-time UX. Fits naturally in this spec |
