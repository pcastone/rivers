# URL Query Parameter Lifecycle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the full URL query parameter lifecycle per `rivers-query-param-spec.md` — multi-value parsing, `ctx.request.queryAll`, header parameter mapping, type coercion for uuid/date/array, default value coercion, outbound query params for HTTP driver, and startup validation of parameter mappings.

**Architecture:** Most infrastructure exists. The changes are: (1) extend `parse_query_string` to preserve multi-value params, (2) add `query_all` to `ParsedRequest`, (3) add `location` field to `DataViewParameterConfig`, (4) extend type coercion for uuid/date/array, (5) add header parameter mapping source, (6) add startup validation rules, (7) add outbound static query params to HTTP driver DataViews.

**Tech Stack:** Rust, serde, percent-encoding, Axum HTTP parsing

**Spec:** `docs/arch/rivers-query-param-spec (1).md`

---

## File Map

| Task | File | Action |
|------|------|--------|
| 1 | `crates/riversd/src/server/view_dispatch.rs` | Modify — multi-value query parsing |
| 1 | `crates/riversd/src/view_engine/types.rs` | Modify — add `query_all` to ParsedRequest |
| 2 | `crates/riversd/src/view_engine/pipeline.rs` | Modify — header mapping + array param handling |
| 3 | `crates/rivers-runtime/src/dataview.rs` | Modify — add `location` to DataViewParameterConfig |
| 3 | `crates/rivers-runtime/src/dataview_engine.rs` | Modify — extend type coercion (uuid, date, array, decimal) + default coercion |
| 4 | `crates/rivers-driver-sdk/src/http_driver.rs` | Modify — static query_params + outbound encoding |
| 5 | `crates/rivers-runtime/src/validate_crossref.rs` | Modify — add VAL-QP-1 through VAL-QP-5 |
| 6 | `crates/rivers-runtime/src/validate_structural.rs` | Modify — add `query_params` to DataView known fields |
| 7 | Documentation + final validation | |

---

### Task 1: Multi-Value Query Parsing + `queryAll`

**Files:**
- Modify: `crates/riversd/src/server/view_dispatch.rs:426-444`
- Modify: `crates/riversd/src/view_engine/types.rs:7-42`

- [ ] **Step 1: Add `query_all` field to ParsedRequest**

In `crates/riversd/src/view_engine/types.rs`, add a new field to `ParsedRequest` after `query_params`:

```rust
    /// All query string values per key (preserves duplicates).
    /// Serialized as "queryAll" — `ctx.request.queryAll` in handlers.
    #[serde(rename = "queryAll")]
    pub query_all: HashMap<String, Vec<String>>,
```

Update the `ParsedRequest::new()` constructor to include `query_all: HashMap::new()`.

- [ ] **Step 2: Create multi-value query parser**

In `crates/riversd/src/server/view_dispatch.rs`, add a new function alongside `parse_query_string`:

```rust
/// Parse query string preserving all values per key (for duplicate keys).
/// Returns (first_value_map, all_values_map).
pub(super) fn parse_query_string_multi(query: &str) -> (HashMap<String, String>, HashMap<String, Vec<String>>) {
    let mut first: HashMap<String, String> = HashMap::new();
    let mut all: HashMap<String, Vec<String>> = HashMap::new();

    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let mut parts = pair.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => percent_encoding::percent_decode_str(k)
                .decode_utf8_lossy()
                .into_owned(),
            None => continue,
        };
        let value = parts.next()
            .map(|v| percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned())
            .unwrap_or_default();

        first.entry(key.clone()).or_insert_with(|| value.clone());
        all.entry(key).or_default().push(value);
    }

    (first, all)
}
```

- [ ] **Step 3: Update query parsing call site**

Find where `parse_query_string` is called (around line 171-175). Replace:

```rust
let (query, query_all) = request
    .uri()
    .query()
    .map(|q| parse_query_string_multi(q))
    .unwrap_or_default();
```

Update the `ParsedRequest` construction to include `query_all`.

- [ ] **Step 4: Write tests**

Add tests to `view_dispatch.rs` or a test module:

