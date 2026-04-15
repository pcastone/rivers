//! Layer 1 — Structural TOML Validation.
//!
//! Per `rivers-bundle-validation-spec.md` §4.1 and
//! `rivers-bundle-validation-friction.md` FR-1.
//!
//! Validates that every TOML file in a bundle uses only known keys,
//! includes all required fields, and that field values conform to expected
//! formats (UUID, semver, app-type enum, nopassword/lockbox exclusion).
//!
//! **Approach:** Parse each file as `toml::Value`, then walk the value tree
//! comparing keys against known field sets. This avoids adding
//! `deny_unknown_fields` to the runtime config structs (which would break
//! the existing `load_bundle()` path).

use std::path::Path;

use crate::validate_format::suggest_key;
use crate::validate_result::{error_codes, ValidationResult};

// ── Known field sets (FR-1) ───────────────────────────────────────

/// Bundle `manifest.toml` — all required.
const BUNDLE_MANIFEST_FIELDS: &[&str] = &["bundleName", "bundleVersion", "source", "apps"];
const BUNDLE_MANIFEST_REQUIRED: &[&str] = &["bundleName", "bundleVersion", "source", "apps"];

/// App `manifest.toml`.
const APP_MANIFEST_FIELDS: &[&str] = &[
    "appName", "description", "version", "type", "appId",
    "entryPoint", "appEntryPoint", "source", "spa", "init",
];
const APP_MANIFEST_REQUIRED: &[&str] = &[
    "appName", "version", "type", "appId", "entryPoint", "source",
];

/// `spa` sub-table in app manifest.
const SPA_FIELDS: &[&str] = &["root", "indexFile", "fallback", "maxAge"];
const SPA_REQUIRED: &[&str] = &["root", "indexFile"];

/// `init` sub-table in app manifest.
const INIT_FIELDS: &[&str] = &["module", "entrypoint"];
const INIT_REQUIRED: &[&str] = &["module", "entrypoint"];

/// `resources.toml` top-level.
const RESOURCES_FIELDS: &[&str] = &["datasources", "services", "keystores"];

/// Datasource entry in `[[datasources]]`.
const DATASOURCE_DECL_FIELDS: &[&str] = &[
    "name", "driver", "x-type", "lockbox", "nopassword", "required",
    "host", "port", "database", "username", "password", "service",
];
const DATASOURCE_DECL_REQUIRED: &[&str] = &["name", "driver", "x-type", "required"];

/// Service entry in `[[services]]`.
const SERVICE_DECL_FIELDS: &[&str] = &["name", "appId", "required"];
const SERVICE_DECL_REQUIRED: &[&str] = &["name", "appId", "required"];

/// Keystore entry in `[[keystores]]`.
const KEYSTORE_DECL_FIELDS: &[&str] = &["name", "lockbox", "required"];
const KEYSTORE_DECL_REQUIRED: &[&str] = &["name", "lockbox", "required"];

/// `app.toml` top-level.
const APP_CONFIG_FIELDS: &[&str] = &["data", "api", "static_files"];

/// `[data]` section.
const DATA_CONFIG_FIELDS: &[&str] = &["datasources", "dataviews", "keystore"];

/// DataView config.
const DATAVIEW_FIELDS: &[&str] = &[
    "name", "datasource", "query", "parameters", "caching", "max_rows",
    "invalidates", "get_schema", "post_schema", "put_schema", "delete_schema",
    "get_query", "post_query", "put_query", "delete_query",
    "return_schema", "get_parameters", "post_parameters", "put_parameters",
    "delete_parameters", "streaming", "validate_result", "strict_parameters",
];
const DATAVIEW_REQUIRED: &[&str] = &["name", "datasource"];

/// DataView parameter.
const PARAMETER_FIELDS: &[&str] = &["name", "type", "required", "default", "location"];
const PARAMETER_REQUIRED: &[&str] = &["name", "type"];

/// DataView caching config.
const CACHING_FIELDS: &[&str] = &[
    "ttl_seconds", "l1_enabled", "l1_max_bytes", "l1_max_entries",
    "l2_enabled", "l2_max_value_bytes",
];
const CACHING_REQUIRED: &[&str] = &["ttl_seconds"];

/// `[api]` section.
const API_CONFIG_FIELDS: &[&str] = &["views"];

/// View config.
const VIEW_FIELDS: &[&str] = &[
    "path", "method", "view_type", "response_format", "auth", "handler",
    "parameter_mapping", "process_pool", "libs", "datasources", "dataviews",
    "primary", "streaming", "streaming_format", "stream_timeout_ms",
    "allow_outbound_http", "allow_env_vars", "session_stage", "methods",
    "guard", "guard_config", "rate_limit_per_minute", "rate_limit_burst_size",
    "websocket_mode", "max_connections", "sse_tick_interval_ms",
    "sse_trigger_events", "sse_event_buffer_size",
    "session_revalidation_interval_s", "polling", "event_handlers",
    "on_stream", "ws_hooks", "on_event",
];
const VIEW_REQUIRED: &[&str] = &["path", "method", "view_type", "handler"];

