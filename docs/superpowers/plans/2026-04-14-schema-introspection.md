# Schema Introspection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** At startup, validate DataView schema fields against actual database query results for SQL drivers. Hard fail with detailed error messages and "did you mean?" suggestions on mismatch.

**Architecture:** Add `supports_introspection()` and `introspect_columns()` to the Driver trait. SQL drivers implement by executing `SELECT ... LIMIT 0` and extracting column names. Bundle loader runs introspection after pool creation, compares schema fields, and refuses to start on mismatch. Datasource config gains `introspect = false` opt-out.

**Tech Stack:** Rust, rivers-driver-sdk traits, tokio-postgres/mysql_async/rusqlite column metadata APIs

**Spec:** `docs/arch/rivers-schema-introspection-spec.md`

---

## File Map

| Task | File | Action |
|------|------|--------|
| 1 | `crates/rivers-driver-sdk/src/traits.rs` | Modify — add introspection methods to Driver trait |
| 2 | `crates/rivers-drivers-builtin/src/postgres.rs` | Modify — implement introspect_columns |
| 2 | `crates/rivers-drivers-builtin/src/mysql.rs` | Modify — implement introspect_columns |
| 2 | `crates/rivers-drivers-builtin/src/sqlite.rs` | Modify — implement introspect_columns |
| 3 | `crates/rivers-runtime/src/datasource.rs` | Modify — add `introspect` field |
| 3 | `crates/rivers-runtime/src/validate_structural.rs` | Modify — add to DATASOURCE_DECL_FIELDS |
| 4 | `crates/riversd/src/schema_introspection.rs` | Create — introspection logic and error formatting |
| 5 | `crates/riversd/src/bundle_loader/load.rs` | Modify — call introspection after pool creation |
| 6 | Documentation and task tracking | Modify |

---

### Task 1: Driver Trait — Introspection Methods

**Files:**
- Modify: `crates/rivers-driver-sdk/src/traits.rs`

- [ ] **Step 1: Add introspection methods to DatabaseDriver trait**

In the `DatabaseDriver` trait (around line 524), add after `param_style()`:

```rust
    /// Whether this driver supports schema introspection at startup.
    fn supports_introspection(&self) -> bool {
        false
    }

    /// Introspect the columns returned by a query.
    /// Executes the query with LIMIT 0 and returns column names.
    async fn introspect_columns(
        &self,
        conn: &mut Box<dyn Connection>,
        query: &str,
    ) -> Result<Vec<String>, DriverError> {
        Err(DriverError::Unsupported(format!(
            "{} does not support schema introspection",
            self.name()
        )))
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rivers-driver-sdk`
Expected: Compiles. Default implementations mean no driver breaks.

- [ ] **Step 3: Commit**

```bash
git add crates/rivers-driver-sdk/src/traits.rs
git commit -m "feat(introspection): add supports_introspection and introspect_columns to DatabaseDriver trait"
```

---

### Task 2: SQL Driver Implementations

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/postgres.rs`
- Modify: `crates/rivers-drivers-builtin/src/mysql.rs`
- Modify: `crates/rivers-drivers-builtin/src/sqlite.rs`

- [ ] **Step 1: PostgreSQL — implement introspection**

In `PostgresDriver`'s `DatabaseDriver` impl, add:

```rust
fn supports_introspection(&self) -> bool {
    true
}