```rust
#[test]
fn parse_multi_preserves_duplicates() {
    let (first, all) = parse_query_string_multi("tag=a&tag=b&tag=c");
    assert_eq!(first.get("tag"), Some(&"a".to_string()));
    assert_eq!(all.get("tag").unwrap(), &vec!["a", "b", "c"]);
}

#[test]
fn parse_multi_single_value() {
    let (first, all) = parse_query_string_multi("limit=10");
    assert_eq!(first.get("limit"), Some(&"10".to_string()));
    assert_eq!(all.get("limit").unwrap(), &vec!["10"]);
}

#[test]
fn parse_multi_empty_value() {
    let (first, all) = parse_query_string_multi("key=");
    assert_eq!(first.get("key"), Some(&"".to_string()));
}

#[test]
fn parse_multi_bare_key() {
    let (first, all) = parse_query_string_multi("key");
    assert_eq!(first.get("key"), Some(&"".to_string()));
}

#[test]
fn parse_multi_percent_encoded() {
    let (first, _) = parse_query_string_multi("name=John%20Doe&city=S%C3%A3o%20Paulo");
    assert_eq!(first.get("name"), Some(&"John Doe".to_string()));
    assert_eq!(first.get("city"), Some(&"São Paulo".to_string()));
}
```

- [ ] **Step 5: Verify and commit**

Run: `cargo check -p riversd && cargo test -p riversd --lib`

```bash
git add crates/riversd/src/server/view_dispatch.rs crates/riversd/src/view_engine/types.rs
git commit -m "feat(query-params): multi-value query parsing and ctx.request.queryAll"
```

---

### Task 2: Header Parameter Mapping + Array Handling

**Files:**
- Modify: `crates/riversd/src/view_engine/pipeline.rs:13-47`

- [ ] **Step 1: Add header parameter mapping source**

In `apply_parameter_mapping()`, after the body mapping section, add header mapping:

```rust
        // Map header parameters (spec §4.1)
        if let Some(ref header_mapping) = mapping.header {
            for (header_name, dv_param) in header_mapping {
                if let Some(value) = request.headers.get(header_name) {
                    params.insert(dv_param.clone(), serde_json::Value::String(value.clone()));
                }
            }
        }
```

Note: Check the `ParameterMapping` struct in `rivers-runtime` to see if `header` field exists. If not, add it:

```rust
pub struct ParameterMapping {
    pub query: HashMap<String, String>,
    pub path: HashMap<String, String>,
    pub body: HashMap<String, String>,
    pub header: HashMap<String, String>,  // Add this
}
```

- [ ] **Step 2: Add array parameter handling for query params**

When the DataView parameter declares `type = "array"`, the view layer should:
1. Check `query_all` for multiple values (Pattern 1: repeated key)
2. If single value with commas, split on `,` (Pattern 2: comma-separated)

Update the query mapping section in `apply_parameter_mapping()`:

```rust
        // Map query parameters — handle arrays (spec §5.2)
        for (http_param, dv_param) in &mapping.query {
            // Check queryAll for multi-value (Pattern 1: repeated key)
            if let Some(values) = request.query_all.get(http_param) {
                if values.len() > 1 {
                    // Multiple values — pass as JSON array
                    let arr: Vec<serde_json::Value> = values.iter()
                        .map(|v| serde_json::Value::String(v.clone()))
                        .collect();
                    params.insert(dv_param.clone(), serde_json::Value::Array(arr));
                    continue;
                }
            }
            // Single value — check for comma-separated (Pattern 2)
            if let Some(value) = request.query_params.get(http_param) {
                params.insert(dv_param.clone(), serde_json::Value::String(value.clone()));
            }
        }
```

Note: Comma splitting is handled during type coercion in the DataView engine (Task 3), not here. The pipeline passes the raw string; the engine splits on comma if the declared type is `array`.

- [ ] **Step 3: Verify and commit**

Run: `cargo check -p riversd && cargo check -p rivers-runtime`

```bash
git add crates/riversd/src/view_engine/pipeline.rs crates/rivers-runtime/src/view.rs
git commit -m "feat(query-params): header parameter mapping and multi-value query handling"
```

---

### Task 3: Type Coercion Extensions + Location Field

**Files:**
- Modify: `crates/rivers-runtime/src/dataview.rs`
- Modify: `crates/rivers-runtime/src/dataview_engine.rs`