/// Handler config.
const HANDLER_FIELDS: &[&str] = &[
    "type", "dataview", "language", "module", "entrypoint", "resources",
];
const HANDLER_REQUIRED: &[&str] = &["type"];

/// Parameter mapping config.
const PARAM_MAPPING_FIELDS: &[&str] = &["query", "path", "body", "header"];

/// `[static_files]` section.
const STATIC_FILES_FIELDS: &[&str] = &["enabled", "root", "index_file", "spa_fallback"];

// ── Public API ────────────────────────────────────────────────────

/// Validate structural correctness of all TOML files in a bundle.
///
/// Returns a `Vec<ValidationResult>` for the `structural_toml` layer.
/// The caller adds these to a `ValidationReport`.
pub fn validate_structural(bundle_dir: &Path) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    // ── 1. Bundle manifest ────────────────────────────────────────
    let manifest_path = bundle_dir.join("manifest.toml");
    let manifest_rel = "manifest.toml";

    let bundle_value = match parse_toml_file(&manifest_path, manifest_rel) {
        Ok(v) => v,
        Err(r) => {
            results.push(r);
            return results; // Cannot continue without bundle manifest
        }
    };

    // Structural checks on bundle manifest
    if let Some(table) = bundle_value.as_table() {
        check_unknown_keys(table, BUNDLE_MANIFEST_FIELDS, manifest_rel, "", &mut results);
        check_required_fields(table, BUNDLE_MANIFEST_REQUIRED, manifest_rel, "", &mut results);

        // S010 — bundleVersion semver check
        if let Some(v) = table.get("bundleVersion") {
            if let Some(s) = v.as_str() {
                if !is_valid_semver(s) {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S010,
                            manifest_rel,
                            format!("bundleVersion '{}' is not valid semver (expected X.Y.Z)", s),
                        )
                        .with_field("bundleVersion"),
                    );
                }
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        manifest_rel,
                        "bundleVersion must be a string",
                    )
                    .with_field("bundleVersion"),
                );
            }
        }

        // Validate apps is an array of strings
        if let Some(apps) = table.get("apps") {
            if let Some(arr) = apps.as_array() {
                for (i, item) in arr.iter().enumerate() {
                    if !item.is_str() {
                        results.push(
                            ValidationResult::fail(
                                error_codes::S004,
                                manifest_rel,
                                format!("apps[{}] must be a string", i),
                            )
                            .with_field("apps"),
                        );
                    }
                }
            } else {
                results.push(
                    ValidationResult::fail(
                        error_codes::S004,
                        manifest_rel,
                        "apps must be an array",
                    )
                    .with_field("apps"),
                );
            }
        }
    } else {
        results.push(ValidationResult::fail(
            error_codes::S001,
            manifest_rel,
            "bundle manifest.toml root is not a table",
        ));
        return results;
    }

    // Pass for bundle manifest if no errors so far
    if results.iter().all(|r| r.status == crate::validate_result::ValidationStatus::Pass) {
        results.push(ValidationResult::pass(manifest_rel, "bundle manifest valid"));
    }

    // ── 2. Per-app files ──────────────────────────────────────────
    let apps = extract_app_names(&bundle_value);
    for app_name in &apps {
        let app_dir = bundle_dir.join(app_name);

        // App manifest
        let app_manifest_rel = format!("{}/manifest.toml", app_name);
        let app_manifest_path = app_dir.join("manifest.toml");
        match parse_toml_file(&app_manifest_path, &app_manifest_rel) {
            Ok(v) => validate_app_manifest(&v, &app_manifest_rel, app_name, &mut results),
            Err(r) => results.push(r),
        }

        // Resources
        let resources_rel = format!("{}/resources.toml", app_name);
        let resources_path = app_dir.join("resources.toml");
        match parse_toml_file(&resources_path, &resources_rel) {
            Ok(v) => validate_resources(&v, &resources_rel, app_name, &mut results),
            Err(r) => results.push(r),
        }

        // App config
        let app_toml_rel = format!("{}/app.toml", app_name);
        let app_toml_path = app_dir.join("app.toml");
        match parse_toml_file(&app_toml_path, &app_toml_rel) {
            Ok(v) => validate_app_config(&v, &app_toml_rel, app_name, &mut results),
            Err(r) => results.push(r),
        }
    }

    results
}

// ── File parsing ──────────────────────────────────────────────────

