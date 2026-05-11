//! Layer 4 — Syntax verification for schema JSON, handler modules, and imports.
//!
//! Per `rivers-bundle-validation-spec.md` §4.4.
//!
//! This module validates:
//! - Schema JSON files are structurally valid (C006-C008)
//! - Handler modules compile (C001-C003) — via engine dylib FFI
//! - Handler entrypoints exist in exports (C002)
//! - Relative import paths resolve within the app boundary (C004-C005)
//! - DataView query fields contain exactly one SQL statement (C010, §SS-1..SS-6)

use std::path::Path;

use rivers_driver_sdk::{HttpMethod, SchemaDefinition, SchemaSyntaxError};

use crate::loader::LoadedBundle;
use crate::validate_engine::EngineHandles;
use crate::validate_result::{error_codes, ValidationResult};
use crate::view::HandlerConfig;

/// Validate syntax of all code artifacts and schema files in a bundle.
///
/// Runs schema JSON validation (pure Rust), handler compile checks (via engine
/// FFI if available), and import path resolution.
pub fn validate_syntax(
    bundle_dir: &Path,
    bundle: &LoadedBundle,
    engines: &EngineHandles,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    for app in &bundle.apps {
        let app_name = &app.manifest.app_name;
        let app_dir = &app.app_dir;

        // ── Schema JSON validation (C006-C009) ──────────────────────
        for (dv_name, dv) in &app.config.data.dataviews {
            // W-DV-CURSOR-1: cursor_key set but no ORDER BY in query.
            // Cursor pagination is non-deterministic without a sort order.
            if dv.cursor_key.is_some() {
                // Check all query variants for ORDER BY presence.
                let queries: &[Option<&str>] = &[
                    dv.query.as_deref(),
                    dv.get_query.as_deref(),
                    dv.post_query.as_deref(),
                    dv.put_query.as_deref(),
                    dv.delete_query.as_deref(),
                ];
                let any_query_has_order_by = queries
                    .iter()
                    .filter_map(|q| *q)
                    .any(|q| q.to_uppercase().contains("ORDER BY"));
                let any_query_exists = queries.iter().any(|q| q.is_some());

                if any_query_exists && !any_query_has_order_by {
                    let display = format!("{}/app.toml", app_name);
                    let mut result = ValidationResult::warn(
                        error_codes::W007,
                        format!(
                            "dataview '{}': cursor_key is set but query has no ORDER BY \
                             clause — cursor pagination requires deterministic ordering",
                            dv_name
                        ),
                    )
                    .with_app(app_name)
                    .with_table_path(&format!("data.dataviews.{}", dv_name))
                    .with_field("cursor_key");
                    result.file = Some(display);
                    results.push(result);
                }
            }

            // C-DV-COMPOSE-3: enrich strategy requires join_key (P2.9)
            if dv.compose_strategy.as_deref() == Some("enrich") && dv.join_key.is_none() {
                let display = format!("{}/app.toml", app_name);
                let result = ValidationResult::fail(
                    "C-DV-COMPOSE-3",
                    &display,
                    format!(
                        "DataView '{}' uses compose_strategy 'enrich' but join_key is not set",
                        dv_name
                    ),
                )
                .with_app(app_name)
                .with_table_path(&format!("data.dataviews.{}", dv_name))
                .with_field("join_key");
                results.push(result);
            }

            // SS-1..SS-6 (C010): each query field must contain exactly one SQL statement.
            // Semicolons inside string literals and SQL comments do NOT trigger this.
            {
                let query_fields: &[(&str, Option<&str>)] = &[
                    ("query",        dv.query.as_deref()),
                    ("get_query",    dv.get_query.as_deref()),
                    ("post_query",   dv.post_query.as_deref()),
                    ("put_query",    dv.put_query.as_deref()),
                    ("delete_query", dv.delete_query.as_deref()),
                ];
                let display = format!("{}/app.toml", app_name);
                for (field, sql_opt) in query_fields {
                    let Some(sql) = sql_opt else { continue };
                    if has_multiple_statements(sql) {
                        results.push(
                            ValidationResult::fail(
                                error_codes::C010,
                                &display,
                                format!(
                                    "DataView '{}' field '{}' contains multiple statements \
                                     (semicolon detected). Use a handler with Rivers.db.tx \
                                     for multi-query operations.",
                                    dv_name, field
                                ),
                            )
                            .with_app(app_name)
                            .with_table_path(&format!("data.dataviews.{}", dv_name))
                            .with_field(*field),
                        );
                    }
                }
            }

            // TF-3 (W008): transaction=true on a driver that does not support transactions
            // → validation warning (not error). Uses static §10.3 matrix.
            if dv.transaction {
                // Drivers known to NOT support transactions per spec §10.3.
                const NON_TRANSACTIONAL: &[&str] = &[
                    "redis", "elasticsearch", "couchdb", "cassandra",
                    "kafka", "ldap", "faker", "http", "filesystem", "exec",
                    "nats", "rabbitmq", "influxdb", "neo4j", "mongodb_atlas",
                ];
                let driver_for_txn = app
                    .config
                    .data
                    .datasources
                    .get(&dv.datasource)
                    .map(|ds| ds.driver.as_str())
                    .unwrap_or("");
                if NON_TRANSACTIONAL.contains(&driver_for_txn) {
                    let display = format!("{}/app.toml", app_name);
                    let mut result = ValidationResult::warn(
                        error_codes::W008,
                        format!(
                            "DataView '{}' has transaction=true but driver '{}' does not support transactions",
                            dv_name, driver_for_txn
                        ),
                    )
                    .with_app(app_name)
                    .with_table_path(&format!("data.dataviews.{}", dv_name))
                    .with_field("transaction");
                    result.file = Some(display);
                    results.push(result);
                }
            }

            // Look up the driver name so broker-specific checks can run (C009).
            let driver_name = app
                .config
                .data
                .datasources
                .get(&dv.datasource)
                .map(|ds| ds.driver.as_str())
                .unwrap_or("");

            // (method, schema_ref) pairs — method is used for broker schema checks.
            let schema_refs: &[(Option<HttpMethod>, Option<&str>)] = &[
                (Some(HttpMethod::GET), dv.return_schema.as_deref()),
                (Some(HttpMethod::GET), dv.get_schema.as_deref()),
                (Some(HttpMethod::POST), dv.post_schema.as_deref()),
                (Some(HttpMethod::PUT), dv.put_schema.as_deref()),
                (Some(HttpMethod::DELETE), dv.delete_schema.as_deref()),
            ];
            for (method, schema_ref) in schema_refs.iter() {
                let Some(schema_ref) = schema_ref else { continue };
                let schema_path = app_dir.join(schema_ref);
                let display_path = format!("{}/{}", app_name, schema_ref);
                if schema_path.exists() {
                    let schema_results =
                        validate_schema_json(&schema_path, &display_path, app_name, dv_name);
                    results.extend(schema_results);

                    // C009: driver-specific broker schema constraints.
                    if matches!(driver_name, "nats" | "rabbitmq" | "kafka") {
                        if let Some(method) = method {
                            let broker_results = validate_broker_schema(
                                &schema_path,
                                &display_path,
                                app_name,
                                dv_name,
                                driver_name,
                                *method,
                            );
                            results.extend(broker_results);
                        }
                    }
                }
                // Missing files are handled by Layer 2 (existence checks).
            }
        }

        // ── MCP-VAL-FED-1: federation entry must have non-empty alias and url ──
        for (view_name, view) in &app.config.api.views {
            for (i, fed) in view.federation.iter().enumerate() {
                let display = format!("{}/app.toml", app_name);
                if fed.alias.is_empty() {
                    results.push(
                        ValidationResult::fail(
                            "MCP-VAL-FED-1",
                            &display,
                            format!(
                                "view '{}' federation[{}]: 'alias' must not be empty",
                                view_name, i
                            ),
                        )
                        .with_app(app_name)
                        .with_table_path(&format!("api.views.{}.federation[{}]", view_name, i))
                        .with_field("alias"),
                    );
                }
                if fed.url.is_empty() {
                    results.push(
                        ValidationResult::fail(
                            "MCP-VAL-FED-1",
                            &display,
                            format!(
                                "view '{}' federation[{}]: 'url' must not be empty",
                                view_name, i
                            ),
                        )
                        .with_app(app_name)
                        .with_table_path(&format!("api.views.{}.federation[{}]", view_name, i))
                        .with_field("url"),
                    );
                }
                // Validate alias matches [a-z0-9_]+ pattern
                if !fed.alias.is_empty() && !fed.alias.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                    results.push(
                        ValidationResult::fail(
                            "MCP-VAL-FED-1",
                            &display,
                            format!(
                                "view '{}' federation[{}]: 'alias' must match [a-z0-9_]+, got '{}'",
                                view_name, i, fed.alias
                            ),
                        )
                        .with_app(app_name)
                        .with_table_path(&format!("api.views.{}.federation[{}]", view_name, i))
                        .with_field("alias"),
                    );
                }
            }
        }

        // ── Handler compile checks (C001-C003) ─────────────────────
        for (_view_name, view) in &app.config.api.views {
            if let HandlerConfig::Codecomponent {
                language,
                module,
                entrypoint,
                ..
            } = &view.handler
            {
                check_codecomponent_handler(
                    language, module, entrypoint, app_dir, app_name, engines, &mut results,
                );
            }

            // CB-OTLP Track O1.3 — per-signal handlers.{metrics,logs,traces}.
            // Same compile + entrypoint-in-exports check as the single
            // `handler:` form. Failures surface as the existing C001/C002/C003
            // codes; the structural layer already marks misconfigured OTLP
            // views with `[X-OTLP-N]` markers in their messages.
            if let Some(ref otlp_handlers) = view.handlers {
                for (_signal, h) in otlp_handlers {
                    if let HandlerConfig::Codecomponent {
                        language,
                        module,
                        entrypoint,
                        ..
                    } = h
                    {
                        check_codecomponent_handler(
                            language, module, entrypoint, app_dir, app_name, engines, &mut results,
                        );
                    }
                }
            }

            // Also check event handler pipeline stages
            if let Some(ref eh) = view.event_handlers {
                for stages in [&eh.pre_process, &eh.handlers, &eh.post_process, &eh.on_error] {
                    for stage in stages {
                        let module_path = app_dir.join(&stage.module);
                        if module_path.exists() {
                            if let Ok(source) = std::fs::read_to_string(&module_path) {
                                let display_path =
                                    format!("{}/{}", app_name, stage.module);
                                let import_results = validate_imports(
                                    &source,
                                    &module_path,
                                    app_dir,
                                    &display_path,
                                    app_name,
                                );
                                results.extend(import_results);
                            }
                        }
                    }
                }
            }
        }
    }

    // If no engines are available and no checks ran, add info
    let _ = bundle_dir; // used for future engine discovery

    results
}

