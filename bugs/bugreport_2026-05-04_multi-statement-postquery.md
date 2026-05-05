# Bug Report — 2026-05-04

## Summary

Multi-statement DataView queries (e.g. `post_query = "INSERT …; UPDATE …;"`)
silently execute **only the first statement**. All subsequent statements are
discarded with no error, no warning, and no validation hint. The DataView
returns 200 / success, masking the partial write.

This affects **every database driver** (SQLite, PostgreSQL, MySQL, Cassandra)
on the per-method `*_query` path used by `[[data.dataviews.*]]`. There is no
spec coverage of multi-statement support either way.

## Symptoms — observed by CB project

CB DataViews chained two writes per tool. Only the first ran:

| Tool | Statement 1 (ran) | Statement 2+ (silently dropped) |
|------|-------------------|----------------------------------|
| `cb_mark_complete` | `INSERT INTO decisions …` (D011 created) | `UPDATE wip SET status='complete' …` (WP019 stayed `in_progress`) |
| `cb_pull_task`     | `UPDATE wip SET status='active' …`        | `UPDATE projects SET active_wip_id=…` (still `NULL`) |
| `cb_complete_goal` | `INSERT INTO tasks …`                     | `DELETE FROM wip …` + `UPDATE projects SET active_work_item_goal_id=NULL …` |

All three tools returned HTTP 200 with no error. The smoke-test suite reported
green checks because the response shape was valid — the false positive was
detectable only by inspecting the database directly.

## Environment

- Rivers (workspace as of commit `36fc84e`, branch `mcp`)
- SQLite was the driver in CB's repro; the same defect exists in postgres /
  mysql / cassandra paths (different mechanism, same outcome)
- Affects every per-method query field: `get_query`, `post_query`, `put_query`,
  `delete_query` and the legacy unified `query` field

## Root Cause — per driver

All database drivers route DataView dispatch through APIs that are
**single-statement by contract**. The drivers do not split, loop over, or batch
multi-statement input — they hand the entire string to a prepared-statement API
that parses only the first statement.

| Driver | API used | Behavior on multi-statement input | Source |
|--------|----------|-----------------------------------|--------|
| SQLite | `conn.prepare(stmt)` (= `sqlite3_prepare_v2`) for SELECT/INSERT/UPDATE/DELETE | Parses first statement; tail pointer discarded | `crates/rivers-drivers-builtin/src/sqlite.rs:531,585,615` |
| PostgreSQL | `client.query(stmt, params)` / `client.execute(stmt, params)` (extended protocol) | Errors on `;` in many cases, otherwise first statement only | `crates/rivers-drivers-builtin/src/postgres.rs:188,239,262,284` |
| MySQL | `conn.exec_iter(stmt, params)` (binary protocol) | Single statement only — `multi_statements` URL flag does not apply to prepared protocol | `crates/rivers-drivers-builtin/src/mysql.rs:284,327,365` |
| Cassandra | `session.prepare(stmt)` | CQL prepared statements are single-statement by design | `crates/rivers-plugin-cassandra/src/lib.rs:118,162` |

Multi-statement-capable APIs **exist** in each driver but are reserved for
non-DataView paths:

- SQLite: `conn.execute_batch()` — used only by `ddl_execute()` and `ping`
- PostgreSQL: `client.batch_execute()` — used only for `BEGIN`/`COMMIT`/`ROLLBACK`
- MySQL: `query_iter()` (text protocol) — not used by `Connection::execute()`

So the runtime **has** the capability and **does not wire it** into the user-facing query path.

## Validation gap

`crates/rivers-runtime/src/validate_syntax.rs` walks `*_query` strings but does
not check whether they contain multiple statements. A DataView with
`post_query = "INSERT …; UPDATE …;"` passes all four validation layers
(`structural`, `existence`, `crossref`, `syntax`) and is loaded successfully.

There is no documented contract in any spec
(`docs/arch/rivers-data-layer-spec.md`, `rivers-driver-spec.md`,
`rivers-technology-path-spec.md`) stating multi-statement queries are or are
not supported. `grep -ri "multi.statement\|semicolon" docs/` returns zero hits.

## Severity

**P1.** The defect is silent — handlers and DataView consumers receive a
success response while persisted state is incomplete. This is the worst class
of database bug: it survives smoke tests, only surfaces during direct DB
inspection, and corrupts cross-table invariants.

CB has hit this on three production tools. Any other Rivers consumer who has
written a multi-statement post_query has the same hidden bug.

## Workaround (CB has adopted)

Split each multi-statement query into N single-statement DataViews; have the
caller (handler / CC) sequence the calls, ideally inside an explicit
transaction (`begin_transaction` / `commit_transaction`). This loses
atomicity unless the caller wraps the sequence in a transaction.

## Fix options

Two layers; either is sufficient as a stopgap, both together is the proper fix.

### Option A — Driver-level multi-statement support (preferred)

Per driver, route multi-statement input through a statement-splitter:

- **SQLite**: loop `sqlite3_prepare_v2` over the tail pointer; bind shared
  parameters into each statement that references them; execute each. Or fall
  back to `execute_batch` when no parameters are bound. Wrap the loop in an
  implicit transaction so partial-failure rolls back.
- **PostgreSQL**: when no params, use `simple_query()` which accepts
  multi-statement. With params, prepare/execute each statement individually.
- **MySQL**: use `query_iter()` (text protocol) for parameterless multi-
  statement; otherwise iterate prepared per statement.
- **Cassandra**: reject — CQL has no multi-statement; suggest `BATCH ... APPLY BATCH`.

Each driver wraps the loop in `begin/commit/rollback` so multi-statement
queries are atomic by default.

### Option B — Validation rejection (safety net)

If A is not in scope yet, `validate_syntax.rs` should detect multi-statement
input (count semicolons outside string literals / comments) and **fail bundle
load** with a clear error message: "DataView X.post_query contains multiple
statements; multi-statement queries are not currently supported by driver Y.
Split into separate DataViews and sequence in a handler."

This is far better than the current silent-truncation behavior and removes
the false-positive smoke-test class.

### Spec update

`docs/arch/rivers-data-layer-spec.md` and `rivers-driver-spec.md` must state
the contract explicitly — either "multi-statement supported, atomic per
DataView dispatch" (after Option A) or "single statement only; runtime
rejects multi-statement at validation" (after Option B).

## Recommended sequence

1. Land **Option B** immediately on the `mcp` branch — closes the silent-
   failure mode for every existing user, costs <50 LoC.
2. File **Option A** as a follow-up MINOR-bump feature (per CLAUDE.md
   versioning: this is "genuinely new ground" — multi-statement is a new
   capability, not a fix to a documented-but-missing API).
3. Update spec + tutorial in the same PR as whichever of A/B lands.

## Reproduction

Minimal repro using SQLite:

```toml
# resources.toml
[[datasources]]
id = "test-db"
type = "sqlite"
path = "/tmp/repro.db"

# app.toml
[data.dataviews.write_two]
datasource_id = "test-db"
post_query = """
INSERT INTO log (msg) VALUES ('first');
INSERT INTO log (msg) VALUES ('second');
"""

[api.views.write_two]
path = "/write"
methods = ["POST"]
dataview = "write_two"
```

Then `POST /write` returns 200; `SELECT count(*) FROM log` returns `1`.

## Cross-references

- `docs/bugs/cb-rivers-feature-request.md` — CB project context (filed P0
  asks for MCP that this bug surfaced underneath)
- CLAUDE.md "Build philosophy: always make sure canary is working no failures
  it is our production" — silent multi-statement truncation violates this
  directly; canary tests cannot detect it without DB inspection