/// Parse a TOML file into a `toml::Value`. Returns `Err(ValidationResult)` on failure.
fn parse_toml_file(path: &Path, rel_path: &str) -> Result<toml::Value, ValidationResult> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        ValidationResult::fail(
            error_codes::S001,
            rel_path,
            format!("cannot read file: {}", e),
        )
    })?;

    content.parse::<toml::Value>().map_err(|e| {
        ValidationResult::fail(
            error_codes::S001,
            rel_path,
            format!("TOML parse error: {}", e),
        )
    })
}

// ── App manifest validation ───────────────────────────────────────

fn validate_app_manifest(
    value: &toml::Value,
    file: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(error_codes::S001, file, "app manifest root is not a table")
                    .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, APP_MANIFEST_FIELDS, file, "", results);
    check_required_fields(table, APP_MANIFEST_REQUIRED, file, "", results);

    // S008 — appId UUID format
    if let Some(v) = table.get("appId") {
        if let Some(s) = v.as_str() {
            if !is_valid_uuid(s) {
                results.push(
                    ValidationResult::fail(
                        error_codes::S008,
                        file,
                        format!("appId '{}' is not a valid UUID (expected 8-4-4-4-12 hex)", s),
                    )
                    .with_field("appId")
                    .with_app(app_name),
                );
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "appId must be a string")
                    .with_field("appId")
                    .with_app(app_name),
            );
        }
    }

    // S009 — type enum
    if let Some(v) = table.get("type") {
        if let Some(s) = v.as_str() {
            if s != "app-main" && s != "app-service" {
                results.push(
                    ValidationResult::fail(
                        error_codes::S009,
                        file,
                        format!("type '{}' is invalid — must be 'app-main' or 'app-service'", s),
                    )
                    .with_field("type")
                    .with_app(app_name),
                );
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "type must be a string")
                    .with_field("type")
                    .with_app(app_name),
            );
        }
    }

    // Validate spa sub-table if present
    if let Some(spa) = table.get("spa") {
        if let Some(spa_table) = spa.as_table() {
            check_unknown_keys(spa_table, SPA_FIELDS, file, "spa", results);
            check_required_fields(spa_table, SPA_REQUIRED, file, "spa", results);
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "spa must be a table")
                    .with_field("spa")
                    .with_app(app_name),
            );
        }
    }

    // Validate init sub-table if present
    if let Some(init) = table.get("init") {
        if let Some(init_table) = init.as_table() {
            check_unknown_keys(init_table, INIT_FIELDS, file, "init", results);
            check_required_fields(init_table, INIT_REQUIRED, file, "init", results);
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "init must be a table")
                    .with_field("init")
                    .with_app(app_name),
            );
        }
    }

    // If no errors were added for this file, emit a pass
    let has_errors = results.iter().any(|r| {
        r.file.as_deref() == Some(file)
            && r.status == crate::validate_result::ValidationStatus::Fail
    });
    if !has_errors {
        results.push(
            ValidationResult::pass(file, "app manifest valid").with_app(app_name),
        );
    }
}

// ── Resources validation ──────────────────────────────────────────