async fn introspect_columns(
    &self,
    conn: &mut Box<dyn Connection>,
    query: &str,
) -> Result<Vec<String>, DriverError> {
    let limited = format!("SELECT * FROM ({}) AS _introspect LIMIT 0", query);
    let result = conn.execute(&rivers_driver_sdk::Query {
        operation: "select".to_string(),
        target: String::new(),
        statement: limited,
        parameters: std::collections::HashMap::new(),
    }).await?;
    // Column names come from the keys of the first row, but with LIMIT 0 there are no rows.
    // We need the column metadata from the driver instead.
    // For tokio-postgres, we need to use the statement metadata approach.
    // Alternative: execute with LIMIT 1 on an empty result set and read column names.
    // The implementer should check if tokio-postgres exposes column names on the statement
    // or if a different approach is needed (e.g., using prepare() to get column metadata).
    todo!("implementer: extract column names from postgres LIMIT 0 result metadata")
}
```

**Important note for the implementer:** The `execute()` method returns `QueryResult` which only has `rows` — no column metadata. For LIMIT 0 queries, `rows` is empty, so column names aren't available through the current API.

**Better approach:** Use `tokio_postgres::Client::prepare()` which returns a `Statement` with column metadata:

```rust
async fn introspect_columns(
    &self,
    conn: &mut Box<dyn Connection>,
    query: &str,
) -> Result<Vec<String>, DriverError> {
    // We need access to the underlying tokio_postgres::Client.
    // This requires either:
    // a) Downcasting the Connection to PostgresConnection
    // b) Adding an introspect method to the Connection trait
    // c) Adding a column_names() method to QueryResult
    
    // Option (c) is cleanest — modify QueryResult to optionally carry column names:
    // pub column_names: Option<Vec<String>>
    // Then have each driver populate it on execute().
    // For LIMIT 0, even with 0 rows the column names are available.
}
```

**Recommended approach:** Add `pub column_names: Option<Vec<String>>` to `QueryResult` in `rivers-driver-sdk`. Then each SQL driver populates it during `execute()`. The introspect method just does a LIMIT 0 query and reads column_names from the result.

The implementer should:
1. Add `column_names: Option<Vec<String>>` to `QueryResult`
2. Update `QueryResult::empty()` to set `column_names: None`
3. Populate column names in postgres/mysql/sqlite `execute()` implementations
4. Implement `introspect_columns()` as a LIMIT 0 query that reads the column names

- [ ] **Step 2: MySQL — implement introspection**

Same pattern as PostgreSQL but using `mysql_async` column metadata. MySQL's `ResultSet` provides `columns_ref()` which gives column names — the mysql driver already uses this (line 189 of mysql.rs). Adapt to populate `column_names` on `QueryResult`.

- [ ] **Step 3: SQLite — implement introspection**

SQLite via `rusqlite` provides column names via `Statement::column_names()`. The sqlite driver runs in `spawn_blocking` — adapt accordingly.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p rivers-drivers-builtin`
Expected: Compiles.

- [ ] **Step 5: Write tests**

Each driver needs a test that calls `introspect_columns()` on a known query and verifies the column list. Tests should SKIP if the database is unreachable.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-driver-sdk/src/types.rs crates/rivers-drivers-builtin/src/postgres.rs crates/rivers-drivers-builtin/src/mysql.rs crates/rivers-drivers-builtin/src/sqlite.rs
git commit -m "feat(introspection): implement introspect_columns for postgres, mysql, sqlite"
```

---

### Task 3: Datasource Config — Add `introspect` Field

**Files:**
- Modify: `crates/rivers-runtime/src/datasource.rs`
- Modify: `crates/rivers-runtime/src/validate_structural.rs`

- [ ] **Step 1: Add `introspect` field to DatasourceConfig**

In the datasource config struct (find `pub struct DatasourceConfig` or equivalent in `datasource.rs`), add:

```rust
    /// Whether to run schema introspection at startup. Defaults to true.
    #[serde(default = "default_introspect")]
    pub introspect: bool,
```

Add the default function:

```rust
fn default_introspect() -> bool {
    true
}
```

- [ ] **Step 2: Add to DATASOURCE_DECL_FIELDS**

In `validate_structural.rs`, find `DATASOURCE_DECL_FIELDS` (around line 47) and add `"introspect"`.

- [ ] **Step 3: Add deserialization test**

```rust
#[test]
fn datasource_config_introspect_defaults_true() {
    let toml_str = r#"
        name = "test"
        driver = "postgres"
    "#;
    let cfg: DatasourceConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.introspect);
}

#[test]
fn datasource_config_introspect_false() {
    let toml_str = r#"
        name = "test"
        driver = "postgres"
        introspect = false
    "#;
    let cfg: DatasourceConfig = toml::from_str(toml_str).unwrap();
    assert!(!cfg.introspect);
}
```

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p rivers-runtime && cargo test -p rivers-runtime --lib`

```bash
git add crates/rivers-runtime/src/datasource.rs crates/rivers-runtime/src/validate_structural.rs
git commit -m "feat(introspection): add introspect field to datasource config"
```

---

### Task 4: Introspection Logic Module

**Files:**
- Create: `crates/riversd/src/schema_introspection.rs`

- [ ] **Step 1: Create the introspection module**