/// Compile-check a single codecomponent handler and verify its entrypoint
/// is exported. Extracted helper so both the top-level `view.handler` form
/// and the OTLP per-signal `view.handlers.*` map share the same logic
/// (CB-OTLP Track O1.3).
///
/// Emits:
/// - `C001` (JS/TS SyntaxError),
/// - `C002` (entrypoint not in exports — JS/TS or WASM),
/// - `C003` (WASM validation failed),
/// - `C004/C005` (import resolution — via `validate_imports`),
/// - or a skip when the relevant engine dylib isn't loaded.
fn check_codecomponent_handler(
    language: &str,
    module: &str,
    entrypoint: &str,
    app_dir: &Path,
    app_name: &str,
    engines: &EngineHandles,
    results: &mut Vec<ValidationResult>,
) {
    let module_path = app_dir.join(module);
    let display_path = format!("{}/{}", app_name, module);

    if !module_path.exists() {
        return; // Layer 2 handles missing files
    }

    let is_wasm = matches!(language, "wasm");
    let is_js_ts = matches!(
        language,
        "typescript"
            | "ts"
            | "typescript_strict"
            | "ts_strict"
            | "javascript"
            | "js"
            | "javascript_v8"
            | "js_v8"
    );

    if is_js_ts {
        if let Some(ref v8) = engines.v8 {
            match std::fs::read(&module_path) {
                Ok(source) => {
                    let filename = module_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    match v8.compile_check(&source, &filename) {
                        Ok(check_result) => {
                            if check_result.exports.contains(&entrypoint.to_string()) {
                                results.push(
                                    ValidationResult::pass(
                                        &display_path,
                                        format!("compiles, export '{}' found", entrypoint),
                                    )
                                    .with_app(app_name)
                                    .with_exports(check_result.exports)
                                    .with_entrypoint_verified(true),
                                );
                            } else {
                                results.push(
                                    ValidationResult::fail(
                                        error_codes::C002,
                                        &display_path,
                                        format!(
                                            "entrypoint '{}' not found in exports — available: [{}]",
                                            entrypoint,
                                            check_result.exports.join(", ")
                                        ),
                                    )
                                    .with_app(app_name)
                                    .with_exports(check_result.exports)
                                    .with_entrypoint_verified(false),
                                );
                            }
                        }
                        Err(err) => {
                            let mut result = ValidationResult::fail(
                                error_codes::C001,
                                &display_path,
                                format!("SyntaxError: {}", err.message),
                            )
                            .with_app(app_name)
                            .with_error_type("SyntaxError");
                            if let Some(line) = err.line {
                                if let Some(col) = err.column {
                                    result = result.with_location(line, col);
                                }
                            }
                            results.push(result);
                        }
                    }

                    if let Ok(source_str) = std::str::from_utf8(&source) {
                        let import_results = validate_imports(
                            source_str,
                            &module_path,
                            app_dir,
                            &display_path,
                            app_name,
                        );
                        results.extend(import_results);
                    }
                }
                Err(_) => {
                    // Can't read file — Layer 2 would have caught this
                }
            }
        } else {
            results.push(
                ValidationResult::skip(
                    "v8",
                    "V8 engine dylib not available — TS/JS syntax check skipped",
                )
                .with_app(app_name),
            );
        }
    } else if is_wasm {
        if let Some(ref wasm) = engines.wasmtime {
            match std::fs::read(&module_path) {
                Ok(bytes) => {
                    let filename = module_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    match wasm.compile_check(&bytes, &filename) {
                        Ok(check_result) => {
                            if check_result.exports.contains(&entrypoint.to_string()) {
                                results.push(
                                    ValidationResult::pass(
                                        &display_path,
                                        format!(
                                            "valid WASM, export '{}' found",
                                            entrypoint
                                        ),
                                    )
                                    .with_app(app_name)
                                    .with_exports(check_result.exports)
                                    .with_entrypoint_verified(true),
                                );
                            } else {
                                results.push(
                                    ValidationResult::fail(
                                        error_codes::C002,
                                        &display_path,
                                        format!(
                                            "entrypoint '{}' not found in WASM exports — available: [{}]",
                                            entrypoint,
                                            check_result.exports.join(", ")
                                        ),
                                    )
                                    .with_app(app_name)
                                    .with_exports(check_result.exports)
                                    .with_entrypoint_verified(false),
                                );
                            }
                        }
                        Err(err) => {
                            results.push(
                                ValidationResult::fail(
                                    error_codes::C003,
                                    &display_path,
                                    format!("WASM validation failed: {}", err.message),
                                )
                                .with_app(app_name),
                            );
                        }
                    }
                }
                Err(_) => {}
            }
        } else {
            results.push(
                ValidationResult::skip(
                    "wasmtime",
                    "Wasmtime engine dylib not available — WASM check skipped",
                )
                .with_app(app_name),
            );
        }
    }

    // Run import resolution for JS/TS even without engine (file existence)
    if is_js_ts && engines.v8.is_none() {
        if let Ok(source) = std::fs::read_to_string(&module_path) {
            let import_results =
                validate_imports(&source, &module_path, app_dir, &display_path, app_name);
            results.extend(import_results);
        }
    }
}