fn validate_resources(
    value: &toml::Value,
    file: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S001,
                    file,
                    "resources.toml root is not a table",
                )
                .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, RESOURCES_FIELDS, file, "", results);

    // Validate [[datasources]]
    if let Some(ds) = table.get("datasources") {
        if let Some(arr) = ds.as_array() {
            for (i, entry) in arr.iter().enumerate() {
                let table_path = format!("datasources[{}]", i);
                if let Some(entry_table) = entry.as_table() {
                    check_unknown_keys(entry_table, DATASOURCE_DECL_FIELDS, file, &table_path, results);
                    check_required_fields(entry_table, DATASOURCE_DECL_REQUIRED, file, &table_path, results);

                    // S006 — nopassword and lockbox mutual exclusion
                    let has_nopassword = entry_table
                        .get("nopassword")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let has_lockbox = entry_table.get("lockbox").is_some();

                    if has_nopassword && has_lockbox {
                        results.push(
                            ValidationResult::fail(
                                error_codes::S006,
                                file,
                                format!(
                                    "'nopassword=true' and 'lockbox' are mutually exclusive in {}",
                                    table_path
                                ),
                            )
                            .with_table_path(&table_path)
                            .with_app(app_name),
                        );
                    }
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!("{} must be a table", table_path),
                        )
                        .with_table_path(&table_path)
                        .with_app(app_name),
                    );
                }
            }
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    "datasources must be an array of tables",
                )
                .with_field("datasources")
                .with_app(app_name),
            );
        }
    }

    // Validate [[services]]
    if let Some(svcs) = table.get("services") {
        if let Some(arr) = svcs.as_array() {
            for (i, entry) in arr.iter().enumerate() {
                let table_path = format!("services[{}]", i);
                if let Some(entry_table) = entry.as_table() {
                    check_unknown_keys(entry_table, SERVICE_DECL_FIELDS, file, &table_path, results);
                    check_required_fields(entry_table, SERVICE_DECL_REQUIRED, file, &table_path, results);

                    // S008 — appId UUID format in service declarations
                    if let Some(v) = entry_table.get("appId") {
                        if let Some(s) = v.as_str() {
                            if !is_valid_uuid(s) {
                                results.push(
                                    ValidationResult::fail(
                                        error_codes::S008,
                                        file,
                                        format!(
                                            "appId '{}' in {} is not a valid UUID",
                                            s, table_path,
                                        ),
                                    )
                                    .with_table_path(&table_path)
                                    .with_field("appId")
                                    .with_app(app_name),
                                );
                            }
                        }
                    }
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!("{} must be a table", table_path),
                        )
                        .with_table_path(&table_path)
                        .with_app(app_name),
                    );
                }
            }
        }
    }

    // Validate [[keystores]]
    if let Some(ks) = table.get("keystores") {
        if let Some(arr) = ks.as_array() {
            for (i, entry) in arr.iter().enumerate() {
                let table_path = format!("keystores[{}]", i);
                if let Some(entry_table) = entry.as_table() {
                    check_unknown_keys(entry_table, KEYSTORE_DECL_FIELDS, file, &table_path, results);
                    check_required_fields(entry_table, KEYSTORE_DECL_REQUIRED, file, &table_path, results);
                } else {
                    results.push(
                        ValidationResult::fail(
                            error_codes::S004,
                            file,
                            format!("{} must be a table", table_path),
                        )
                        .with_table_path(&table_path)
                        .with_app(app_name),
                    );
                }
            }
        }
    }

    // Emit pass if no errors
    let has_errors = results.iter().any(|r| {
        r.file.as_deref() == Some(file)
            && r.status == crate::validate_result::ValidationStatus::Fail
    });
    if !has_errors {
        results.push(
            ValidationResult::pass(file, "resources valid").with_app(app_name),
        );
    }
}

// ── App config validation ─────────────────────────────────────────

fn validate_app_config(
    value: &toml::Value,
    file: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(error_codes::S001, file, "app.toml root is not a table")
                    .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, APP_CONFIG_FIELDS, file, "", results);

    // [data] section
    if let Some(data) = table.get("data") {
        if let Some(data_table) = data.as_table() {
            check_unknown_keys(data_table, DATA_CONFIG_FIELDS, file, "data", results);

            // data.datasources — map of datasource configs (runtime detail, not validated structurally here
            // beyond unknown key at the data level — datasource config structs are complex and runtime-owned)

            // data.dataviews — map of DataView configs
            if let Some(dvs) = data_table.get("dataviews") {
                if let Some(dvs_table) = dvs.as_table() {
                    for (dv_name, dv_value) in dvs_table {
                        let table_path = format!("data.dataviews.{}", dv_name);
                        validate_dataview(dv_value, file, &table_path, app_name, results);
                    }
                }
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "data must be a table")
                    .with_field("data")
                    .with_app(app_name),
            );
        }
    }

    // [api] section
    if let Some(api) = table.get("api") {
        if let Some(api_table) = api.as_table() {
            check_unknown_keys(api_table, API_CONFIG_FIELDS, file, "api", results);

            if let Some(views) = api_table.get("views") {
                if let Some(views_table) = views.as_table() {
                    for (view_name, view_value) in views_table {
                        let table_path = format!("api.views.{}", view_name);
                        validate_view(view_value, file, &table_path, app_name, results);
                    }
                }
            }
        } else {
            results.push(
                ValidationResult::fail(error_codes::S004, file, "api must be a table")
                    .with_field("api")
                    .with_app(app_name),
            );
        }
    }

    // [static_files] section
    if let Some(sf) = table.get("static_files") {
        if let Some(sf_table) = sf.as_table() {
            check_unknown_keys(sf_table, STATIC_FILES_FIELDS, file, "static_files", results);
        } else {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    "static_files must be a table",
                )
                .with_field("static_files")
                .with_app(app_name),
            );
        }
    }

    // Emit pass if no errors for this file
    let has_errors = results.iter().any(|r| {
        r.file.as_deref() == Some(file)
            && r.status == crate::validate_result::ValidationStatus::Fail
    });
    if !has_errors {
        results.push(
            ValidationResult::pass(file, "app config valid").with_app(app_name),
        );
    }
}

// ── DataView validation ───────────────────────────────────────────