```rust
//! Schema introspection — validates DataView fields against database columns at startup.

use std::collections::HashMap;
use rivers_runtime::rivers_driver_sdk::DriverError;

/// A single schema mismatch found during introspection.
#[derive(Debug)]
pub struct SchemaMismatch {
    pub dataview_name: String,
    pub field_name: String,
    pub available_columns: Vec<String>,
    pub suggestion: Option<String>,
}

impl std::fmt::Display for SchemaMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DataView '{}' field '{}' not found — available: {}",
            self.dataview_name,
            self.field_name,
            self.available_columns.join(", ")
        )?;
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " — did you mean '{}'?", suggestion)?;
        }
        Ok(())
    }
}

/// Compare schema field names against actual query column names.
/// Returns a list of mismatches.
pub fn check_fields_against_columns(
    dataview_name: &str,
    schema_fields: &[String],
    actual_columns: &[String],
) -> Vec<SchemaMismatch> {
    let mut mismatches = Vec::new();

    for field in schema_fields {
        if !actual_columns.iter().any(|c| c == field) {
            let suggestion = suggest_column(field, actual_columns);
            mismatches.push(SchemaMismatch {
                dataview_name: dataview_name.to_string(),
                field_name: field.clone(),
                available_columns: actual_columns.to_vec(),
                suggestion,
            });
        }
    }

    mismatches
}

/// Suggest a column name using Levenshtein distance (max distance 2).
fn suggest_column(unknown: &str, columns: &[String]) -> Option<String> {
    let mut best: Option<(&str, usize)> = None;
    for col in columns {
        let dist = levenshtein(unknown, col);
        if dist <= 2 {
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((col, dist));
            }
        }
    }
    best.map(|(s, _)| s.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut matrix = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() { matrix[i][0] = i; }
    for j in 0..=b.len() { matrix[0][j] = j; }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i-1] == b[j-1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i-1][j] + 1)
                .min(matrix[i][j-1] + 1)
                .min(matrix[i-1][j-1] + cost);
        }
    }
    matrix[a.len()][b.len()]
}

/// Format all mismatches into a single error message for startup failure.
pub fn format_introspection_errors(mismatches: &[SchemaMismatch]) -> String {
    if mismatches.len() == 1 {
        format!("schema introspection failed: {}", mismatches[0])
    } else {
        let details: Vec<String> = mismatches.iter().map(|m| format!("  {}", m)).collect();
        format!(
            "schema introspection failed — {} mismatches found:\n{}",
            mismatches.len(),
            details.join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_fields_match() {
        let fields = vec!["id".into(), "name".into(), "qty".into()];
        let columns = vec!["id".into(), "name".into(), "qty".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert!(mismatches.is_empty());
    }

    #[test]
    fn one_field_missing() {
        let fields = vec!["id".into(), "namee".into()];
        let columns = vec!["id".into(), "name".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].field_name, "namee");
        assert_eq!(mismatches[0].suggestion, Some("name".to_string()));
    }

    #[test]
    fn no_suggestion_for_distant_field() {
        let fields = vec!["zzzzz".into()];
        let columns = vec!["id".into(), "name".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert_eq!(mismatches.len(), 1);
        assert!(mismatches[0].suggestion.is_none());
    }

    #[test]
    fn multiple_mismatches_collected() {
        let fields = vec!["idd".into(), "namee".into(), "qtyz".into()];
        let columns = vec!["id".into(), "name".into(), "qty".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert_eq!(mismatches.len(), 3);
    }

    #[test]
    fn error_format_single() {
        let mismatches = vec![SchemaMismatch {
            dataview_name: "orders".into(),
            field_name: "qtyz".into(),
            available_columns: vec!["id".into(), "qty".into()],
            suggestion: Some("qty".into()),
        }];
        let msg = format_introspection_errors(&mismatches);
        assert!(msg.contains("orders"));
        assert!(msg.contains("qtyz"));
        assert!(msg.contains("qty"));
    }

    #[test]
    fn error_format_multiple() {
        let mismatches = vec![
            SchemaMismatch {
                dataview_name: "orders".into(),
                field_name: "qtyz".into(),
                available_columns: vec!["qty".into()],
                suggestion: Some("qty".into()),
            },
            SchemaMismatch {
                dataview_name: "orders".into(),
                field_name: "idd".into(),
                available_columns: vec!["id".into()],
                suggestion: Some("id".into()),
            },
        ];
        let msg = format_introspection_errors(&mismatches);
        assert!(msg.contains("2 mismatches"));
    }
}
```