// ── Schema JSON Validation ──────────────────────────────────────

/// Validate a schema JSON file for structural correctness.
///
/// Checks:
/// - Valid JSON (C006)
/// - Has `type` field (C007)
/// - `required` array entries match `properties` keys (C008)
pub fn validate_schema_json(
    path: &Path,
    display_path: &str,
    app_name: &str,
    _dv_name: &str,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return results, // File existence checked in Layer 2
    };

    // C006: Valid JSON
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            results.push(
                ValidationResult::fail(
                    error_codes::C006,
                    display_path,
                    format!("invalid JSON in schema — {e}"),
                )
                .with_app(app_name),
            );
            return results;
        }
    };

    let obj = match json.as_object() {
        Some(o) => o,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::C006,
                    display_path,
                    "schema root is not a JSON object",
                )
                .with_app(app_name),
            );
            return results;
        }
    };

    // C007: Has `type` field
    if !obj.contains_key("type") {
        results.push(
            ValidationResult::fail(
                error_codes::C007,
                display_path,
                "schema missing 'type' field",
            )
            .with_app(app_name),
        );
    }

    // C008: required entries match properties keys
    if let Some(required) = obj.get("required").and_then(|v| v.as_array()) {
        if let Some(properties) = obj.get("properties").and_then(|v| v.as_object()) {
            for req_val in required {
                if let Some(req_name) = req_val.as_str() {
                    if !properties.contains_key(req_name) {
                        results.push(
                            ValidationResult::fail(
                                error_codes::C008,
                                display_path,
                                format!(
                                    "schema 'required' array references property '{}' not in 'properties'",
                                    req_name
                                ),
                            )
                            .with_app(app_name),
                        );
                    }
                }
            }
        }
    }

    // If no errors, it passes
    if results.is_empty() {
        results.push(
            ValidationResult::pass(display_path, "schema JSON valid").with_app(app_name),
        );
    }

    results
}