fn validate_dataview(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    format!("{} must be a table", table_path),
                )
                .with_table_path(table_path)
                .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, DATAVIEW_FIELDS, file, table_path, results);
    check_required_fields(table, DATAVIEW_REQUIRED, file, table_path, results);

    // Validate caching sub-table
    if let Some(caching) = table.get("caching") {
        if let Some(caching_table) = caching.as_table() {
            let caching_path = format!("{}.caching", table_path);
            check_unknown_keys(caching_table, CACHING_FIELDS, file, &caching_path, results);
            check_required_fields(caching_table, CACHING_REQUIRED, file, &caching_path, results);
        }
    }

    // Validate parameters arrays
    for param_key in &[
        "parameters",
        "get_parameters",
        "post_parameters",
        "put_parameters",
        "delete_parameters",
    ] {
        if let Some(params) = table.get(*param_key) {
            if let Some(arr) = params.as_array() {
                for (i, p) in arr.iter().enumerate() {
                    let param_path = format!("{}.{}[{}]", table_path, param_key, i);
                    if let Some(pt) = p.as_table() {
                        check_unknown_keys(pt, PARAMETER_FIELDS, file, &param_path, results);
                        check_required_fields(pt, PARAMETER_REQUIRED, file, &param_path, results);
                    }
                }
            }
        }
    }
}

// ── View validation ───────────────────────────────────────────────

fn validate_view(
    value: &toml::Value,
    file: &str,
    table_path: &str,
    app_name: &str,
    results: &mut Vec<ValidationResult>,
) {
    let table = match value.as_table() {
        Some(t) => t,
        None => {
            results.push(
                ValidationResult::fail(
                    error_codes::S004,
                    file,
                    format!("{} must be a table", table_path),
                )
                .with_table_path(table_path)
                .with_app(app_name),
            );
            return;
        }
    };

    check_unknown_keys(table, VIEW_FIELDS, file, table_path, results);
    check_required_fields(table, VIEW_REQUIRED, file, table_path, results);

    // Validate handler sub-table
    if let Some(handler) = table.get("handler") {
        if let Some(handler_table) = handler.as_table() {
            let handler_path = format!("{}.handler", table_path);
            check_unknown_keys(handler_table, HANDLER_FIELDS, file, &handler_path, results);
            check_required_fields(handler_table, HANDLER_REQUIRED, file, &handler_path, results);
        }
    }

    // Validate parameter_mapping sub-table
    if let Some(pm) = table.get("parameter_mapping") {
        if let Some(pm_table) = pm.as_table() {
            let pm_path = format!("{}.parameter_mapping", table_path);
            check_unknown_keys(pm_table, PARAM_MAPPING_FIELDS, file, &pm_path, results);
        }
    }
}

// ── Core helpers ──────────────────────────────────────────────────

/// Check for unknown keys in a TOML table. Emits S002 for each unknown key
/// with a "did you mean?" suggestion if a close match exists.
fn check_unknown_keys(
    table: &toml::map::Map<String, toml::Value>,
    known: &[&str],
    file: &str,
    table_path: &str,
    results: &mut Vec<ValidationResult>,
) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            let mut result = ValidationResult::warn(
                error_codes::S002,
                format!("unknown key '{}' in [{}]", key, if table_path.is_empty() { "root" } else { table_path }),
            )
            .with_table_path(if table_path.is_empty() { "root" } else { table_path })
            .with_field(key.clone());

            result.file = Some(file.to_string());

            if let Some(suggestion) = suggest_key(key, known) {
                result = result.with_suggestion(suggestion);
            }

            results.push(result);
        }
    }
}

/// Check for missing required fields. Emits S003 for each missing field.
fn check_required_fields(
    table: &toml::map::Map<String, toml::Value>,
    required: &[&str],
    file: &str,
    table_path: &str,
    results: &mut Vec<ValidationResult>,
) {
    for &field in required {
        if !table.contains_key(field) {
            results.push(
                ValidationResult::fail(
                    error_codes::S003,
                    file,
                    format!(
                        "missing required field '{}' in [{}]",
                        field,
                        if table_path.is_empty() { "root" } else { table_path },
                    ),
                )
                .with_table_path(if table_path.is_empty() { "root" } else { table_path })
                .with_field(field),
            );
        }
    }
}

// ── Value validators ──────────────────────────────────────────────

