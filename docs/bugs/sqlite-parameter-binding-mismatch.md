# Bug Report: SQLite Driver Parameter Binding Mismatch

**Component:** `rivers-drivers-builtin::sqlite` (`crates/rivers-drivers-builtin/src/sqlite.rs`)  
**Severity:** Blocks all parameterized SQLite queries  
**Rivers Version:** 0.52.5  
**Reported:** 2026-04-02

---

## Summary

The SQLite driver's `bind_params()` function always binds parameters with the `:` prefix, but the SQL query text retains `$`-prefixed parameter placeholders from the DataView TOML. This causes rusqlite to fail with `Invalid parameter name: :id` because the prepared statement expects `$id` but receives `:id` in the binding map.

## Spec vs Implementation

### What the specs say

**`rivers-data-layer-spec.md` §8.3:**
> Named parameters (`:name`, `@name`, `$name`) or auto-prefixed.

**`rivers-driver-schema-validation-spec.md` §8.3:**
> Parameters should use `:name`, `@name`, or `$name` style

**`rivers-schema-spec-v2.md`, `rivers-technology-path-spec.md` (all examples):**
```toml
query = "SELECT * FROM orders WHERE status = $status ORDER BY created_at DESC LIMIT $limit"
query = "SELECT * FROM orders WHERE id = $id"
query = "UPDATE orders SET amount = $amount, status = $status WHERE id = $id RETURNING *"
```

All spec examples and the app-keystore tutorial use `$param` syntax in DataView queries.

### What the driver does

**`sqlite.rs` lines 163-190 (`bind_params`):**
```rust
fn bind_params(parameters: &HashMap<String, QueryValue>) -> Vec<(String, Box<dyn rusqlite::types::ToSql>)> {
    parameters
        .iter()
        .map(|(key, val)| {
            let name = if key.starts_with(':') || key.starts_with('@') || key.starts_with('$') {
                key.clone()
            } else {
                format!(":{}", key)        // <-- always defaults to ':'
            };
            // ...
        })
}
```

**`dataview_engine.rs` lines 455-465 (`build_query`):**
```rust
pub fn build_query(config: &DataViewConfig, params: &HashMap<String, QueryValue>, method: &str) -> Query {
    let statement = config.query_for_method(method).unwrap_or_default();
    let mut query = Query::new(&config.datasource, statement);
    for (k, v) in params {
        query.parameters.insert(k.clone(), v.clone());  // keys are bare: "id", "name"
    }
    query
}
```

The DataView engine passes parameter keys **without any prefix** (bare names like `id`, `limit`). The `bind_params` function then adds `:` prefix, producing `:id`, `:limit`.

But the SQL query text still contains `$id`, `$limit` as written in the TOML.

## The Mismatch

| Layer | Parameter name | Example |
|-------|---------------|---------|
| TOML query text | `$id` | `WHERE id = $id` |
| DataView engine (Query.parameters) | `id` | bare key, no prefix |
| `bind_params()` output | `:id` | always adds `:` prefix |
| rusqlite prepared statement | `$id` | parsed from query text |

rusqlite's named parameter binding requires the binding name to **exactly match** the placeholder in the SQL. When the query has `$id`, the binding must be `$id`, not `:id`.

## Reproduction

```toml
# app.toml
[data.dataviews.get_subnet]
name       = "get_subnet"
datasource = "inventory_db"
query      = "SELECT * FROM subnets WHERE id = $id"

[[data.dataviews.get_subnet.parameters]]
name = "id"
type = "string"
required = true
```

```
GET /api/subnets/abc123
→ Error: sqlite execute: Invalid parameter name: :id
```

## Root Cause

`bind_params()` at line 172 defaults to `:` prefix for bare parameter names. But `$` is the dominant prefix in all spec examples and the TOML convention. The function should either:

1. **Match the prefix used in the query** — scan the SQL for `$name`, `:name`, or `@name` and use the matching prefix
2. **Default to `$` instead of `:`** — since all spec examples use `$param`
3. **Normalize the query** — rewrite `$param` → `:param` in the SQL before preparing

## Suggested Fix

**Option A (minimal, recommended):** Change the default prefix in `bind_params()` from `:` to `$`:

```rust
// line 172: change format!(":{}", key) to:
format!("${}", key)
```

**Option B (robust):** Scan the query text to detect which prefix style is used, then match it:

```rust
fn detect_param_prefix(statement: &str) -> &'static str {
    if statement.contains('$') { "$" }
    else if statement.contains('@') { "@" }
    else { ":" }
}
```

**Option C (normalize):** Rewrite the query before preparing to use a consistent prefix (e.g., replace `$(\w+)` with `:$1`). This adds complexity but makes all three styles interchangeable.

## Additional Issues Found During Investigation

### 1. ViewContext.app_id not populated (view_dispatch.rs:172)

```rust
let mut view_ctx = view_engine::ViewContext::new(
    parsed,
    trace_id.clone(),
    String::new(), // app_id — populated after bundle deployment  ← ALWAYS EMPTY
    String::new(), // node_id
    "dev".to_string(),
);
```

The `app_entry_point` is available on line 143 but never passed to ViewContext. This means `ctx.app_id` is always empty in handlers, and DataView/datasource auto-namespacing doesn't work.

**Fix:** Replace `String::new()` with `app_entry_point.clone()` on line 172.

### 2. SQLite datasource path uses `database` field, not nested config

The SQLite driver reads `params.database` (line 60), but common TOML convention puts the path under `[data.datasources.*.config]`. The `database` field must be a top-level datasource attribute:

```toml
# WRONG — driver ignores this, uses in-memory
[data.datasources.inventory_db.config]
path = "data/inventory.db"

# CORRECT — driver reads params.database
[data.datasources.inventory_db]
database = "data/inventory.db"
```

This should be documented more clearly or the driver should fall back to `options["path"]`.

### 3. init_handler datasource_configs not populated

The `execute_init_handler()` in `deployment.rs` (line 502-552) doesn't populate `datasource_configs` on the TaskContextBuilder. This means `ctx.datasource()` is unavailable in init handlers, even though the spec says init handlers should have access to datasources via `app.dataview()`.

---

**Files affected:**
- `crates/rivers-drivers-builtin/src/sqlite.rs` (bind_params default prefix)
- `crates/riversd/src/server/view_dispatch.rs` (app_id not set)
- `crates/riversd/src/deployment.rs` (init_handler missing datasource_configs)