- [ ] **Step 1: Add `location` field to DataViewParameterConfig**

In `crates/rivers-runtime/src/dataview.rs`, add to `DataViewParameterConfig`:

```rust
    /// Source location for this parameter: "path", "query", "body", "header".
    /// Used by HTTP driver for outbound parameter placement.
    #[serde(default)]
    pub location: Option<String>,
```

- [ ] **Step 2: Extend type coercion for uuid, date, decimal, array**

In `crates/rivers-runtime/src/dataview_engine.rs`, find `coerce_param_type()` and extend it:

```rust
pub fn coerce_param_type(value: &QueryValue, target_type: &str) -> Option<QueryValue> {
    match (value, target_type.to_lowercase().as_str()) {
        // Existing
        (QueryValue::String(s), "integer") => s.parse::<i64>().ok().map(QueryValue::Integer),
        (QueryValue::String(s), "float" | "decimal") => s.parse::<f64>().ok().map(QueryValue::Float),
        (QueryValue::String(s), "boolean") => match s.as_str() {
            "true" | "1" => Some(QueryValue::Boolean(true)),
            "false" | "0" => Some(QueryValue::Boolean(false)),
            _ => None,
        },
        // New: uuid validation (keep as string, validate format)
        (QueryValue::String(s), "uuid") => {
            if s.len() == 36
                && s.chars().nth(8) == Some('-')
                && s.chars().nth(13) == Some('-')
                && s.chars().nth(18) == Some('-')
                && s.chars().nth(23) == Some('-')
                && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            {
                Some(QueryValue::String(s.clone()))
            } else {
                None
            }
        }
        // New: date validation (keep as string, validate YYYY-MM-DD)
        (QueryValue::String(s), "date") => {
            if s.len() == 10
                && s.chars().nth(4) == Some('-')
                && s.chars().nth(7) == Some('-')
            {
                Some(QueryValue::String(s.clone()))
            } else {
                None
            }
        }
        // New: array from comma-separated string (spec §5.2 Pattern 2)
        (QueryValue::String(s), "array") => {
            let parts: Vec<serde_json::Value> = s.split(',')
                .map(|v| serde_json::Value::String(v.trim().to_string()))
                .collect();
            Some(QueryValue::Array(parts))
        }
        // Existing
        (QueryValue::Float(f), "integer") => {
            let i = *f as i64;
            if (i as f64 - f).abs() < f64::EPSILON { Some(QueryValue::Integer(i)) } else { None }
        }
        (QueryValue::Integer(i), "float" | "decimal") => Some(QueryValue::Float(*i as f64)),
        _ => None,
    }
}
```

- [ ] **Step 3: Ensure default values go through type coercion (spec QP-11)**

In `DataViewRequestBuilder::build_for()`, find where defaults are applied. Verify that `json_value_to_query_value(d, &param_def.param_type)` coerces the default. If the default is `"25"` on an integer param, it should produce `QueryValue::Integer(25)`.

Check the existing `json_value_to_query_value` function — if it takes the default as a `serde_json::Value` string and the target type, ensure it calls `coerce_param_type`. If not, wire it up.

- [ ] **Step 4: Write tests**

```rust
#[test]
fn coerce_uuid_valid() {
    let v = QueryValue::String("550e8400-e29b-41d4-a716-446655440000".into());
    assert!(coerce_param_type(&v, "uuid").is_some());
}

#[test]
fn coerce_uuid_invalid() {
    let v = QueryValue::String("not-a-uuid".into());
    assert!(coerce_param_type(&v, "uuid").is_none());
}

#[test]
fn coerce_date_valid() {
    let v = QueryValue::String("2026-04-15".into());
    assert!(coerce_param_type(&v, "date").is_some());
}

#[test]
fn coerce_date_invalid() {
    let v = QueryValue::String("04/15/2026".into());
    assert!(coerce_param_type(&v, "date").is_none());
}

#[test]
fn coerce_array_from_csv() {
    let v = QueryValue::String("a,b,c".into());
    let result = coerce_param_type(&v, "array").unwrap();
    match result {
        QueryValue::Array(arr) => assert_eq!(arr.len(), 3),
        _ => panic!("expected array"),
    }
}

#[test]
fn coerce_decimal_synonym() {
    let v = QueryValue::String("19.99".into());
    assert!(coerce_param_type(&v, "decimal").is_some());
}

#[test]
fn default_integer_coerced() {
    // Default "25" on integer param should produce Integer(25)
    let default_val = serde_json::json!("25");
    let result = json_value_to_query_value(&default_val, "integer");
    assert!(matches!(result, Some(QueryValue::Integer(25))));
}
```