/// Check if a string is a valid UUID (8-4-4-4-12 hex pattern).
fn is_valid_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let expected_lens = [8, 4, 4, 4, 12];
    for (part, &expected) in parts.iter().zip(expected_lens.iter()) {
        if part.len() != expected || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

/// Check if a string is valid semver (X.Y.Z where X, Y, Z are non-negative integers).
fn is_valid_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    for part in &parts {
        if part.is_empty() {
            return false;
        }
        // No leading zeros (except "0" itself)
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// Extract app directory names from a parsed bundle manifest value.
fn extract_app_names(value: &toml::Value) -> Vec<String> {
    value
        .get("apps")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_result::ValidationStatus;
    use std::path::PathBuf;

    /// Create a minimal valid bundle in a temporary directory.
    fn create_valid_bundle(dir: &Path) -> PathBuf {
        let bundle_dir = dir.join("test-bundle");
        let app_dir = bundle_dir.join("test-app");
        let schemas_dir = app_dir.join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();

        // Bundle manifest
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "https://test.example.com"
apps          = ["test-app"]
"#,
        )
        .unwrap();

        // App manifest
        std::fs::write(
            app_dir.join("manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-service"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "service"
source     = "https://test.example.com"
"#,
        )
        .unwrap();

        // Resources
        std::fs::write(
            app_dir.join("resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true
"#,
        )
        .unwrap();

        // App config
        std::fs::write(
            app_dir.join("app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"
query      = "schemas/item.schema.json"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();

        bundle_dir
    }

    // ── Valid bundle passes ────────────────────────────────────────

    #[test]
    fn valid_bundle_all_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());
        let results = validate_structural(&bundle_dir);

        let fails: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .collect();

        assert!(
            fails.is_empty(),
            "expected no failures but got: {:?}",
            fails
        );

        // Should have pass results for each file
        let passes: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Pass)
            .collect();
        assert!(
            passes.len() >= 4,
            "expected at least 4 pass results, got {}",
            passes.len()
        );
    }

    // ── S001: TOML parse error ────────────────────────────────────

    #[test]
    fn s001_invalid_toml_syntax() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Corrupt the bundle manifest
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            "bundleName = [invalid toml",
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S001"))
            .expect("expected S001 error");

        assert_eq!(fail.file.as_deref(), Some("manifest.toml"));
        assert!(fail.message.contains("TOML parse error"));
    }

    // ── S002: unknown key with did-you-mean ───────────────────────

    #[test]
    fn s002_unknown_key_in_bundle_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Add unknown key
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "test"
apps          = ["test-app"]
bundleNme     = "typo"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S002"))
            .expect("expected S002 error");

        assert_eq!(fail.file.as_deref(), Some("manifest.toml"));
        assert!(fail.message.contains("bundleNme"));
        assert!(
            fail.suggestion.as_ref().unwrap().contains("bundleName"),
            "suggestion should mention bundleName, got: {:?}",
            fail.suggestion
        );
    }

    #[test]
    fn s002_unknown_key_in_view() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
veiew_type = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("veiew_type")
            })
            .expect("expected S002 for veiew_type");

        assert!(fail.suggestion.as_ref().unwrap().contains("view_type"));
        assert_eq!(
            fail.table_path.as_deref(),
            Some("api.views.items")
        );
    }

    // ── S003: missing required field ──────────────────────────────

    #[test]
    fn s003_missing_required_field_bundle_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Missing apps
        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "test"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.field.as_deref() == Some("apps")
            })
            .expect("expected S003 for 'apps'");

        assert_eq!(fail.file.as_deref(), Some("manifest.toml"));
    }

    #[test]
    fn s003_missing_required_field_datasource() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Missing driver
        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
x-type     = "faker"
nopassword = true
required   = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.field.as_deref() == Some("driver")
            })
            .expect("expected S003 for missing 'driver'");

        assert_eq!(
            fail.file.as_deref(),
            Some("test-app/resources.toml")
        );
    }

    // ── S004: wrong type ──────────────────────────────────────────

    #[test]
    fn s004_apps_not_array() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0.0"
source        = "test"
apps          = "not-an-array"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S004")
                    && r.field.as_deref() == Some("apps")
            })
            .expect("expected S004 for apps type");

        assert!(fail.message.contains("must be an array"));
    }

    // ── S006: nopassword and lockbox mutual exclusion ─────────────

    #[test]
    fn s006_nopassword_and_lockbox() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
lockbox    = "lockbox://db/test"
required   = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S006"))
            .expect("expected S006 error");

        assert!(fail.message.contains("mutually exclusive"));
    }

    // ── S008: appId not valid UUID ────────────────────────────────

    #[test]
    fn s008_invalid_uuid() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-service"
appId      = "not-a-uuid"
entryPoint = "service"
source     = "test"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S008"))
            .expect("expected S008 error");

        assert!(fail.message.contains("not a valid UUID"));
        assert_eq!(fail.field.as_deref(), Some("appId"));
    }

    #[test]
    fn s008_valid_uuid_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Default test bundle has a valid UUID, so just check no S008
        let results = validate_structural(&bundle_dir);
        let s008 = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S008"));
        assert!(s008.is_none(), "valid UUID should not produce S008");
    }

    // ── S009: invalid app_type ────────────────────────────────────

    #[test]
    fn s009_invalid_app_type() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "worker"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "service"
