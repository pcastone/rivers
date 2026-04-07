# Bug Report — 2026-04-06

## Summary
SQLite SELECT queries return null — INSERT succeeds but reads fail, no DB file created on disk.

## Symptoms
- `ctx.dataview()` INSERT/CREATE queries execute successfully (tables created, data inserted, no errors)
- `ctx.dataview()` SELECT queries return `null` immediately after INSERT in the same handler
- No SQLite `.db` file is created on disk despite `database = "path/to/file.db"` in config
- IPAM team reports this worked in v0.52.10, broken in v0.53.0

Example handler output:
```json
{"cidr":"10.0.1.0/24","id":"3cb75e91","ip_count":254,"readback":null}
```

## Environment
Rivers v0.53.0, macOS (Darwin), SQLite datasource, V8 CodeComponent handlers, IPAM bundle.

## Root Cause (investigation — pending confirmation)

The SQLite driver (`crates/rivers-drivers-builtin/src/sqlite.rs:60`) uses `params.database` as the file path:

```rust
let path = params.database.clone();
let conn = rusqlite::Connection::open(&path) ...
```

`ConnectionParams` is built in `crates/riversd/src/bundle_loader/load.rs:243-246`:
```rust
host: ds.host.clone().unwrap_or_default(),      // → params.host
database: ds.database.clone().unwrap_or_default(), // → params.database
```

**Two possible causes:**

### Theory A: `host` vs `database` field mismatch
If the IPAM team's `resources.toml` uses `host = "path/to/inventory.db"` instead of `database = "path/to/inventory.db"`, then `params.database` will be empty. `rusqlite::Connection::open("")` opens an **in-memory database**. Each `connect()` call gets a separate in-memory DB, so INSERT in connection A is invisible to SELECT in connection B.

This would explain all three symptoms:
1. INSERT succeeds (writes to ephemeral in-memory DB)
2. SELECT returns null (reads from different in-memory DB)
3. No file on disk (no path was given to rusqlite)

### Theory B: Relative path + directory missing
If `database = "netinventory-service/data/inventory.db"` is correct but the parent directory `netinventory-service/data/` doesn't exist, rusqlite may fail silently or open in-memory mode depending on the version.

**Recommended fix (regardless of root cause):**
The SQLite driver should:
1. Check `params.database` first, fall back to `params.host` (many SQLite tools use `host` for the path)
2. Create parent directories if they don't exist
3. Log a warning if `database` is empty

## Fix Applied
Fixed in commit 9699297 (2026-04-06), with tracing in e3a2811 and test coverage in b4e83a8.

## Independent Findings

**Reviewed: 2026-04-07 — Bug is FIXED. No additional work required.**

### Code Review

Verified `crates/rivers-drivers-builtin/src/sqlite.rs:56-94`. The `connect()` method now:

1. **Falls back from `database` → `host`** (line 61-64) — resolves Theory A (host/database mismatch). If `params.database` is empty, `params.host` is used instead.
2. **Returns explicit error if both are empty** (line 66-68) — no more silent in-memory DB.
3. **Creates parent directories** (line 78-85) — resolves Theory B (missing dirs). Skips for `:memory:`.
4. **Logs the resolved path and source field** (line 71-76) — aids future debugging.

### Test Coverage

Five regression tests at `sqlite.rs:654-731` cover all failure modes:

| Test | Validates |
|------|-----------|
| `connect_uses_database_field` | Happy path — `database` field works |
| `connect_falls_back_to_host_when_database_empty` | Theory A — `host` fallback |
| `connect_errors_when_both_empty` | Error message when no path given |
| `connect_creates_parent_directories` | Theory B — nested dir creation |
| `connect_insert_then_select_across_connections` | Original bug — cross-connection persistence |

### Bundle Loader

`crates/riversd/src/bundle_loader/load.rs:243-246` is unchanged — `host` and `database` are still passed through from TOML as-is. The fix correctly lives in the driver layer, not the loader, which is the right boundary.

### Verdict

The fix addresses both theories from the root cause analysis. The IPAM team confirmation is no longer blocking — regardless of whether they used `host=` or `database=`, the driver now handles both. Recommend closing this bug.

## Occurrence Log
| Date | Context | Notes |
|------|---------|-------|
| 2026-04-06 | IPAM team deploy from v0.53.0 source | Third SQLite-related issue in this sprint |