- [ ] **Step 5: Verify and commit**

Run: `cargo check -p rivers-runtime && cargo test -p rivers-runtime --lib`

```bash
git add crates/rivers-runtime/src/dataview.rs crates/rivers-runtime/src/dataview_engine.rs
git commit -m "feat(query-params): uuid/date/array/decimal type coercion and location field"
```

---

### Task 4: Outbound Query Parameters (HTTP Driver)

**Files:**
- Modify: `crates/rivers-runtime/src/dataview.rs`
- Modify: `crates/rivers-driver-sdk/src/http_driver.rs`
- Modify: `crates/rivers-runtime/src/validate_structural.rs`

- [ ] **Step 1: Add `query_params` to DataViewConfig**

In `crates/rivers-runtime/src/dataview.rs`, add to `DataViewConfig`:

```rust
    /// Static query parameters appended to every outbound HTTP request.
    #[serde(default)]
    pub query_params: HashMap<String, String>,
```

- [ ] **Step 2: Add `query_params` to DATAVIEW_FIELDS**

In `validate_structural.rs`, add `"query_params"` to `DATAVIEW_FIELDS`.

- [ ] **Step 3: Wire static query params into HTTP driver**

In `crates/rivers-driver-sdk/src/http_driver.rs`, find where outbound HTTP requests are assembled. Ensure:
1. Static `query_params` from DataView config are applied first
2. Dynamic params with `location = "query"` are appended after
3. Dynamic overrides static for same key (spec QP-14)
4. Values are percent-encoded per RFC 3986 (spec QP-13)
5. Empty string values produce `?key=`, null/absent omit key (spec QP-15)

The implementer should trace how HTTP DataView requests are built — likely in `resolve_path_template` or the execute path of the HTTP driver.

- [ ] **Step 4: Add `query_params` to all DataViewConfig struct initializers**

Search for `DataViewConfig {` and add `query_params: HashMap::new()` to each.

- [ ] **Step 5: Verify and commit**

Run: `cargo check --workspace`

```bash
git add crates/rivers-runtime/src/dataview.rs crates/rivers-runtime/src/validate_structural.rs crates/rivers-driver-sdk/src/http_driver.rs
git commit -m "feat(query-params): static outbound query params for HTTP driver DataViews"
```

---

### Task 5: Startup Validation Rules (VAL-QP-1 through VAL-QP-5)

**Files:**
- Modify: `crates/rivers-runtime/src/validate_crossref.rs`

- [ ] **Step 1: Add parameter mapping validation**

In `validate_crossref()`, inside the per-app loop, add:

```rust
    // VAL-QP-1: Every key in parameter_mapping.query must map to a declared DataView parameter
    for (view_name, view_config) in &app.config.api.views {
        if let Some(ref mapping) = view_config.parameter_mapping {
            if let rivers_runtime::view::HandlerConfig::Dataview { ref dataview } = view_config.handler {
                if let Some(dv_config) = app.config.data.dataviews.get(dataview) {
                    let declared_params: Vec<&str> = dv_config.parameters
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect();

                    // Check query mappings
                    for (_http_param, dv_param) in &mapping.query {
                        if !declared_params.contains(&dv_param.as_str()) {
                            results.push(ValidationResult::fail(
                                "QP-1",
                                &format!("{}/app.toml", app.manifest.app_name),
                                format!(
                                    "view '{}' parameter_mapping.query maps to undeclared DataView parameter '{}'",
                                    view_name, dv_param
                                ),
                            ));
                        }
                    }

                    // VAL-QP-2: Check path mappings against view path segments
                    for (http_param, _dv_param) in &mapping.path {
                        let path_pattern = &view_config.path;
                        let segment = format!("{{{}}}", http_param);
                        if !path_pattern.contains(&segment) {
                            results.push(ValidationResult::fail(
                                "QP-2",
                                &format!("{}/app.toml", app.manifest.app_name),
                                format!(
                                    "view '{}' parameter_mapping.path key '{}' has no matching {{{}}} in path '{}'",
                                    view_name, http_param, http_param, path_pattern
                                ),
                            ));
                        }
                    }
                }
            }

            // VAL-QP-5: No duplicate right-side values within a single mapping section
            let mut seen_query: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for (_http_param, dv_param) in &mapping.query {
                if !seen_query.insert(dv_param.as_str()) {
                    results.push(ValidationResult::fail(
                        "QP-5",
                        &format!("{}/app.toml", app.manifest.app_name),
                        format!(
                            "view '{}' parameter_mapping.query has duplicate DataView param '{}'",
                            view_name, dv_param
                        ),
                    ));
                }
            }
        }
    }

    // VAL-QP-3: Required params with no mapping and no default — startup warning
    for (dv_name, dv_config) in &app.config.data.dataviews {
        for param in &dv_config.parameters {
            if param.required && param.default.is_none() {
                // Check if any view maps to this parameter
                let mapped = app.config.api.views.values().any(|v| {
                    v.parameter_mapping.as_ref().map_or(false, |m| {
                        m.query.values().any(|p| p == &param.name)
                        || m.path.values().any(|p| p == &param.name)
                        || m.body.values().any(|p| p == &param.name)
                    })
                });
                if !mapped {
                    let mut result = ValidationResult::warn(
                        "QP-3",
                        format!(
                            "DataView '{}' parameter '{}' is required with no default and no mapping — will always fail at runtime",
                            dv_name, param.name
                        ),
                    );
                    result.file = Some(format!("{}/app.toml", app.manifest.app_name));
                    results.push(result);
                }
            }
        }
    }
```

Note: The implementer needs to check how `ParameterMapping` struct is accessed — it may be `mapping.query`, `mapping.path`, `mapping.body`, `mapping.header` as `HashMap<String, String>`. Also check the `ApiViewConfig` struct to find `parameter_mapping` and `path` fields.

- [ ] **Step 2: Write tests**

Add tests following the existing `validate_crossref` test patterns — construct a test bundle with orphan mappings and verify the correct error codes.

- [ ] **Step 3: Verify and commit**

Run: `cargo test -p rivers-runtime --lib -- validate_crossref`

```bash
git add crates/rivers-runtime/src/validate_crossref.rs
git commit -m "feat(query-params): add VAL-QP-1 through VAL-QP-5 parameter mapping validation"
```

---

### Task 6: Max Query String Length + 414 Response

**Files:**
- Modify: `crates/riversd/src/server/view_dispatch.rs`

- [ ] **Step 1: Add query string length check**

Before query parsing (around line 171), add a length check:

```rust
    // QP-3: Max query string length (default 8192 bytes)
    let max_query_bytes = 8192; // TODO: make configurable via ServerConfig
    if let Some(query_str) = request.uri().query() {
        if query_str.len() > max_query_bytes {
            return crate::error_response::uri_too_long(
                format!("query string exceeds maximum length ({} > {} bytes)", query_str.len(), max_query_bytes)
            ).into_response();
        }
    }
```

Add `uri_too_long` to `error_response.rs` if it doesn't exist:

```rust
pub fn uri_too_long(message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::URI_TOO_LONG, Json(ErrorResponse::new(414, message)))
}
```

- [ ] **Step 2: Verify and commit**

```bash
git add crates/riversd/src/server/view_dispatch.rs crates/riversd/src/error_response.rs
git commit -m "feat(query-params): reject query strings exceeding max length with 414"
```

---

### Task 7: Documentation + Final Validation

- [ ] **Step 1: Verify full workspace compiles**

Run: `cargo check --workspace`

- [ ] **Step 2: Run all tests**

Run: `cargo test -p riversd --lib && cargo test -p rivers-runtime --lib`

- [ ] **Step 3: Validate address-book-bundle**

Run: `cargo run -p riverpackage -- validate address-book-bundle`

- [ ] **Step 4: Validate canary-bundle**

Run: `cargo run -p riverpackage -- validate canary-bundle`

- [ ] **Step 5: Commit**

```bash
git commit -m "docs: query parameter lifecycle implementation complete"
```