source     = "test"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S009"))
            .expect("expected S009 error");

        assert!(fail.message.contains("worker"));
        assert!(fail.message.contains("app-main"));
        assert!(fail.message.contains("app-service"));
    }

    // ── S010: invalid semver ──────────────────────────────────────

    #[test]
    fn s010_invalid_semver() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "1.0"
source        = "test"
apps          = ["test-app"]
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S010"))
            .expect("expected S010 error");

        assert!(fail.message.contains("not valid semver"));
    }

    #[test]
    fn s010_leading_zero_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("manifest.toml"),
            r#"
bundleName    = "test-bundle"
bundleVersion = "01.0.0"
source        = "test"
apps          = ["test-app"]
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S010"));
        assert!(fail.is_some(), "leading zeros should be invalid semver");
    }

    // ── UUID validator unit tests ─────────────────────────────────

    #[test]
    fn uuid_validator() {
        assert!(is_valid_uuid("aaaaaaaa-bbbb-cccc-dddd-000000000001"));
        assert!(is_valid_uuid("c7a3e1f0-8b2d-4d6e-9f1a-3c5b7d9e2f4a"));
        assert!(is_valid_uuid("F47AC10B-58CC-4372-A567-0E02B2C3D479"));
        assert!(!is_valid_uuid("not-a-uuid"));
        assert!(!is_valid_uuid("aaaaaaaa-bbbb-cccc-dddd"));
        assert!(!is_valid_uuid("aaaaaaaa-bbbb-cccc-dddd-00000000000g"));
        assert!(!is_valid_uuid(""));
        assert!(!is_valid_uuid("aaaaaaaa_bbbb_cccc_dddd_000000000001"));
    }

    // ── Semver validator unit tests ───────────────────────────────

    #[test]
    fn semver_validator() {
        assert!(is_valid_semver("1.0.0"));
        assert!(is_valid_semver("0.0.1"));
        assert!(is_valid_semver("10.20.30"));
        assert!(!is_valid_semver("1.0"));
        assert!(!is_valid_semver("1.0.0.0"));
        assert!(!is_valid_semver("v1.0.0"));
        assert!(!is_valid_semver("1.0.0-beta"));
        assert!(!is_valid_semver("01.0.0"));
        assert!(!is_valid_semver(""));
        assert!(!is_valid_semver(".."));
    }

    // ── Address book bundle validation ────────────────────────────

    #[test]
    fn address_book_bundle_validates() {
        let bundle_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../address-book-bundle");

        if !bundle_dir.exists() {
            // Skip if address-book-bundle not present
            return;
        }

        let results = validate_structural(&bundle_dir);

        let fails: Vec<_> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .collect();

        // The address book bundle uses some fields not in the minimal friction-doc
        // field sets (e.g., response_format on views, config sub-table on datasources).
        // Those are intentionally allowed by the field sets — verify no unexpected failures.
        for fail in &fails {
            // Print for debugging if this test fails
            eprintln!(
                "FAIL [{}] {} — {} (field: {:?}, suggestion: {:?})",
                fail.error_code.as_deref().unwrap_or("?"),
                fail.file.as_deref().unwrap_or("?"),
                fail.message,
                fail.field,
                fail.suggestion,
            );
        }

        // We expect zero hard failures — the field sets should cover the address book
        assert!(
            fails.is_empty(),
            "address-book-bundle should pass structural validation, got {} failures",
            fails.len()
        );
    }

    // ── Nested validation ─────────────────────────────────────────

    #[test]
    fn dataview_caching_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[data.dataviews.items.caching]
ttl_seconds  = 60
ttl_secnds   = 30

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("ttl_secnds")
            })
            .expect("expected S002 for unknown 'ttl_secnds' in caching");

        assert!(
            fail.table_path
                .as_ref()
                .unwrap()
                .contains("caching"),
        );
        // "ttl_secnds" is distance 1 from "ttl_seconds", so we get a suggestion
        assert!(
            fail.suggestion
                .as_ref()
                .unwrap()
                .contains("ttl_seconds"),
        );
    }

    #[test]
    fn handler_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type      = "dataview"
dataview  = "items"
dataveiw  = "items"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("dataveiw")
            })
            .expect("expected S002 for unknown 'dataveiw' in handler");

        assert!(fail.suggestion.as_ref().unwrap().contains("dataview"));
    }

    #[test]
    fn parameter_mapping_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[data.dataviews.items]
name       = "items"
datasource = "data"

[api.views.items]
path       = "items"
method     = "GET"
view_type  = "Rest"
auth       = "none"

[api.views.items.handler]
type     = "dataview"
dataview = "items"

[api.views.items.parameter_mapping]
qeury = {limit = "limit"}
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("qeury")
            })
            .expect("expected S002 for unknown 'qeury' in parameter_mapping");

        assert!(fail.suggestion.as_ref().unwrap().contains("query"));
    }

    // ── Multiple errors collected ─────────────────────────────────

    #[test]
    fn collects_all_errors_in_one_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Introduce multiple errors
        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