- [ ] **Step 2: Register module**

In `crates/riversd/src/lib.rs`, add:

```rust
/// Schema introspection — validates DataView fields against database columns at startup.
pub mod schema_introspection;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p riversd --lib -- schema_introspection`
Expected: All 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/riversd/src/schema_introspection.rs crates/riversd/src/lib.rs
git commit -m "feat(introspection): add schema introspection module with Levenshtein suggestions"
```

---

### Task 5: Wire Introspection into Bundle Loading

**Files:**
- Modify: `crates/riversd/src/bundle_loader/load.rs`

- [ ] **Step 1: Add introspection call after pool creation**

In `load_and_wire_bundle()`, after DataView registration and pool creation (after the circuit breaker registry block), add:

```rust
    // ── Schema introspection (schema-introspection-spec §4) ──
    {
        let mut all_mismatches: Vec<crate::schema_introspection::SchemaMismatch> = Vec::new();

        // Group DataViews by datasource
        for (ds_name, ds_config) in &app.config.data.datasources {
            // Skip if introspection disabled
            if !ds_config.introspect {
                tracing::info!(datasource = %ds_name, "schema introspection skipped (introspect = false)");
                continue;
            }

            // Check if driver supports introspection
            // The implementer needs to resolve the driver from the DriverFactory
            // and check supports_introspection(). If false, skip.

            // Acquire connection from pool
            // Execute LIMIT 0 queries for each DataView using this datasource
            // Compare schema fields against returned column names
            // Collect mismatches

            for (dv_name, dv_config) in &app.config.data.dataviews {
                if dv_config.datasource != *ds_name {
                    continue;
                }
                let query = match &dv_config.query {
                    Some(q) => q,
                    None => continue, // No query — skip
                };

                // Get schema fields for this DataView
                // The implementer needs to resolve the schema file and extract field names
                // This depends on how schemas are loaded — check the existing schema loading code

                // Call driver.introspect_columns(conn, query)
                // Compare fields vs columns using check_fields_against_columns()
                // Append mismatches to all_mismatches
            }
        }

        if !all_mismatches.is_empty() {
            let msg = crate::schema_introspection::format_introspection_errors(&all_mismatches);
            tracing::error!("{}", msg);
            return Err(ServerError::Config(msg));
        }
    }
```

Note: The exact integration depends on how the DriverFactory, connection pool, and schema loading work in the bundle loader. The implementer should:
1. Trace how pool connections are acquired (look at existing pool usage in the loader)
2. Trace how schema fields are loaded (check DataView schema resolution)
3. Resolve the driver from the DriverFactory to call `supports_introspection()`

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p riversd`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/riversd/src/bundle_loader/load.rs
git commit -m "feat(introspection): wire schema introspection into bundle loading — hard fail on mismatch"
```

---

### Task 6: Documentation and Task Tracking

**Files:**
- Modify: `todo/ProgramReviewTasks.md`

- [ ] **Step 1: Mark schema introspection tasks complete**

Update all Schema-to-Database Validation items from `- [ ]` to `- [x]`.

- [ ] **Step 2: Commit**

```bash
git add todo/ProgramReviewTasks.md
git commit -m "docs: mark schema introspection tasks complete"
```

---

### Task 7: Final Validation

- [ ] **Step 1: Full workspace compile**

Run: `cargo check --workspace`
Expected: Compiles clean.

- [ ] **Step 2: Run riversd tests**

Run: `cargo test -p riversd --lib`
Expected: All tests pass including new introspection tests.

- [ ] **Step 3: Run rivers-runtime tests**

Run: `cargo test -p rivers-runtime --lib`
Expected: All tests pass.

- [ ] **Step 4: Validate address-book-bundle**

Run: `cargo run -p riverpackage -- validate address-book-bundle`
Expected: 0 errors (address-book-bundle uses faker driver which doesn't support introspection — should be skipped).

- [ ] **Step 5: Integration test with live database (if available)**

Create a test DataView pointing at a real postgres table with a typo field. Verify startup fails with the expected error message including column list and Levenshtein suggestion.
