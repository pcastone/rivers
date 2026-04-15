# Rivers Schema Introspection Specification

**Document Type:** Implementation Specification
**Scope:** Schema-to-database validation at startup for SQL drivers
**Status:** Approved
**Version:** 1.0

---

## Table of Contents

1. [Overview](#1-overview)
2. [Driver Trait Extension](#2-driver-trait-extension)
3. [Datasource Config](#3-datasource-config)
4. [Startup Introspection Flow](#4-startup-introspection-flow)
5. [Error Messaging](#5-error-messaging)
6. [Testing Strategy](#6-testing-strategy)
7. [Future: Non-SQL Drivers](#7-future-non-sql-drivers)

---

## 1. Overview

Schema introspection validates DataView schema fields against actual database query results at startup. If a DataView declares a field that the query doesn't produce (e.g., `orderDate2` when the column is `orderDate`), riversd refuses to start with a detailed error message.

### Design Decisions

- **Enabled by default** — introspection runs automatically for SQL drivers. Operators opt out per-datasource with `introspect = false`.
- **LIMIT 0 approach** — the actual DataView query is executed with `LIMIT 0` to get column metadata without fetching data. Works for any query shape (joins, subqueries, views, CTEs).
- **Column names only (v1)** — field name mismatches are checked. Type comparison is deferred to a future version.
- **SQL drivers only (v1)** — postgres, mysql, sqlite. Non-SQL drivers return `Unsupported` and are skipped.
- **Hard fail** — any mismatch prevents startup. All mismatches are collected and reported together.

---

## 2. Driver Trait Extension

Two new methods on the `Driver` trait in `rivers-driver-sdk`:

```rust
fn supports_introspection(&self) -> bool {
    false
}

async fn introspect_columns(
    &self,
    conn: &mut dyn Connection,
    query: &str,
) -> Result<Vec<String>, DriverError>;
```

Default: `supports_introspection()` returns `false`. `introspect_columns()` returns `Err(DriverError::Unsupported)`.

Postgres, MySQL, and SQLite override both. Their `introspect_columns()` implementations execute the query with `LIMIT 0` appended (dialect-appropriate) and extract column names from the result metadata.

Non-SQL drivers (faker, memcached, http, exec, and all plugin drivers) inherit the defaults and are skipped automatically during introspection.

### Constraints

| ID | Rule |
|---|---|
| DRV-1 | Default `supports_introspection()` MUST return `false`. |
| DRV-2 | Default `introspect_columns()` MUST return `Err(DriverError::Unsupported)`. |
| DRV-3 | SQL driver implementations MUST execute the query with `LIMIT 0` — no data rows fetched. |
| DRV-4 | Returned column names MUST be in the order the query produces them. |
| DRV-5 | If the `LIMIT 0` query fails (syntax error, missing table), the error MUST propagate — this is itself a validation failure. |

---

## 3. Datasource Config

A new optional attribute on datasource config in `app.toml`:

```toml
[data.datasources.postgres-main]
name       = "postgres-main"
driver     = "postgres"
introspect = false
```

- `introspect` defaults to `true` for drivers that support introspection.
- Setting `introspect = false` skips introspection for all DataViews on that datasource.
- Non-SQL drivers ignore this field — their `supports_introspection()` returns `false` regardless.

### Use Cases for `introspect = false`

- Database migration in progress — schema is temporarily ahead of config.
- Read-only database where `LIMIT 0` queries may not be permitted.
- Development with rapidly changing schemas.

### Constraints

| ID | Rule |
|---|---|
| CFG-1 | `introspect` MUST default to `true`. |
| CFG-2 | `introspect = false` MUST skip introspection for all DataViews on that datasource. |
| CFG-3 | `introspect` MUST be added to the known datasource fields in structural validation. |

---

## 4. Startup Introspection Flow

After pool creation and DataView registration during bundle loading:

1. Group DataViews by datasource.
2. For each datasource where `introspect != false` and `driver.supports_introspection() == true`:
   a. Acquire a connection from the pool.
   b. For each DataView using that datasource:
      - Skip DataViews without a `query` field.
      - Call `driver.introspect_columns(conn, &dataview.query)`.
      - Compare each schema field name against the returned column list.
      - If a field name is missing: record the mismatch with Levenshtein suggestion.
   c. Release the connection.
3. If any mismatches were found: collect all errors, log them, and refuse to start.

All mismatches are collected and reported together — not fail-on-first. The operator sees every problem in one error output.

### Skipped DataViews

- DataViews without a `query` field (handler-only views).
- DataViews on datasources with `introspect = false`.
- DataViews on non-SQL drivers (driver returns `Unsupported`).

### Constraints

| ID | Rule |
|---|---|
| FLOW-1 | Introspection MUST run after pool creation (connections available) and after DataView registration. |
| FLOW-2 | One connection per datasource — reused across all DataViews on that datasource. |
| FLOW-3 | All mismatches MUST be collected before failing. No fail-on-first. |
| FLOW-4 | Hard fail — if any mismatches exist, riversd MUST refuse to start. |
| FLOW-5 | Connection MUST be released back to pool after introspection completes (even on error). |

---

## 5. Error Messaging

### Format

```
FATAL: DataView 'search_orders' field 'orderDate2' not found in query results
       — available columns: id, warehouseId, orderDate, locCode, qty
       — did you mean 'orderDate'?
```

### Levenshtein Suggestion

Uses the same `suggest_key()` function (Levenshtein distance) as bundle validation S002 warnings. The unknown field name is compared against the actual column list. If a close match exists (distance ≤ 2), the suggestion is included. If no close match, the "did you mean?" line is omitted.

### Multiple Mismatches

When multiple fields are wrong, each gets its own error line:

```
FATAL: schema introspection failed — 3 mismatches found:
  DataView 'search_orders' field 'orderDate2' not found — available: id, warehouseId, orderDate, locCode, qty — did you mean 'orderDate'?
  DataView 'search_orders' field 'qtyz' not found — available: id, warehouseId, orderDate, locCode, qty — did you mean 'qty'?
  DataView 'update_stock' field 'warehouse_id' not found — available: id, warehouseId, qty — did you mean 'warehouseId'?
```

### Constraints

| ID | Rule |
|---|---|
| ERR-1 | Error messages MUST include the DataView name, the missing field name, and the full list of available columns. |
| ERR-2 | Levenshtein suggestions MUST be included when a close match exists (distance ≤ 2). |
| ERR-3 | All mismatches MUST be reported in a single error output. |

---

## 6. Testing Strategy

### Unit Tests — Driver

- Postgres: `introspect_columns()` on `SELECT id, name FROM users` returns `["id", "name"]`.
- MySQL: same pattern.
- SQLite: same pattern.
- Non-SQL driver: `supports_introspection()` returns `false`, `introspect_columns()` returns `Err(DriverError::Unsupported)`.

### Unit Tests — Validation Logic

- All fields match — no error.
- One field missing — error with available columns listed.
- Missing field close to an existing column — "did you mean?" suggestion included.
- Missing field with no close match — error without suggestion.
- Multiple mismatches — all reported together.
- DataView without `query` field — skipped, no error.
- Datasource with `introspect = false` — skipped, no error.

### Integration Tests (Against Test Infrastructure)

- Postgres: create table, DataView with correct fields → startup succeeds.
- Postgres: DataView with typo field → startup fails with detailed error including column list and suggestion.
- MySQL: same two patterns.
- SQLite: same two patterns.
- Datasource with `introspect = false` → startup succeeds regardless of field mismatches.

### Canary Tests

- Canary DataView with intentionally mismatched field on a test datasource — verifies startup failure message.

---

## 7. Future: Non-SQL Drivers

Not in scope for v1. Potential future additions:

| Driver | Introspection Method | Complexity |
|--------|---------------------|------------|
| MongoDB | Sample document from collection | Medium — schemaless, best-effort |
| Elasticsearch | `GET /{index}/_mapping` API | Low — well-defined mapping API |
| Cassandra | `system_schema.columns` query | Low — structured metadata |
| Neo4j | `CALL db.schema.nodeTypeProperties()` | Medium — graph schema differs from tabular |
| CouchDB | Schemaless — skip | N/A |
| InfluxDB | Measurements metadata query | Medium |
| Redis | Key-type check only | Low but limited value |
| LDAP | Schema subentry query | Medium |

Each would implement `supports_introspection() → true` and provide a driver-specific `introspect_columns()` implementation.