// ── Broker Schema Validation (C009) ────────────────────────────

/// Validate driver-specific broker schema constraints (C009).
///
/// Called after structural JSON validation (`validate_schema_json`) for dataviews
/// backed by broker drivers. Deserializes the schema as a `SchemaDefinition` and
/// applies the same rules as the driver's `check_schema_syntax` implementation so
/// the validation pipeline catches missing fields (e.g. NATS requires `subject`)
/// at build time rather than silently accepting invalid configs.
fn validate_broker_schema(
    path: &Path,
    display_path: &str,
    app_name: &str,
    _dv_name: &str,
    driver_name: &str,
    method: HttpMethod,
) -> Vec<ValidationResult> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let schema: SchemaDefinition = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => return vec![], // C006 already reported by validate_schema_json
    };
    let result = check_broker_schema_syntax(driver_name, &schema, method);
    match result {
        Ok(()) => vec![],
        Err(e) => vec![
            ValidationResult::fail(
                error_codes::C009,
                display_path,
                format!("broker schema constraint: {e}"),
            )
            .with_app(app_name),
        ],
    }
}

/// Driver-specific schema constraint checking for broker drivers.
///
/// Mirrors the logic in each broker plugin's `check_schema_syntax` so that
/// `rivers-runtime` can validate without depending on the plugin crates.
fn check_broker_schema_syntax(
    driver_name: &str,
    schema: &SchemaDefinition,
    method: HttpMethod,
) -> Result<(), SchemaSyntaxError> {
    match driver_name {
        "nats" => {
            if schema.schema_type != "message" {
                return Err(SchemaSyntaxError::UnsupportedType {
                    schema_type: schema.schema_type.clone(),
                    driver: "nats".into(),
                    supported: vec!["message".into()],
                    schema_file: String::new(),
                });
            }
            if !schema.extra.contains_key("subject") {
                return Err(SchemaSyntaxError::MissingRequiredField {
                    field: "subject".into(),
                    driver: "nats".into(),
                    schema_file: String::new(),
                });
            }
            if matches!(method, HttpMethod::PUT | HttpMethod::DELETE) {
                return Err(SchemaSyntaxError::UnsupportedMethod {
                    method: method.as_str().into(),
                    driver: "nats".into(),
                    schema_file: String::new(),
                });
            }
        }
        "rabbitmq" => {
            if schema.schema_type != "message" {
                return Err(SchemaSyntaxError::UnsupportedType {
                    schema_type: schema.schema_type.clone(),
                    driver: "rabbitmq".into(),
                    supported: vec!["message".into()],
                    schema_file: String::new(),
                });
            }
            if method == HttpMethod::POST && !schema.extra.contains_key("exchange") {
                return Err(SchemaSyntaxError::MissingRequiredField {
                    field: "exchange".into(),
                    driver: "rabbitmq".into(),
                    schema_file: String::new(),
                });
            }
            if method == HttpMethod::GET && !schema.extra.contains_key("queue") {
                return Err(SchemaSyntaxError::MissingRequiredField {
                    field: "queue".into(),
                    driver: "rabbitmq".into(),
                    schema_file: String::new(),
                });
            }
            if matches!(method, HttpMethod::PUT | HttpMethod::DELETE) {
                return Err(SchemaSyntaxError::UnsupportedMethod {
                    method: method.as_str().into(),
                    driver: "rabbitmq".into(),
                    schema_file: String::new(),
                });
            }
        }
        "kafka" => {
            if schema.schema_type != "message" {
                return Err(SchemaSyntaxError::UnsupportedType {
                    schema_type: schema.schema_type.clone(),
                    driver: "kafka".into(),
                    supported: vec!["message".into()],
                    schema_file: String::new(),
                });
            }
            if !schema.extra.contains_key("topic") {
                return Err(SchemaSyntaxError::MissingRequiredField {
                    field: "topic".into(),
                    driver: "kafka".into(),
                    schema_file: String::new(),
                });
            }
            if matches!(method, HttpMethod::PUT | HttpMethod::DELETE) {
                return Err(SchemaSyntaxError::UnsupportedMethod {
                    method: method.as_str().into(),
                    driver: "kafka".into(),
                    schema_file: String::new(),
                });
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Import Path Resolution ──────────────────────────────────────

/// Extract relative import paths from JS/TS source.
///
/// Per spec FR-10: only checks relative paths (`./` or `../`).
/// Bare specifiers are skipped. Absolute paths are errors.
fn extract_relative_imports(source: &str) -> Vec<(String, bool)> {
    let mut imports = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Match: import ... from "..." or import ... from '...'
        // Also match: export ... from "..."
        if let Some(from_idx) = trimmed.find(" from ") {
            let after_from = &trimmed[from_idx + 6..];
            if let Some(path) = extract_string_literal(after_from) {
                if path.starts_with("./") || path.starts_with("../") {
                    imports.push((path.to_string(), false));
                } else if path.starts_with('/') {
                    imports.push((path.to_string(), true)); // absolute = error
                }
                // Bare specifiers silently skipped
            }
        }
    }

    imports
}

/// Extract a string literal from the start of a string slice.
fn extract_string_literal(s: &str) -> Option<&str> {
    let s = s.trim();
    let (quote, rest) = if s.starts_with('"') {
        ('"', &s[1..])
    } else if s.starts_with('\'') {
        ('\'', &s[1..])
    } else {
        return None;
    };

    rest.find(quote).map(|end| &rest[..end])
}

/// Validate import paths in a JS/TS source file.
fn validate_imports(
    source: &str,
    source_path: &Path,
    app_dir: &Path,
    display_path: &str,
    app_name: &str,
) -> Vec<ValidationResult> {
    let mut results = Vec::new();
    let source_dir = source_path.parent().unwrap_or(app_dir);
    let libraries_dir = app_dir.join("libraries");

    for (import_path, is_absolute) in extract_relative_imports(source) {
        if is_absolute {
            results.push(
                ValidationResult::fail(
                    error_codes::C004,
                    display_path,
                    format!(
                        "import '{}' uses absolute path — paths must be relative",
                        import_path
                    ),
                )
                .with_app(app_name),
            );
            continue;
        }

        // Resolve relative to the importing file's directory
        let resolved = source_dir.join(&import_path);

        // Try with common extensions if no extension provided
        let candidates = if resolved.extension().is_some() {
            vec![resolved.clone()]
        } else {
            vec![
                resolved.with_extension("ts"),
                resolved.with_extension("js"),
                resolved.with_extension("mjs"),
                resolved.clone(),
            ]
        };

        let found = candidates.iter().any(|c| c.exists());

        if !found {
            results.push(
                ValidationResult::fail(
                    error_codes::C005,
                    display_path,
                    format!("import '{}' target file not found", import_path),
                )
                .with_app(app_name),
            );
            continue;
        }

        // Check that the resolved path stays within libraries/
        if libraries_dir.exists() {
            if let Ok(canonical) = resolved.canonicalize().or_else(|_| {
                candidates
                    .iter()
                    .find_map(|c| c.canonicalize().ok())
                    .ok_or(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "not found",
                    ))
            }) {
                if let Ok(canonical_libs) = libraries_dir.canonicalize() {
                    if !canonical.starts_with(&canonical_libs) {
                        results.push(
                            ValidationResult::fail(
                                error_codes::C004,
                                display_path,
                                format!(
                                    "import '{}' resolves outside {}/libraries/ — cross-app imports not permitted",
                                    import_path, app_name
                                ),
                            )
                            .with_app(app_name),
                        );
                    }
                }
            }
        }
    }

    results
}

/// Returns `Some(field_name)` if the SQL string contains a statement-terminating
/// semicolon (i.e., a `;` that is not inside a string literal or SQL comment).
///
/// Algorithm (§2.4):
/// 1. Strip `--` line comments and `/* */` block comments from consideration
///    by tracking position rather than modifying the string.
/// 2. Walk chars tracking single-quote open/closed, respecting `''` as an
///    escaped single quote.
/// 3. A `;` encountered outside a quoted string is a violation.
pub(crate) fn has_multiple_statements(sql: &str) -> bool {
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while i < len {
        // ── Block comment ────────────────────────────────────────────
        if !in_single_quote && !in_line_comment && i + 1 < len
            && chars[i] == '/' && chars[i + 1] == '*'
        {
            in_block_comment = true;
            i += 2;
            continue;
        }
        if in_block_comment {
            if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        // ── Line comment ─────────────────────────────────────────────
        if !in_single_quote && i + 1 < len && chars[i] == '-' && chars[i + 1] == '-' {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if in_line_comment {
            if chars[i] == '\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        // ── Single-quote handling ─────────────────────────────────────
        if chars[i] == '\'' {
            if in_single_quote {
                // '' escape sequence → stay in quote
                if i + 1 < len && chars[i + 1] == '\'' {
                    i += 2;
                    continue;
                }
                in_single_quote = false;
            } else {
                in_single_quote = true;
            }
            i += 1;
            continue;
        }

        // ── Semicolon outside any context ─────────────────────────────
        if !in_single_quote && chars[i] == ';' {
            return true;
        }

        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Schema JSON tests ───────────────────────────────────────────

    #[test]
    fn valid_schema_passes() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("item.schema.json");
        std::fs::write(
            &schema_path,
            r#"{"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}"#,
        )
        .unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/item.schema.json", "app", "items");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, crate::validate_result::ValidationStatus::Pass);
    }

    #[test]
    fn c006_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("bad.json");
        std::fs::write(&schema_path, "{ not valid json }").unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/bad.json", "app", "dv");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error_code.as_deref(), Some("C006"));
    }

    #[test]
    fn c006_non_object_root() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("array.json");
        std::fs::write(&schema_path, "[1, 2, 3]").unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/array.json", "app", "dv");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error_code.as_deref(), Some("C006"));
    }

    #[test]
    fn c007_missing_type_field() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("notype.json");
        std::fs::write(&schema_path, r#"{"properties": {"id": {"type": "integer"}}}"#).unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/notype.json", "app", "dv");
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C007")));
    }

    #[test]
    fn c008_required_not_in_properties() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("badreq.json");
        std::fs::write(
            &schema_path,
            r#"{"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id", "name"]}"#,
        )
        .unwrap();

        let results = validate_schema_json(&schema_path, "app/schemas/badreq.json", "app", "dv");
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C008")));
        assert!(results.iter().any(|r| r.message.contains("name")));
    }

    // ── Import extraction tests ─────────────────────────────────────

    #[test]
    fn extract_relative_imports_basic() {
        let source = r#"
import { helper } from "./utils/helper";
import data from "../shared/data";
import lodash from "lodash";
export { foo } from './local';
"#;
        let imports = extract_relative_imports(source);
        assert_eq!(imports.len(), 3);
        assert_eq!(imports[0].0, "./utils/helper");
        assert!(!imports[0].1);
        assert_eq!(imports[1].0, "../shared/data");
        assert!(!imports[1].1);
        assert_eq!(imports[2].0, "./local");
        assert!(!imports[2].1);
    }

    #[test]
    fn extract_absolute_import_flagged() {
        let source = r#"import bad from "/absolute/path";"#;
        let imports = extract_relative_imports(source);
        assert_eq!(imports.len(), 1);
        assert!(imports[0].1); // is_absolute = true
    }

    #[test]
    fn bare_specifiers_skipped() {
        let source = r#"
import lodash from "lodash";
import React from "react";
import { foo } from "@org/pkg";
"#;
        let imports = extract_relative_imports(source);
        assert!(imports.is_empty());
    }

    #[test]
    fn validate_imports_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        let handler = dir.path().join("handler.ts");
        std::fs::write(&handler, r#"import { foo } from "./missing";"#).unwrap();

        let results = validate_imports(
            &std::fs::read_to_string(&handler).unwrap(),
            &handler,
            dir.path(),
            "app/handler.ts",
            "app",
        );
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C005")));
    }

    #[test]
    fn validate_imports_absolute_path_error() {
        let dir = tempfile::tempdir().unwrap();
        let handler = dir.path().join("handler.ts");
        std::fs::write(&handler, r#"import { foo } from "/etc/passwd";"#).unwrap();

        let results = validate_imports(
            &std::fs::read_to_string(&handler).unwrap(),
            &handler,
            dir.path(),
            "app/handler.ts",
            "app",
        );
        assert!(results.iter().any(|r| r.error_code.as_deref() == Some("C004")));
    }

    #[test]
    fn validate_imports_existing_relative() {
        let dir = tempfile::tempdir().unwrap();
        let libs = dir.path().join("libraries");
        std::fs::create_dir_all(&libs).unwrap();
        let handler = libs.join("handler.ts");
        let helper = libs.join("helper.ts");
        std::fs::write(&handler, r#"import { foo } from "./helper";"#).unwrap();
        std::fs::write(&helper, "export const foo = 1;").unwrap();

        let results = validate_imports(
            &std::fs::read_to_string(&handler).unwrap(),
            &handler,
            dir.path(),
            "app/libraries/handler.ts",
            "app",
        );
        // No errors — import resolves correctly
        assert!(
            results.is_empty() || results.iter().all(|r| r.status != crate::validate_result::ValidationStatus::Fail),
            "valid import should not produce errors: {:?}",
            results
        );
    }

    // ── C-DV-COMPOSE-3: enrich without join_key ─────────────────────

    #[test]
    fn c_dv_compose_3_enrich_without_join_key() {
        use crate::bundle::{AppManifest, BundleManifest, ResourcesConfig};
        use crate::loader::{LoadedApp, LoadedBundle};
        use crate::validate_engine::EngineHandles;
        use crate::bundle::{AppConfig, AppApiConfig, AppDataConfig};
        use crate::dataview::DataViewConfig;
        use std::collections::HashMap;
        use std::path::PathBuf;

        let dv = DataViewConfig {
            name: "enriched".into(),
            datasource: "ds".into(),
            query: None,
            parameters: vec![],
            return_schema: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
            get_parameters: vec![],
            post_parameters: vec![],
            put_parameters: vec![],
            delete_parameters: vec![],
            streaming: false,
            circuit_breaker_id: None,
            prepared: false,
            query_params: HashMap::new(),
            caching: None,
            invalidates: vec![],
            validate_result: false,
            strict_parameters: false,
            max_rows: 1000,
            skip_introspect: false,
            cursor_key: None,
            source_views: vec!["base".into(), "extra".into()],
            compose_strategy: Some("enrich".into()),
            join_key: None, // missing — should trigger C-DV-COMPOSE-3
            enrich_mode: "nest".into(),
            transaction: false,
        };

        let mut dataviews = HashMap::new();
        dataviews.insert("enriched".into(), dv);

        let app = LoadedApp {
            manifest: AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "00000000-0000-0000-0000-000000000001".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
                init: None,
            },
            resources: ResourcesConfig {
                datasources: vec![],
                keystores: vec![],
                services: vec![],
            },
            config: AppConfig {
                data: AppDataConfig {
                    datasources: HashMap::new(),
                    dataviews,
                    keystore: HashMap::new(),
                },
                api: AppApiConfig { views: HashMap::new() },
                static_files: None,
                base: Default::default(),
            },
            app_dir: PathBuf::from("/tmp/test-app"),
        };

        let bundle = LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "test".into(),
                bundle_version: "1.0.0".into(),
                source: None,
                apps: vec!["test-app".into()],
            },
            apps: vec![app],
        };

        let dir = tempfile::tempdir().unwrap();
        let results = validate_syntax(dir.path(), &bundle, &EngineHandles::none());

        assert!(
            results.iter().any(|r| {
                r.error_code.as_deref() == Some("C-DV-COMPOSE-3")
                    && r.status == crate::validate_result::ValidationStatus::Fail
            }),
            "enrich without join_key should emit C-DV-COMPOSE-3, got: {:?}",
            results
        );
    }

    #[test]
    fn c_dv_compose_3_enrich_with_join_key_passes() {
        use crate::bundle::{AppManifest, BundleManifest, ResourcesConfig};
        use crate::loader::{LoadedApp, LoadedBundle};
        use crate::validate_engine::EngineHandles;
        use crate::bundle::{AppConfig, AppApiConfig, AppDataConfig};
        use crate::dataview::DataViewConfig;
        use std::collections::HashMap;
        use std::path::PathBuf;

        let dv = DataViewConfig {
            name: "enriched".into(),
            datasource: "ds".into(),
            query: None,
            parameters: vec![],
            return_schema: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
            get_parameters: vec![],
            post_parameters: vec![],
            put_parameters: vec![],
            delete_parameters: vec![],
            streaming: false,
            circuit_breaker_id: None,
            prepared: false,
            query_params: HashMap::new(),
            caching: None,
            invalidates: vec![],
            validate_result: false,
            strict_parameters: false,
            max_rows: 1000,
            skip_introspect: false,
            cursor_key: None,
            source_views: vec!["base".into(), "extra".into()],
            compose_strategy: Some("enrich".into()),
            join_key: Some("order_id".into()), // set — should NOT trigger C-DV-COMPOSE-3
            enrich_mode: "nest".into(),
            transaction: false,
        };

        let mut dataviews = HashMap::new();
        dataviews.insert("enriched".into(), dv);

        let app = LoadedApp {
            manifest: AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "00000000-0000-0000-0000-000000000001".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
                init: None,
            },
            resources: ResourcesConfig {
                datasources: vec![],
                keystores: vec![],
                services: vec![],
            },
            config: AppConfig {
                data: AppDataConfig {
                    datasources: HashMap::new(),
                    dataviews,
                    keystore: HashMap::new(),
                },
                api: AppApiConfig { views: HashMap::new() },
                static_files: None,
                base: Default::default(),
            },
            app_dir: PathBuf::from("/tmp/test-app"),
        };

        let bundle = LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "test".into(),
                bundle_version: "1.0.0".into(),
                source: None,
                apps: vec!["test-app".into()],
            },
            apps: vec![app],
        };

        let dir = tempfile::tempdir().unwrap();
        let results = validate_syntax(dir.path(), &bundle, &EngineHandles::none());

        assert!(
            !results.iter().any(|r| r.error_code.as_deref() == Some("C-DV-COMPOSE-3")),
            "enrich with join_key should not emit C-DV-COMPOSE-3, got: {:?}",
            results
        );
    }

    // ── Single-statement scanner tests (TXN-A.5) ───────────────────

    #[test]
    fn ss_plain_semicolon_rejected() {
        assert!(has_multiple_statements("SELECT 1;"), "bare trailing ; must be rejected");
    }

    #[test]
    fn ss_two_statements_rejected() {
        assert!(has_multiple_statements("SELECT 1; SELECT 2"));
    }

    #[test]
    fn ss_trailing_whitespace_rejected() {
        assert!(has_multiple_statements("SELECT 1;  "), "trailing whitespace after ; still a violation");
    }

    #[test]
    fn ss_no_semicolon_accepted() {
        assert!(!has_multiple_statements("SELECT 1"));
        assert!(!has_multiple_statements("SELECT id, name FROM users WHERE id = $1"));
    }

    #[test]
    fn ss_semicolon_in_string_literal_accepted() {
        // SS-5: semicolons inside string literals must NOT trigger the error
        assert!(!has_multiple_statements("SELECT * FROM t WHERE name = 'foo;bar'"));
    }

    #[test]
    fn ss_line_comment_with_semicolon_accepted() {
        // SS-6: semicolons in -- comments must NOT trigger the error
        assert!(!has_multiple_statements("SELECT 1 -- this is a comment;"));
        assert!(!has_multiple_statements("-- comment;\nSELECT 1"));
    }

    #[test]
    fn ss_block_comment_with_semicolon_accepted() {
        assert!(!has_multiple_statements("/* a;b */ SELECT 1"));
        assert!(!has_multiple_statements("SELECT /* ;; */ 1"));
    }

    #[test]
    fn ss_escaped_quote_in_string_accepted() {
        // '' is an escaped single quote; the ; is inside the string
        assert!(!has_multiple_statements("SELECT * FROM t WHERE name = 'it''s;ok'"));
    }

    #[test]
    fn ss_empty_string_accepted() {
        assert!(!has_multiple_statements(""));
        assert!(!has_multiple_statements("   "));
    }

    #[test]
    fn ss_c010_emitted_in_validate_syntax() {
        use std::collections::HashMap;
        use crate::bundle::{AppConfig, AppApiConfig, AppDataConfig};
        use crate::loader::{LoadedApp, LoadedBundle};
        use crate::bundle::{AppManifest, BundleManifest, ResourcesConfig};
        use crate::dataview::DataViewConfig;
        use std::path::PathBuf;

        let dv = DataViewConfig {
            name: "bad_dv".into(),
            datasource: "db".into(),
            query: Some("SELECT 1; SELECT 2".into()),
            parameters: vec![],
            return_schema: None,
            get_query: None,
            post_query: None,
            put_query: None,
            delete_query: None,
            get_schema: None,
            post_schema: None,
            put_schema: None,
            delete_schema: None,
            get_parameters: vec![],
            post_parameters: vec![],
            put_parameters: vec![],
            delete_parameters: vec![],
            streaming: false,
            circuit_breaker_id: None,
            prepared: false,
            query_params: HashMap::new(),
            caching: None,
            invalidates: vec![],
            validate_result: false,
            strict_parameters: false,
            max_rows: 1000,
            skip_introspect: false,
            cursor_key: None,
            source_views: vec![],
            compose_strategy: None,
            join_key: None,
            enrich_mode: "nest".into(),
            transaction: false,
        };
        let mut dataviews = HashMap::new();
        dataviews.insert("bad_dv".into(), dv);

        let app = LoadedApp {
            manifest: AppManifest {
                app_name: "test-app".into(),
                description: None,
                version: None,
                app_type: "app-service".into(),
                app_id: "00000000-0000-0000-0000-000000000002".into(),
                entry_point: None,
                app_entry_point: None,
                source: None,
                spa: None,
                init: None,
            },
            resources: ResourcesConfig {
                datasources: vec![],
                keystores: vec![],
                services: vec![],
            },
            config: AppConfig {
                data: AppDataConfig {
                    datasources: HashMap::new(),
                    dataviews,
                    keystore: HashMap::new(),
                },
                api: AppApiConfig { views: HashMap::new() },
                static_files: None,
                base: Default::default(),
            },
            app_dir: PathBuf::from("/tmp/test-app"),
        };
        let bundle = LoadedBundle {
            manifest: BundleManifest {
                bundle_name: "test".into(),
                bundle_version: "1.0.0".into(),
                source: None,
                apps: vec!["test-app".into()],
            },
            apps: vec![app],
        };

        let dir = tempfile::tempdir().unwrap();
        let results = validate_syntax(dir.path(), &bundle, &EngineHandles::none());
        let c010 = results.iter().find(|r| r.error_code.as_deref() == Some("C010"));
        assert!(c010.is_some(), "C010 must be emitted for multi-statement query; got: {:?}", results);
        let msg = &c010.unwrap().message;
        assert!(msg.contains("bad_dv"), "message must name the DataView");
        assert!(msg.contains("query"), "message must name the field");
        assert!(msg.contains("Rivers.db.tx"), "message must reference Rivers.db.tx");
    }
}
