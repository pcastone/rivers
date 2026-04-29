# Rivers v0.55.0 — `Rivers.db.query` / `Rivers.db.execute` documented but not implemented

**Filed:** 2026-04-26
**Reporter:** CB team
**Rivers version:** 0.55.0+1219260426
**Severity:** Spec-vs-implementation gap. Spec advertises a capability the runtime lacks.

---

## TL;DR

Spec §5.2 of `rivers-processpool-runtime-spec-v2.md` documents `Rivers.db.query(token, sql, params)` and `Rivers.db.execute(token, sql, params)` as part of the V8 isolate's API surface. The implementation in `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` only installs `Rivers.db.{begin,commit,rollback,batch}`. The two query primitives are missing.

Either ship the missing methods or remove them from the spec.

---

## Spec contract (the contract)

`docs/arch/rivers-processpool-runtime-spec-v2.md` §5.2, lines 281–296:

```typescript
declare const Rivers: {
    // Present if datasource declared in view
    db: {
        query(token: string, sql: string, params?: any[]): Promise<QueryResult>;
        execute(token: string, sql: string, params?: any[]): Promise<ExecuteResult>;
    };
    // Present if dataview declared in view
    view: {
        query(token: string, params?: Record<string, any>): Promise<any>;
    };
    ...
};
```

Section §4.2 (line 210) reinforces:

> When the handler calls `Rivers.db.query(token, sql, params)`, the host receives the token, resolves it to the actual connection from the host-side connection pool, executes the query, and returns the result to the isolate.

This is positioned as a first-class API for handler authors who need raw SQL beyond what DataViews can declaratively express.

---

## Implementation reality (the gap)

`crates/riversd/src/process_pool/v8_engine/rivers_global.rs`:

```rust
// Lines 706–724 (only methods installed on Rivers.db):
let db_begin_fn      = v8::Function::new(scope, db_begin_callback)?;
let db_commit_fn     = v8::Function::new(scope, db_commit_callback)?;
let db_rollback_fn   = v8::Function::new(scope, db_rollback_callback)?;
let db_batch_fn      = v8::Function::new(scope, db_batch_callback)?;
// No db_query_fn or db_execute_fn installed.
```

A search across the repository confirms:

```bash
$ grep -rn "db_query_callback\|db_execute_callback\|fn db_query\|fn db_execute" crates/
# (no results)
```

---

## Reproducer

Minimal handler that should work per spec but fails at runtime:

```typescript
export function probe(ctx) {
    // Per spec §5.2 — this should work.
    const result = Rivers.db.query("my_db", "SELECT 1 AS answer", []);
    ctx.resdata = result;
}
```

Observed:

```
TypeError: Rivers.db.query is not a function
```

---

## Why this matters (use case)

CB has 133 inline SQL call sites across 11 handler files (post G.1 pilot). The migration path under the current Rivers surface is:

- Declare ~70 DataViews in `app.toml` (one per distinct query shape, after dedup)
- Rewrite ~100 call sites to `(await ctx.dataview("name", params)).rows`
- Spread `[data.dataviews.<name>]` blocks across `app.toml` (~1500 LOC of TOML)

If `Rivers.db.query` ships per spec, the migration path collapses to:

- One sed rewrite from `ctx.sql("cb_db", sql, params)` → `Rivers.db.query("cb_db", sql, params)`
- Zero `app.toml` changes
- Zero query-shape consolidation work

We estimate the difference as **~2 weeks of focused engineering vs. ~1 day**. For a system that compose-reads its model (12 separate aggregations across 4 entity types in `reports.ts`, e.g.), the DataView model is a poor fit; the spec already acknowledges this by exposing `Rivers.db.query` as an escape hatch.

---

## Recommended fix

Implement `db_query_callback` and `db_execute_callback` mirroring the existing `db_batch_callback` pattern in `rivers_global.rs`. Both already share the connection-pool resolution path used by `Rivers.db.batch`; the new methods are thin variants:

```rust
// db_query_callback: args = (datasource, sql, params?)
// returns: Promise<{ rows, affected_rows, last_insert_id }>

fn db_query_callback(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);
    let sql     = args.get(1).to_rust_string_lossy(scope);
    let params  = parse_params(args.get(2));
    // Reuse the active TASK_TRANSACTION connection if held; else acquire from pool.
    // Same routing logic batch_callback already uses.
    let result = block_on(execute_query(&ds_name, &sql, params))?;
    rv.set(json_to_v8(scope, result));
}
```

Estimated effort: ~150 LOC + ~50 LOC of tests. The driver-facing query path already exists (`DatabaseDriver::query/execute` traits in `rivers-driver-sdk/src/lib.rs`); the V8 callback wrappers are the missing piece.

`Rivers.db.execute` is identical except it's for non-query writes (INSERT/UPDATE/DELETE) where you don't need rows back, only `affected_rows` and `last_insert_id`.

---

## Alternative: spec correction

If the design intent is "DataViews only, no inline SQL", strike the `query` and `execute` declarations from `rivers-processpool-runtime-spec-v2.md` §5.2 and acknowledge that explicitly. We'd accept that and migrate to DataViews — but right now reading the spec gives a wrong answer about Rivers' capabilities.

---

## Offer

We can submit the implementation as a Rivers PR if it would help. The work is mechanical (callbacks already have a clear template); we'd add regression coverage matching the existing canary-bundle SQL test patterns, and run the full test suite plus our probe bundle (`docs/rivers-upstream/cb-ts-repro-bundle/`) before submitting.

Estimated turnaround: 2–3 days from green light.

---

## References

- Spec: `docs/arch/rivers-processpool-runtime-spec-v2.md` §5.2 (lines 281–296), §4.2 (line 210)
- Implementation gap: `crates/riversd/src/process_pool/v8_engine/rivers_global.rs:706–724`
- Existing template to copy: `db_batch_callback` in same file
- Driver capability already present: `rivers-driver-sdk/src/lib.rs` — `DatabaseDriver::query` / `DatabaseDriver::execute`