type       = "worker"
appId      = "not-a-uuid"
entryPoint = "service"
source     = "test"
foo        = "bar"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);

        let error_codes: Vec<&str> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Fail)
            .filter_map(|r| r.error_code.as_deref())
            .collect();

        let warn_codes: Vec<&str> = results
            .iter()
            .filter(|r| r.status == ValidationStatus::Warn)
            .filter_map(|r| r.error_code.as_deref())
            .collect();

        // S002 (unknown key 'foo') is now a warning, S003/S008/S009 remain errors
        assert!(warn_codes.contains(&"S002"), "missing S002 warning: {:?}", warn_codes);
        assert!(error_codes.contains(&"S003"), "missing S003: {:?}", error_codes);
        assert!(error_codes.contains(&"S008"), "missing S008: {:?}", error_codes);
        assert!(error_codes.contains(&"S009"), "missing S009: {:?}", error_codes);
    }

    // ── Missing file ──────────────────────────────────────────────

    #[test]
    fn missing_bundle_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = tmp.path().join("empty-bundle");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        let results = validate_structural(&bundle_dir);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].error_code.as_deref(), Some("S001"));
        assert!(results[0].message.contains("cannot read file"));
    }

    #[test]
    fn missing_app_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        // Remove app manifest
        std::fs::remove_file(bundle_dir.join("test-app/manifest.toml")).unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S001")
                    && r.file
                        .as_deref()
                        .map(|f| f.contains("test-app/manifest.toml"))
                        .unwrap_or(false)
            })
            .expect("expected S001 for missing app manifest");

        assert!(fail.message.contains("cannot read file"));
    }

    // ── Static files section ──────────────────────────────────────

    #[test]
    fn static_files_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/app.toml"),
            r#"
[static_files]
enabled      = true
root         = "libraries/spa"
index_file   = "index.html"
spa_fallback = true
typo_field   = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("typo_field")
            })
            .expect("expected S002 for unknown field in static_files");

        assert!(fail.table_path.as_deref() == Some("static_files"));
    }

    // ── Keystore validation ───────────────────────────────────────

    #[test]
    fn keystore_unknown_key_in_resources() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true

[[keystores]]
name     = "app-keys"
lockbox  = "test/keystore-key"
required = true
unknwon  = "value"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("unknwon")
            })
            .expect("expected S002 for unknown key in keystore");

        assert!(
            fail.table_path.as_deref().unwrap().contains("keystores"),
        );
    }

    // ── Service appId UUID ────────────────────────────────────────

    #[test]
    fn service_invalid_app_id() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/resources.toml"),
            r#"
[[datasources]]
name       = "data"
driver     = "faker"
x-type     = "faker"
nopassword = true
required   = true

[[services]]
name     = "other-app"
appId    = "bad-uuid"
required = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| r.error_code.as_deref() == Some("S008"))
            .expect("expected S008 for bad UUID in service");

        assert!(fail.message.contains("bad-uuid"));
    }

    // ── SPA and init sub-tables ───────────────────────────────────

    #[test]
    fn spa_config_unknown_key() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-main"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "main"
source     = "test"

[spa]
root      = "libraries/spa"
indexFile  = "index.html"
fallback  = true
unknownFld = "oops"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S002")
                    && r.field.as_deref() == Some("unknownFld")
            })
            .expect("expected S002 for unknown key in spa");

        assert!(fail.table_path.as_deref() == Some("spa"));
    }

    #[test]
    fn spa_config_missing_required() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-main"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "main"
source     = "test"

[spa]
fallback = true
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);

        let missing: Vec<_> = results
            .iter()
            .filter(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.table_path.as_deref() == Some("spa")
            })
            .collect();

        assert!(
            missing.len() >= 2,
            "expected missing root and indexFile in spa, got {} errors",
            missing.len()
        );
    }

    #[test]
    fn init_config_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_dir = create_valid_bundle(tmp.path());

        std::fs::write(
            bundle_dir.join("test-app/manifest.toml"),
            r#"
appName    = "test-app"
version    = "1.0.0"
type       = "app-service"
appId      = "aaaaaaaa-bbbb-cccc-dddd-000000000001"
entryPoint = "service"
source     = "test"

[init]
module = "handlers/init.ts"
"#,
        )
        .unwrap();

        let results = validate_structural(&bundle_dir);
        let fail = results
            .iter()
            .find(|r| {
                r.error_code.as_deref() == Some("S003")
                    && r.field.as_deref() == Some("entrypoint")
                    && r.table_path.as_deref() == Some("init")
            })
            .expect("expected S003 for missing entrypoint in init");

        assert!(fail.message.contains("entrypoint"));
    }
}
