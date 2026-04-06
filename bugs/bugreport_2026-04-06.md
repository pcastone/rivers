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
Pending — need confirmation from IPAM team on their exact `resources.toml` config.

## Occurrence Log
| Date | Context | Notes |
|------|---------|-------|
| 2026-04-06 | IPAM team deploy from v0.53.0 source | Third SQLite-related issue in this sprint |
